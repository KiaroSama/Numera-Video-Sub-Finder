use crate::{
    ci_guard::{self, Result, unlinked},
    ci_native_helpers::{Member, files},
};
use serde_json::Value;
use std::{
    fs::{self, File, OpenOptions},
    io::ErrorKind,
    path::{Path, PathBuf},
};

pub(super) const ORIGINAL_MARKER: &[u8] = b"__TAURI_BUNDLE_TYPE_VAR_UNK";
const INSPECT_LIMIT: u64 = 256 * 1024 * 1024;

pub(super) fn desktop_version(version: &str) -> bool {
    let mut parts = version.split('.');
    (0..3).all(|_| {
        parts
            .next()
            .is_some_and(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    }) && parts.next().is_none()
}

pub(super) fn owned(root: &Path, path: &Path) -> Result<PathBuf> {
    let path = ci_guard::inside(
        root,
        path,
        "Bundle input/output must remain inside the owning checkout.",
    )?;
    let mut cursor = root.to_path_buf();
    for name in path.strip_prefix(root)? {
        cursor.push(name);
        match unlinked(&cursor) {
            Ok(_) => (),
            Err(error) if error.kind() == ErrorKind::NotFound => break,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(path)
}

pub(super) fn copy(src: &Path, dest: &Path) -> Result<()> {
    if !unlinked(src)?.is_file() {
        return Err("Bundle copy requires a regular unlinked input file.".into());
    }
    fs::create_dir_all(dest.parent().ok_or("Invalid bundle destination.")?)?;
    let mut output = OpenOptions::new().write(true).create_new(true).open(dest)?;
    std::io::copy(&mut File::open(src)?, &mut output)?;
    fs::set_permissions(dest, fs::metadata(src)?.permissions())?;
    Ok(())
}

pub(super) fn write_json(path: &Path, contents: &Value) -> Result<()> {
    let output = OpenOptions::new().write(true).create_new(true).open(path)?;
    serde_json::to_writer_pretty(output, contents)?;
    Ok(())
}

pub(super) fn inventory(dir: &Path, root: &Path) -> Result<Vec<Member>> {
    let mut members = files(dir)?
        .iter()
        .map(|path| Member::read(path, root))
        .collect::<Result<Vec<_>>>()?;
    members.sort_unstable_by(|a, b| a.file.cmp(&b.file));
    Ok(members)
}

pub(super) fn archive_bytes(source: &Path, extract: &Path) -> Result<()> {
    let expected = inventory(source, source)?;
    if files(extract)?.len() != expected.len() {
        return Err("Archive inventory contains missing or additional files.".into());
    }
    for member in expected {
        member.verify(&extract.join(&member.file))?;
    }
    Ok(())
}

pub(super) fn marker(format: &str) -> Result<&'static [u8]> {
    match format {
        "msi" => Ok(b"__TAURI_BUNDLE_TYPE_VAR_MSI"),
        "deb" => Ok(b"__TAURI_BUNDLE_TYPE_VAR_DEB"),
        "rpm" => Ok(b"__TAURI_BUNDLE_TYPE_VAR_RPM"),
        "appimage" => Ok(b"__TAURI_BUNDLE_TYPE_VAR_APP"),
        _ => Err("Unsupported bundle marker contract.".into()),
    }
}

pub(super) fn has_marker(bytes: &[u8], marker: &[u8]) -> bool {
    bytes.windows(marker.len()).any(|window| window == marker)
}

pub(super) fn patched_gui(
    source: &Path,
    installed: &Path,
    marker: &[u8],
) -> Result<(Vec<u8>, Vec<u8>)> {
    for file in [source, installed] {
        if !unlinked(file)?.is_file() || fs::metadata(file)?.len() > INSPECT_LIMIT {
            return Err(
                "Bundle executable exceeds inspection byte budget or is not regular.".into(),
            );
        }
    }
    let mut expected = fs::read(source)?;
    patch_marker(&mut expected, marker)?;
    Ok((expected, fs::read(installed)?))
}

fn patch_marker(bytes: &mut [u8], marker: &[u8]) -> Result<()> {
    if marker.len() != ORIGINAL_MARKER.len() {
        return Err("Invalid native bundle marker length.".into());
    }
    let mut matches = bytes
        .windows(ORIGINAL_MARKER.len())
        .enumerate()
        .filter(|(_, bytes)| *bytes == ORIGINAL_MARKER);
    let (index, _) = matches
        .next()
        .ok_or("Original GUI is missing its native bundle marker.")?;
    if matches.next().is_some() {
        return Err("Original GUI contains duplicate native bundle markers.".into());
    }
    bytes[index..index + ORIGINAL_MARKER.len()].copy_from_slice(marker);
    Ok(())
}

pub(super) fn cpu_list(status: &str) -> Result<String> {
    let allowed = status
        .lines()
        .find_map(|line| line.strip_prefix("Cpus_allowed_list:"))
        .ok_or("Unable to find native bundle CPU allowance.")?
        .trim();
    let mut cpus = Vec::new();
    for range in allowed.split(',') {
        let (from, to) = range.split_once('-').unwrap_or((range, range));
        let (from, to) = (from.parse::<u32>()?, to.parse::<u32>()?);
        if from > to {
            return Err("Invalid descending bundle CPU allowance range.".into());
        }
        cpus.extend((from..=to).take(2 - cpus.len()).map(|cpu| cpu.to_string()));
        if cpus.len() == 2 {
            break;
        }
    }
    if cpus.is_empty() {
        return Err("No allowed native bundle CPUs.".into());
    }
    Ok(cpus.join(","))
}

#[cfg(not(windows))]
pub(super) fn linux_icons(
    desktop: &Path,
    tree: &Path,
    format: &str,
    checks: &mut Vec<String>,
) -> Result<()> {
    use crate::ci_native_helpers::{basename, entries, sha};
    let is_png = |file: &PathBuf| file.extension().is_some_and(|ext| ext == "png");
    let approved = entries(&desktop.join("icons"))?
        .into_iter()
        .filter(is_png)
        .map(|file| sha(&file))
        .collect::<Result<std::collections::BTreeSet<_>>>()?;
    let icons = owned(tree, &tree.join("usr/share/icons/hicolor"))?;
    let shipped = files(&icons)?
        .into_iter()
        .filter(is_png)
        .collect::<Vec<_>>();
    let mut exact_name = false;
    for file in shipped {
        if !approved.contains(&sha(&file)?) {
            return Err(format!("Packaged icon differs from the approved logo: {format}.").into());
        }
        exact_name |= basename(&file)? == "numera-video-sub-finder.png";
    }
    if !exact_name {
        return Err(format!("No hicolor icon matches the desktop Icon name: {format}.").into());
    }
    let desktop_entries = owned(tree, &tree.join("usr/share/applications"))?;
    let entries = entries(&desktop_entries)?
        .into_iter()
        .filter(|file| file.extension().is_some_and(|ext| ext == "desktop"))
        .collect::<Vec<_>>();
    let [entry] = entries.as_slice() else {
        return Err("Package must ship one desktop entry.".into());
    };
    if !unlinked(entry)?.is_file()
        || !fs::read_to_string(entry)?
            .lines()
            .any(|line| line == "Icon=numera-video-sub-finder")
    {
        return Err(
            format!("Desktop entry does not reference the approved icon: {format}.").into(),
        );
    }
    checks.push(format!("{format}-hicolor"));
    if format == "appimage" {
        let anchored = fs::canonicalize(tree)?;
        let icon = fs::canonicalize(tree.join(".DirIcon"))?;
        if !icon.starts_with(anchored) || !unlinked(&icon)?.is_file() {
            return Err("AppImage .DirIcon does not resolve to a file inside the AppDir.".into());
        }
        if !approved.contains(&sha(&icon)?) {
            return Err("AppImage .DirIcon differs from the approved logo.".into());
        }
        checks.push("appimage-diricon".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_only_the_first_two_allowed_cpus() {
        assert_eq!(
            cpu_list("Name: xtask\nCpus_allowed_list:\t4,8-11\n").expect("valid allowance"),
            "4,8"
        );
        assert!(cpu_list("Cpus_allowed_list: 8-4\n").is_err());
    }

    #[test]
    fn msi_marker_matches_the_runtime_bundle_type() -> Result<()> {
        let replacement = marker("msi")?;
        assert_eq!(replacement, b"__TAURI_BUNDLE_TYPE_VAR_MSI");
        let mut binary = ORIGINAL_MARKER.to_vec();
        patch_marker(&mut binary, replacement)?;
        assert_eq!(binary, replacement);
        Ok(())
    }

    #[test]
    fn rejects_duplicate_unpatched_gui_markers() {
        let mut bytes = ORIGINAL_MARKER.repeat(2);
        assert!(patch_marker(&mut bytes, marker("msi").expect("known format")).is_err());
        assert_eq!(bytes, ORIGINAL_MARKER.repeat(2));
    }
}
