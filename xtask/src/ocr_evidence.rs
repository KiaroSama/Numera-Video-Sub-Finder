use image::RgbImage;
use nvsf_app::{Paths, inference::Ocr, media::MediaRuntime, tesseract::Tesseract};
use nvsf_core::Rect;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    io::Write,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub(super) fn tensor_summary(stage: &str, shape: &[usize], values: &[f32]) -> Result<Value> {
    let count = shape
        .iter()
        .try_fold(1usize, |n, &d| n.checked_mul(d))
        .ok_or("Tensor shape overflow")?;
    if count != values.len() || count == 0 || count > 8_000_000 {
        return Err("Tensor evidence exceeds its shape/element budget".into());
    }
    let mut hash = Sha256::new();
    let (mut min, mut max, mut nonfinite) = (f32::INFINITY, f32::NEG_INFINITY, 0usize);
    for value in values {
        hash.update(value.to_bits().to_le_bytes());
        if value.is_finite() {
            min = min.min(*value);
            max = max.max(*value);
        } else {
            nonfinite += 1;
        }
    }
    let top1: Vec<_> = if stage == "recognizer-output"
        && shape.len() == 3
        && shape[0] == 1
        && shape[1] <= 4096
        && shape[2] > 0
    {
        values
            .chunks_exact(shape[2])
            .map(|row| {
                row.iter()
                    .enumerate()
                    .filter(|(_, v)| v.is_finite())
                    .fold(None, |best: Option<(usize, &f32)>, candidate| match best {
                        Some((_, score)) if score >= candidate.1 => best,
                        _ => Some(candidate),
                    })
                    .map(|(index, score)| json!({"index":index,"score":score}))
            })
            .collect()
    } else {
        Vec::new()
    };
    Ok(
        json!({"stage":stage,"shape":shape,"elements":count,"nonfinite":nonfinite,
        "min":min.is_finite().then_some(min),"max":max.is_finite().then_some(max),
        "sha256":hash.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>(),"top1":top1}),
    )
}

fn pixel_comparison(native: &RgbImage, reference: &RgbImage) -> Result<Value> {
    let expected = u64::from(native.width()) * u64::from(native.height()) * 3;
    if native.dimensions() != reference.dimensions()
        || native.as_raw().len() as u64 != expected
        || reference.as_raw().len() as u64 != expected
    {
        return Err("Reference/native active RGB dimensions differ".into());
    }
    let (mut count, mut maximum, mut first) = (0usize, 0u8, None);
    for (i, (a, b)) in native.as_raw().iter().zip(reference.as_raw()).enumerate() {
        let delta = a.abs_diff(*b);
        if delta > 0 {
            count += 1;
            maximum = maximum.max(delta);
            first.get_or_insert(i);
        }
    }
    Ok(json!({"different_samples":count,"max_channel_delta":maximum,"first_sample":first}))
}

fn image_summary(image: &RgbImage) -> Value {
    json!({"width":image.width(),"height":image.height(),"rgb_sha256":Sha256::digest(image.as_raw()).iter().map(|b| format!("{b:02x}")).collect::<String>()})
}

fn persist(output: &Path, report: &Value) -> Result<()> {
    std::fs::write(
        output.join("pipeline.json"),
        serde_json::to_vec_pretty(report)?,
    )?;
    Ok(())
}

pub fn capture(ffmpeg: &Path, fixture_dir: &Path, data_root: &Path, output: &Path) -> Result<()> {
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
        || std::env::var("RUNNER_ENVIRONMENT").as_deref() != Ok("github-hosted")
    {
        return Err("OCR evidence is restricted to hosted generated fixtures".into());
    }
    let marker: Value =
        serde_json::from_slice(&std::fs::read(fixture_dir.join("CORPUS_STATUS.json"))?)?;
    if marker["synthetic"] != true || marker["language"] != "en" {
        return Err("Only the generated English fixture is admitted".into());
    }
    std::fs::create_dir(output)?;
    let started = Instant::now();
    let input = fixture_dir.join("en.mp4").canonicalize()?;
    let mut report = json!({"schema":1,"source_sha":std::env::var("GITHUB_SHA")?,"target":std::env::var("NVSF_TARGET_ID")?,
        "synthetic":true,"source_sha256":crate::fixture::sha(&input)?,"status":"capturing","samples":[]});
    persist(output, &report)?;
    let paths = Paths::new(Some(data_root))?;
    let runtime = MediaRuntime::load(&paths)?;
    let info = runtime.probe(&input)?;
    if info.width != 640
        || info.height != 360
        || info.duration_ms != Some(6000)
        || info.rotation != 0
    {
        return Err("Unexpected generated fixture geometry/timing".into());
    }
    report["media_health"] = serde_json::to_value(runtime.health())?;
    let native = runtime.preview(&input, 0)?;
    native.save(output.join("native.png"))?;
    report["native"] = image_summary(&native);
    persist(output, &report)?;
    let raw = output.join("reference.rgb");
    let mut command = Command::new(ffmpeg);
    command
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-threads",
            "2",
            "-filter_threads",
            "2",
            "-i",
        ])
        .arg(&input)
        .args([
            "-map",
            "0:v:0",
            "-frames:v",
            "1",
            "-sws_flags",
            "bilinear",
            "-c:v",
            "rawvideo",
            "-threads",
            "2",
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            "-n",
        ])
        .arg(&raw);
    crate::fixture::bounded(&mut command, Duration::from_secs(15))?;
    if std::fs::metadata(&raw)?.len() != 640 * 360 * 3 {
        return Err("Reference RGB byte length mismatch".into());
    }
    let reference =
        RgbImage::from_raw(640, 360, std::fs::read(&raw)?).ok_or("Invalid reference RGB")?;
    reference.save(output.join("reference.png"))?;
    report["reference"] = image_summary(&reference);
    report["pixel_comparison"] = pixel_comparison(&native, &reference)?;
    persist(output, &report)?;
    let mut ocr = Ocr::new(&paths, "en", "cpu", 2, false)?;
    #[cfg(windows)]
    let mut verifier = {
        let directory = tempfile::Builder::new()
            .prefix("tessdata 日本 path with spaces ")
            .tempdir_in(output)?;
        let mut relocated = paths.clone();
        relocated.models = directory.path().canonicalize()?.join("models");
        std::fs::create_dir_all(relocated.models.join("tessdata"))?;
        let selected = relocated.models.join("tessdata/eng.traineddata");
        std::fs::copy(nvsf_app::models::verify_tessdata(&paths, "en")?, &selected)?;
        let verifier = Tesseract::new(&relocated, "en")?;
        std::fs::write(&selected, b"owned corrupt model fixture")?;
        if Tesseract::new(&relocated, "en").is_ok()
            || std::fs::read(&selected)? != b"owned corrupt model fixture"
        {
            return Err("Tesseract accepted or modified a corrupt relocated model".into());
        }
        nvsf_app::models::verify_tessdata(&paths, "en")?;
        // The initialized native engine owns its model; temporary input bytes can disappear.
        directory.close()?;
        report["windows_model_io"] = json!({"unicode_spaced_root":true,"corrupt_pack_rejected":true,"original_pack_preserved":true});
        verifier
    };
    #[cfg(not(windows))]
    let mut verifier = Tesseract::new(&paths, "en")?;
    report["provider"] = json!(ocr.provider);
    report["models"] = serde_json::to_value(
        nvsf_app::models::specs()?
            .into_iter()
            .filter(|spec| spec.id == "det" || spec.id == "en-ja")
            .collect::<Vec<_>>(),
    )?;
    persist(output, &report)?;
    let mut tensor_bytes = 0usize;
    for (label, frame) in [("native", &native), ("reference", &reference)] {
        if started.elapsed() > Duration::from_secs(90) {
            return Err("OCR evidence exceeded its deadline".into());
        }
        let (x, y, w, h) = Rect::default().pixels(frame.width(), frame.height())?;
        let roi = image::imageops::crop_imm(frame, x, y, w, h).to_image();
        roi.save(output.join(format!("{label}-roi.png")))?;
        let mut tensors = Vec::new();
        let mut observe = |stage: &str, shape: &[usize], values: &[f32]| -> nvsf_app::Result<()> {
            let result = (|| -> Result<()> {
                if started.elapsed() > Duration::from_secs(90) {
                    return Err("OCR evidence exceeded its deadline".into());
                }
                let summary = tensor_summary(stage, shape, values)?;
                let size = values.len().checked_mul(4).ok_or("Tensor byte overflow")?;
                tensor_bytes = tensor_bytes
                    .checked_add(size)
                    .ok_or("Tensor total overflow")?;
                if tensor_bytes > 64 * 1024 * 1024 {
                    return Err("Tensor evidence exceeds64MiB".into());
                }
                let name = format!("{label}-{}-{stage}.f32le", tensors.len());
                let mut file = std::fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(output.join(&name))?;
                let mut bytes = Vec::with_capacity(size);
                for value in values {
                    bytes.extend_from_slice(&value.to_bits().to_le_bytes());
                }
                file.write_all(&bytes)?;
                file.sync_all()?;
                std::fs::write(
                    output.join(format!("{name}.json")),
                    serde_json::to_vec_pretty(&summary)?,
                )?;
                tensors.push(json!({"file":name,"tensor":summary}));
                Ok(())
            })();
            result.map_err(|e| nvsf_app::fail(e.to_string()))
        };
        let boxes = ocr.detect_observed(&roi, &mut observe)?;
        if boxes.len() > 8 {
            return Err("Generated fixture exceeds eight diagnostic crops".into());
        }
        let mut recognized = Vec::new();
        for (index, &(bx, by, bw, bh)) in boxes.iter().enumerate() {
            if started.elapsed() > Duration::from_secs(90) {
                return Err("OCR evidence exceeded its deadline".into());
            }
            let crop = image::imageops::crop_imm(&roi, bx, by, bw, bh).to_image();
            crop.save(output.join(format!("{label}-crop-{index}.png")))?;
            let ordinary = ocr.recognize_observed(&crop, &mut observe)?;
            let independent = verifier.recognize(&crop, false)?;
            recognized.push(json!({"box":[bx,by,bw,bh],"image":image_summary(&crop),"primary":ordinary,"verifier":independent}));
        }
        report["samples"].as_array_mut().ok_or("Missing diagnostic samples")?.push(json!({"label":label,"roi":image_summary(&roi),"boxes":boxes,"recognized":recognized,"tensors":tensors}));
        persist(output, &report)?;
    }
    crate::ocr_replay::capture(&mut ocr, &paths, output, started, tensor_bytes)?;
    report["status"] =
        json!("captured; ordinary Hello/recurrence/timing acceptance is separate and unchanged");
    report["elapsed_ms"] = json!(started.elapsed().as_millis());
    persist(output, &report)?;
    println!(
        "Generated fixture OCR stage evidence retained; no quality or recognition success implied"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tensor_fingerprint_preserves_bits_and_rejects_invalid_shape() {
        let positive = tensor_summary("input", &[1, 2], &[0.0, 1.0]).unwrap();
        let negative = tensor_summary("input", &[1, 2], &[-0.0, 1.0]).unwrap();
        assert_ne!(positive["sha256"], negative["sha256"]);
        assert_eq!(positive["elements"], 2);
        assert!(tensor_summary("input", &[3], &[0.0, 1.0]).is_err());
        assert!(tensor_summary("input", &[usize::MAX, 2], &[]).is_err());
        let bad = tensor_summary("output", &[1], &[f32::NAN]).unwrap();
        assert_eq!(bad["nonfinite"], 1);
        let tied = tensor_summary("recognizer-output", &[1, 1, 2], &[0.5, 0.5]).unwrap();
        assert_eq!(tied["top1"][0]["index"], 0);
    }

    #[test]
    fn pixel_comparison_checks_active_rgb_samples_not_color_metadata() {
        let a = RgbImage::from_raw(2, 1, vec![1, 2, 3, 4, 5, 6]).unwrap();
        let b = RgbImage::from_raw(2, 1, vec![1, 2, 9, 4, 5, 6]).unwrap();
        let diff = pixel_comparison(&a, &b).unwrap();
        assert_eq!(diff["different_samples"], 1);
        assert_eq!(diff["max_channel_delta"], 6);
        assert_eq!(diff["first_sample"], 2);
        assert!(pixel_comparison(&a, &RgbImage::new(1, 1)).is_err());
        let trailing = RgbImage::from_raw(2, 1, vec![1, 2, 3, 4, 5, 6, 7]).unwrap();
        assert!(pixel_comparison(&a, &trailing).is_err());
    }
}
