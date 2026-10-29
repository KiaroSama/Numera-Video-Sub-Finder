use super::*;
use std::io::{self, Cursor};

const HASH: &str = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";

#[test]
fn resumed_and_restarted_streams_activate_only_the_declared_bytes() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("model.onnx");
    let partial = destination.with_extension("partial");
    let cancel = AtomicBool::new(false);
    let transfer = Transfer {
        bytes: 11,
        sha256: HASH,
        destination: &destination,
        cancel: &cancel,
        deadline: Instant::now() + Duration::from_secs(2),
    };
    std::fs::write(&destination, b"old model").unwrap();
    std::fs::write(&partial, b"hello ").unwrap();
    transfer
        .receive(6, true, "bytes 6-10/11", Cursor::new(b"world"))
        .unwrap();
    assert_eq!(std::fs::read(&destination).unwrap(), b"hello world");
    assert!(!partial.exists());
    std::fs::write(&destination, b"old model").unwrap();
    std::fs::write(&partial, b"hello ").unwrap();
    transfer
        .receive(6, false, "", Cursor::new(b"hello world"))
        .unwrap();
    assert_eq!(std::fs::read(&destination).unwrap(), b"hello world");
    assert!(!partial.exists());
}

#[test]
fn invalid_ranges_short_oversized_and_corrupt_streams_preserve_installed_model() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("model.onnx");
    let partial = destination.with_extension("partial");
    let cancel = AtomicBool::new(false);
    let transfer = Transfer {
        bytes: 11,
        sha256: HASH,
        destination: &destination,
        cancel: &cancel,
        deadline: Instant::now() + Duration::from_secs(2),
    };
    std::fs::write(&destination, b"old model").unwrap();
    for range in ["", "bytes 5-10/11", "bytes 6-10/12"] {
        std::fs::write(&partial, b"hello ").unwrap();
        assert!(
            transfer
                .receive(6, true, range, Cursor::new(b"world"))
                .unwrap_err()
                .to_string()
                .contains("Invalid resumed model response range")
        );
        assert_eq!(std::fs::read(&partial).unwrap(), b"hello ");
        assert_eq!(std::fs::read(&destination).unwrap(), b"old model");
    }
    for (data, message) in [
        (b"hello".as_slice(), "Incomplete model download"),
        (b"hello world!".as_slice(), "exceeds declared size"),
        (b"wrong bytes".as_slice(), "Model integrity failed"),
    ] {
        let error = transfer
            .receive(0, false, "", Cursor::new(data))
            .unwrap_err();
        assert!(error.to_string().contains(message), "{error}");
        assert_eq!(std::fs::read(&destination).unwrap(), b"old model");
        assert!(partial.is_file());
    }
}

#[test]
fn cancelled_valid_cached_model_is_not_reported_as_success() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("model.onnx");
    std::fs::write(&destination, b"hello world").unwrap();
    assert!(matches!(
        install(
            "https://example.com/model",
            11,
            HASH,
            &destination,
            &AtomicBool::new(true)
        ),
        Err(crate::AppError::Cancelled)
    ));
    assert_eq!(std::fs::read(destination).unwrap(), b"hello world");
}

#[test]
fn bounded_hash_checks_cancellation_after_data_eof_and_failed_reads() {
    let cancel = AtomicBool::new(false);
    let deadline = Instant::now() + Duration::from_secs(2);
    assert_eq!(
        hash_reader(Cursor::new(b"hello world"), 11, &cancel, deadline).unwrap(),
        HASH
    );
    assert!(matches!(
        hash_reader(CancelOnRead(&cancel), 11, &cancel, deadline),
        Err(crate::AppError::Cancelled)
    ));
    cancel.store(false, Ordering::Release);
    struct CancelAtEnd<'a>(&'a AtomicBool, bool);
    impl Read for CancelAtEnd<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.1 {
                self.0.store(true, Ordering::Release);
                Ok(0)
            } else {
                self.1 = true;
                buffer[..11].copy_from_slice(b"hello world");
                Ok(11)
            }
        }
    }
    assert!(matches!(
        hash_reader(CancelAtEnd(&cancel, false), 11, &cancel, deadline),
        Err(crate::AppError::Cancelled)
    ));
    cancel.store(false, Ordering::Release);
    assert!(hash_reader(Cursor::new(b"hello world!"), 11, &cancel, deadline).is_err());
    assert!(hash_reader(Cursor::new(b"hello"), 11, &cancel, deadline).is_err());
    assert!(
        hash_reader(Cursor::new(b"hello world"), 11, &cancel, Instant::now())
            .unwrap_err()
            .to_string()
            .contains("600-second deadline")
    );
    assert!(
        matches!(hash_reader(FailedRead, 11, &cancel, deadline), Err(crate::AppError::Io(ref e)) if e.kind() == io::ErrorKind::TimedOut)
    );
}

struct CancelOnRead<'a>(&'a AtomicBool);
impl Read for CancelOnRead<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        buffer[..11].copy_from_slice(b"hello world");
        self.0.store(true, Ordering::Release);
        Ok(11)
    }
}
struct FailedRead;
impl Read for FailedRead {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "controlled stalled read",
        ))
    }
}

#[test]
fn late_read_cancel_timeout_and_expired_deadline_never_activate_payload() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("model.onnx");
    let partial = destination.with_extension("partial");
    let cancel = AtomicBool::new(false);
    std::fs::write(&destination, b"old model").unwrap();
    let mut transfer = Transfer {
        bytes: 11,
        sha256: HASH,
        destination: &destination,
        cancel: &cancel,
        deadline: Instant::now() + Duration::from_secs(2),
    };
    assert!(matches!(
        transfer.receive(0, false, "", CancelOnRead(&cancel)),
        Err(crate::AppError::Cancelled)
    ));
    assert_eq!(std::fs::read(&partial).unwrap(), b"hello world");
    assert_eq!(std::fs::read(&destination).unwrap(), b"old model");
    cancel.store(false, Ordering::Release);
    let error = transfer
        .receive(6, true, "bytes 6-10/11", FailedRead)
        .unwrap_err();
    assert!(matches!(error, crate::AppError::Io(ref e) if e.kind() == io::ErrorKind::TimedOut));
    assert_eq!(std::fs::read(&destination).unwrap(), b"old model");
    let before = std::fs::read(&partial).unwrap();
    transfer.deadline = Instant::now();
    assert!(
        transfer
            .receive(0, false, "", Cursor::new(b"hello world"))
            .unwrap_err()
            .to_string()
            .contains("600-second deadline")
    );
    assert_eq!(std::fs::read(&partial).unwrap(), before);
    assert_eq!(std::fs::read(&destination).unwrap(), b"old model");
    assert!(std::fs::read_dir(root.path()).unwrap().all(|e| {
        let name = e.unwrap().file_name();
        name == "model.onnx" || name == "model.partial"
    }));
}
