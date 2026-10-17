use crate::ocr_evidence::tensor_summary;
use nvsf_app::{Paths, inference::Ocr};
use serde_json::{Value, json};
use std::{error::Error, path::Path, time::Instant};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn cpu_profile(bytes: &[u8]) -> Result<usize> {
    let events: Vec<Value> = serde_json::from_slice(bytes)?;
    let mut nodes = 0;
    for event in events.iter().filter(|event| event["cat"] == "Node") {
        if event["args"]["provider"] != "CPUExecutionProvider" {
            return Err("CPU replay executed a missing or non-CPU node provider".into());
        }
        nodes += 1;
    }
    if nodes == 0 {
        return Err("CPU replay profile contains no actual node execution".into());
    }
    Ok(nodes)
}

fn read_tensor(path: &Path, shape: &[usize]) -> Result<Vec<f32>> {
    let count = shape
        .iter()
        .try_fold(1usize, |n, &d| n.checked_mul(d))
        .filter(|&n| n > 0 && n <= 8_000_000)
        .ok_or("Replay input shape exceeds budget")?;
    if std::fs::metadata(path)?.len() != (count * 4) as u64 {
        return Err("Replay input byte length differs from shape".into());
    }
    let bytes = std::fs::read(path)?;
    if bytes.len() != count * 4 {
        return Err("Replay input changed during read".into());
    }
    let values = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect::<Vec<_>>();
    if values.iter().any(|v| !v.is_finite()) {
        return Err("Nonfinite replay input".into());
    }
    Ok(values)
}

pub fn capture(
    ocr: &mut Ocr,
    paths: &Paths,
    output: &Path,
    started: Instant,
    prior_tensor_bytes: usize,
) -> Result<()> {
    let mut report = json!({"schema":1,"source_sha":std::env::var("GITHUB_SHA")?,"target":std::env::var("NVSF_TARGET_ID")?,"status":"capturing","experiments":[]});
    let root = output.join("replay");
    std::fs::create_dir(&root)?;
    // The same evidenced source coordinates isolate recognition from detector-box variation.
    let roi = image::open(output.join("native-roi.png"))?.into_rgb8();
    if roi.width() != 640 || roi.height() != 108 {
        return Err("Unexpected replay ROI".into());
    }
    let crop = image::imageops::crop_imm(&roi, 216, 35, 209, 35).to_image();
    let mut fixed_input = None;
    ocr.recognize_observed(&crop, &mut |stage, shape, values| {
        if stage == "recognizer-input" {
            fixed_input = Some((shape.to_vec(), values.to_vec()));
        }
        Ok(())
    })?;
    let (rec_shape, rec_values) =
        fixed_input.ok_or("Missing actual fixed-crop recognizer input")?;
    let det_shape = [1, 3, 160, 960];
    let det_values = read_tensor(&output.join("native-0-detector-input.f32le"), &det_shape)?;
    let mut bytes_written = prior_tensor_bytes;
    for (model, shape, values) in [
        ("det", det_shape.as_slice(), det_values.as_slice()),
        ("en-ja", rec_shape.as_slice(), rec_values.as_slice()),
    ] {
        let input = tensor_summary("input", shape, values)?;
        if model == "en-ja" {
            bytes_written = bytes_written
                .checked_add(values.len() * 4)
                .ok_or("Replay input byte overflow")?;
            if bytes_written > 64 * 1024 * 1024 {
                return Err("Replay exceeds shared tensor budget".into());
            }
            let bytes = values
                .iter()
                .flat_map(|v| v.to_bits().to_le_bytes())
                .collect::<Vec<_>>();
            std::fs::write(root.join("fixed-recognizer-input.f32le"), bytes)?;
        }
        for disable in [false, true] {
            if started.elapsed().as_secs() >= 90 {
                return Err("Replay exceeded shared OCR deadline".into());
            }
            let label = format!("{model}-{}", if disable { "disabled" } else { "default" });
            let mut result = Value::Null;
            let (profile, runtime) = ocr.replay_observed(
                paths,
                model,
                (shape, values),
                disable,
                &root.join(&label),
                &mut |stage, dimensions, tensor| {
                    if started.elapsed().as_secs() >= 90 {
                        return Err(nvsf_app::fail("Replay exceeded shared OCR deadline"));
                    }
                    let summary = tensor_summary(stage, dimensions, tensor)
                        .map_err(|e| nvsf_app::fail(e.to_string()))?;
                    bytes_written = bytes_written
                        .checked_add(tensor.len() * 4)
                        .ok_or_else(|| nvsf_app::fail("Replay byte overflow"))?;
                    if bytes_written > 64 * 1024 * 1024 {
                        return Err(nvsf_app::fail("Replay exceeds shared64MiB tensor budget"));
                    }
                    let mut bytes = Vec::with_capacity(tensor.len() * 4);
                    for value in tensor {
                        bytes.extend_from_slice(&value.to_bits().to_le_bytes());
                    }
                    std::fs::write(root.join(format!("{label}.f32le")), bytes)?;
                    result = summary;
                    Ok(())
                },
            )?;
            if started.elapsed().as_secs() >= 90 {
                return Err("Replay exceeded shared OCR deadline".into());
            }
            let profile = Path::new(&profile).canonicalize()?;
            if !profile.starts_with(root.canonicalize()?)
                || profile.metadata()?.len() > 8 * 1024 * 1024
            {
                return Err("Replay profile escaped its owned root/byte budget".into());
            }
            let profile_bytes = std::fs::read(&profile)?;
            if profile_bytes.len() > 8 * 1024 * 1024 {
                return Err("Replay profile changed beyond its byte budget".into());
            }
            let cpu_nodes = cpu_profile(&profile_bytes)?;
            report["experiments"].as_array_mut().ok_or("Missing replay experiments")?.push(json!({"model":model,"input":input,"optimization":if disable{"disabled"}else{"runtime-default"},"output":result,"profile":profile.file_name().and_then(|s|s.to_str()),"runtime_build":runtime,"provider":"CPUExecutionProvider","executed_nodes":cpu_nodes}));
            std::fs::write(
                root.join("report.json"),
                serde_json::to_vec_pretty(&report)?,
            )?;
            println!("Completed generated {label} tensor replay; production policy unchanged");
        }
    }
    if started.elapsed().as_secs() >= 90 {
        return Err("Replay exceeded shared OCR deadline".into());
    }
    let runtime = paths.runtime(if cfg!(windows) {
        "onnxruntime.dll"
    } else if cfg!(target_os = "macos") {
        "libonnxruntime.dylib"
    } else {
        "libonnxruntime.so"
    })?;
    report["runtime_sha256"] = json!(crate::fixture::sha(&runtime)?);
    if started.elapsed().as_secs() >= 90 {
        return Err("Replay exceeded shared OCR deadline".into());
    }
    report["status"] = json!("captured; optimization and provider diagnosis only");
    std::fs::write(
        root.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cpu_profile_requires_actual_cpu_nodes_and_rejects_implicit_acceleration() {
        assert_eq!(
            cpu_profile(br#"[{"cat":"Node","args":{"provider":"CPUExecutionProvider"}}]"#).unwrap(),
            1
        );
        for bytes in [
            b"[]".as_slice(),
            br#"[{"cat":"Node","args":{}}]"#,
            br#"[{"cat":"Node","args":{"provider":"CPUExecutionProvider"}},{"cat":"Node","args":{"provider":"CoreMLExecutionProvider"}}]"#,
            b"{}",
        ] {
            assert!(cpu_profile(bytes).is_err());
        }
    }

    #[test]
    fn replay_input_requires_exact_finite_little_endian_values() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("input.f32le");
        std::fs::write(&path, [0, 0, 128, 63]).unwrap();
        assert_eq!(read_tensor(&path, &[1]).unwrap(), vec![1.0]);
        assert!(read_tensor(&path, &[2]).is_err());
        assert!(read_tensor(&path, &[usize::MAX, 2]).is_err());
        std::fs::write(&path, [0, 0, 192, 127]).unwrap();
        assert!(read_tensor(&path, &[1]).is_err());
    }
}
