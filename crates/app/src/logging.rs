use crate::Paths;
use chrono::Utc;
use std::{
    fs::OpenOptions,
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Clone)]
pub struct Log {
    file: Option<Arc<Mutex<std::fs::File>>>,
    redactions: Arc<Vec<String>>,
    pub path: Option<PathBuf>,
}

impl Log {
    pub fn new(paths: &Paths, name: &str) -> Self {
        let redactions = Arc::new(
            std::env::vars_os()
                .filter_map(|(key, value)| {
                    let key = key.to_str()?.to_ascii_uppercase();
                    let value = value.into_string().ok()?;
                    (value.len() >= 8
                        && [
                            "TOKEN",
                            "PASSWORD",
                            "SECRET",
                            "API_KEY",
                            "ACCESS_KEY",
                            "CONNECTION_STRING",
                        ]
                        .iter()
                        .any(|suffix| key.ends_with(suffix)))
                    .then_some(value)
                })
                .collect::<Vec<_>>(),
        );
        let stamp = Utc::now().format("%Y-%m-%d_%H-%M-%S_UTC").to_string();
        for suffix in 0..100 {
            let p = paths.logs.join(format!(
                "{name}_{stamp}{}.log",
                if suffix == 0 {
                    String::new()
                } else {
                    format!("_{suffix}")
                }
            ));
            match OpenOptions::new().create_new(true).write(true).open(&p) {
                Ok(file) => {
                    let log = Self {
                        file: Some(Arc::new(Mutex::new(file))),
                        redactions: redactions.clone(),
                        path: Some(p),
                    };
                    log.event(
                        "INFO",
                        "APP",
                        &format!(
                            "Startup version={} os={} arch={}",
                            env!("CARGO_PKG_VERSION"),
                            std::env::consts::OS,
                            std::env::consts::ARCH
                        ),
                    );
                    if let Err(error) = retain_logs(&paths.logs, name, log.path.as_deref()) {
                        log.event(
                            "WARNING",
                            "LOGGING",
                            &format!("Log retention incomplete: {error}"),
                        );
                    }
                    return log;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    eprintln!("WARNING: File logging unavailable: {e}");
                    break;
                }
            }
        }
        Self {
            file: None,
            redactions,
            path: None,
        }
    }
    pub fn event(&self, level: &str, component: &str, message: &str) {
        let mut clean = message.replace(['\r', '\n'], " ");
        for secret in self.redactions.iter() {
            clean = clean.replace(secret, "[REDACTED]");
        }
        clean = redact_fields(&clean);
        let line = format!(
            "[{}] [{level}] [{component}] {clean}\n",
            Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
        );
        if let Some(f) = &self.file {
            if let Ok(mut f) = f.lock() {
                if f.write_all(line.as_bytes())
                    .and_then(|_| f.flush())
                    .is_err()
                {
                    eprintln!("WARNING: Diagnostic log write failed");
                }
            }
        } else {
            eprint!("{line}");
        }
    }
}
fn redact_fields(message: &str) -> String {
    let mut mask_next = false;
    message
        .split_whitespace()
        .map(|word| {
            if mask_next {
                mask_next = word.eq_ignore_ascii_case("bearer");
                return "[REDACTED]".to_string();
            }
            let lower = word.to_ascii_lowercase();
            if lower == "bearer" || lower == "authorization:" {
                mask_next = true;
                return word.to_string();
            }
            if let Some((key, _)) = word.split_once(['=', ':']) {
                if [
                    "token",
                    "password",
                    "secret",
                    "api_key",
                    "access_key",
                    "authorization",
                ]
                .contains(&key.to_ascii_lowercase().as_str())
                {
                    return format!("{key}=[REDACTED]");
                }
            }
            word.to_string()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn retain_logs(
    root: &std::path::Path,
    name: &str,
    current: Option<&std::path::Path>,
) -> std::io::Result<()> {
    let cutoff =
        std::time::SystemTime::now().checked_sub(std::time::Duration::from_secs(30 * 24 * 3600));
    let mut files = Vec::new();
    for entry in std::fs::read_dir(root)?.take(10_000) {
        let entry = entry?;
        let path = entry.path();
        if current == Some(path.as_path()) || !entry.file_type()?.is_file() {
            continue;
        }
        let Some(filename) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(suffix) = filename.strip_prefix(&format!("{name}_")) else {
            continue;
        };
        if !suffix.ends_with(".log") || !suffix.contains("_UTC") {
            continue;
        }
        let metadata = entry.metadata()?;
        let modified = metadata.modified()?;
        // Never remove logs from concurrently active recent runs, even above the count cap.
        if modified.elapsed().unwrap_or_default() < std::time::Duration::from_secs(24 * 3600) {
            continue;
        }
        files.push((modified, path));
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));
    for (index, (modified, path)) in files.into_iter().enumerate() {
        if index >= 100 || cutoff.is_some_and(|c| modified < c) {
            std::fs::remove_file(path)?;
        }
    }
    Ok(())
}

impl Drop for Log {
    fn drop(&mut self) {
        if let Some(file) = &self.file {
            if Arc::strong_count(file) == 1 {
                self.event("INFO", "APP", "Shutdown; log handler closed");
            }
        }
    }
}
