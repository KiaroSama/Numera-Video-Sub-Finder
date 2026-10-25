use crate::{
    Result, fail,
    storage::{JobLock, hash_file},
};
use std::{
    io::{Read, Write},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub fn install(
    url: &str,
    bytes: u64,
    sha256: &str,
    destination: &Path,
    cancel: &AtomicBool,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(600);
    if !url.starts_with("https://")
        || bytes == 0
        || bytes > 2 * 1024 * 1024 * 1024
        || sha256.len() != 64
        || !sha256.bytes().all(|c| c.is_ascii_hexdigit())
    {
        return Err(fail("Invalid immutable model download contract"));
    }
    let _lock = JobLock::acquire(&destination.with_extension("download.lock"))?;
    let partial = destination.with_extension("partial");
    if partial.is_symlink() || destination.is_symlink() {
        return Err(fail("Model destination must not be a symlink"));
    }
    if destination.is_file()
        && destination.metadata()?.len() == bytes
        && hash_file(destination)? == sha256
    {
        return Ok(());
    }
    let offset = if partial.is_file() {
        partial.metadata()?.len()
    } else {
        0
    };
    if offset > bytes {
        return Err(fail(
            "Partial model exceeds expected bytes; preserve it and choose a clean destination",
        ));
    }
    check_transfer(cancel, deadline)?;
    if offset == bytes {
        return activate(&partial, destination, bytes, sha256, cancel, deadline);
    }
    let client = reqwest::blocking::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::limited(4))
        .build()
        .map_err(|e| fail(e.to_string()))?;
    acquire(
        &client,
        url,
        offset,
        &Transfer {
            bytes,
            sha256,
            destination,
            cancel,
            deadline,
        },
    )
}

fn acquire(
    client: &reqwest::blocking::Client,
    url: &str,
    offset: u64,
    transfer: &Transfer<'_>,
) -> Result<()> {
    let mut request = client.get(url);
    if offset > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={offset}-"));
    }
    check_transfer(transfer.cancel, transfer.deadline)?;
    let response = request.send();
    check_transfer(transfer.cancel, transfer.deadline)?;
    let mut response = response
        .and_then(|r| r.error_for_status())
        .map_err(|e| fail(format!("Model download failed: {e}")))?;
    if !matches!(
        response.status(),
        reqwest::StatusCode::OK | reqwest::StatusCode::PARTIAL_CONTENT
    ) {
        return Err(fail(
            "Model response must contain a complete or resumed payload",
        ));
    }
    let partial_content = response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    let range = response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    transfer.receive(offset, partial_content, &range, &mut response)
}

struct Transfer<'a> {
    bytes: u64,
    sha256: &'a str,
    destination: &'a Path,
    cancel: &'a AtomicBool,
    deadline: Instant,
}

impl Transfer<'_> {
    fn receive(
        &self,
        mut offset: u64,
        partial_content: bool,
        range: &str,
        mut response: impl Read,
    ) -> Result<()> {
        let Self {
            bytes,
            sha256,
            destination,
            cancel,
            deadline,
        } = *self;
        let partial = destination.with_extension("partial");
        if offset > 0 && !partial_content {
            offset = 0;
        }
        if partial_content
            && (offset >= bytes || range != format!("bytes {offset}-{}/{bytes}", bytes - 1))
        {
            return Err(fail("Invalid resumed model response range"));
        }
        check_transfer(cancel, deadline)?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(offset == 0)
            .open(&partial)?;
        use std::io::Seek;
        file.seek(std::io::SeekFrom::Start(offset))?;
        let mut buffer = [0u8; 65536];
        let mut received = offset;
        loop {
            check_transfer(cancel, deadline)?;
            let n = match response.read(&mut buffer) {
                Ok(count) => count,
                Err(error) => {
                    check_transfer(cancel, deadline)?;
                    return Err(error.into());
                }
            };
            if n == 0 {
                check_transfer(cancel, deadline)?;
                break;
            }
            received = received
                .checked_add(n as u64)
                .ok_or_else(|| fail("Model size overflow"))?;
            if received > bytes {
                return Err(fail("Model download exceeds declared size"));
            }
            file.write_all(&buffer[..n])?;
        }
        file.flush()?;
        file.sync_all()?;
        drop(file);
        if received != bytes {
            return Err(fail("Incomplete model download; partial bytes retained"));
        }
        activate(&partial, destination, bytes, sha256, cancel, deadline)
    }
}

fn check_transfer(cancel: &AtomicBool, deadline: Instant) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        return Err(crate::AppError::Cancelled);
    }
    if Instant::now() >= deadline {
        return Err(fail(
            "Model transfer exceeded 600-second deadline; partial bytes retained",
        ));
    }
    Ok(())
}

fn activate(
    partial: &Path,
    destination: &Path,
    bytes: u64,
    sha256: &str,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<()> {
    check_transfer(cancel, deadline)?;
    if partial.metadata()?.len() != bytes || hash_file(partial)? != sha256 {
        return Err(fail(
            "Model integrity failed; partial bytes retained, existing model unchanged",
        ));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| fail("Missing model parent"))?;
    check_transfer(cancel, deadline)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    let mut source = std::fs::File::open(partial)?.take(bytes + 1);
    let mut buffer = [0u8; 65536];
    loop {
        check_transfer(cancel, deadline)?;
        let count = source.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        tmp.write_all(&buffer[..count])?;
    }
    drop(source);
    tmp.as_file().sync_all()?;
    if tmp.as_file().metadata()?.len() != bytes || hash_file(tmp.path())? != sha256 {
        return Err(fail(
            "Model changed during activation; existing model preserved",
        ));
    }
    check_transfer(cancel, deadline)?;
    tmp.persist(destination).map_err(|e| e.error)?;
    std::fs::remove_file(partial)?;
    Ok(())
}

#[cfg(test)]
#[path = "model_download_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "model_http_tests.rs"]
mod http_tests;
