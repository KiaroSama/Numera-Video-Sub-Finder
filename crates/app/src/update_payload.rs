use crate::{Result, fail};

/// Structural admission only; the caller must authenticate metadata and signature first.
/// No archive members are extracted or executed here.
pub fn validate(bytes: &[u8], target: &str) -> Result<()> {
    if bytes.is_empty() || bytes.len() as u64 > crate::updates::MAX_UPDATE_BYTES {
        return Err(fail("Update payload exceeds its byte budget"));
    }
    let parts: Vec<_> = target.split('-').collect();
    if parts.len() != 3
        || !matches!(parts[0], "windows" | "linux")
        || !matches!(parts[1], "x86_64" | "aarch64")
    {
        return Err(fail("Unsupported update target"));
    }
    match (parts[0], parts[2]) {
        ("linux", "appimage") => elf(bytes, parts[1], true),
        ("windows", "msi") if bytes.starts_with(b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1") => Ok(()),
        ("linux", "deb") if bytes.starts_with(b"!<arch>\n") => Ok(()),
        ("linux", "rpm") if bytes.starts_with(b"\xed\xab\xee\xdb") => Ok(()),
        (_, "portable") if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"\x1f\x8b") => {
            Ok(())
        }
        _ => Err(fail(
            "Update payload format does not match its signed target",
        )),
    }
}

fn elf(bytes: &[u8], arch: &str, appimage: bool) -> Result<()> {
    if bytes.len() < 64
        || &bytes[..6] != b"\x7fELF\x02\x01"
        || (appimage && &bytes[8..11] != b"AI\x02")
        || !matches!(u16::from_le_bytes([bytes[16], bytes[17]]), 2 | 3)
        || u16::from_le_bytes([bytes[18], bytes[19]]) != if arch == "aarch64" { 183 } else { 62 }
    {
        return Err(fail("Update ELF/AppImage architecture or header mismatch"));
    }
    Ok(())
}
