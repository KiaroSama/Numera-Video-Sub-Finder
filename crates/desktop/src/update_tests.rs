use crate::{model_manager, state::Host, updates};
use nvsf_core::maintenance::Preferences;
use std::sync::atomic::Ordering;

#[test]
fn disabled_automatic_check_returns_local_status_without_release_request() {
    let directory = tempfile::tempdir().unwrap();
    let paths = nvsf_app::Paths::new(Some(directory.path())).unwrap();
    nvsf_app::updates::save_preferences(
        &paths,
        &Preferences {
            enabled: false,
            last_attempt: None,
            ..Default::default()
        },
    )
    .unwrap();
    let log = nvsf_app::logging::Log::new(&paths, "nvsf-desktop");
    let log_path = log.path.clone().unwrap();
    let host = Host::new(paths, log);
    let startup_log = std::fs::read_to_string(&log_path).unwrap();
    assert_eq!(
        startup_log.matches("[INFO] [APP] Startup version=").count(),
        1
    );
    assert_eq!(std::fs::read_dir(&host.0.paths.logs).unwrap().count(), 1);
    let status = updates::check(&host, false).unwrap();
    assert!(!status.enabled);
    assert_eq!(status.state, "idle");
    assert!(status.token.is_none());
    assert_eq!(status.downloaded, 0);
    assert!(
        nvsf_app::updates::preferences(&host.0.paths)
            .unwrap()
            .last_attempt
            .is_none()
    );
}

#[test]
fn shared_resource_reservation_refuses_source_closing_and_concurrent_operations() {
    let directory = tempfile::tempdir().unwrap();
    let paths = nvsf_app::Paths::new(Some(directory.path())).unwrap();
    let log = nvsf_app::logging::Log::new(&paths, "nvsf-desktop");
    let host = Host::new(paths, log);
    host.0.source_busy.store(true, Ordering::Release);
    assert!(model_manager::begin(&host).is_err());
    host.0.source_busy.store(false, Ordering::Release);
    host.0.closing.store(true, Ordering::Release);
    assert!(model_manager::begin(&host).is_err());
    host.0.closing.store(false, Ordering::Release);
    host.0.maintenance.installing.store(true, Ordering::Release);
    assert!(host.ensure_idle().is_err());
    assert!(model_manager::begin(&host).is_err());
    assert!(!host.shutdown());
    host.0
        .maintenance
        .installing
        .store(false, Ordering::Release);
    let operation = model_manager::begin(&host).unwrap();
    assert!(model_manager::begin(&host).is_err());
    assert!(
        !host.shutdown(),
        "active resource transfer must block exit and request cancellation"
    );
    assert!(host.0.model_cancel.load(Ordering::Acquire));
    drop(operation);
    assert!(!host.0.model_busy.load(Ordering::Acquire));
    assert!(host.ensure_idle().is_ok());
}
