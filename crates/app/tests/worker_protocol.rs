use nvsf_app::worker::{Event, protocol};

#[test]
fn worker_messages_require_the_expected_version_and_job_identity() {
    let bytes = protocol::encode("job-17", &Event::Failed("خطا".into()), 1024).unwrap();
    let event: Event = protocol::decode(&bytes, "job-17", 1024).unwrap();
    assert!(matches!(event, Event::Failed(text) if text == "خطا"));
    for text in [
        r#"{"schema":2,"job_id":"job-17","payload":{"type":"Failed","data":"x"}}"#,
        r#"{"schema":1,"job_id":"job-18","payload":{"type":"Failed","data":"x"}}"#,
        r#"{"schema":1,"job_id":"job-17","extra":true,"payload":{"type":"Failed","data":"x"}}"#,
        r#"{"type":"Failed","data":"legacy"}"#,
    ] {
        assert!(protocol::decode::<Event>(text.as_bytes(), "job-17", 1024).is_err());
    }
    for job in ["", "../other", "line\nbreak"] {
        assert!(protocol::encode(job, &Event::Failed(String::new()), 1024).is_err());
    }
}

#[test]
fn typed_failures_preserve_categories_without_parsing_messages() {
    use nvsf_app::AppError;
    for error in [
        AppError::Provider("provider detail".into()),
        AppError::Cancelled,
        AppError::InvalidConfig("configuration detail".into()),
        nvsf_app::fail("Required provider unavailable: not a category"),
    ] {
        let expected = std::mem::discriminant(&error);
        let event = Event::Failure(protocol::Failure::from_error(&error));
        let bytes = protocol::encode("job-17", &event, 4096).unwrap();
        let Event::Failure(decoded) = protocol::decode(&bytes, "job-17", 4096).unwrap() else {
            panic!("Typed failure was lost");
        };
        assert_eq!(std::mem::discriminant(&decoded.into_error()), expected);
    }
    let unknown = br#"{"schema":1,"job_id":"job-17","payload":{"type":"Failure","data":{"kind":"unknown","message":"x"}}}"#;
    assert!(protocol::decode::<Event>(unknown, "job-17", 4096).is_err());
}

#[test]
fn bounded_codec_rejects_oversized_serialization_and_reads() {
    let bytes = protocol::encode("job-17", &"literal", 1024).unwrap();
    assert!(protocol::encode("job-17", &"literal", bytes.len()).is_ok());
    assert!(protocol::encode("job-17", &"literal", bytes.len() - 1).is_err());
    assert!(protocol::decode::<String>(&bytes, "job-17", bytes.len() - 1).is_err());
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("control.json");
    std::fs::write(&file, &bytes).unwrap();
    assert!(protocol::read::<String>(&file, "job-17", bytes.len() - 1).is_err());
    assert_eq!(
        protocol::read::<String>(&file, "job-17", bytes.len()).unwrap(),
        "literal"
    );
    assert!(!protocol::flag(dir.path(), "job-17", "pause").unwrap());
    let flag = dir.path().join("pause");
    std::fs::write(&flag, protocol::encode("job-17", &"pause", 1024).unwrap()).unwrap();
    assert!(protocol::flag(dir.path(), "job-17", "pause").unwrap());
    assert!(protocol::flag(dir.path(), "other-job", "pause").is_err());
    std::fs::write(&flag, protocol::encode("job-17", &"cancel", 1024).unwrap()).unwrap();
    assert!(protocol::flag(dir.path(), "job-17", "pause").is_err());
    std::fs::write(&flag, b"pause").unwrap();
    assert!(protocol::flag(dir.path(), "job-17", "pause").is_err());
    std::fs::write(
        &flag,
        br#"{"schema":2,"job_id":"job-17","payload":"pause"}"#,
    )
    .unwrap();
    assert!(protocol::flag(dir.path(), "job-17", "pause").is_err());
    std::fs::write(&flag, vec![b'x'; 1025]).unwrap();
    assert!(protocol::flag(dir.path(), "job-17", "pause").is_err());
}
