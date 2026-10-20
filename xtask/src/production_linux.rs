//! Failure-only, allowlisted native wait/library evidence; never retain raw process maps.
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path};
#[cfg(target_os = "linux")]
use std::{
    io::Read,
    time::{Duration, Instant},
};

#[cfg(target_os = "linux")]
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[cfg(target_os = "linux")]
fn bounded(path: &Path, max: u64) -> Result<String> {
    let mut value = String::new();
    std::fs::File::open(path)?
        .take(max + 1)
        .read_to_string(&mut value)?;
    if value.len() as u64 > max {
        return Err("Native process evidence exceeds byte budget".into());
    }
    Ok(value)
}

fn libraries(maps: &str, package: &Path) -> Vec<Value> {
    let mut seen = BTreeSet::new();
    let mut output = Vec::new();
    for line in maps.lines().take(8192) {
        // Map columns contain no slash; the pathname may include spaces.
        let Some(start) = line.find('/') else {
            continue;
        };
        let path = Path::new(&line[start..]);
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.is_ascii()
            || name.len() > 160
            || !name.contains(".so")
            || ![
                "libgtk-3.",
                "libgdk-3.",
                "libglib-2.0.",
                "libgio-2.0.",
                "libgobject-2.0.",
                "libpango",
                "libharfbuzz.",
                "libfontconfig.",
                "libwebkit2gtk-",
                "libjavascriptcoregtk-",
            ]
            .iter()
            .any(|prefix| name.starts_with(prefix))
        {
            continue;
        }
        let origin = if path.starts_with(package) {
            "package"
        } else if path.starts_with("/usr/lib") || path.starts_with("/lib") {
            "system"
        } else {
            "other"
        };
        if seen.insert((name.to_owned(), origin)) {
            output.push(json!({"name":name,"origin":origin}));
            if output.len() >= 64 {
                break;
            }
        }
    }
    output
}

#[cfg(target_os = "linux")]
pub fn capture(pid: u32, package: &Path, output: &Path) -> Result<()> {
    if !cfg!(target_os = "linux") || std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
        return Err("Native process evidence requires a hosted Linux runner".into());
    }
    let deadline = Instant::now() + Duration::from_secs(1);
    let root = Path::new("/proc").join(pid.to_string());
    let maps = bounded(&root.join("maps"), 1024 * 1024)
        .map(|s| libraries(&s, package))
        .map_err(|_| "Native library mappings unavailable");
    let mut threads = Vec::new();
    for entry in std::fs::read_dir(root.join("task"))?.take(32) {
        if Instant::now() >= deadline {
            return Err("Native process evidence exceeded1s".into());
        }
        let entry = entry?;
        let Some(id) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let wait = bounded(&entry.path().join("wchan"), 128).ok().filter(|s| {
            s.bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c.is_ascii_whitespace())
        });
        threads.push(json!({"tid":id,"kernel_wait":wait.map(|s|s.trim().to_owned())}));
    }
    let policy = bounded(Path::new("/proc/sys/kernel/yama/ptrace_scope"), 16)
        .ok()
        .and_then(|s| s.trim().parse::<u8>().ok())
        .filter(|n| *n <= 3);
    if Instant::now() >= deadline {
        return Err("Native process evidence exceeded1s".into());
    }
    std::fs::write(
        output.join("native-wait.json"),
        serde_json::to_vec_pretty(&json!({
            "owned_pid":pid,"ptrace_scope":policy,"threads":threads,"libraries":maps,
            "limitation":"One failure-latched snapshot; kernel wait/library identity does not establish a user-space root cause"
        }))?,
    )?;
    Ok(())
}

pub(super) fn interrupted_window_query(message: &str) -> bool {
    let Some(text) = message
        .strip_prefix("X11 native window query failed (xdotool and virtual display required): ")
    else {
        return false;
    };
    let lines: Vec<_> = text.lines().map(str::trim).collect();
    lines.len() == 5
        && lines[0] == "X Error of failed request:  BadWindow (invalid Window parameter)"
        && lines[1] == "Major opcode of failed request:  15 (X_QueryTree)"
        && lines[2]
            .strip_prefix("Resource id in failed request:  0x")
            .is_some_and(|id| !id.is_empty() && u64::from_str_radix(id, 16).is_ok())
        && [
            (3, "Serial number of failed request:  "),
            (4, "Current serial number in output stream:  "),
        ]
        .iter()
        .all(|(index, prefix)| {
            lines[*index]
                .strip_prefix(prefix)
                .is_some_and(|value| value.parse::<u64>().is_ok())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_exact_disappearing_window_query_is_an_interrupted_observation() {
        let diagnostic = "X11 native window query failed (xdotool and virtual display required): X Error of failed request:  BadWindow (invalid Window parameter)\n  Major opcode of failed request:  15 (X_QueryTree)\n  Resource id in failed request:  0x400009\n  Serial number of failed request:  93\n  Current serial number in output stream:  93";
        assert!(interrupted_window_query(diagnostic));
        for rejected in [
            diagnostic.replace("BadWindow", "BadAccess"),
            diagnostic.replace("15 (X_QueryTree)", "20 (X_GetProperty)"),
            diagnostic.replace("0x400009", "invalid"),
            format!("{diagnostic}\nUnable to open display"),
            "BadWindow".into(),
            String::new(),
        ] {
            assert!(!interrupted_window_query(&rejected));
        }
    }

    #[test]
    fn library_summary_excludes_addresses_private_paths_and_unrelated_mappings() {
        let maps = "abc-def r-xp 000 00:00 1 /usr/lib/libgtk-3.so.0.2404.32\nabc-def r--p 000 00:00 1 /usr/lib/libgtk-3.so.0.2404.32\nabc-def r-xp 000 00:00 1 /private/package/runtimes/libharfbuzz.so.14\nabc-def r-xp 000 00:00 1 /private/person/customer.bin\n";
        let result = libraries(maps, Path::new("/private/package"));
        assert_eq!(result.len(), 2);
        assert_eq!(
            result[0],
            json!({"name":"libgtk-3.so.0.2404.32","origin":"system"})
        );
        assert_eq!(
            result[1],
            json!({"name":"libharfbuzz.so.14","origin":"package"})
        );
        let text = serde_json::to_string(&result).unwrap();
        assert!(
            !text.contains("private") && !text.contains("abc-def") && !text.contains("customer")
        );
    }
}
