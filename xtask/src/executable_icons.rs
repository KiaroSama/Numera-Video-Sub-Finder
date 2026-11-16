//! Reads linked PE icon resources as inert bytes; never loads or executes the binary.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    error::Error,
    io::{Read, Write},
    path::Path,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const MAX_FILE: usize = 256 * 1024 * 1024;

fn bytes(data: &[u8], at: usize, length: usize) -> Result<&[u8]> {
    data.get(at..at.checked_add(length).ok_or("Icon offset overflow")?)
        .ok_or_else(|| "Truncated executable/icon resource".into())
}
fn u16_at(data: &[u8], at: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(bytes(data, at, 2)?.try_into()?))
}
fn u32_at(data: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(bytes(data, at, 4)?.try_into()?))
}
fn hash(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

struct Section {
    rva: usize,
    virtual_size: usize,
    raw: usize,
    raw_size: usize,
}
struct Pe<'a> {
    data: &'a [u8],
    sections: Vec<Section>,
    root: usize,
    size: usize,
}
impl<'a> Pe<'a> {
    fn parse(data: &'a [u8]) -> Result<Self> {
        if data.len() > MAX_FILE || bytes(data, 0, 2)? != b"MZ" {
            return Err("Invalid bounded PE image".into());
        }
        let pe = u32_at(data, 0x3c)? as usize;
        if bytes(data, pe, 4)? != b"PE\0\0" {
            return Err("Missing PE signature".into());
        }
        let count = usize::from(u16_at(data, pe + 6)?);
        if count == 0 || count > 96 {
            return Err("PE section count exceeds limit".into());
        }
        let optional = pe + 24;
        let optional_size = usize::from(u16_at(data, pe + 20)?);
        bytes(data, optional, optional_size)?;
        let (directory, number) = match u16_at(data, optional)? {
            0x10b => (96, 92),
            0x20b => (112, 108),
            _ => return Err("Unsupported PE optional header".into()),
        };
        if optional_size < directory + 24 || u32_at(data, optional + number)? < 3 {
            return Err("Missing PE resource directory".into());
        }
        let headers = u32_at(data, optional + 60)? as usize;
        let table = optional + optional_size;
        if headers < table + count * 40 || headers > data.len() {
            return Err("Invalid PE header size".into());
        }
        let mut sections = Vec::<Section>::new();
        for index in 0..count {
            let at = table + index * 40;
            bytes(data, at, 40)?;
            let section = Section {
                virtual_size: u32_at(data, at + 8)? as usize,
                rva: u32_at(data, at + 12)? as usize,
                raw_size: u32_at(data, at + 16)? as usize,
                raw: u32_at(data, at + 20)? as usize,
            };
            let span = section.virtual_size.max(section.raw_size);
            if section.rva < headers || section.rva.checked_add(span).is_none() {
                return Err("Invalid PE section RVA".into());
            }
            if section.raw_size > 0 {
                if section.raw < headers {
                    return Err("PE section overlaps headers".into());
                }
                bytes(data, section.raw, section.raw_size)?;
            }
            for old in &sections {
                if intersects(
                    section.rva,
                    span,
                    old.rva,
                    old.virtual_size.max(old.raw_size),
                ) || (section.raw_size > 0
                    && old.raw_size > 0
                    && intersects(section.raw, section.raw_size, old.raw, old.raw_size))
                {
                    return Err("Overlapping PE sections".into());
                }
            }
            sections.push(section);
        }
        let root = u32_at(data, optional + directory + 16)? as usize;
        let size = u32_at(data, optional + directory + 20)? as usize;
        if size < 16 || size > 16 * 1024 * 1024 {
            return Err("PE resource directory exceeds limit".into());
        }
        let result = Self {
            data,
            sections,
            root,
            size,
        };
        result.map(root, size)?;
        Ok(result)
    }
    fn map(&self, rva: usize, size: usize) -> Result<&'a [u8]> {
        for section in &self.sections {
            if let Some(offset) = rva.checked_sub(section.rva)
                && offset <= section.raw_size
                && size <= section.raw_size - offset
            {
                return bytes(self.data, section.raw + offset, size);
            }
        }
        Err("Resource RVA is not backed by one PE section".into())
    }
    fn resource(&self, at: usize, size: usize) -> Result<&'a [u8]> {
        if at > self.size || size > self.size - at {
            return Err("Resource tree offset escapes directory".into());
        }
        self.map(
            self.root.checked_add(at).ok_or("Resource RVA overflow")?,
            size,
        )
    }
}
fn intersects(a: usize, len_a: usize, b: usize, len_b: usize) -> bool {
    len_a > 0 && len_b > 0 && a < b.saturating_add(len_b) && b < a.saturating_add(len_a)
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Name {
    Text(Vec<u16>),
    Id(u16),
}
impl Name {
    fn label(&self) -> String {
        match self {
            Self::Id(id) => id.to_string(),
            Self::Text(text) => String::from_utf16(text).expect("Validated resource name"),
        }
    }
}
struct Entry {
    name: Name,
    offset: usize,
    directory: bool,
}
struct Tree<'a> {
    pe: Pe<'a>,
    visited: BTreeSet<usize>,
    entries: usize,
}
impl<'a> Tree<'a> {
    fn directory(&mut self, offset: usize) -> Result<Vec<Entry>> {
        if !self.visited.insert(offset) {
            return Err("Cyclic or shared resource directory".into());
        }
        let header = self.pe.resource(offset, 16)?;
        let named = usize::from(u16_at(header, 12)?);
        let count = named + usize::from(u16_at(header, 14)?);
        self.entries += count;
        if count > 64 || self.entries > 4096 {
            return Err("Resource entry count exceeds limit".into());
        }
        let raw = self.pe.resource(offset + 16, count * 8)?;
        let mut names = BTreeSet::new();
        let mut result = Vec::new();
        for index in 0..count {
            let id = u32_at(raw, index * 8)?;
            let name = if id & 0x80000000 != 0 {
                if index >= named {
                    return Err("Invalid resource name ordering".into());
                }
                let at = (id & 0x7fffffff) as usize;
                let length = usize::from(u16_at(self.pe.resource(at, 2)?, 0)?);
                if length == 0 || length > 256 {
                    return Err("Invalid resource name length".into());
                }
                let raw = self.pe.resource(at + 2, length * 2)?;
                let text = raw
                    .chunks_exact(2)
                    .map(|v| u16::from_le_bytes([v[0], v[1]]))
                    .collect::<Vec<_>>();
                if text.contains(&0) || String::from_utf16(&text).is_err() {
                    return Err("Invalid UTF-16 resource name".into());
                }
                Name::Text(text)
            } else {
                if index < named || id > u32::from(u16::MAX) {
                    return Err("Invalid numeric resource identity".into());
                }
                Name::Id(id as u16)
            };
            if !names.insert(name.clone()) {
                return Err("Duplicate resource identity".into());
            }
            let target = u32_at(raw, index * 8 + 4)?;
            result.push(Entry {
                name,
                offset: (target & 0x7fffffff) as usize,
                directory: target & 0x80000000 != 0,
            });
        }
        result.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(result)
    }
    fn children(&mut self, entry: &Entry) -> Result<Vec<Entry>> {
        if !entry.directory {
            return Err("Missing resource subtree".into());
        }
        self.directory(entry.offset)
    }
    fn payload(&self, entry: &Entry) -> Result<&'a [u8]> {
        if entry.directory {
            return Err("Unexpected resource subtree at language leaf".into());
        }
        let raw = self.pe.resource(entry.offset, 16)?;
        let rva = u32_at(raw, 0)? as usize;
        let length = u32_at(raw, 4)? as usize;
        if length == 0 || length > 8 * 1024 * 1024 || u32_at(raw, 12)? != 0 {
            return Err("Invalid icon resource payload size/reserved field".into());
        }
        let at = rva
            .checked_sub(self.pe.root)
            .ok_or("Icon payload precedes resource directory")?;
        self.pe.resource(at, length)
    }
}

fn ico_entries(icon: &[u8]) -> Result<Vec<(&[u8], &[u8])>> {
    if icon.len() > 8 * 1024 * 1024 || u16_at(icon, 0)? != 0 || u16_at(icon, 2)? != 1 {
        return Err("Invalid bounded ICO header".into());
    }
    let count = usize::from(u16_at(icon, 4)?);
    if count == 0 || count > 64 {
        return Err("ICO entry count exceeds limit".into());
    }
    let table_end = 6 + count * 16;
    bytes(icon, 0, table_end)?;
    let mut result = Vec::new();
    let mut spans = Vec::new();
    for index in 0..count {
        let entry = bytes(icon, 6 + index * 16, 16)?;
        let size = u32_at(entry, 8)? as usize;
        let at = u32_at(entry, 12)? as usize;
        if entry[3] != 0 || size == 0 || at < table_end {
            return Err("Invalid ICO entry".into());
        }
        let payload = bytes(icon, at, size)?;
        if spans
            .iter()
            .any(|&(old, len)| intersects(at, size, old, len))
        {
            return Err("Overlapping ICO image payloads".into());
        }
        spans.push((at, size));
        result.push((&entry[..12], payload));
    }
    Ok(result)
}

fn compare(binary: &[u8], icon: &[u8]) -> Result<Value> {
    let expected = ico_entries(icon)?;
    let mut tree = Tree {
        pe: Pe::parse(binary)?,
        visited: BTreeSet::new(),
        entries: 0,
    };
    let types = tree.directory(0)?;
    let groups = tree.children(
        types
            .iter()
            .find(|e| e.name == Name::Id(14))
            .ok_or("Missing RT_GROUP_ICON")?,
    )?;
    let first = groups.first().ok_or("Empty icon group directory")?;
    let languages = tree.children(first)?;
    if languages.is_empty() {
        return Err("Missing icon group languages".into());
    }
    let icons = tree.children(
        types
            .iter()
            .find(|e| e.name == Name::Id(3))
            .ok_or("Missing RT_ICON")?,
    )?;
    let mut images = Vec::new();
    for entry in &icons {
        let Name::Id(id) = &entry.name else {
            return Err("Named RT_ICON child is unsupported".into());
        };
        let translations = tree.children(entry)?;
        if translations.is_empty() {
            return Err("Missing icon image language".into());
        }
        for language in translations {
            let Name::Id(lang) = &language.name else {
                return Err("Named icon language".into());
            };
            images.push((*id, *lang, tree.payload(&language)?));
        }
    }
    let mut language_ids = Vec::new();
    for language in &languages {
        let Name::Id(lang) = &language.name else {
            return Err("Named icon group language".into());
        };
        let group = tree.payload(language)?;
        if u16_at(group, 0)? != 0
            || u16_at(group, 2)? != 1
            || usize::from(u16_at(group, 4)?) != expected.len()
            || group.len() != 6 + expected.len() * 14
        {
            return Err("Linked icon group differs from source ICO".into());
        }
        let mut ids = BTreeSet::new();
        for (index, (descriptor, payload)) in expected.iter().enumerate() {
            let entry = bytes(group, 6 + index * 14, 14)?;
            let id = u16_at(entry, 12)?;
            if !ids.insert(id) || &entry[..12] != *descriptor {
                return Err("Linked icon dimensions/format differ from source ICO".into());
            }
            let actual = images
                .iter()
                .find(|(child, child_lang, _)| *child == id && child_lang == lang)
                .ok_or("Dangling or wrong-language RT_ICON reference")?;
            if actual.2 != *payload {
                return Err("Embedded icon pixels differ from source ICO".into());
            }
        }
        language_ids.push(lang);
    }
    Ok(
        json!({"group":first.name.label(),"images":expected.len(),"languages":language_ids,
        "groups_present":groups.len(),"selection":"first named ordinal group, otherwise lowest numeric group"}),
    )
}

fn read_file(path: &Path, limit: usize) -> Result<Vec<u8>> {
    if !std::fs::symlink_metadata(path)?.is_file() || path.metadata()?.len() > limit as u64 {
        return Err("Icon comparison input must be a bounded regular file".into());
    }
    let mut result = Vec::new();
    std::fs::File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut result)?;
    if result.len() > limit {
        return Err("Icon comparison input grew beyond limit".into());
    }
    Ok(result)
}

pub fn verify(binary: &Path, icon: &Path, output: &Path) -> Result<()> {
    let source = std::env::var("GITHUB_SHA")?;
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
        || std::env::var("RUNNER_ENVIRONMENT").as_deref() != Ok("github-hosted")
        || source.len() != 40
        || !source.bytes().all(|b| b.is_ascii_hexdigit())
        || nvsf_core::BUILD_SOURCE_SHA != Some(source.as_str())
    {
        return Err("Executable icon verification requires matching hosted source".into());
    }
    let binary_bytes = read_file(binary, MAX_FILE)?;
    let icon_bytes = read_file(icon, 8 * 1024 * 1024)?;
    let proof = compare(&binary_bytes, &icon_bytes)?;
    let record = json!({"schema":1,"source_sha":source,"passed":true,"tests":1,"failed":0,"skipped":0,
        "binary":binary.file_name().and_then(|v|v.to_str()).ok_or("Invalid binary name")?,
        "binary_sha256":hash(&binary_bytes),"icon_sha256":hash(&icon_bytes),"resources":proof,
        "proof":"all first-group language descriptors and image payloads match approved ICO bytes",
        "not_proved":["shell rendering","platform signing trust","installer lifecycle"]});
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?
        .write_all(&serde_json::to_vec_pretty(&record)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put16(bytes: &mut [u8], at: usize, value: u16) {
        bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }
    fn put32(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    fn fixture() -> (Vec<u8>, Vec<u8>) {
        let mut pe = vec![0; 0x600];
        pe[..2].copy_from_slice(b"MZ");
        put32(&mut pe, 0x3c, 0x80);
        pe[0x80..0x84].copy_from_slice(b"PE\0\0");
        put16(&mut pe, 0x84, 0x8664);
        put16(&mut pe, 0x86, 1);
        put16(&mut pe, 0x94, 240);
        put16(&mut pe, 0x98, 0x20b);
        put32(&mut pe, 0x98 + 60, 0x200);
        put32(&mut pe, 0x98 + 108, 16);
        put32(&mut pe, 0x98 + 112 + 16, 0x1000);
        put32(&mut pe, 0x98 + 112 + 20, 0x200);
        let section = 0x188;
        put32(&mut pe, section + 8, 0x400);
        put32(&mut pe, section + 12, 0x1000);
        put32(&mut pe, section + 16, 0x400);
        put32(&mut pe, section + 20, 0x200);
        let resource = &mut pe[0x200..];
        for (at, count) in [(0, 2), (0x20, 1), (0x40, 1), (0x60, 1), (0x80, 1)] {
            put16(resource, at + 14, count);
        }
        for (at, name, target) in [
            (0x10, 3, 0x80000020),
            (0x18, 14, 0x80000040),
            (0x30, 1, 0x80000060),
            (0x50, 1, 0x80000080),
            (0x70, 0x409, 0xa0),
            (0x90, 0x409, 0xb0),
        ] {
            put32(resource, at, name);
            put32(resource, at + 4, target);
        }
        put32(resource, 0xa0, 0x10c0);
        put32(resource, 0xa4, 4);
        put32(resource, 0xb0, 0x10d0);
        put32(resource, 0xb4, 20);
        resource[0xc0..0xc4].copy_from_slice(b"LOGO");
        let mut ico = vec![0; 26];
        put16(&mut ico, 2, 1);
        put16(&mut ico, 4, 1);
        ico[6] = 32;
        ico[7] = 32;
        put16(&mut ico, 10, 1);
        put16(&mut ico, 12, 32);
        put32(&mut ico, 14, 4);
        put32(&mut ico, 18, 22);
        ico[22..].copy_from_slice(b"LOGO");
        resource[0xd0..0xd6].copy_from_slice(&ico[..6]);
        resource[0xd6..0xe2].copy_from_slice(&ico[6..18]);
        put16(resource, 0xe2, 1);
        (pe, ico)
    }

    #[test]
    fn compares_linked_group_and_child_bytes_not_only_resource_presence() {
        let (mut pe, ico) = fixture();
        let proof = compare(&pe, &ico).unwrap();
        assert_eq!(proof["group"], "1");
        assert_eq!(proof["images"], 1);
        assert_eq!(proof["languages"], serde_json::json!([1033]));
        pe[0x2c0] = b'X';
        assert!(compare(&pe, &ico).is_err());
    }

    #[test]
    fn rejects_missing_dangling_truncated_and_mismatched_resources() {
        let (original, ico) = fixture();
        for (offset, value) in [(0x218, 15), (0x2e2, 2), (0x2d6, 16)] {
            let mut pe = original.clone();
            pe[offset] = value;
            assert!(compare(&pe, &ico).is_err());
        }
        for end in [0, 64, 0x100, 0x2d8] {
            assert!(compare(&original[..end], &ico).is_err());
        }
        let mut bad = ico.clone();
        put32(&mut bad, 18, 0xfffffff0);
        assert!(compare(&original, &bad).is_err());
        put16(&mut bad, 4, 65);
        assert!(compare(&original, &bad).is_err());
    }

    #[test]
    fn rejects_shadowing_first_group_and_checks_every_group_language() {
        let (original, ico) = fixture();
        let mut pe = original.clone();
        put16(&mut pe, 0x24e, 2);
        put32(&mut pe, 0x250, 0);
        put32(&mut pe, 0x254, 0x800000f0);
        put32(&mut pe, 0x258, 1);
        put32(&mut pe, 0x25c, 0x80000080);
        put16(&mut pe, 0x2fe, 1);
        put32(&mut pe, 0x300, 0x409);
        put32(&mut pe, 0x304, 0x110);
        put32(&mut pe, 0x310, 0x1120);
        put32(&mut pe, 0x314, 20);
        let group = original[0x2d0..0x2e4].to_vec();
        pe[0x320..0x334].copy_from_slice(&group);
        pe[0x326] = 16;
        assert!(compare(&pe, &ico).is_err());
        let mut pe = original;
        put16(&mut pe, 0x28e, 2);
        put32(&mut pe, 0x298, 0x410);
        put32(&mut pe, 0x29c, 0xb0);
        assert!(compare(&pe, &ico).is_err());
    }

    #[test]
    fn rejects_overlapping_sections_resource_cycles_and_wrong_language() {
        let (original, ico) = fixture();
        let mut pe = original.clone();
        put16(&mut pe, 0x86, 2);
        let first = pe[0x188..0x1b0].to_vec();
        pe[0x1b0..0x1d8].copy_from_slice(&first);
        assert!(compare(&pe, &ico).is_err());
        let mut pe = original.clone();
        put32(&mut pe, 0x214, 0x80000000);
        assert!(compare(&pe, &ico).is_err());
        let mut pe = original;
        put32(&mut pe, 0x290, 0x410);
        assert!(compare(&pe, &ico).is_err());
    }
}
