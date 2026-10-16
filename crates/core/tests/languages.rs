use nvsf_core::{ExtractConfig, languages};
use std::collections::HashSet;

#[test]
fn official_catalog_is_complete_unique_and_pinned() {
    let catalog = languages::catalog();
    assert_eq!(catalog.len(), 123);
    let mut codes = HashSet::new();
    for language in catalog {
        assert!(codes.insert(&language.code));
        assert!(!language.name.trim().is_empty());
        assert!(language.bytes > 0 && language.bytes < 32 * 1024 * 1024);
        assert_eq!(language.sha256.len(), 64);
        assert!(language.sha256.bytes().all(|c| c.is_ascii_hexdigit()));
        assert!(language.url.starts_with("https://raw.githubusercontent.com/tesseract-ocr/tessdata_best/e12c65a915945e4c28e237a9b52bc4a8f39a0cec/"));
        assert!(
            language
                .url
                .ends_with(&format!("/{}.traineddata", language.code))
        );
        assert!(languages::find(&language.code).is_some());
    }
    for helper in ["osd", "equ", "frk"] {
        assert!(languages::find(helper).is_none());
    }
}

#[test]
fn priority_aliases_and_native_codes_share_identity_and_routing() {
    for (alias, code, paddle) in [
        ("en", "eng", Some("en-ja")),
        ("fa", "fas", Some("fa")),
        ("ja", "jpn", Some("en-ja")),
    ] {
        assert_eq!(languages::find(alias).unwrap().code, code);
        assert_eq!(languages::paddle_model(alias), paddle);
        assert_eq!(languages::paddle_model(code), paddle);
    }
    for code in ["fra", "deu", "rus", "hin", "heb", "tha", "kor"] {
        assert!(languages::find(code).is_some());
        assert_eq!(languages::paddle_model(code), None);
    }
    for code in ["ara", "urd", "pus", "uig", "snd"] {
        assert_eq!(languages::paddle_model(code), Some("fa"));
    }
    assert!(languages::find("jpn_vert").unwrap().vertical);
    assert!(!languages::find("jpn").unwrap().vertical);
}

#[test]
fn language_validation_rejects_paths_expressions_and_unknown_models() {
    for code in [
        "",
        "../eng",
        "script/Latin",
        "eng+fra",
        "unknown",
        "eng\0",
        "EN",
        "osd",
    ] {
        assert!(languages::find(code).is_none(), "{code:?}");
        let config = ExtractConfig {
            input: "in.mp4".into(),
            output: "out.srt".into(),
            language: code.into(),
            ..Default::default()
        };
        assert!(config.validate().is_err(), "{code:?}");
    }
    for code in ["en", "fa", "ja", "fra", "rus", "hin", "chi_sim_vert"] {
        let config = ExtractConfig {
            input: "in.mp4".into(),
            output: "out.srt".into(),
            language: code.into(),
            ..Default::default()
        };
        assert!(config.validate().is_ok(), "{code}");
    }
}
