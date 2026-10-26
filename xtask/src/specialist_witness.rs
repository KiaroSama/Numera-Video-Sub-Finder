//! Actual native specialist admission on a known synthetic numeric crop, never Japanese quality.
use image::RgbImage;
use nvsf_app::{Paths, inference::Ocr, media::MediaRuntime, models};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    path::Path,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const DEADLINE: Duration = Duration::from_secs(90);
const SPECIALIST_CLASSES: usize = 4401;

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn literal(specialist: &str, baseline: &str) -> Result<()> {
    if specialist != "16" || baseline != "16" {
        return Err(format!("Synthetic numeric admission requires exact16: specialist={specialist:?} baseline={baseline:?}").into());
    }
    Ok(())
}

#[derive(Default)]
struct Tensors {
    input: Option<Value>,
    output: Option<Value>,
}

impl Tensors {
    fn observe(
        &mut self,
        stage: &str,
        shape: &[usize],
        values: &[f32],
        classes: usize,
    ) -> Result<()> {
        let elements = shape.iter().try_fold(1usize, |n, d| n.checked_mul(*d));
        if elements != Some(values.len())
            || values.is_empty()
            || values.len() > 8_000_000
            || values.iter().any(|v| !v.is_finite())
        {
            return Err("Invalid or oversized specialist tensor evidence".into());
        }
        let slot = match stage {
            "recognizer-input"
                if shape.len() == 4
                    && shape[..3] == [1, 3, 48]
                    && (320..=3200).contains(&shape[3])
                    && values.iter().all(|v| (-1.0..=1.0).contains(v)) =>
            {
                &mut self.input
            }
            "recognizer-output"
                if shape.len() == 3
                    && shape[0] == 1
                    && (1..=4096).contains(&shape[1])
                    && shape[2] == classes =>
            {
                &mut self.output
            }
            _ => {
                return Err("Specialist input or dictionary/output tensor contract differs".into());
            }
        };
        if slot.is_some() {
            return Err("Duplicate recognition tensor observation".into());
        }
        *slot = Some(crate::ocr_evidence::tensor_summary(stage, shape, values)?);
        Ok(())
    }

    fn complete(&self) -> Result<()> {
        if self.input.is_none() || self.output.is_none() {
            return Err("Actual input and output tensors are both required".into());
        }
        Ok(())
    }
}

fn budget(start: Instant) -> Result<()> {
    if start.elapsed() >= DEADLINE {
        return Err("Specialist witness exceeded90-second cooperative budget".into());
    }
    Ok(())
}

fn recognize(
    paths: &Paths,
    language: &str,
    classes: usize,
    crop: &RgbImage,
    start: Instant,
) -> Result<(String, Value)> {
    budget(start)?;
    let mut ocr = Ocr::new(paths, language, "cpu", 2, false)?;
    if ocr.provider != "cpu" {
        return Err("Specialist admission requires actual CPU session policy".into());
    }
    budget(start)?;
    let mut tensors = Tensors::default();
    let recognition = ocr.recognize_observed(crop, &mut |stage, shape, values| {
        budget(start).map_err(|e| nvsf_app::AppError::Message(e.to_string()))?;
        tensors
            .observe(stage, shape, values, classes)
            .map_err(|e| nvsf_app::AppError::Message(e.to_string()))
    })?;
    tensors.complete()?;
    if recognition
        .confidence
        .is_none_or(|c| !c.is_finite() || !(0.0..=1.0).contains(&c))
    {
        return Err("Specialist confidence is absent or invalid".into());
    }
    budget(start)?;
    println!("Synthetic specialist admission: {language} native recognition completed");
    Ok((
        recognition.text.clone(),
        json!({"language":language,"provider":ocr.provider,
        "text":recognition.text,"confidence":recognition.confidence,
        "input":tensors.input,"output":tensors.output,"dictionary_classes":classes}),
    ))
}

fn metadata_classes(id: &str) -> Result<usize> {
    let metadata = models::metadata(id)?;
    let spec = models::specs()?
        .into_iter()
        .find(|s| s.id == id)
        .ok_or("Missing model contract")?;
    if hash(metadata.as_bytes()) != spec.metadata_sha256 {
        return Err("Model dictionary metadata hash differs".into());
    }
    let (_, dictionary) = metadata
        .split_once("  character_dict:\n")
        .ok_or("Missing character dictionary")?;
    let count = dictionary
        .lines()
        .filter(|line| line.starts_with("  - "))
        .count();
    if count == 0 || count > 50_000 {
        return Err("Invalid bounded recognition dictionary".into());
    }
    Ok(count + 2)
}

/// Requires the existing generated fixture and provisioned native/models data; performs no download.
/// Caller must run under the owned120-second wall/60-second idle hosted guard.
pub fn verify(fixture_dir: &Path, data_root: &Path, output: &Path) -> Result<()> {
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
        || std::env::var("RUNNER_ENVIRONMENT").as_deref() != Ok("github-hosted")
    {
        return Err("Specialist admission requires public hosted Actions".into());
    }
    let source = std::env::var("GITHUB_SHA")?;
    if nvsf_core::BUILD_SOURCE_SHA != Some(source.as_str()) {
        return Err("Specialist witness source identity differs".into());
    }
    let target = std::env::var("NVSF_TARGET_ID")?;
    let architecture = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => return Err("Unsupported native specialist architecture".into()),
    };
    if target != format!("{}-{architecture}", std::env::consts::OS) {
        return Err("Specialist native target identity differs".into());
    }
    let marker: Value =
        serde_json::from_slice(&std::fs::read(fixture_dir.join("CORPUS_STATUS.json"))?)?;
    if marker["synthetic"] != true || marker["language"] != "en" {
        return Err("Only the declared generated English fixture is admitted".into());
    }
    let work = Path::new(".ci-work").canonicalize()?;
    let parent = output
        .parent()
        .ok_or("Specialist report directory needs a parent")?
        .canonicalize()?;
    if !parent.starts_with(&work) || output.exists() || output.is_symlink() {
        return Err(
            "Specialist report directory must be new and contained in owned CI work".into(),
        );
    }
    std::fs::create_dir(output)?;
    let start = Instant::now();
    let input = fixture_dir.join("en.mp4").canonicalize()?;
    let before = crate::fixture::sha(&input)?;
    let mut report = json!({"schema":1,"source_sha":source,"target":target,"passed":false,
        "tests":1,"failed":1,"skipped":0,"synthetic":true,"profile":"numeric-specialist-admission",
        "expected":"16","at_ms":5500,"source_sha256":before,"real_japanese_parity":"NOT_RUN",
        "not_proved":["Japanese text/vertical/furigana quality","real corpus CER or timing","physical accelerator qualification"]});
    let persist = |record: &Value| -> Result<()> {
        std::fs::write(
            output.join("specialist.json"),
            serde_json::to_vec_pretty(record)?,
        )?;
        Ok(())
    };
    persist(&report)?;
    let result = (|| -> Result<()> {
        // Provisioning already created these roots; avoid seeding or altering package data here.
        let root = data_root.canonicalize()?;
        let runtimes = std::env::var_os("NVSF_RUNTIME_ROOT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| Path::new(".ci-work/runtimes").to_path_buf())
            .canonicalize()?;
        let paths = Paths {
            models: root.join("models"),
            projects: root.join("projects"),
            logs: root.join("logs"),
            cache: root.join("cache"),
            root,
            runtimes,
        };
        let mut admitted = Vec::new();
        for id in ["det", "ja-specialist", "en-ja"] {
            models::verify(&paths, id)?;
            let spec = models::specs()?
                .into_iter()
                .find(|s| s.id == id)
                .ok_or("Missing model spec")?;
            admitted.push(
                json!({"id":id,"revision":spec.revision,"sha256":spec.sha256,
                "bytes":spec.bytes,"metadata_sha256":spec.metadata_sha256}),
            );
        }
        budget(start)?;
        println!("Synthetic specialist admission: native and model identities verified");
        let runtime = MediaRuntime::load(&paths)?;
        let info = runtime.probe(&input)?;
        if (info.width, info.height, info.duration_ms, info.rotation) != (640, 360, Some(6000), 0) {
            return Err(
                "Specialist numeric source geometry/timing differs from fixture recipe".into(),
            );
        }
        let frame = runtime.preview(&input, 5500)?;
        if frame.dimensions() != (640, 360) {
            return Err("Specialist preview geometry differs".into());
        }
        let frame_hash = hash(frame.as_raw());
        // The existing font40 numeric16 is centered at y=290; fixed margins contain both digits.
        let crop = image::imageops::crop_imm(&frame, 280, 282, 80, 58).to_image();
        let crop_hash = hash(crop.as_raw());
        crop.save(output.join("numeric-crop.png"))?;
        let specialist_classes = metadata_classes("ja-specialist")?;
        if specialist_classes != SPECIALIST_CLASSES {
            return Err("Pinned Japanese specialist must expose4401 CTC classes".into());
        }
        let baseline_classes = metadata_classes("en-ja")?;
        let (specialist, specialist_record) =
            recognize(&paths, "ja-specialist", specialist_classes, &crop, start)?;
        let (baseline, baseline_record) = recognize(&paths, "ja", baseline_classes, &crop, start)?;
        report["observations"] = json!([specialist_record, baseline_record]);
        report["models"] = json!(admitted);
        report["crop"] = json!({"x":280,"y":282,"width":80,"height":58,"rgb_sha256":crop_hash});
        report["frame_rgb_sha256"] = json!(frame_hash);
        persist(&report)?;
        literal(&specialist, &baseline)?;
        if hash(frame.as_raw()) != frame_hash
            || hash(crop.as_raw()) != crop_hash
            || crate::fixture::sha(&input)? != before
        {
            return Err("Specialist admission modified original source or crop pixels".into());
        }
        for id in ["det", "ja-specialist", "en-ja"] {
            models::verify(&paths, id)?;
        }
        budget(start)?;
        report["passed"] = json!(true);
        report["failed"] = json!(0);
        Ok(())
    })();
    report["elapsed_ms"] = json!(start.elapsed().as_millis());
    if let Err(error) = &result {
        report["error"] = json!(error.to_string());
    }
    persist(&report)?;
    result
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_specialist_metadata_matches_the_independently_inspected_onnx_vocabulary() {
        assert_eq!(metadata_classes("ja-specialist").unwrap(), 4401);
        assert!(metadata_classes("en-ja").unwrap() > 4401);
        assert!(metadata_classes("untrusted-model").is_err());
    }

    #[test]
    fn literal_admission_requires_both_real_recognizers_and_complete_tensor_evidence() {
        let shape = [1, 2, 4401];
        let mut tensors = Tensors::default();
        tensors
            .observe(
                "recognizer-input",
                &[1, 3, 48, 320],
                &vec![0.0; 46_080],
                4401,
            )
            .unwrap();
        tensors
            .observe("recognizer-output", &shape, &vec![0.0; 8802], 4401)
            .unwrap();
        assert!(tensors.complete().is_ok());
        assert!(literal("16", "16").is_ok());
        for (specialist, baseline) in [
            ("", "16"),
            ("16", ""),
            ("15", "16"),
            ("16", "1 6"),
            ("１６", "16"),
        ] {
            assert!(literal(specialist, baseline).is_err());
        }
        assert!(Tensors::default().complete().is_err());
    }

    #[test]
    fn tensor_admission_rejects_wrong_vocabulary_shape_nonfinite_and_duplicate_observation() {
        for (shape, values) in [
            (vec![1, 1, 4400], vec![0.0; 4400]),
            (vec![2, 1, 4401], vec![0.0; 8802]),
            (vec![1, 0, 4401], vec![]),
            (vec![1, 1, 4401], vec![f32::NAN; 4401]),
            (vec![1, 1, 4401], vec![0.0; 4400]),
        ] {
            assert!(
                Tensors::default()
                    .observe("recognizer-output", &shape, &values, 4401)
                    .is_err()
            );
        }
        let mut tensors = Tensors::default();
        assert!(
            tensors
                .observe(
                    "recognizer-input",
                    &[1, 3, 32, 320],
                    &vec![0.0; 30_720],
                    4401
                )
                .is_err()
        );
        assert!(
            tensors
                .observe(
                    "recognizer-input",
                    &[1, 3, 48, 320],
                    &vec![1.1; 46_080],
                    4401
                )
                .is_err()
        );
        tensors
            .observe("recognizer-output", &[1, 1, 4401], &vec![0.0; 4401], 4401)
            .unwrap();
        assert!(
            tensors
                .observe("recognizer-output", &[1, 1, 4401], &vec![0.0; 4401], 4401)
                .is_err()
        );
        assert!(tensors.complete().is_err());
    }
}
