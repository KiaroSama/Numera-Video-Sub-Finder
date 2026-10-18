//! One original-process crash witness; raw debugger output never leaves bounded memory.
use serde_json::{Value, json};

fn frames(text: &str) -> Vec<Value> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim().trim_start_matches("* ");
            let mut fields = line.strip_prefix("NVSF_FRAME|")?.splitn(4, '|');
            let index = fields.next()?.parse::<u32>().ok().filter(|n| *n < 24)?;
            let module = fields.next()?;
            let symbol = fields.next()?;
            let offset = fields.next()?.trim();
            if module.is_empty()
                || module.len() > 160
                || symbol.len() > 512
                || module.contains(['/', '\\'])
                || symbol.contains(['/', '\\'])
                || !offset
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() || b"x +".contains(&b))
            {
                return None;
            }
            Some(json!({"index":index,"module":module,"symbol":symbol,"function_offset":offset}))
        })
        .take(24)
        .collect()
}

fn attachment(text: &str, pid: u32) -> Value {
    let stopped = format!("Process {pid} stopped");
    json!({
        "stopped_marker":text.lines().any(|line|line.trim() == stopped),
        "error_marker":text.lines().any(|line|line.trim_start().starts_with("error:")),
        "stop_frame_marker":text.lines().any(|line|line.trim().trim_start_matches("* ").starts_with("NVSF_STOP|"))
    })
}

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use std::{
        io::{ErrorKind, Read},
        os::{fd::AsRawFd, unix::process::CommandExt},
        path::{Path, PathBuf},
        process::{Child, ChildStderr, ChildStdout, Command, Stdio},
        time::{Duration, Instant},
    };
    type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

    pub struct Debugger {
        child: Child,
        stdout: ChildStdout,
        stderr: ChildStderr,
        text: Vec<u8>,
        overflow: bool,
        pid: u32,
        destination: PathBuf,
        finished: bool,
    }
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
        fn fcntl(fd: i32, command: i32, ...) -> i32;
    }

    fn nonblocking(pipe: &impl AsRawFd) -> Result<()> {
        let fd = pipe.as_raw_fd();
        // SAFETY: live owned Darwin pipe; F_GETFL/F_SETFL preserve existing flags.
        let flags = unsafe { fcntl(fd, 3) };
        if flags == -1 || unsafe { fcntl(fd, 4, flags | 4) } == -1 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }

    fn drain(pipe: &mut impl Read, output: &mut Vec<u8>, overflow: &mut bool) -> Result<()> {
        let mut bytes = [0u8; 4096];
        for _ in 0..17 {
            match pipe.read(&mut bytes) {
                Ok(0) => break,
                Ok(n) => {
                    let remaining = 65536usize.saturating_sub(output.len());
                    output.extend_from_slice(&bytes[..n.min(remaining)]);
                    *overflow |= n > remaining;
                    if *overflow {
                        break;
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    impl Debugger {
        pub fn attach(pid: u32, destination: &Path) -> Result<Self> {
            if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
                return Err("Native crash debugger is hosted macOS only".into());
            }
            let mut command = Command::new("lldb");
            command.args(["--no-lldbinit", "--batch", "--source-quietly"]);
            for setting in [
                "settings set auto-confirm true",
                "settings set target.load-script-from-symbol-file false",
                "settings set target.load-cwd-lldbinit false",
                "settings set stop-line-count-before 0",
                "settings set stop-line-count-after 0",
                "settings set stop-disassembly-count 0",
                "settings set thread-format 'NVSF_THREAD|${thread.index}\\n'",
                "settings set thread-stop-format 'NVSF_THREAD|${thread.index}\\n'",
                "settings set frame-format 'NVSF_STOP|${frame.index}|${module.file.basename}|${function.name-without-args}|${function.pc-offset}\\n'",
            ] {
                command.args(["--one-line-before-file", setting]);
            }
            command.args([
                "--one-line",
                &format!("process attach --pid {pid}"),
                "--one-line",
                "process status",
                "--one-line",
                "process continue",
            ]);
            for action in [
                "settings set frame-format 'NVSF_FRAME|${frame.index}|${module.file.basename}|${function.name-without-args}|${function.pc-offset}\\n'",
                "thread backtrace -c 24",
                "process kill",
                "quit",
            ] {
                command.args(["--one-line-on-crash", action]);
            }
            command
                .process_group(0)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child = command.spawn()?;
            let stdout = child.stdout.take().expect("piped stdout configured");
            let stderr = child.stderr.take().expect("piped stderr configured");
            let mut debugger = Self {
                child,
                stdout,
                stderr,
                text: Vec::new(),
                overflow: false,
                pid,
                destination: destination.into(),
                finished: false,
            };
            let mut failure = "pipe_error";
            let admitted: Result<()> = (|| {
                nonblocking(&debugger.stdout)?;
                nonblocking(&debugger.stderr)?;
                let deadline = Instant::now() + Duration::from_secs(3);
                loop {
                    debugger.pump()?;
                    if debugger.overflow {
                        failure = "output_limit";
                        return Err("Native debugger exceeded64KiB before attachment".into());
                    }
                    if attachment(&String::from_utf8_lossy(&debugger.text), pid)["stopped_marker"]
                        == true
                    {
                        return Ok(());
                    }
                    if debugger.child.try_wait()?.is_some() {
                        failure = "debugger_exited";
                        return Err("Native debugger attachment unavailable; no entitlement or trust settings were changed".into());
                    }
                    if Instant::now() >= deadline {
                        failure = "deadline";
                        return Err(
                            "Native debugger did not acknowledge owned-PID attachment within3s"
                                .into(),
                        );
                    }
                    std::thread::park_timeout(Duration::from_millis(10));
                }
            })();
            if let Err(error) = admitted {
                let summary = json!({
                    "owned_pid":pid,"phase":"attachment","reason":failure,
                    "debugger_status":debugger.child.try_wait().map(|status|status.map(|status|status.to_string())).ok(),
                    "captured_bytes":debugger.text.len(),"output_overflow":debugger.overflow,
                    "markers":attachment(&String::from_utf8_lossy(&debugger.text),pid),
                    "limitation":"No raw output retained; absent status does not establish permission denial or screenshot failure"
                });
                std::fs::write(
                    destination.join("native-debugger-attachment.json"),
                    serde_json::to_vec_pretty(&summary)?,
                )?;
                return Err(error);
            }
            Ok(debugger)
        }

        fn pump(&mut self) -> Result<()> {
            drain(&mut self.stdout, &mut self.text, &mut self.overflow)?;
            drain(&mut self.stderr, &mut self.text, &mut self.overflow)
        }

        pub fn finish(&mut self) -> Result<()> {
            if self.finished {
                return Ok(());
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            let exited = loop {
                self.pump()?;
                if self.child.try_wait()?.is_some() {
                    break true;
                }
                if self.overflow || Instant::now() >= deadline {
                    break false;
                }
                std::thread::park_timeout(Duration::from_millis(10));
            };
            self.stop()?;
            self.pump()?;
            std::fs::write(
                self.destination.join("native-debugger.json"),
                serde_json::to_vec_pretty(&json!({
                    "owned_pid":self.pid,"debugger_exited":exited,"output_overflow":self.overflow,
                    "frames":frames(&String::from_utf8_lossy(&self.text)),
                    "limitation":"One attach to original process before original screenshot; no raw output retained; absent frames remain inconclusive"
                }))?,
            )?;
            if self.overflow {
                return Err("Native debugger output exceeded64KiB".into());
            }
            Ok(())
        }

        fn stop(&mut self) -> Result<()> {
            if self.finished {
                return Ok(());
            }
            // SAFETY: exact owned child/debugserver group, never the application/shared group.
            unsafe {
                kill(-(self.child.id() as i32), 9);
            }
            let _ = self.child.kill();
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                if self.child.try_wait()?.is_some() {
                    break;
                }
                if Instant::now() >= deadline {
                    return Err("Native debugger failed to reap within3s".into());
                }
                std::thread::park_timeout(Duration::from_millis(10));
            }
            // SAFETY: signal0 observes the same owned group after its root was reaped.
            loop {
                // SAFETY: observe only this debugger's dedicated process group.
                let exists = unsafe { kill(-(self.child.id() as i32), 0) };
                if exists != 0 && std::io::Error::last_os_error().raw_os_error() == Some(3) {
                    break;
                }
                if Instant::now() >= deadline {
                    return Err(
                        "Native debugger descendant cleanup could not be verified within3s".into(),
                    );
                }
                std::thread::park_timeout(Duration::from_millis(10));
            }
            self.finished = true;
            Ok(())
        }
    }
    impl Drop for Debugger {
        fn drop(&mut self) {
            if let Err(error) = self.stop() {
                eprintln!("[ERROR] [CI] Native debugger cleanup failed: {error}");
            }
        }
    }
}

#[cfg(target_os = "macos")]
pub use native::Debugger;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn attachment_evidence_requires_the_owned_status_and_never_retains_raw_output() {
        let summary = attachment(
            "Process 43 stopped\nerror: private authentication detail\n/private/input\nNVSF_STOP|0|AppKit|attach_time|+4\n",
            42,
        );
        assert_eq!(summary["stopped_marker"], false);
        assert_eq!(summary["error_marker"], true);
        assert_eq!(summary["stop_frame_marker"], true);
        assert_eq!(
            attachment(" Process 42 stopped \n", 42)["stopped_marker"],
            true
        );
        let encoded = serde_json::to_string(&summary).unwrap();
        assert!(!encoded.contains("private") && !encoded.contains("AppKit"));
        assert_eq!(attachment("", 42)["error_marker"], false);
    }

    #[test]
    fn debugger_summary_keeps_only_bounded_module_and_function_identity() {
        let raw = "Process 42 stopped\n/private/person/input\nNVSF_STOP|0|AppKit|attach_time|+4\nNVSF_FRAME|0|WebKit|-[WKWebView takeSnapshotWithConfiguration:completionHandler:]| + 24\n* NVSF_FRAME|1|AppKit|NSImage::TIFFRepresentation|+0x12\nNVSF_FRAME|2|/private/path|secret|+1\nNVSF_FRAME|24|AppKit|overflow|+3\nregisters PRIVATE_BODY\n";
        let summary = frames(raw);
        assert_eq!(summary.len(), 2);
        assert_eq!(summary[0]["module"], "WebKit");
        assert_eq!(summary[1]["index"], 1);
        let text = serde_json::to_string(&summary).unwrap();
        assert!(
            !text.contains("PRIVATE_BODY")
                && !text.contains("private")
                && !text.contains("registers")
        );
        assert!(frames("NVSF_FRAME|0|WebKit|symbol|/private/value").is_empty());
        assert!(frames("NVSF_STOP|0|AppKit|attach_time|+4").is_empty());
        assert!(!text.contains("attach_time"));
    }
}
