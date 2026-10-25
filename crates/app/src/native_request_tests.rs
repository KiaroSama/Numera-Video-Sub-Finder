use super::*;

#[test]
fn request_policy_rejects_invalid_time_language_and_region() {
    assert!(
        Operation::Preview {
            at_ms: 360_000_000_000
        }
        .validate()
        .is_err()
    );
    assert!(
        Operation::Suggest {
            language: "../../eng".into()
        }
        .validate()
        .is_err()
    );
    assert!(
        Operation::Recognize {
            at_ms: 0,
            region: Rect {
                x: -1.0,
                ..Rect::default()
            },
            language: "en".into(),
        }
        .validate()
        .is_err()
    );
    assert!(
        Operation::Recognize {
            at_ms: 100,
            region: Rect::default(),
            language: "fa".into(),
        }
        .validate()
        .is_ok()
    );
}

#[test]
fn responses_are_typed_bounded_and_preserve_unreadable_failure() {
    let operation = Operation::Preview { at_ms: 0 };
    assert!(
        operation
            .accepts(&Response::Suggest(vec![Rect::default()]))
            .is_err()
    );
    assert!(
        operation
            .accepts(&Response::Preview {
                width: 7680,
                height: 4320,
                bytes: 8_388_608
            })
            .is_ok()
    );
    for response in [
        Response::Preview {
            width: 0,
            height: 1,
            bytes: 1,
        },
        Response::Preview {
            width: 7681,
            height: 4320,
            bytes: 1,
        },
        Response::Preview {
            width: 1,
            height: 1,
            bytes: 8_388_609,
        },
        Response::Failed("Unreadable; previous text preserved".into()),
    ] {
        assert!(operation.accepts(&response).is_err());
    }
    let recognize = Operation::Recognize {
        at_ms: 0,
        region: Rect::default(),
        language: "ja".into(),
    };
    for text in [
        Recognition {
            text: "".into(),
            confidence: None,
            crop: None,
        },
        Recognition {
            text: "字幕".into(),
            confidence: Some(f32::NAN),
            crop: None,
        },
        Recognition {
            text: "字幕".into(),
            confidence: Some(1.1),
            crop: None,
        },
        Recognition {
            text: "字幕".into(),
            confidence: None,
            crop: Some("foreign".into()),
        },
    ] {
        assert!(recognize.accepts(&Response::Recognize(text)).is_err());
    }
    assert!(
        recognize
            .accepts(&Response::Recognize(Recognition {
                text: "字幕\nسلام".into(),
                confidence: Some(0.75),
                crop: None,
            }))
            .is_ok()
    );
}

#[test]
fn private_payload_read_is_bounded_and_schema_rejects_foreign_authority() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("bounded.json");
    std::fs::write(&file, b"12345").unwrap();
    assert_eq!(read_bounded(&file, 5).unwrap(), b"12345");
    assert!(read_bounded(&file, 4).is_err());
    assert!(
        serde_json::from_str::<Operation>(r#"{"operation":"Preview","at_ms":0,"command":"sh"}"#)
            .is_err()
    );
    assert!(
        serde_json::from_str::<Response>(
            r#"{"result":"Preview","data":{"width":1,"height":1,"bytes":1},"command":"sh"}"#
        )
        .is_err()
    );
}
