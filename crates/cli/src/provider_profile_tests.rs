use super::*;
use crate::{Cli, Commands};
use clap::Parser;
use serde_json::json;

fn absolute_directory() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\profiles\new run")
    } else {
        PathBuf::from("/profiles/new run")
    }
}

fn trace(provider: &str, name: &str, operation: &str) -> Vec<u8> {
    serde_json::to_vec(&json!([{
        "cat": "Node", "ph": "X", "name": name, "dur": 1.0,
        "args": {"provider": provider, "op_name": operation}
    }]))
    .unwrap_or_else(|error| panic!("{error}"))
}

fn files() -> ProfileFiles {
    let root = absolute_directory();
    ProfileFiles {
        detector: root.join("detector.json"),
        recognizer: root.join("recognizer.json"),
        runtime_build: "ORT 1.31.0 fixture build".into(),
    }
}

#[test]
fn profiling_flag_is_opt_in_and_preserves_the_requested_directory() {
    let mut args = vec!["nvsf", "extract", "--input", "a", "--output", "b"];
    let cli = Cli::try_parse_from(&args).unwrap_or_else(|error| panic!("{error}"));
    let Commands::Extract(extract) = cli.command else {
        panic!("Expected extraction arguments");
    };
    assert!(extract.provider_profile.is_none());
    let root = absolute_directory();
    let root = root.to_str().unwrap_or_else(|| panic!("UTF-8 fixture"));
    args.extend(["--device", "cuda", "--provider-profile", root]);
    let cli = Cli::try_parse_from(args).unwrap_or_else(|error| panic!("{error}"));
    let Commands::Extract(extract) = cli.command else {
        panic!("Expected extraction arguments");
    };
    assert_eq!(extract.provider_profile.as_deref(), Some(Path::new(root)));
    assert_eq!(extract.device, "cuda");
}

#[test]
fn profiling_arguments_require_absolute_normal_paths_and_explicit_onnx_device_language() {
    let root = absolute_directory();
    for device in ["cpu", "cuda", "directml"] {
        assert!(validate_options(&root, device, "en").is_ok());
    }
    for path in [
        Path::new("relative"),
        Path::new(""),
        Path::new("../profiles"),
    ] {
        assert!(validate_options(path, "cuda", "en").is_err());
    }
    assert!(validate_options(&root.join(".."), "cuda", "en").is_err());
    assert!(validate_options(&root, "auto", "en").is_err());
    assert!(validate_options(&root, "unknown", "en").is_err());
    assert!(validate_options(&root, "cuda", "unknown").is_err());
    let cpu_language = nvsf_core::languages::catalog()
        .iter()
        .find(|language| nvsf_core::languages::paddle_model(&language.code).is_none())
        .unwrap_or_else(|| panic!("Expected a Tesseract-only language"));
    assert!(validate_options(&root, "cuda", &cpu_language.code).is_err());
}

#[test]
fn record_proves_both_stages_and_contains_no_registration_claim() {
    let detector = br#"[{"cat":"Node","ph":"X","name":"detector/Conv_kernel_time","dur":1.0,"args":{"provider":"CUDAExecutionProvider","op_name":"Conv"}}]"#.to_vec();
    let recognizer = trace(
        "CUDAExecutionProvider",
        "recognizer/MatMul_kernel_time",
        "MatMul",
    );
    let record = build_record("cuda", true, &files(), &detector, &recognizer)
        .unwrap_or_else(|error| panic!("{error}"));
    let json = serde_json::to_value(record).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(json["schema"], 1);
    assert_eq!(json["requested_device"], "cuda");
    assert_eq!(json["strict"], true);
    assert_eq!(json["runtime_build"], "ORT 1.31.0 fixture build");
    for stage in ["detector", "recognizer"] {
        assert_eq!(json[stage]["requested_compute"], 1);
        assert_eq!(json[stage]["providers"]["CUDAExecutionProvider"], 1);
        assert_eq!(json[stage]["compute_providers"]["CUDAExecutionProvider"], 1);
        assert_eq!(json[stage]["cpu_fallback_nodes"], json!([]));
        assert_eq!(json[stage]["cpu_fallback_nodes_omitted"], 0);
        assert_eq!(json[stage]["profile"]["file"], format!("{stage}.json"));
        assert_eq!(
            json[stage]["profile"]["sha256"].as_str().map(str::len),
            Some(64)
        );
    }
    assert_eq!(
        json["detector"]["profile"]["sha256"],
        "66571b84270c1c613008e7f4478d7c1a8a50a9ea9b7913606f2b61b18128d8b8"
    );
    assert_ne!(
        json["detector"]["profile"]["sha256"],
        json["recognizer"]["profile"]["sha256"]
    );
    assert!(json.get("registered_providers").is_none());
}

#[test]
fn wrong_empty_or_transfer_only_profiles_never_prove_required_gpu() {
    let cuda = trace("CUDAExecutionProvider", "gpu_kernel_time", "Conv");
    let cpu = trace("CPUExecutionProvider", "cpu_kernel_time", "Conv");
    let transfer = trace("CUDAExecutionProvider", "copy_kernel_time", "MemcpyToHost");
    for invalid in [
        b"[]".as_slice(),
        b"{}",
        b"not JSON",
        cpu.as_slice(),
        transfer.as_slice(),
    ] {
        assert!(build_record("cuda", true, &files(), &cuda, invalid).is_err());
        assert!(build_record("cuda", true, &files(), invalid, &cuda).is_err());
    }
    assert!(build_record("cuda", false, &files(), &cuda, &cpu).is_err());
    assert!(build_record("cpu", false, &files(), &cpu, &cpu).is_ok());
}

#[test]
fn fallback_names_are_bounded_and_transfers_are_not_compute_fallback() {
    let mut events = vec![json!({
        "cat":"Node", "ph":"X", "name":"gpu_kernel_time", "dur":1,
        "args":{"provider":"CUDAExecutionProvider", "op_name":"Conv"}
    })];
    for index in 0..FALLBACK_LIMIT + 3 {
        events.push(json!({
            "cat":"Node", "ph":"X", "name":format!("cpu-{index}_kernel_time"), "dur":0,
            "args":{"provider":"CPUExecutionProvider", "op_name":"Shape"}
        }));
    }
    events.push(json!({
        "cat":"Node", "ph":"X", "name":"copy_kernel_time", "dur":1,
        "args":{"provider":"CPUExecutionProvider", "op_name":"MemcpyFromHost"}
    }));
    let mixed = serde_json::to_vec(&events).unwrap_or_else(|error| panic!("{error}"));
    let record = build_record("cuda", false, &files(), &mixed, &mixed)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(record.detector.cpu_fallback_nodes.len(), FALLBACK_LIMIT);
    assert_eq!(record.detector.cpu_fallback_nodes_omitted, 3);
    assert_eq!(record.detector.other_compute, FALLBACK_LIMIT + 3);
    assert_eq!(record.detector.transfer_events, 1);
    assert_eq!(
        record.detector.compute_providers["CPUExecutionProvider"],
        FALLBACK_LIMIT + 3
    );
    assert_eq!(
        record.detector.providers["CPUExecutionProvider"],
        FALLBACK_LIMIT + 4
    );
    assert_eq!(record.detector.cpu_fallback_nodes[0], "cpu-0_kernel_time");
    // Strict GPU admits ORT-forced CPU shape operators but not other CPU compute.
    assert!(build_record("cuda", true, &files(), &mixed, &mixed).is_ok());
    let mut compute = events.clone();
    compute[1]["args"]["op_name"] = json!("MatMul");
    let compute = serde_json::to_vec(&compute).unwrap_or_else(|error| panic!("{error}"));
    assert!(build_record("cuda", true, &files(), &compute, &mixed).is_err());
    for invalid_name in [
        json!(null),
        json!(""),
        json!("bad\nname"),
        json!("x".repeat(NAME_LIMIT + 1)),
    ] {
        // Even names omitted by the output cap must remain valid evidence.
        events[FALLBACK_LIMIT + 1]["name"] = invalid_name;
        let malformed = serde_json::to_vec(&events).unwrap_or_else(|error| panic!("{error}"));
        assert!(build_record("cuda", false, &files(), &malformed, &mixed).is_err());
    }
}

#[test]
fn evidence_identity_and_runtime_metadata_fail_closed() {
    let cuda = trace("CUDAExecutionProvider", "gpu_kernel_time", "Conv");
    let valid_files = files();
    let oversized = vec![b' '; profiles::PROFILE_LIMIT + 1];
    assert!(build_record("cuda", true, &valid_files, &oversized, &cuda).is_err());
    let mut files = files();
    files.runtime_build.clear();
    assert!(build_record("cuda", true, &files, &cuda, &cuda).is_err());
    files.runtime_build = "x".repeat(RUNTIME_LIMIT + 1);
    assert!(build_record("cuda", true, &files, &cuda, &cuda).is_err());
    files.runtime_build = "ORT 1.31.0".into();
    files.recognizer = files.detector.clone();
    assert!(build_record("cuda", true, &files, &cuda, &cuda).is_err());
    files.recognizer = absolute_directory().join("other").join("recognizer.json");
    assert!(build_record("cuda", true, &files, &cuda, &cuda).is_err());
    files.recognizer = absolute_directory().join("recognizer.json");
    files.detector = absolute_directory().join("bad\nname.json");
    assert!(build_record("cuda", true, &files, &cuda, &cuda).is_err());
}
