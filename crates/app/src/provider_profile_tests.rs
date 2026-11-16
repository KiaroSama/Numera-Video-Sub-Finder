use super::*;
use serde_json::json;

fn node(provider: &str, operation: &str, duration: f64) -> serde_json::Value {
    json!({"cat":"Node","ph":"X","name":"private-node_kernel_time","dur":duration,
        "args":{"provider":provider,"op_name":operation}})
}

#[test]
fn both_stages_require_actual_requested_compute_not_registration_or_transfers() {
    let cuda = serde_json::to_vec(&vec![
        node("CUDAExecutionProvider", "Conv", 12.0),
        node("CUDAExecutionProvider", "MemcpyFromHost", 2.0),
    ])
    .unwrap();
    let pair = parse_pair(&cuda, &cuda, "cuda", true).unwrap();
    assert_eq!(pair.detector.requested_compute, 1);
    assert_eq!(pair.recognizer.requested_compute, 1);
    assert_eq!(pair.detector.transfer_events, 1);
    assert_eq!(pair.detector.providers["CUDAExecutionProvider"], 2);
    assert_eq!(pair.detector.duration_us, 14.0);
    let memcpy =
        serde_json::to_vec(&vec![node("CUDAExecutionProvider", "MemcpyToHost", 1.0)]).unwrap();
    for absent in [b"[]".as_slice(), memcpy.as_slice()] {
        assert!(parse_pair(&cuda, absent, "cuda", true).is_err());
        assert!(parse_pair(absent, &cuda, "cuda", true).is_err());
    }
}

#[test]
fn mixed_placement_is_reported_but_cannot_qualify_strict_gpu() {
    let mixed = serde_json::to_vec(&vec![
        node("CUDAExecutionProvider", "Conv", 1.0),
        node("CPUExecutionProvider", "Shape", 0.0),
    ])
    .unwrap();
    let summary = parse(&mixed, "cuda", false).unwrap();
    assert_eq!(summary.requested_compute, 1);
    assert_eq!(summary.other_compute, 1);
    assert!(parse(&mixed, "cuda", true).is_err());
    let cpu = serde_json::to_vec(&vec![node("CPUExecutionProvider", "Conv", 1.0)]).unwrap();
    assert!(parse(&cpu, "cuda", false).is_err());
    assert_eq!(parse(&cpu, "cpu", true).unwrap().requested_compute, 1);
}

#[test]
fn profile_validation_fails_closed_on_missing_malformed_and_oversized_evidence() {
    for events in [
        json!([]),
        json!([{"cat":"Session","ph":"X","dur":1}]),
        json!([{"cat":"Node","ph":"X","dur":1,"args":{"op_name":"Conv"}}]),
        json!([{"cat":"Node","ph":"X","dur":1,"args":{"provider":"CUDAExecutionProvider"}}]),
        json!([{"cat":"Node","ph":"B","dur":1,"args":{"provider":"CUDAExecutionProvider","op_name":"Conv"}}]),
        json!([node("UnknownExecutionProvider", "Conv", 1.0)]),
        json!([node("CUDAExecutionProvider", "Conv", -1.0)]),
        json!([{"cat":"Node","ph":"X","dur":"NaN","args":{"provider":"CUDAExecutionProvider","op_name":"Conv"}}]),
        json!([{"cat":"Node","ph":"X","dur":1,"args":{"provider":"CUDAExecutionProvider","op_name":""}}]),
    ] {
        assert!(parse(&serde_json::to_vec(&events).unwrap(), "cuda", true).is_err());
    }
    for bytes in [b"{}".as_slice(), b"not JSON", b"[NaN]"] {
        assert!(parse(bytes, "cuda", true).is_err());
    }
    assert!(parse(&vec![b' '; PROFILE_LIMIT + 1], "cuda", true).is_err());
    let events = vec![json!(null); EVENT_LIMIT + 1];
    assert!(parse(&serde_json::to_vec(&events).unwrap(), "cuda", true).is_err());
    assert!(parse(b"[]", "unadmitted", false).is_err());
}

#[test]
fn profile_paths_must_stay_in_the_reserved_directory_and_bound_bytes() {
    let root = tempfile::tempdir().unwrap();
    let owned = root.path().join("profiles");
    let canonical = reserve(&owned).unwrap();
    assert!(reserve(&owned).is_err());
    let inside = canonical.join("detector.json");
    std::fs::write(&inside, b"[]").unwrap();
    assert_eq!(
        profile_path(&canonical, inside.to_str().unwrap()).unwrap(),
        inside
    );
    let outside = root.path().join("elsewhere.json");
    std::fs::write(&outside, b"[]").unwrap();
    assert!(profile_path(&canonical, outside.to_str().unwrap()).is_err());
    std::fs::OpenOptions::new()
        .write(true)
        .open(&inside)
        .unwrap()
        .set_len(PROFILE_LIMIT as u64 + 1)
        .unwrap();
    assert!(profile_path(&canonical, inside.to_str().unwrap()).is_err());
    #[cfg(unix)]
    {
        let link = canonical.join("linked.json");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        assert!(profile_path(&canonical, link.to_str().unwrap()).is_err());
    }
}
