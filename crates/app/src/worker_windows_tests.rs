use super::*;
use std::{
    io::{Read, Write},
    os::windows::process::CommandExt,
    process::Stdio,
    time::{Duration, Instant},
};

#[test]
fn owned_job_closure_stops_the_admitted_worker() {
    const CHILD: &str = "NVSF_WORKER_JOB_CONTRACT";
    if let Some(marker) = std::env::var_os(CHILD) {
        await_start().unwrap();
        std::fs::write(&marker, b"native-work-admitted").unwrap();
        let mut byte = [0u8; 1];
        let _ = std::io::stdin().read_exact(&mut byte);
        std::fs::write(
            std::path::PathBuf::from(marker).with_extension("survived"),
            b"unexpected-return",
        )
        .unwrap();
        panic!("Worker survived its owning job");
    }
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("admitted.txt");
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "worker::windows::tests::owned_job_closure_stops_the_admitted_worker",
            "--nocapture",
        ])
        .env(CHILD, &marker)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x08000000)
        .spawn()
        .unwrap();
    struct Cleanup(std::process::Child);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let deadline = Instant::now() + Duration::from_secs(2);
            while !matches!(self.0.try_wait(), Ok(Some(_))) {
                if Instant::now() >= deadline {
                    eprintln!("Worker test cleanup exceeded two seconds");
                    return;
                }
                std::thread::park_timeout(Duration::from_millis(10));
            }
        }
    }
    let mut child = Cleanup(child);
    let job = Job::attach(&child.0).expect("Job admission failed");
    assert!(!marker.exists(), "Native work ran before owner admission");
    let mut lease = child.0.stdin.take().unwrap();
    lease.write_all(b"S").unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while std::fs::read(&marker).ok().as_deref() != Some(b"native-work-admitted") {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "Worker exited before readiness"
        );
        assert!(
            Instant::now() < deadline,
            "Worker admission exceeded three seconds"
        );
        std::thread::park_timeout(Duration::from_millis(10));
    }
    assert_eq!(std::fs::read(&marker).unwrap(), b"native-work-admitted");
    drop(job);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert_ne!(
                status.code(),
                Some(101),
                "Worker exited through the fixture panic instead of job closure"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Owned worker survived job closure"
        );
        std::thread::park_timeout(Duration::from_millis(10));
    }
    assert!(
        !marker.with_extension("survived").exists(),
        "Worker returned from its still-open lease"
    );
    drop(lease);
}
