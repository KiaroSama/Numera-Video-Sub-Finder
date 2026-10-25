use super::*;
use std::{io, thread, time::Instant};
#[path = "maintenance_http_fixture.rs"]
mod fixture;
use fixture::{BOUND, Server, client};

#[test]
fn metadata_real_http_stall_disconnect_and_byte_overflow_never_return_accepted_bytes() {
    let client = client();
    for (reply, stall, limit) in [
        (
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n".as_slice(),
            true,
            2,
        ),
        (
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{".as_slice(),
            false,
            2,
        ),
        (
            b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\n{}x".as_slice(),
            false,
            2,
        ),
    ] {
        let server = Server::start(reply, stall, None);
        let start = Instant::now();
        let response = client.get(server.url()).send().unwrap();
        assert!(server.request().starts_with("GET /immutable "));
        assert!(read_metadata(response, limit, start + BOUND).is_err());
        assert!(start.elapsed() < BOUND);
    }
    let server = Server::start(
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}",
        false,
        None,
    );
    assert_eq!(
        metadata(client.get(server.url()).send().unwrap(), 2).unwrap(),
        b"{}"
    );
}

#[test]
fn metadata_eof_returning_after_deadline_is_not_success() {
    struct LateEof {
        deadline: Instant,
    }
    impl Read for LateEof {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            while Instant::now() < self.deadline {
                thread::park_timeout(self.deadline.saturating_duration_since(Instant::now()));
            }
            Ok(0)
        }
    }
    let deadline = Instant::now() + Duration::from_millis(10);
    let error = read_metadata(LateEof { deadline }, 2, deadline).unwrap_err();
    assert!(error.to_string().contains("transfer deadline"));
}

#[test]
fn test_transport_does_not_relax_production_official_https_or_embedded_signature_policy() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::new(Some(root.path())).unwrap();
    let project = paths.projects.join("edited.json");
    std::fs::write(&project, b"existing user edits").unwrap();
    for url in [
        "http://github.com/KiaroSama/Numera-Video-Sub-Finder/releases/download/v1/app",
        "https://example.com/KiaroSama/Numera-Video-Sub-Finder/releases/download/v1/app",
        "https://github.com/KiaroSama/Numera-Video-Sub-Finder/releases/download/v1/app?token=x",
    ] {
        assert!(official_asset(url).is_err());
    }
    assert!(verify_signature(b"{}", b"invalid signature").is_err());
    assert_eq!(std::fs::read(project).unwrap(), b"existing user edits");
}
