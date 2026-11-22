#[path = "ci_authenticode_tests.rs"]
mod tests;
#[cfg(windows)]
#[path = "ci_authenticode_windows.rs"]
mod windows;

use crate::ci_guard::Result;
#[cfg(windows)]
use crate::ci_guard::unlinked;
use std::path::Path;

#[cfg(windows)]
pub(crate) use windows::{disable, enable, sign, verify};

const MAX_PE: usize = 256 * 1024 * 1024;
#[cfg(windows)]
pub(crate) const SIGNER_SHA256: &str =
    "fd066bb47fb069723b76650fe628cf4726b6f710b8245097d055312a8a912317";
#[cfg(windows)]
const SIGNER_THUMBPRINT: &str = "B2A45C4E8257197BBF067ED94F83E626AC6CE1C5";

fn valid_pin(pin: &str) -> bool {
    pin.len() == 64 && pin.bytes().all(|byte| byte.is_ascii_hexdigit())
}

struct Layout {
    checksum: usize,
    directory: usize,
    certificate: Option<std::ops::Range<usize>>,
}

fn range(bytes: &[u8], offset: usize, len: usize) -> Result<&[u8]> {
    bytes
        .get(offset..offset.saturating_add(len))
        .ok_or_else(|| "PE range is outside the file.".into())
}

fn u16_at(bytes: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(range(bytes, offset, 2)?.try_into()?))
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<usize> {
    Ok(u32::from_le_bytes(range(bytes, offset, 4)?.try_into()?) as usize)
}

fn layout(bytes: &[u8]) -> Result<Layout> {
    if !(64..=MAX_PE).contains(&bytes.len()) || !bytes.starts_with(b"MZ") {
        return Err("Invalid bounded PE image.".into());
    }
    let pe = u32_at(bytes, 60)?;
    range(bytes, pe, 24)?;
    if pe < 64 || range(bytes, pe, 4)? != b"PE\0\0" {
        return Err("Invalid PE signature.".into());
    }
    let sections = usize::from(u16_at(bytes, pe + 6)?);
    if !(1..=96).contains(&sections) {
        return Err("Invalid PE section count.".into());
    }
    let size = usize::from(u16_at(bytes, pe + 20)?);
    let optional = pe + 24;
    range(bytes, optional, size)?;
    let directories = match u16_at(bytes, optional)? {
        0x20b => 112,
        0x10b => 96,
        _ => return Err("Unsupported PE optional header.".into()),
    };
    if size < directories + 40 {
        return Err("Unsupported PE optional header.".into());
    }
    let count = u32_at(bytes, optional + directories - 4)?;
    if !(5..=16).contains(&count) || size < directories + 8 * count {
        return Err("Invalid PE data-directory count.".into());
    }
    let headers = u32_at(bytes, optional + 60)?;
    let section_table = optional + size;
    range(bytes, section_table, 40 * sections)?;
    if headers < section_table + 40 * sections || headers > bytes.len() {
        return Err("Invalid PE header extent.".into());
    }
    let mut seen: Vec<std::ops::Range<usize>> = Vec::new();
    let mut end = headers;
    for i in 0..sections {
        let entry = section_table + 40 * i;
        let len = u32_at(bytes, entry + 16)?;
        if len == 0 {
            continue;
        }
        let start = u32_at(bytes, entry + 20)?;
        range(bytes, start, len)?;
        if start < headers {
            return Err("PE section overlaps headers.".into());
        }
        let stop = start + len;
        if seen
            .iter()
            .any(|section| start < section.end && section.start < stop)
        {
            return Err("PE sections overlap.".into());
        }
        seen.push(start..stop);
        end = end.max(stop);
    }
    let directory = optional + directories + 32;
    let certificate = u32_at(bytes, directory)?;
    let table_size = u32_at(bytes, directory + 4)?;
    if (certificate == 0) != (table_size == 0) {
        return Err("Incomplete PE certificate directory.".into());
    }
    let certificate = if certificate == 0 {
        None
    } else {
        range(bytes, certificate, table_size)?;
        if certificate < end
            || !certificate.is_multiple_of(8)
            || table_size < 8
            || certificate + table_size != bytes.len()
        {
            return Err("Certificate table must be aligned and end exactly at EOF.".into());
        }
        let record = u32_at(bytes, certificate)?;
        if !(10..=1_048_584).contains(&record)
            || record.next_multiple_of(8) != table_size
            || u16_at(bytes, certificate + 4)? != 0x200
            || u16_at(bytes, certificate + 6)? != 2
        {
            return Err("Exactly one PKCS7 WIN_CERTIFICATE is required.".into());
        }
        if bytes[certificate + record..].iter().any(|byte| *byte != 0) {
            return Err("Nonzero certificate alignment padding.".into());
        }
        Some(certificate..certificate + record)
    };
    Ok(Layout {
        checksum: optional + 64,
        directory,
        certificate,
    })
}

pub(crate) fn unsigned_pe(mut bytes: Vec<u8>) -> Result<Vec<u8>> {
    let layout = layout(&bytes)?;
    if let Some(certificate) = layout.certificate {
        bytes.truncate(certificate.start);
    }
    bytes[layout.checksum..layout.checksum + 4].fill(0);
    bytes[layout.directory..layout.directory + 8].fill(0);
    Ok(bytes)
}

#[cfg(any(windows, test))]
fn der<'a>(bytes: &mut &'a [u8], tag: u8) -> Result<&'a [u8]> {
    let head = range(bytes, 0, 2)?;
    if head[0] != tag {
        return Err("Unexpected Authenticode ASN.1 tag.".into());
    }
    let mut length = usize::from(head[1]);
    let mut cursor = 2;
    if length >= 128 {
        let octets = length & 127;
        if !(1..=3).contains(&octets) {
            return Err("Unsupported ASN.1 length.".into());
        }
        let encoded = range(bytes, cursor, octets)?;
        length = encoded
            .iter()
            .fold(0, |value, byte| (value << 8) | usize::from(*byte));
        if encoded[0] == 0 || length < 128 {
            return Err("Nonminimal ASN.1 length.".into());
        }
        cursor += octets;
    }
    let body = range(bytes, cursor, length)?;
    *bytes = &bytes[cursor + length..];
    Ok(body)
}

#[cfg(windows)]
fn embedded(bytes: &[u8]) -> Result<&[u8]> {
    let record = layout(bytes)?
        .certificate
        .ok_or("Embedded signature is missing.")?;
    let payload = &bytes[record.start + 8..record.end];
    let mut trailing = payload;
    der(&mut trailing, 0x30)?;
    if trailing.len() >= 8 || trailing.iter().any(|byte| *byte != 0) {
        return Err("Trailing CMS bytes are not permitted.".into());
    }
    Ok(&payload[..payload.len() - trailing.len()])
}

#[cfg(any(windows, test))]
fn require_sha256_indirect(mut content: &[u8]) -> Result<()> {
    let mut sequence = der(&mut content, 0x30)?;
    if !content.is_empty() {
        return Err("Trailing indirect data.".into());
    }
    der(&mut sequence, 0x30)?;
    let mut digest = der(&mut sequence, 0x30)?;
    if !sequence.is_empty() {
        return Err("Invalid indirect-data fields.".into());
    }
    let mut algorithm = der(&mut digest, 0x30)?;
    let hash = der(&mut digest, 4)?;
    if !digest.is_empty() || hash.len() != 32 {
        return Err("Invalid PE SHA256 digest.".into());
    }
    if der(&mut algorithm, 6)? != [0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01] {
        return Err("PE digest algorithm must be SHA256.".into());
    }
    if !algorithm.is_empty() && (!der(&mut algorithm, 5)?.is_empty() || !algorithm.is_empty()) {
        return Err("Invalid digest parameters.".into());
    }
    Ok(())
}

#[cfg(windows)]
fn read_image(path: &Path) -> Result<(std::fs::File, Vec<u8>)> {
    use std::{io::Read, os::windows::fs::OpenOptionsExt};
    if !unlinked(path)?.is_file() {
        return Err("A regular signature input is required.".into());
    }
    // Deny write/delete sharing until the caller completes native and CMS checks.
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(path)?;
    let size = file.metadata()?.len();
    if !(64..=MAX_PE as u64).contains(&size) {
        return Err("PE image exceeds its byte bound.".into());
    }
    let mut bytes = vec![0; size as usize];
    file.read_exact(&mut bytes)?;
    Ok((file, bytes))
}

#[cfg(windows)]
pub(crate) fn unsigned_file(path: &Path) -> Result<Vec<u8>> {
    unsigned_pe(read_image(path)?.1)
}

pub(crate) fn contracts() -> Result<()> {
    if !cfg!(windows) || std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
        return Err("Authenticode tests require Windows GitHub Actions.".into());
    }
    tests::structural()?;
    println!(
        "Authenticode structural normalization and invalid-input checks passed; actual signed-file checks remain separate."
    );
    Ok(())
}

pub(crate) fn sign_allowlisted(path: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        windows::sign_allowlisted(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err("Development Authenticode verification requires Windows GitHub Actions.".into())
    }
}

#[cfg(windows)]
pub(crate) fn signature_negatives(path: &Path) -> Result<()> {
    use std::io::Write;
    verify(path, SIGNER_SHA256)?;
    let (_, mut bytes) = read_image(path)?;
    let index = 1024.min(bytes.len() / 2);
    bytes[index] ^= 0xff;
    let parent = path.parent().ok_or("Missing signature input parent.")?;
    let mut tampered = tempfile::Builder::new()
        .prefix("negative-tamper-")
        .suffix(".exe")
        .tempfile_in(parent)?;
    tampered.write_all(&bytes)?;
    let tampered = tampered.into_temp_path();
    if verify(&tampered, SIGNER_SHA256).is_ok() || verify(path, &"0".repeat(64)).is_ok() {
        return Err("Tampered or wrong-signer input was not rejected.".into());
    }
    tampered.close()?;
    Ok(())
}

pub(crate) fn identity(action: &str, path: Option<&Path>, pin: Option<&str>) -> Result<()> {
    #[cfg(windows)]
    {
        match (action, path, pin) {
            ("sign", Some(path), None) => {
                println!("{}", serde_json::to_string_pretty(&sign(path)?)?);
                Ok(())
            }
            ("verify", Some(path), pin) => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&verify(path, pin.unwrap_or(SIGNER_SHA256))?)?
                );
                Ok(())
            }
            ("negative-checks", Some(path), None) => signature_negatives(path),
            ("enable", None, None) => {
                enable()?;
                println!("{SIGNER_THUMBPRINT}");
                Ok(())
            }
            ("disable", None, None) => disable(),
            _ => Err("Unknown Authenticode action or invalid path/pin arguments.".into()),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (action, path, pin);
        Err("Development Authenticode verification requires Windows GitHub Actions.".into())
    }
}

#[cfg(test)]
mod der_tests {
    use super::require_sha256_indirect;

    #[test]
    fn rejects_an_indirect_digest_with_a_different_algorithm_or_trailing_data() {
        let mut bytes = vec![
            0x30, 53, 0x30, 0, 0x30, 49, 0x30, 13, 6, 9, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04,
            0x02, 0x01, 5, 0, 4, 32,
        ];
        bytes.extend([0; 32]);
        assert!(require_sha256_indirect(&bytes).is_ok());
        bytes[18] = 0x02;
        assert!(require_sha256_indirect(&bytes).is_err());
        bytes[18] = 0x01;
        bytes.push(0);
        assert!(require_sha256_indirect(&bytes).is_err());
    }
}
