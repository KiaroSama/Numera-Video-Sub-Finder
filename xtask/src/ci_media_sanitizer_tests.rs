use super::{artifact_contract, test_result};

#[test]
fn cargo_records_allow_non_json_build_output_only() {
    artifact_contract(None).expect("actual mixed-output contract");
}

#[test]
fn sanitizer_summary_rejects_duplicate_runs_and_unexpected_filtering() {
    let text = "test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 9 filtered out; finished in 0.1s";
    assert_eq!(
        test_result("callbacks", text, Some(3)).expect("callbacks"),
        (3, 9)
    );
    assert!(test_result("geometry", text, None).is_err());
    assert!(test_result("callbacks", &format!("{text}\n{text}"), Some(3)).is_err());

    let native = "test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.1s";
    assert_eq!(
        test_result("native-abi-geometry", native, Some(20)).unwrap(),
        (20, 0)
    );
    let stale = "test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.1s";
    assert!(test_result("native-abi-geometry", stale, Some(20)).is_err());
}
