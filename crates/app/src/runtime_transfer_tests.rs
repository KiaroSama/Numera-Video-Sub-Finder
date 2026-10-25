use super::*;
use std::io::{self, Cursor};
use std::time::Instant;
#[path = "maintenance_http_fixture.rs"]
mod fixture;

#[test]
fn real_http_runtime_stall_disconnect_and_cancel_clean_staging_and_preserve_old_data() {
    use fixture::{BOUND, Server, client};
    use std::sync::Arc;
    let root = tempfile::tempdir().unwrap();
    let selection = root.path().join("runtime-selection.json");
    std::fs::write(&selection, b"previous selection").unwrap();
    let client = client();
    for (reply, stall, cancelled) in [
        (
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n".as_slice(),
            true,
            false,
        ),
        (
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nwo".as_slice(),
            false,
            false,
        ),
        (
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n".as_slice(),
            true,
            true,
        ),
    ] {
        let cancel = Arc::new(AtomicBool::new(false));
        let server = Server::start(reply, stall, cancelled.then(|| cancel.clone()));
        let stage = tempfile::tempdir_in(root.path()).unwrap();
        let entry = Entry {
            file: "payload.bin".into(),
            bytes: 5,
            sha256: "a".repeat(64),
        };
        let start = Instant::now();
        let response = client.get(server.url()).send().unwrap();
        assert!(server.request().starts_with("GET /immutable "));
        let error =
            receive_entry(&entry, stage.path(), &cancel, start + BOUND, response).unwrap_err();
        assert!(start.elapsed() < BOUND);
        if cancelled {
            assert!(matches!(error, crate::AppError::Cancelled), "{error}");
        }
        drop(stage);
        assert_eq!(std::fs::read(&selection).unwrap(), b"previous selection");
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }
}

#[test]
fn cancellation_on_final_eof_is_not_lost_and_existing_application_data_is_untouched() {
    struct CancelEof<'a>(&'a AtomicBool);
    impl Read for CancelEof<'_> {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            self.0.store(true, Ordering::Release);
            Ok(0)
        }
    }
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project.json");
    let selection = root.path().join("runtime-selection.json");
    std::fs::write(&project, b"existing edits").unwrap();
    std::fs::write(&selection, b"previous selection").unwrap();
    let cancel = AtomicBool::new(false);
    let entry = Entry {
        file: "payload.bin".into(),
        bytes: 1,
        sha256: "a".repeat(64),
    };
    let stage = tempfile::tempdir_in(root.path()).unwrap();
    assert!(matches!(
        receive_entry(
            &entry,
            stage.path(),
            &cancel,
            Instant::now() + Duration::from_secs(1),
            CancelEof(&cancel)
        ),
        Err(crate::AppError::Cancelled)
    ));
    drop(stage);
    assert_eq!(std::fs::read(project).unwrap(), b"existing edits");
    assert_eq!(std::fs::read(selection).unwrap(), b"previous selection");
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 2);
}

#[test]
fn runtime_eof_after_total_deadline_is_refused_before_native_admission() {
    struct LateEof(Instant);
    impl Read for LateEof {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            while Instant::now() < self.0 {
                std::thread::park_timeout(self.0.saturating_duration_since(Instant::now()));
            }
            Ok(0)
        }
    }
    let root = tempfile::tempdir().unwrap();
    let cancel = AtomicBool::new(false);
    let entry = Entry {
        file: "payload.bin".into(),
        bytes: 1,
        sha256: "a".repeat(64),
    };
    let deadline = Instant::now() + Duration::from_millis(10);
    let error =
        receive_entry(&entry, root.path(), &cancel, deadline, LateEof(deadline)).unwrap_err();
    assert!(error.to_string().contains("600-second deadline"));
}

#[test]
fn expired_runtime_transfer_and_oversized_bytes_are_refused_without_activation() {
    let root = tempfile::tempdir().unwrap();
    let cancel = AtomicBool::new(false);
    let entry = Entry {
        file: "payload.bin".into(),
        bytes: 1,
        sha256: "a".repeat(64),
    };
    assert!(
        receive_entry(
            &entry,
            root.path(),
            &cancel,
            std::time::Instant::now(),
            Cursor::new(b"x")
        )
        .is_err()
    );
    assert!(!root.path().join("payload.bin").exists());
    assert!(
        receive_entry(
            &entry,
            root.path(),
            &cancel,
            Instant::now() + Duration::from_secs(1),
            Cursor::new(b"xx")
        )
        .is_err()
    );
    assert_eq!(std::fs::read(root.path().join("payload.bin")).unwrap(), b"");
}

#[test]
fn unauthenticated_repair_never_fetches_and_preserves_selection_projects_and_models() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::new(Some(root.path())).unwrap();
    let files = [
        paths.root.join("runtime-selection.json"),
        paths.projects.join("existing.json"),
        paths.models.join("existing.onnx"),
    ];
    for file in &files {
        std::fs::write(file, b"preserved bytes").unwrap();
    }
    let cancel = AtomicBool::new(false);
    for consent in [false, true] {
        assert!(
            repair_with_fetch(
                &paths,
                b"{}",
                b"invalid signature",
                "https://github.com/KiaroSama/Numera-Video-Sub-Finder/releases/download/v0.1.0",
                consent,
                &cancel,
                |_| -> Result<Cursor<&[u8]>> {
                    panic!("Unapproved or unsigned repair acquired payload")
                },
            )
            .is_err()
        );
        for file in &files {
            assert_eq!(std::fs::read(file).unwrap(), b"preserved bytes");
        }
        assert!(!paths.root.join("runtimes").exists());
    }
}
