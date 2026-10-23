use std::{error::Error, fs::File, io::Read, path::Path};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub(super) fn shown(log: &str, pid: u32) -> bool {
    let owned = log.lines().any(|line| {
        line.contains("[INFO] [DESKTOP] phase=paths-ready ")
            && line.ends_with(&format!(" pid={pid}"))
    });
    owned
        && log
            .lines()
            .any(|line| line.ends_with("[INFO] [DESKTOP] phase=window-show-accepted"))
}

pub(super) fn diagnostic(logs: &Path) -> Result<(String, u64)> {
    if !logs.is_dir() {
        return Ok((String::new(), 0));
    }
    let mut text = String::new();
    let mut length = 0;
    for entry in std::fs::read_dir(logs)?.take(20) {
        let path = entry?.path();
        if !path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("nvsf-desktop_"))
        {
            continue;
        }
        let file = File::open(&path)?;
        if file.metadata()?.len() > 1024 * 1024 {
            return Err("Production startup log exceeds bounded diagnostic size".into());
        }
        let mut part = String::new();
        file.take(1024 * 1024 + 1).read_to_string(&mut part)?;
        length += part.len() as u64;
        text.push_str(&part);
    }
    Ok((text, length))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_query_waits_for_owned_frontend_show_not_process_or_builder_liveness() {
        let owned = "[timestamp] [INFO] [DESKTOP] phase=paths-ready elapsed_ms=2149 pid=68822\n";
        let built = "[timestamp] [INFO] [DESKTOP] phase=native-build-returned\n";
        let show = "[timestamp] [INFO] [DESKTOP] phase=window-show-accepted\n";
        for log in [
            String::new(),
            owned.into(),
            format!("{owned}{built}"),
            show.into(),
        ] {
            assert!(!shown(&log, 68822));
        }
        assert!(!shown(&format!("{owned}{show}"), 6882));
        assert!(!shown(
            &format!("{owned}{}failed\n", show.trim_end()),
            68822
        ));
        assert!(shown(&format!("{owned}{built}{show}"), 68822));
        assert!(shown(
            &format!("{owned}{show}").replace('\n', "\r\n"),
            68822
        ));
    }
}
