use nvsf_core::maintenance::{Preferences, authorize_install, newer_stable};

#[test]
fn automatic_checks_are_daily_but_manual_checks_ignore_cadence() {
    let mut preferences = Preferences::default();
    assert!(preferences.due(100, false));
    preferences.last_attempt = Some(100);
    assert!(!preferences.due(86_499, false));
    assert!(preferences.due(86_500, false));
    assert!(preferences.due(101, true));
    preferences.enabled = false;
    assert!(!preferences.due(200_000, false));
    assert!(preferences.due(101, true));
    preferences.enabled = true;
    assert!(!preferences.due(99, false));
}

#[test]
fn only_newer_stable_versions_can_be_offered() {
    assert!(newer_stable("0.1.0", "v0.2.0").unwrap());
    for version in ["0.1.0", "0.0.9", "0.2.0-alpha.1", "0.1.0+rebuilt"] {
        assert!(!newer_stable("0.1.0", version).unwrap());
    }
    for (current, offered) in [
        ("0.1.0+first", "0.1.0+second"),
        ("0.1.0+second", "0.1.0+first"),
        ("0.1.1+first", "0.1.0+second"),
        ("0.1.0+first", "0.1.0"),
    ] {
        assert!(!newer_stable(current, offered).unwrap());
    }
    for (current, offered) in [
        ("0.1.0+second", "0.1.1+first"),
        ("0.1.0+second", "0.2.0+first"),
        ("0.1.0+second", "1.0.0+first"),
    ] {
        assert!(newer_stable(current, offered).unwrap());
    }
    assert!(newer_stable("0.1.0-alpha.1", "0.1.0").unwrap());
    assert!(newer_stable("0.1.0", "not a version").is_err());
}

#[test]
fn consent_never_overrides_busy_dirty_or_unsupported_installation() {
    assert!(authorize_install(true, false, false, true).is_ok());
    for (consent, busy, dirty, supported) in [
        (false, false, false, true),
        (true, true, false, true),
        (true, false, true, true),
        (true, false, false, false),
    ] {
        assert!(authorize_install(consent, busy, dirty, supported).is_err());
    }
}
