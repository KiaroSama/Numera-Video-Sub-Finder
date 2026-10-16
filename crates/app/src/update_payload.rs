use crate::{Result, fail};
use std::io::Read;

/// Structural admission only; the caller must authenticate metadata and signature first.
/// No archive members are extracted or executed here.
pub fn validate(bytes: &[u8], target: &str) -> Result<()> {
    if bytes.is_empty() || bytes.len() as u64 > crate::updates::MAX_UPDATE_BYTES {
        return Err(fail("Update payload exceeds its byte budget"));
    }
    let parts: Vec<_> = target.split('-').collect();
    if parts.len() != 3 || !matches!(parts[1], "x86_64" | "aarch64") {
        return Err(fail("Unsupported update target"));
    }
    match (parts[0], parts[2]) {
        ("linux", "appimage") => elf(bytes, parts[1], true),
        ("windows", "nsis") => pe(bytes),
        ("darwin", "app") => app_archive(bytes, parts[1]),
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

fn pe(bytes: &[u8]) -> Result<()> {
    if bytes.len() < 64 || &bytes[..2] != b"MZ" {
        return Err(fail("Update executable has no PE header"));
    }
    let offset = u32::from_le_bytes(
        bytes[60..64]
            .try_into()
            .map_err(|_| fail("Invalid PE offset"))?,
    ) as usize;
    let header = bytes
        .get(offset..offset.saturating_add(6))
        .ok_or_else(|| fail("Update PE header is out of bounds"))?;
    // Tauri uses an x86 NSIS installer stub even for x64/ARM64 payloads. The signed
    // release inventory binds the contained application architecture, not this stub.
    if &header[..4] != b"PE\0\0" || u16::from_le_bytes([header[4], header[5]]) != 0x14c {
        return Err(fail("Update NSIS stub header mismatch"));
    }
    Ok(())
}

fn macho(bytes: &[u8], arch: &str) -> Result<()> {
    if bytes.len() < 32
        || &bytes[..4] != b"\xcf\xfa\xed\xfe"
        || u32::from_le_bytes(
            bytes[4..8]
                .try_into()
                .map_err(|_| fail("Invalid Mach-O header"))?,
        ) != if arch == "aarch64" {
            0x0100000c
        } else {
            0x01000007
        }
        || u32::from_le_bytes(
            bytes[12..16]
                .try_into()
                .map_err(|_| fail("Invalid Mach-O type"))?,
        ) != 2
    {
        return Err(fail("Update application Mach-O architecture/type mismatch"));
    }
    Ok(())
}

fn app_archive(bytes: &[u8], arch: &str) -> Result<()> {
    if !bytes.starts_with(b"\x1f\x8b") {
        return Err(fail("Application update must be a gzip archive"));
    }
    let decoder = flate2::bufread::GzDecoder::new(bytes);
    // Bound decompression as well as declared member lengths; parsing never allocates a member.
    let mut archive = tar::Archive::new(decoder.take(2 * 1024 * 1024 * 1024 + 1));
    let mut names = std::collections::HashSet::new();
    let mut root = None::<String>;
    let mut total = 0u64;
    let mut gui = false;
    let mut cli = false;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    for entry in archive.entries()? {
        if std::time::Instant::now() >= deadline {
            return Err(fail("Update archive inspection exceeded deadline"));
        }
        let mut entry = entry?;
        if names.len() >= 16384 {
            return Err(fail("Update archive has too many members"));
        }
        let path = entry.path()?;
        let path = path
            .to_str()
            .ok_or_else(|| fail("Update archive filename is not UTF-8"))?
            .trim_end_matches('/')
            .to_owned();
        let parts: Vec<_> = path.split('/').collect();
        if parts.is_empty()
            || parts.iter().any(|part| {
                part.is_empty() || *part == "." || *part == ".." || part.contains(['\\', ':', '\0'])
            })
            || !parts[0].ends_with(".app")
            || !names.insert(path.clone())
        {
            return Err(fail("Unsafe or duplicate application archive member"));
        }
        if root.as_ref().is_some_and(|root| root != parts[0]) {
            return Err(fail("Update archive contains multiple application roots"));
        }
        root.get_or_insert_with(|| parts[0].into());
        total = total
            .checked_add(entry.size())
            .ok_or_else(|| fail("Update archive size overflow"))?;
        if total > 2 * 1024 * 1024 * 1024 {
            return Err(fail("Update archive exceeds decompressed byte budget"));
        }
        let kind = entry.header().entry_type();
        if !kind.is_file() && !kind.is_dir() {
            return Err(fail(
                "Links and special files are not admitted in application updates",
            ));
        }
        let relative = parts[1..].join("/");
        if matches!(
            relative.as_str(),
            "Contents/MacOS/numera-video-sub-finder" | "Contents/MacOS/nvsf"
        ) {
            if !kind.is_file() {
                return Err(fail("Application executable is not a regular member"));
            }
            let mut header = [0u8; 32];
            entry.read_exact(&mut header)?;
            macho(&header, arch)?;
            gui |= relative.ends_with("/numera-video-sub-finder");
            cli |= relative.ends_with("/nvsf");
        }
        let mut buffer = [0u8; 65536];
        loop {
            if std::time::Instant::now() >= deadline {
                return Err(fail("Update archive inspection exceeded deadline"));
            }
            if entry.read(&mut buffer)? == 0 {
                break;
            }
        }
    }
    if !gui || !cli {
        return Err(fail(
            "Update archive is missing the native application or worker",
        ));
    }
    // Consume and validate the gzip trailer; remaining decompressed archive padding must be zero.
    let mut reader = archive.into_inner();
    let mut buffer = [0u8; 8192];
    loop {
        if std::time::Instant::now() >= deadline {
            return Err(fail("Update archive inspection exceeded deadline"));
        }
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        if buffer[..count].iter().any(|byte| *byte != 0) {
            return Err(fail("Unexpected trailing archive data"));
        }
    }
    if reader.limit() == 0 {
        return Err(fail("Update archive exceeds decompressed byte budget"));
    }
    if !reader.into_inner().into_inner().is_empty() {
        return Err(fail("Trailing compressed update data is not admitted"));
    }
    Ok(())
}
