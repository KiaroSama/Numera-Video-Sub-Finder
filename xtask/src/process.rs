use std::{
    error::Error,
    fs::File,
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

pub struct OwnedProcess {
    child: Child,
    pub started: Instant,
}

impl OwnedProcess {
    pub fn spawn(command: &mut Command, capture: &Path) -> Result<Self, Box<dyn Error>> {
        let file = File::create(capture)?;
        command
            .stdin(Stdio::null())
            .stdout(file.try_clone()?)
            .stderr(file);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        // CI's outer OS guard owns the complete inherited tree; do not escape its Unix group.
        if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
            return Err("CI subprocess execution is hosted-only".into());
        }
        let child = command.spawn()?;
        eprintln!("[INFO] [CI] Owned process pid={}", child.id());
        Ok(Self {
            child,
            started: Instant::now(),
        })
    }

    pub fn status(&mut self) -> Result<Option<ExitStatus>, Box<dyn Error>> {
        Ok(self.child.try_wait()?)
    }

    pub fn wait(
        &mut self,
        capture: &Path,
        wall: Duration,
        idle: Duration,
    ) -> Result<(), Box<dyn Error>> {
        let mut length = 0;
        let mut last_output = Instant::now();
        loop {
            if let Some(status) = self.status()? {
                let text = std::fs::read_to_string(capture)?;
                if text.len() <= 1024 * 1024 {
                    eprintln!("{text}");
                }
                return if status.success() {
                    Ok(())
                } else {
                    Err(format!("Owned process failed: {status}").into())
                };
            }
            let current = std::fs::metadata(capture)?.len();
            if current != length {
                length = current;
                last_output = Instant::now();
            }
            if self.started.elapsed() >= wall || last_output.elapsed() >= idle {
                self.stop();
                return Err("Owned process exceeded wall/idle deadline; tree terminated".into());
            }
            std::thread::park_timeout(Duration::from_millis(20));
        }
    }

    pub fn stop(&mut self) {
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            return;
        }
        #[cfg(windows)]
        {
            let pid = self.child.id().to_string();
            use std::os::windows::process::CommandExt;
            let _ = Command::new("taskkill")
                .args(["/PID", &pid, "/T", "/F"])
                .creation_flags(0x08000000)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for OwnedProcess {
    fn drop(&mut self) {
        self.stop();
    }
}
