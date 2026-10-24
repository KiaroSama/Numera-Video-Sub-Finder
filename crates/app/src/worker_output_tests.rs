use super::*;

#[test]
fn output_frames_partial_unicode_and_multiple_events_without_waiting_for_eof() {
    let mut messages = Messages::default();
    assert!(messages.next(false).unwrap().is_none());
    messages
        .append(b"{\"type\":\"Failed\",\"data\":\"")
        .unwrap();
    messages.append(&[0xd8]).unwrap();
    assert!(messages.next(false).unwrap().is_none());
    messages.append(&[0xb3]).unwrap();
    messages
        .append(b"\"}\n{\"type\":\"Failed\",\"data\":\"second\"}\n")
        .unwrap();
    assert!(matches!(messages.next(false).unwrap(), Some(Event::Failed(text)) if text == "س"));
    assert!(matches!(messages.next(false).unwrap(), Some(Event::Failed(text)) if text == "second"));
    assert!(messages.next(false).unwrap().is_none());
    messages
        .append(b"{\"type\":\"Failed\",\"data\":\"final\"}")
        .unwrap();
    assert!(messages.next(false).unwrap().is_none());
    assert!(matches!(messages.next(true).unwrap(), Some(Event::Failed(text)) if text == "final"));
    assert!(messages.empty());
    messages.append(b"not-json\n").unwrap();
    assert!(messages.next(false).is_err());
}

#[test]
fn native_output_remains_nonblocking_until_real_child_data_and_eof() {
    use std::{
        io::Write,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    #[cfg(unix)]
    let mut command = {
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "read token; printf '%s\\n' '{\"type\":\"Failed\",\"data\":\"ready\"}'; read release",
        ]);
        command
    };
    #[cfg(windows)]
    let mut command = {
        use std::os::windows::process::CommandExt;
        let mut command = Command::new("cmd.exe");
        command.args(["/D", "/Q", "/C"]).raw_arg(
            r#"set /p NVSF_INPUT=&echo {"type":"Failed","data":"ready"}&set /p NVSF_RELEASE="#,
        );
        command.creation_flags(0x08000000);
        command
    };
    let child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    struct Cleanup(std::process::Child);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let deadline = Instant::now() + Duration::from_secs(2);
            while !matches!(self.0.try_wait(), Ok(Some(_))) {
                assert!(
                    Instant::now() < deadline,
                    "Native pipe fixture cleanup exceeded two seconds"
                );
                std::thread::park_timeout(Duration::from_millis(10));
            }
        }
    }
    let mut child = Cleanup(child);
    let mut output = Output::new(child.0.stdout.take().unwrap()).unwrap();
    assert!(matches!(output.poll().unwrap(), Poll::Pending));
    let mut input = child.0.stdin.take().unwrap();
    input.write_all(b"go\r\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut received = false;
    loop {
        match output.poll().unwrap() {
            Poll::Event(Event::Failed(text)) => {
                assert_eq!(text, "ready");
                received = true;
                assert!(matches!(output.poll().unwrap(), Poll::Pending));
                input.write_all(b"release\r\n").unwrap();
            }
            Poll::Event(_) => panic!("Unexpected native fixture event"),
            Poll::Ended => break,
            Poll::Progress => {}
            Poll::Pending => std::thread::park_timeout(Duration::from_millis(10)),
        }
        assert!(
            Instant::now() < deadline,
            "Native output fixture exceeded three seconds"
        );
    }
    assert!(received);
    while child.0.try_wait().unwrap().is_none() {
        assert!(
            Instant::now() < deadline,
            "Native fixture process did not exit"
        );
        std::thread::park_timeout(Duration::from_millis(10));
    }
}

#[test]
fn output_rejects_unterminated_and_oversized_frames() {
    let mut messages = Messages::default();
    messages.append(b"{\"type\":").unwrap();
    assert!(messages.next(true).is_err());
    let mut messages = Messages::default();
    let page = [b'x'; 8192];
    for _ in 0..LIMIT / page.len() {
        messages.append(&page).unwrap();
    }
    assert!(messages.append(b"x").is_err());
}
