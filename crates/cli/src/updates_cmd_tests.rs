use super::*;
use std::sync::atomic::AtomicU32;

static SEQ: AtomicU32 = AtomicU32::new(0);

fn temp_paths() -> Paths {
    let id = SEQ.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("nvsf-upd-test-{}-{id}", std::process::id()));
    Paths::new(Some(&root)).expect("paths")
}

#[test]
fn preferences_round_trip_and_updates_config_share_store() {
    let paths = temp_paths();
    let initial = updates::preferences(&paths).expect("default prefs");
    assert!(initial.enabled);
    assert_eq!(initial.schema, 1);

    run_preferences(
        &paths,
        PreferencesCommand::Set {
            update_checks: Some(false),
            reset_last_attempt: false,
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
fn updates_download_fails_closed_without_consent() {
    let paths = temp_paths();
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
    let _ = std::fs::remove_dir_all(&paths.root);
}
