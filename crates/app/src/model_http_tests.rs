use super::*;
use std::{io, sync::Arc};
#[path = "maintenance_http_fixture.rs"]
mod fixture;
use fixture::{BOUND, Server, client};

const HASH: &str = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";

#[test]
fn actual_http_range_resume_and_ignored_range_restart_activate_verified_bytes() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("model.onnx");
    let partial = destination.with_extension("partial");
    let cancel = AtomicBool::new(false);
    let transfer = Transfer {
        bytes: 11,
        sha256: HASH,
        destination: &destination,
        cancel: &cancel,
        deadline: Instant::now() + BOUND,
    };
    let client = client();
    for reply in [
        b"HTTP/1.1 206 Partial Content\r\nContent-Length: 5\r\nContent-Range: bytes 6-10/11\r\nConnection: close\r\n\r\nworld".as_slice(),
        b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\nhello world".as_slice(),
    ] {
        std::fs::write(&destination, b"old model").unwrap();
        std::fs::write(&partial, b"hello ").unwrap();
        let server = Server::start(reply, false, None);
        acquire(&client, &server.url(), 6, &transfer).unwrap();
        assert!(server.request().to_ascii_lowercase().contains("range: bytes=6-\r\n"));
        assert_eq!(std::fs::read(&destination).unwrap(), b"hello world");
        assert!(!partial.exists());
    }
}

#[test]
fn actual_http_rejects_malformed_partial_headers_and_non_payload_status_before_mutation() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("model.onnx");
    let partial = destination.with_extension("partial");
    let cancel = AtomicBool::new(false);
    let client = client();
    for (offset, reply) in [
        (6, b"HTTP/1.1 206 Partial Content\r\nContent-Length: 5\r\nContent-Range: bytes 6-nonsense/11\r\n\r\nworld".as_slice()),
        (6, b"HTTP/1.1 206 Partial Content\r\nContent-Length: 5\r\nContent-Range: bytes 6-9/11\r\n\r\nworld".as_slice()),
        (6, b"HTTP/1.1 206 Partial Content\r\nContent-Length: 5\r\nContent-Range: bytes 6-99/11\r\n\r\nworld".as_slice()),
        (0, b"HTTP/1.1 206 Partial Content\r\nContent-Length: 11\r\nContent-Range: bytes 1-11/11\r\n\r\nhello world".as_slice()),
        (6, b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".as_slice()),
        (6, b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n".as_slice()),
    ] {
        std::fs::write(&destination, b"old model").unwrap();
        std::fs::write(&partial, b"hello ").unwrap();
        let transfer = Transfer {
            bytes: 11, sha256: HASH, destination: &destination, cancel: &cancel,
            deadline: Instant::now() + BOUND,
        };
        let server = Server::start(reply, false, None);
        assert!(acquire(&client, &server.url(), offset, &transfer).is_err());
        assert_eq!(std::fs::read(&partial).unwrap(), b"hello ");
        assert_eq!(std::fs::read(&destination).unwrap(), b"old model");
    }
}

#[test]
fn actual_http_stalled_headers_body_disconnect_and_cancel_are_bounded_and_preserve_data() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("model.onnx");
    let partial = destination.with_extension("partial");
    let client = client();
    for (reply, stall, cancelled) in [
        (b"".as_slice(), true, false),
        (b"HTTP/1.1 206 Partial Content\r\nContent-Length: 5\r\nContent-Range: bytes 6-10/11\r\n\r\n".as_slice(), true, false),
        (b"HTTP/1.1 206 Partial Content\r\nContent-Length: 5\r\nContent-Range: bytes 6-10/11\r\n\r\nwo".as_slice(), false, false),
        (b"".as_slice(), true, true),
        (b"HTTP/1.1 206 Partial Content\r\nContent-Length: 5\r\nContent-Range: bytes 6-10/11\r\n\r\n".as_slice(), true, true),
    ] {
        std::fs::write(&destination, b"old model").unwrap();
        std::fs::write(&partial, b"hello ").unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let server = Server::start(reply, stall, cancelled.then(|| cancel.clone()));
        let transfer = Transfer {
            bytes: 11, sha256: HASH, destination: &destination, cancel: &cancel,
            deadline: Instant::now() + BOUND,
        };
        let start = Instant::now();
        let error = acquire(&client, &server.url(), 6, &transfer).unwrap_err();
        assert!(start.elapsed() < BOUND, "HTTP read exceeded outer fixture bound");
        if cancelled {
            assert!(matches!(error, crate::AppError::Cancelled), "{error}");
        }
        assert_eq!(std::fs::read(&destination).unwrap(), b"old model");
        assert!(std::fs::read(&partial).unwrap().starts_with(b"hello "));
        assert!(std::fs::read_dir(root.path()).unwrap().all(|entry| {
            let name = entry.unwrap().file_name();
            name == "model.onnx" || name == "model.partial"
        }));
    }
}

#[test]
fn cancellation_during_actual_stalled_body_read_wins_over_transport_error() {
    struct CancelDuringRead<'a> {
        response: reqwest::blocking::Response,
        cancel: &'a AtomicBool,
    }
    impl Read for CancelDuringRead<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.cancel.store(true, Ordering::Release);
            self.response.read(buffer)
        }
    }
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("model.onnx");
    let partial = destination.with_extension("partial");
    std::fs::write(&destination, b"old model").unwrap();
    std::fs::write(&partial, b"hello ").unwrap();
    let cancel = AtomicBool::new(false);
    let server = Server::start(b"HTTP/1.1 206 Partial Content\r\nContent-Length: 5\r\nContent-Range: bytes 6-10/11\r\n\r\n", true, None);
    let client = client();
    let response = client.get(server.url()).send().unwrap();
    let transfer = Transfer {
        bytes: 11,
        sha256: HASH,
        destination: &destination,
        cancel: &cancel,
        deadline: Instant::now() + BOUND,
    };
    let start = Instant::now();
    assert!(matches!(
        transfer.receive(
            6,
            true,
            "bytes 6-10/11",
            CancelDuringRead {
                response,
                cancel: &cancel
            }
        ),
        Err(crate::AppError::Cancelled)
    ));
    assert!(start.elapsed() < BOUND);
    assert_eq!(std::fs::read(&partial).unwrap(), b"hello ");
    assert_eq!(std::fs::read(&destination).unwrap(), b"old model");
}

#[test]
fn eof_after_deadline_cannot_activate_and_production_still_refuses_http() {
    struct LateEof {
        deadline: Instant,
        sent: bool,
    }
    impl Read for LateEof {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if !self.sent {
                self.sent = true;
                buffer[..11].copy_from_slice(b"hello world");
                return Ok(11);
            }
            while Instant::now() < self.deadline {
                std::thread::park_timeout(self.deadline.saturating_duration_since(Instant::now()));
            }
            Ok(0)
        }
    }
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("model.onnx");
    let partial = destination.with_extension("partial");
    std::fs::write(&destination, b"old model").unwrap();
    std::fs::write(&partial, b"hello world").unwrap();
    let cancel = AtomicBool::new(false);
    let transfer = Transfer {
        bytes: 11,
        sha256: HASH,
        destination: &destination,
        cancel: &cancel,
        deadline: Instant::now() + Duration::from_millis(10),
    };
    let error = transfer
        .receive(
            0,
            false,
            "",
            LateEof {
                deadline: transfer.deadline,
                sent: false,
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("600-second deadline"));
    assert_eq!(std::fs::read(&destination).unwrap(), b"old model");
    assert!(install("http://127.0.0.1/model", 11, HASH, &destination, &cancel).is_err());
}
