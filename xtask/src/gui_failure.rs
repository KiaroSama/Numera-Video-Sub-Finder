//! Bounded failure evidence for one owned hosted GUI; never copy raw native crash reports.
use crate::gui::Result;
use serde_json::{Value, json};
use std::{io::Read, path::Path, time::SystemTime};

fn text(value: &Value, max: usize) -> Value {
    value
        .as_str()
        .filter(|s| s.len() <= max && !s.contains(['\0', '\r', '\n']))
        .map_or(Value::Null, |s| json!(s))
}

pub fn request_error(error: reqwest::Error, route: &str) -> String {
    let detail = format!("{:?}", error.without_url());
    format!(
        "Native WebDriver request failed route={route}: {}",
        detail.chars().take(8192).collect::<String>()
    )
}

fn crash(bytes: &[u8], pid: u32) -> Result<Option<Value>> {
    let newline = bytes
        .iter()
        .position(|b| *b == b'\n')
        .ok_or("IPS metadata line missing")?;
    let metadata: Value = serde_json::from_slice(&bytes[..newline])?;
    if metadata["bug_type"] != "309" {
        return Ok(None);
    }
    let report: Value = serde_json::from_slice(&bytes[newline + 1..])?;
    if report["pid"].as_u64() != Some(u64::from(pid))
        || report["procName"] != "numera-video-sub-finder"
    {
        return Ok(None);
    }
    let mut threads = Vec::new();
    for thread in report["threads"].as_array().into_iter().flatten().take(16) {
        let mut frames = Vec::new();
        for frame in thread["frames"].as_array().into_iter().flatten().take(24) {
            let image = frame["imageIndex"].as_u64().and_then(|index| {
                usize::try_from(index)
                    .ok()
                    .and_then(|index| report["usedImages"].get(index))
            });
            frames.push(json!({
                "image_index":frame["imageIndex"].as_u64(),
                "image_offset":frame["imageOffset"].as_u64(),
                "symbol":text(&frame["symbol"],512),
                "image_name":image.map(|image|text(&image["name"],128)),
                "image_uuid":image.map(|image|text(&image["uuid"],64)),
                "image_arch":image.map(|image|text(&image["arch"],32))
            }));
        }
        threads.push(json!({"triggered":thread["triggered"].as_bool(),"frames":frames}));
    }
    Ok(Some(json!({
        "owned_pid":pid,"exception_type":text(&report["exception"]["type"],64),
        "exception_signal":text(&report["exception"]["signal"],64),
        "termination_namespace":text(&report["termination"]["namespace"],64),
        "termination_code":report["termination"]["code"].as_u64(),
        "faulting_thread":report["faultingThread"].as_u64(),"threads":threads
    })))
}

pub fn capture(output: &Path, pid: u32, started: SystemTime) -> Result<()> {
    if !cfg!(target_os = "macos") {
        return Ok(());
    }
    let home = std::env::var_os("HOME").ok_or("Native crash report home unavailable")?;
    let root = Path::new(&home).join("Library/Logs/DiagnosticReports");
    if !root.is_dir() {
        return Ok(());
    }
    let mut reports = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    for entry in std::fs::read_dir(root)?.take(256) {
        if std::time::Instant::now() >= deadline {
            return Err("Native crash metadata exceeded1s discovery budget".into());
        }
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("numera-video-sub-finder") || !name.ends_with(".ips") {
            continue;
        }
        let metadata = entry.metadata()?;
        if !entry.file_type()?.is_file()
            || metadata.len() > 2 * 1024 * 1024
            || metadata.modified()? < started
        {
            continue;
        }
        let mut bytes = Vec::new();
        std::fs::File::open(entry.path())?
            .take(2 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 2 * 1024 * 1024 {
            return Err("Native crash metadata grew beyond2MiB".into());
        }
        if let Some(report) = crash(&bytes, pid)? {
            reports.push(report);
            break;
        }
    }
    if std::time::Instant::now() >= deadline {
        return Err("Native crash metadata exceeded1s discovery budget".into());
    }
    std::fs::write(
        output.join("native-fault.json"),
        serde_json::to_vec_pretty(&json!({
            "owned_pid":pid,"matching_reports":reports,
            "limitation":"One immediate bounded snapshot; missing or unsymbolicated report is inconclusive"
        }))?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owned_crash_summary_excludes_private_fields_and_other_processes() {
        let mut report = json!({"pid":42,"procName":"numera-video-sub-finder", "exception":{"type":"EXC_CRASH","signal":"SIGABRT","message":"PRIVATE_BODY"}, "termination":{"namespace":"SIGNAL","code":6,"indicator":"PRIVATE_BODY"}, "faultingThread":0,"asi":"PRIVATE_BODY", "procPath":"/private/user/source", "threads":[{"triggered":true,"threadState":"PRIVATE_BODY", "frames":[{"imageIndex":0,"imageOffset":12,"symbol":"-[WKWebView takeSnapshot]","source":"PRIVATE_BODY"}]}],"usedImages":[{"name":"WebKit","uuid":"fixture","arch":"arm64","path":"/private/user/source"}]});
        let encode = |report: &Value| format!("{{\"bug_type\":\"309\"}}\n{report}").into_bytes();
        let summary = crash(&encode(&report), 42).unwrap().unwrap();
        assert_eq!(summary["threads"][0]["frames"][0]["image_name"], "WebKit");
        let serialized = summary.to_string();
        assert!(serialized.contains("takeSnapshot"));
        let error = reqwest::Client::new().get("http://[").build().unwrap_err();
        let detail = request_error(error, "/screenshot");
        assert!(detail.contains("/screenshot") && detail.contains("source"));
        assert!(!detail.contains("http://["));
        assert!(!serialized.contains("PRIVATE_BODY") && !serialized.contains("/private"));
        assert!(crash(&encode(&report), 43).unwrap().is_none());
        report["procName"] = "other-app".into();
        assert!(crash(&encode(&report), 42).unwrap().is_none());
        assert!(crash(b"{\"bug_type\":\"309\"}\n{}", 42).unwrap().is_none());
        assert!(crash(b"not json", 42).is_err());
    }
}
