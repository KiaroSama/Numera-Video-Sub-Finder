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
    let status = tauri::async_runtime::block_on(updates::check(&host, false)).unwrap();
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
fn native_close_cancels_pending_metadata_before_reporting_shutdown_complete() {
    use std::{future::pending, sync::Arc, time::Duration};

    struct ResponseLease(Arc<std::sync::atomic::AtomicBool>);
    impl Drop for ResponseLease {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    let directory = tempfile::tempdir().unwrap();
    let paths = nvsf_app::Paths::new(Some(directory.path())).unwrap();
    let log = nvsf_app::logging::Log::new(&paths, "nvsf-desktop");
    let host = Host::new(paths, log);
    let (started, ready) = std::sync::mpsc::sync_channel(1);
    let released = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let lease = ResponseLease(released.clone());
    let checking_host = host.clone();
    let mut check = tauri::async_runtime::spawn(async move {
        updates::check_with(&checking_host, false, async move {
            let _lease = lease;
            started.send(()).unwrap();
            pending().await
        })
        .await
    });
    let began = ready.recv_timeout(Duration::from_secs(2));
    let concurrent = tauri::async_runtime::block_on(updates::check_with(&host, true, async {
        panic!("an active metadata request must prevent a second network operation")
    }));
    let close_admitted = host.begin_close();
    let stopped = host.shutdown();
    let released_at_shutdown = released.load(Ordering::Acquire);
    let result = tauri::async_runtime::block_on(async {
        tokio::time::timeout(Duration::from_secs(2), &mut check).await
    });
    if result.is_err() {
        check.abort();
        tauri::async_runtime::block_on(async {
            let _ = tokio::time::timeout(Duration::from_secs(2), check).await;
        });
    }
    assert!(
        began.is_ok(),
        "metadata request must actually be pending before close"
    );
    assert!(close_admitted.unwrap());
    assert!(
        stopped,
        "ordinary close must cancel and finish metadata discovery"
    );
    assert!(
        released_at_shutdown,
        "response must be dropped before shutdown reports success"
    );
    assert_eq!(concurrent.unwrap().state, "checking");
    let status = result.unwrap().unwrap().unwrap();
    assert!(status.token.is_none());
    host.0.closing.store(false, Ordering::Release);
    tauri::async_runtime::block_on(updates::check_with(&host, true, async {
        panic!("shutdown must prevent a late manual request from reaching the network")
    }))
    .unwrap();
}

#[test]
fn native_close_joins_the_owned_hourly_scheduler() {
    let directory = tempfile::tempdir().unwrap();
    let paths = nvsf_app::Paths::new(Some(directory.path())).unwrap();
    nvsf_app::updates::save_preferences(
        &paths,
        &Preferences {
            enabled: false,
            ..Default::default()
        },
    )
    .unwrap();
    let log = nvsf_app::logging::Log::new(&paths, "nvsf-desktop");
    let host = Host::new(paths, log);
    // Admit close before the scheduler can contend for the workspace operation gate.
    assert!(host.begin_close().unwrap());
    updates::schedule_host(&host);
    assert!(host.shutdown());
    assert!(
        host.shutdown(),
        "stopping an already joined scheduler is idempotent"
    );
}

#[test]
fn discovered_release_is_only_an_offer_and_a_failed_check_clears_it() {
    let directory = tempfile::tempdir().unwrap();
    let paths = nvsf_app::Paths::new(Some(directory.path())).unwrap();
    let log = nvsf_app::logging::Log::new(&paths, "nvsf-desktop");
    let host = Host::new(paths, log);
    let mut platforms = std::collections::BTreeMap::new();
    for os in ["windows", "linux", "darwin"] {
        for format in ["portable", "nsis", "msi", "app", "appimage", "deb", "rpm"] {
            platforms.insert(
                format!("{os}-{}-{format}", std::env::consts::ARCH),
                nvsf_app::updates::Artifact {
                    url: "https://github.com/KiaroSama/Numera-Video-Sub-Finder/releases/download/v0.2.0/app.bin".into(),
                    signature: "fixture-not-an-authenticated-signature".into(),
                    sha256: "a".repeat(64),
                    bytes: 123,
                },
            );
        }
    }
    let manifest = nvsf_app::updates::Manifest {
        version: "0.2.0".into(),
        notes: "Fixture offer".into(),
        platforms,
    };
    let status = tauri::async_runtime::block_on(updates::check_with(&host, true, async {
        Ok(Some(manifest))
    }))
    .unwrap();
    assert_eq!(status.state, "available");
    assert_eq!(status.version.as_deref(), Some("0.2.0"));
    assert!(status.token.is_some());
    assert_eq!(status.total, 123);
    assert_eq!(status.downloaded, 0);
    assert!(
        nvsf_app::updates::preferences(&host.0.paths)
            .unwrap()
            .last_attempt
            .is_some()
    );
    assert_eq!(std::fs::read_dir(&host.0.paths.cache).unwrap().count(), 0);
    let skipped = tauri::async_runtime::block_on(updates::check_with(&host, false, async {
        panic!("daily cadence must not poll the release request")
    }))
    .unwrap();
    assert_eq!(skipped.token, status.token);
    let failed = tauri::async_runtime::block_on(updates::check_with(&host, true, async {
        Err(nvsf_app::fail("controlled metadata failure"))
    }))
    .unwrap();
    assert_eq!(failed.state, "error");
    assert!(failed.token.is_none());
    assert!(failed.message.contains("controlled metadata failure"));
    assert_eq!(failed.downloaded, 0);
    let current =
        tauri::async_runtime::block_on(updates::check_with(&host, true, async { Ok(None) }))
            .unwrap();
    assert_eq!(current.state, "current");
    assert!(current.token.is_none());
    assert!(host.shutdown());
}

#[test]
fn busy_and_closing_workspaces_do_not_poll_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let paths = nvsf_app::Paths::new(Some(directory.path())).unwrap();
    let log = nvsf_app::logging::Log::new(&paths, "nvsf-desktop");
    let host = Host::new(paths, log);
    let operation = model_manager::begin(&host).unwrap();
    let result = tauri::async_runtime::block_on(updates::check_with(&host, true, async {
        panic!("resource ownership must prevent metadata polling")
    }));
    assert!(result.err().unwrap().contains("resource operation"));
    assert!(
        nvsf_app::updates::preferences(&host.0.paths)
            .unwrap()
            .last_attempt
            .is_none()
    );
    drop(operation);
    assert!(host.begin_close().unwrap());
    let status = tauri::async_runtime::block_on(updates::check_with(&host, true, async {
        panic!("closing workspace must not poll metadata")
    }))
    .unwrap();
    assert_eq!(status.state, "idle");
    assert!(host.shutdown());
}

#[test]
fn pending_close_survives_operation_contention_without_duplicate_admission() {
    use std::{
        future::Future,
        task::{Context, Poll, Waker},
        time::Duration,
    };
    let directory = tempfile::tempdir().unwrap();
    let paths = nvsf_app::Paths::new(Some(directory.path())).unwrap();
    let log = nvsf_app::logging::Log::new(&paths, "nvsf-desktop");
    let host = Host::new(paths, log);
    tauri::async_runtime::block_on(async {
        let mut pending = Box::pin(host.wait_to_close(Duration::from_secs(2)));
        {
            let _operation = host.0.operation.lock().unwrap();
            let mut context = Context::from_waker(Waker::noop());
            assert!(matches!(pending.as_mut().poll(&mut context), Poll::Pending));
            let mut duplicate = Box::pin(host.wait_to_close(Duration::ZERO));
            assert!(matches!(
                duplicate.as_mut().poll(&mut context),
                Poll::Ready(Ok(false))
            ));
            assert!(!host.0.closing.load(Ordering::Acquire));
        }
        assert!(
            tokio::time::timeout(Duration::from_secs(2), pending)
                .await
                .unwrap()
                .unwrap()
        );
        assert!(host.0.closing.load(Ordering::Acquire));
        assert!(!host.wait_to_close(Duration::ZERO).await.unwrap());
        host.0.closing.store(false, Ordering::Release);
        {
            let _operation = host.0.operation.lock().unwrap();
            let mut expired = Box::pin(host.wait_to_close(Duration::ZERO));
            let mut context = Context::from_waker(Waker::noop());
            assert!(matches!(
                expired.as_mut().poll(&mut context),
                Poll::Ready(Err(_))
            ));
        }
        host.0.maintenance.installing.store(true, Ordering::Release);
        assert!(host.wait_to_close(Duration::from_secs(2)).await.is_err());
        host.0
            .maintenance
            .installing
            .store(false, Ordering::Release);
        assert!(!host.0.closing.load(Ordering::Acquire));
        {
            let _operation = host.0.operation.lock().unwrap();
            let mut cancelled = Box::pin(host.wait_to_close(Duration::from_secs(2)));
            let mut context = Context::from_waker(Waker::noop());
            assert!(matches!(
                cancelled.as_mut().poll(&mut context),
                Poll::Pending
            ));
            drop(cancelled);
        }
        assert!(host.wait_to_close(Duration::from_secs(2)).await.unwrap());
    });
    assert!(host.shutdown());
}

#[test]
fn update_stream_checks_size_digest_cancel_and_cleans_unverified_bytes() {
    use std::{
        io::{self, Cursor, Read},
        time::{Duration, Instant},
    };
    let directory = tempfile::tempdir().unwrap();
    let paths = nvsf_app::Paths::new(Some(directory.path())).unwrap();
    let log = nvsf_app::logging::Log::new(&paths, "nvsf-desktop");
    let host = Host::new(paths, log);
    let artifact = nvsf_app::updates::Artifact {
        url:
            "https://github.com/KiaroSama/Numera-Video-Sub-Finder/releases/download/v0.2.0/app.bin"
                .into(),
        signature: "not-a-signature-this-test-does-not-authenticate".into(),
        sha256: "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9".into(),
        bytes: 11,
    };
    let deadline = || Instant::now() + Duration::from_secs(2);
    let receive = |data: &[u8]| {
        updates::receive_update(&host, &artifact, None, Cursor::new(data), deadline())
    };
    assert_eq!(
        updates::receive_update(
            &host,
            &artifact,
            Some(12),
            Cursor::new(b"hello world"),
            deadline()
        )
        .err()
        .unwrap(),
        "Update payload size differs from release inventory"
    );
    for (data, error) in [
        (
            b"short".as_slice(),
            "Update checksum or length mismatch; installed application unchanged",
        ),
        (
            b"wrong bytes".as_slice(),
            "Update checksum or length mismatch; installed application unchanged",
        ),
        (
            b"hello world!".as_slice(),
            "Update exceeds declared payload size",
        ),
    ] {
        assert_eq!(receive(data).err().unwrap(), error);
        assert_eq!(std::fs::read_dir(&host.0.paths.cache).unwrap().count(), 0);
    }
    assert_eq!(
        updates::receive_update(
            &host,
            &artifact,
            None,
            Cursor::new(b"hello world"),
            Instant::now()
        )
        .err()
        .unwrap(),
        "Update transfer exceeded 600-second deadline"
    );
    host.0.model_cancel.store(true, Ordering::Release);
    assert_eq!(
        receive(b"hello world").err().unwrap(),
        "Update download cancelled; installed application unchanged"
    );
    host.0.model_cancel.store(false, Ordering::Release);
    struct CancelRead<'a>(&'a std::sync::atomic::AtomicBool);
    impl Read for CancelRead<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            buffer[..11].copy_from_slice(b"hello world");
            self.0.store(true, Ordering::Release);
            Ok(11)
        }
    }
    assert_eq!(
        updates::receive_update(
            &host,
            &artifact,
            None,
            CancelRead(&host.0.model_cancel),
            deadline()
        )
        .err()
        .unwrap(),
        "Update download cancelled; installed application unchanged"
    );
    host.0.model_cancel.store(false, Ordering::Release);
    struct FailedRead;
    impl Read for FailedRead {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "controlled stalled update read",
            ))
        }
    }
    assert_eq!(
        updates::receive_update(&host, &artifact, None, FailedRead, deadline())
            .err()
            .unwrap(),
        "controlled stalled update read"
    );
    assert_eq!(std::fs::read_dir(&host.0.paths.cache).unwrap().count(), 0);
    let file = receive(b"hello world").unwrap();
    assert_eq!(std::fs::read(file.path()).unwrap(), b"hello world");
    let status = updates::update_status_inner(&host).unwrap();
    assert_eq!(status.state, "downloading");
    assert_eq!(status.downloaded, 11);
    assert!(
        status.token.is_none(),
        "stream integrity is not signature or installation approval"
    );
    drop(file);
    assert_eq!(std::fs::read_dir(&host.0.paths.cache).unwrap().count(), 0);
    assert!(host.shutdown());
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
    assert!(host.begin_close().is_err());
    assert!(!host.0.closing.load(Ordering::Acquire));
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
    assert!(host.shutdown());
    tauri::async_runtime::block_on(updates::check_with(&host, true, async {
        panic!("a check admitted after shutdown must not send a request")
    }))
    .unwrap();
}
