use super::*;
use std::io::Cursor;
use std::sync::atomic::AtomicU32;

static SEQ: AtomicU32 = AtomicU32::new(0);

fn temp_paths() -> Paths {
    let id = SEQ.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("nvsf-upd-test-{}-{id}", std::process::id()));
    Paths::new(Some(&root)).expect("paths")
}

fn sample_offer(token: &str, downloaded: Option<PathBuf>) -> CachedOffer {
    CachedOffer {
        schema: 1,
        token: token.into(),
        version: "0.5.0".into(),
        notes: "Test release notes".into(),
        target: "linux-x86_64-appimage".into(),
        artifact: updates::Artifact {
            url: "https://github.com/KiaroSama/Numera-Video-Sub-Finder/releases/download/v0.5.0/app.AppImage".into(),
            signature: "invalid-sig".into(),
            sha256: "a".repeat(64),
            bytes: 16,
        },
        checked_at: now_secs(),
        downloaded_path: downloaded,
    }
}

#[test]
fn preferences_round_trip_and_updates_config_share_store() {
    let paths = temp_paths();
    let initial = updates::preferences(&paths).expect("default prefs");
    assert!(initial.enabled);
    assert_eq!(initial.schema, 1);

    run_preferences(&paths, PreferencesCommand::Get).expect("get prefs");
    assert!(
        run_preferences(
            &paths,
            PreferencesCommand::Set {
                update_checks: None,
                reset_last_attempt: false,
            },
        )
        .is_err()
    );

    run_preferences(
        &paths,
        PreferencesCommand::Set {
            update_checks: Some(false),
            reset_last_attempt: true,
        },
    )
    .expect("disable via preferences");
    assert!(!updates::preferences(&paths).expect("reloaded").enabled);

    run_updates(
        &paths,
        UpdatesCommand::Config { enabled: true },
        &Control::default(),
    )
    .expect("enable via updates config");
    assert!(updates::preferences(&paths).expect("reloaded 2").enabled);

    let _ = std::fs::remove_dir_all(&paths.root);
}

#[test]
fn updates_download_and_format_validation_fail_closed() {
    let paths = temp_paths();
    for valid in ["portable", "nsis", "appimage", "deb", "rpm"] {
        validate_format(valid).expect("valid format");
    }
    assert!(validate_format("dmg").is_err());

    let err = run_updates(
        &paths,
        UpdatesCommand::Download {
            token: "tok".into(),
            consent: false,
            output: None,
            overwrite: false,
        },
        &Control::default(),
    )
    .expect_err("must require consent");
    assert!(matches!(err, AppError::InvalidConfig(_)));

    // Missing cached offer on consented download fails cleanly.
    assert!(
        run_updates(
            &paths,
            UpdatesCommand::Download {
                token: "missing".into(),
                consent: true,
                output: None,
                overwrite: false,
            },
            &Control::default(),
        )
        .is_err()
    );

    let offer = sample_offer("tok-1", None);
    save_cached_offer(&paths, &offer).expect("save offer");
    let loaded = load_cached_offer(&paths)
        .expect("load offer")
        .expect("some");
    assert_eq!(loaded.token, "tok-1");
    run_updates(
        &paths,
        UpdatesCommand::Status {
            format: "portable".into(),
        },
        &Control::default(),
    )
    .expect("status with offer");

    // Wrong token fails closed.
    assert!(
        run_updates(
            &paths,
            UpdatesCommand::Download {
                token: "wrong-token".into(),
                consent: true,
                output: None,
                overwrite: false,
            },
            &Control::default(),
        )
        .is_err()
    );

    // Cancelled check fails closed before network.
    let cancel = Control::default();
    cancel.cancel.store(true, Ordering::Release);
    assert!(matches!(
        run_updates(
            &paths,
            UpdatesCommand::Check {
                format: "portable".into(),
            },
            &cancel,
        ),
        Err(AppError::Cancelled)
    ));

    // Invalid cached schema fails closed.
    let mut bad = offer.clone();
    bad.schema = 99;
    save_cached_offer(&paths, &bad).expect("write bad schema");
    assert!(load_cached_offer(&paths).is_err());
    clear_cached_offer(&paths);
    assert!(
        load_cached_offer(&paths)
            .expect("none after clear")
            .is_none()
    );

    let _ = std::fs::remove_dir_all(&paths.root);
}

#[test]
fn updates_verify_and_bounded_reader_enforce_integrity_and_limits() {
    let paths = temp_paths();
    let control = Control::default();

    let mut cur = Cursor::new(b"hello world".to_vec());
    let data = read_bounded_response(&mut cur, 32, &control).expect("within limit");
    assert_eq!(data, b"hello world");

    let mut big = Cursor::new(vec![b'x'; 64]);
    assert!(read_bounded_response(&mut big, 16, &control).is_err());

    let cancelled = Control::default();
    cancelled.cancel.store(true, Ordering::Release);
    let mut cur2 = Cursor::new(b"abc".to_vec());
    assert!(matches!(
        read_bounded_response(&mut cur2, 16, &cancelled),
        Err(AppError::Cancelled)
    ));

    // Missing token/file args fail with InvalidConfig.
    assert!(verify_update(&paths, None, None, None, None, None, None).is_err());
    let payload = paths.cache.join("sample.bin");
    std::fs::write(&payload, b"payload-bytes").expect("write payload");
    assert!(
        verify_update(
            &paths,
            None,
            Some(&payload),
            None,
            Some("0.5.0"),
            None,
            None
        )
        .is_err()
    );
    assert!(verify_update(&paths, None, Some(&payload), Some("sig"), None, None, None).is_err());
    // SHA-256 mismatch rejects tampered payload before signature acceptance.
    let err = verify_update(
        &paths,
        None,
        Some(&payload),
        Some("sig"),
        Some("0.5.0"),
        Some(&"0".repeat(64)),
        Some("linux-x86_64-appimage"),
    )
    .expect_err("hash mismatch must fail");
    assert!(err.to_string().contains("SHA-256"));

    // Cached offer without downloaded file fails cleanly; with wrong token fails cleanly.
    let offer = sample_offer("tok-v", None);
    save_cached_offer(&paths, &offer).expect("save offer");
    assert!(verify_update(&paths, Some("other"), None, None, None, None, None).is_err());
    assert!(verify_update(&paths, Some("tok-v"), None, None, None, None, None).is_err());

    let _ = std::fs::remove_dir_all(&paths.root);
}
