use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU32, Ordering},
};

static NEXT: AtomicU32 = AtomicU32::new(0);

fn root() -> PathBuf {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("nvsf-cli-{}-{id}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(path.join("data")).unwrap();
    path
}

fn nvsf(root: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nvsf"))
        .arg("--data-root")
        .arg(root.join("data"))
        .args(arguments)
        .output()
        .unwrap()
}

fn project(root: &Path, input: &str, completed: bool) -> PathBuf {
    let output = root.join("video.srt");
    let config = json!({
        "input": input,
        "output": output.to_str().unwrap(),
        "data_root": null,
        "language": "en",
        "device": "cpu",
        "quality": "balanced",
        "regions": [{"x":0.0,"y":0.0,"width":1.0,"height":1.0}],
        "exclusions": [],
        "start_ms": 0,
        "end_ms": null,
        "threads": 2,
        "overwrite": false,
        "normalize_persian": false,
        "min_confidence": 0.7,
        "gpu_required": false,
        "accelerator_required": false
    });
    let value = json!({
        "schema": 1,
        "source_hash": "a".repeat(64),
        "config_hash": "b".repeat(64),
        "model_hash": "c".repeat(64),
        "config": config,
        "media": {"width":1920,"height":1080,"duration_ms":10000,"codec":"h264",
                  "time_base_num":1,"time_base_den":1000,"subtitle_streams":0,"rotation":0},
        "cues": [{"id":1,"track":0,"start_ms":0,"end_ms":1000,
                  "text":"ساخته 日本","raw_text":"ساخته 日本","confidence":null,
                  "reviewed":false,"edited":true,"crop":null}],
        "processed_ms": 1500,
        "active_appearances": [],
        "completed": completed,
        "warnings": []
    });
    let path = root.join("video.nvsf.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    path
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn doctor_and_dependencies_report_actual_environment_without_network() {
    let root = root();
    let output = nvsf(&root, &["doctor"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let doctor: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(doctor["os"], std::env::consts::OS);
    assert_eq!(doctor["architecture"], std::env::consts::ARCH);
    assert!(
        doctor["dependencies"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty())
    );
    assert!(
        doctor["acceleration_qualification"]
            .as_str()
            .unwrap()
            .contains("not established")
    );

    let output = nvsf(&root, &["dependencies"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let rows: Value = serde_json::from_slice(&output.stdout).unwrap();
    let rows = rows.as_array().unwrap();
    assert!(!rows.is_empty());
    assert!(rows.iter().all(|row| row["component"].is_string()
        && row["state"].is_string()
        && row["message"].is_string()));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn dependency_repair_without_consent_fails_closed() {
    let root = root();
    let output = nvsf(&root, &["dependencies", "--repair"]);
    assert_eq!(output.status.code(), Some(4));
    assert!(stderr(&output).contains("consent"));
    assert!(std::fs::read_dir(root.join("data")).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("runtime")
    }));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn listings_come_from_the_committed_manifests() {
    let root = root();
    let output = nvsf(&root, &["models", "list"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let models: Value = serde_json::from_slice(&output.stdout).unwrap();
    let ids: Vec<_> = models
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids, ["det", "fa", "en-ja", "ja-specialist"]);
    assert!(
        models
            .as_array()
            .unwrap()
            .iter()
            .all(|row| !row["verified"].as_bool().unwrap_or(false))
    );

    let output = nvsf(&root, &["languages", "list"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let languages: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(languages.as_array().unwrap().len(), 123);
    assert!(
        languages
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["code"] == "jpn")
            .unwrap()["primary"]
            .as_str()
            .unwrap()
            .contains("Paddle")
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn unknown_or_missing_resources_fail_with_precise_messages() {
    let root = root();
    let output = nvsf(&root, &["models", "import", "unknown", "x.onnx"]);
    assert_eq!(output.status.code(), Some(4));
    assert!(stderr(&output).contains("Unknown model identifier"));

    let output = nvsf(&root, &["languages", "verify", "fra"]);
    assert_eq!(output.status.code(), Some(4));
    assert!(stderr(&output).contains("explicitly install or import"));

    let output = nvsf(&root, &["languages", "verify", "../eng"]);
    assert_eq!(output.status.code(), Some(4));
    assert!(stderr(&output).contains("official catalog"));

    let output = nvsf(&root, &["export", "missing.nvsf.json", "out.srt"]);
    assert_eq!(output.status.code(), Some(4));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn batch_rejects_a_missing_output_directory_before_extraction() {
    let root = root();
    let output = nvsf(&root, &["batch", "a.mp4", "--output-dir", "missing"]);
    assert_eq!(output.status.code(), Some(4));
    assert!(stderr(&output).contains("Batch output directory must exist"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn export_writes_the_saved_corrections_and_refuses_silent_overwrite() {
    let root = root();
    let checkpoint = project(&root, "/media/input.mp4", false);
    let srt = root.join("exported.srt");
    let output = nvsf(
        &root,
        &[
            "export",
            checkpoint.to_str().unwrap(),
            srt.to_str().unwrap(),
        ],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let text = std::fs::read_to_string(&srt).unwrap();
    assert!(text.contains("ساخته 日本"));
    assert!(text.contains("00:00:00,000 --> 00:00:01,000"));

    let output = nvsf(
        &root,
        &[
            "export",
            checkpoint.to_str().unwrap(),
            srt.to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(4));
    assert!(stderr(&output).contains("existing or unsafe destination"));
    assert_eq!(std::fs::read_to_string(&srt).unwrap(), text);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn resume_rejects_relative_checkpoint_paths_instead_of_guessing() {
    let root = root();
    let checkpoint = project(&root, root.join("video.srt").to_str().unwrap(), false);
    let output = nvsf(&root, &["resume", checkpoint.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(3));
    assert!(stderr(&output).contains("distinct input/output paths required"));
    std::fs::remove_dir_all(root).unwrap();
}
