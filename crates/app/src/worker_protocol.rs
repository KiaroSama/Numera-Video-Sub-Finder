use crate::{Result, fail};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    io::{Read, Write},
    path::Path,
};

pub const CONTROL_LIMIT: usize = 1024 * 1024;
pub const EVENT_LIMIT: usize = 64 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope<T> {
    schema: u32,
    job_id: String,
    payload: T,
}

pub fn job_id(directory: &Path) -> Result<&str> {
    let id = directory
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or_else(|| fail("Invalid worker job directory"))?;
    validate_id(id)?;
    Ok(id)
}

fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        || matches!(id, "." | "..")
    {
        return Err(fail("Invalid worker job identity"));
    }
    Ok(())
}

struct Bounded {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other("Worker message exceeds byte limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub fn encode<T: Serialize + ?Sized>(job_id: &str, payload: &T, limit: usize) -> Result<Vec<u8>> {
    validate_id(job_id)?;
    let mut writer = Bounded {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(
        &mut writer,
        &Envelope {
            schema: 1,
            job_id: job_id.into(),
            payload,
        },
    )?;
    Ok(writer.bytes)
}

pub fn decode<T: DeserializeOwned>(bytes: &[u8], job_id: &str, limit: usize) -> Result<T> {
    validate_id(job_id)?;
    if bytes.len() > limit {
        return Err(fail("Worker message exceeds byte limit"));
    }
    let envelope: Envelope<T> = serde_json::from_slice(bytes)?;
    if envelope.schema != 1 || envelope.job_id != job_id {
        return Err(fail("Worker message version or job identity mismatch"));
    }
    Ok(envelope.payload)
}

pub fn read<T: DeserializeOwned>(path: &Path, job_id: &str, limit: usize) -> Result<T> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    decode(&bytes, job_id, limit)
}

pub fn flag(directory: &Path, job_id: &str, name: &str) -> Result<bool> {
    if !matches!(name, "pause" | "cancel") {
        return Err(fail("Invalid worker control operation"));
    }
    let mut file = match std::fs::File::open(directory.join(name)) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let mut bytes = Vec::new();
    (&mut file).take(1025).read_to_end(&mut bytes)?;
    let operation: String = decode(&bytes, job_id, 1024)?;
    if operation != name {
        return Err(fail("Worker control operation mismatch"));
    }
    Ok(true)
}
