use crate::{
    AppError, Paths, Result, fail,
    media::{MediaRuntime, MediaSession},
    worker::process,
};
use image::{
    ExtendedColorType, ImageEncoder, RgbImage,
    codecs::{jpeg::JpegEncoder, png::PngEncoder},
};
use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

const REQUEST: &[u8; 8] = b"NVSFPQ1\0";
const PNG: &[u8; 8] = b"NVSFPRV1";
const JPEG: &[u8; 8] = b"NVSFPRV2";
const ERROR: &[u8; 8] = b"NVSFERR1";
const PAYLOAD_LIMIT: usize = 8 * 1024 * 1024;
const ERROR_LIMIT: usize = nvsf_core::media_abi::MAX_ERROR_BYTES;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
struct Request {
    time_ms: u64,
    step: i8,
    fast: bool,
}

impl Request {
    fn new(time_ms: u64, step: i8, fast: bool) -> Result<Self> {
        if !matches!(step, -1..=2) {
            return Err(fail("Preview step must be -1, 0, 1 or 2"));
        }
        if time_ms > 359_999_999_999 {
            return Err(fail("Invalid preview timestamp"));
        }
        Ok(Self {
            time_ms,
            step,
            fast,
        })
    }

    fn bytes(self) -> [u8; 18] {
        let mut bytes = [0; 18];
        bytes[..8].copy_from_slice(REQUEST);
        bytes[8..16].copy_from_slice(&self.time_ms.to_le_bytes());
        bytes[16] = self.step as u8;
        bytes[17] = u8::from(self.fast);
        bytes
    }

    fn read(reader: &mut impl Read) -> Result<Option<Self>> {
        let mut magic = [0; 8];
        if reader.read(&mut magic[..1])? == 0 {
            return Ok(None);
        }
        reader.read_exact(&mut magic[1..])?;
        let mut time = [0; 8];
        reader.read_exact(&mut time)?;
        let mut flags = [0; 2];
        reader.read_exact(&mut flags)?;
        if &magic != REQUEST || flags[1] > 1 {
            return Err(fail("Invalid preview request header"));
        }
        Self::new(u64::from_le_bytes(time), flags[0] as i8, flags[1] == 1).map(Some)
    }
}

#[derive(Debug)]
pub struct Packet {
    header: [u8; 24],
    payload: Vec<u8>,
}

impl Packet {
    pub fn new(time_ms: u64, duration_ms: u64, fast: bool, payload: Vec<u8>) -> Self {
        let mut header = [0; 24];
        header[..8].copy_from_slice(if fast { JPEG } else { PNG });
        header[8..16].copy_from_slice(&time_ms.to_le_bytes());
        header[16..].copy_from_slice(&duration_ms.to_le_bytes());
        Self { header, payload }
    }

    pub fn into_ipc(self) -> Vec<u8> {
        [self.header.as_slice(), self.payload.as_slice()].concat()
    }

    fn write(&self, writer: &mut impl Write) -> Result<()> {
        writer.write_all(&self.header)?;
        writer.write_all(&(self.payload.len() as u32).to_le_bytes())?;
        writer.write_all(&self.payload)?;
        writer.flush()?;
        Ok(())
    }

    fn read(reader: &mut impl Read) -> Result<Self> {
        let mut header = [0; 24];
        reader.read_exact(&mut header[..8])?;
        match &header[..8] {
            magic if magic == ERROR => {
                let message = read_payload(reader, ERROR_LIMIT)?;
                let message =
                    String::from_utf8(message).map_err(|_| fail("Invalid preview error text"))?;
                Err(fail(message))
            }
            magic if magic == PNG || magic == JPEG => {
                reader.read_exact(&mut header[8..])?;
                Ok(Self {
                    header,
                    payload: read_payload(reader, PAYLOAD_LIMIT)?,
                })
            }
            _ => Err(fail("Invalid preview reply header")),
        }
    }
}

fn read_payload(reader: &mut impl Read, limit: usize) -> Result<Vec<u8>> {
    let mut length = [0; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    if !(1..=limit).contains(&length) {
        return Err(fail("Preview reply exceeds payload bound"));
    }
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload)?;
    Ok(payload)
}

fn write_error(writer: &mut impl Write, message: &str) -> Result<()> {
    let end = message.floor_char_boundary(ERROR_LIMIT.min(message.len()));
    writer.write_all(ERROR)?;
    writer.write_all(&(end as u32).to_le_bytes())?;
    writer.write_all(&message.as_bytes()[..end])?;
    writer.flush()?;
    Ok(())
}

pub struct PreviewDaemon {
    path: PathBuf,
    child: process::Process,
    replies: Option<mpsc::Receiver<Result<Packet>>>,
    reader: Option<std::thread::JoinHandle<()>>,
}

impl PreviewDaemon {
    pub fn spawn(paths: &Paths, path: &Path) -> Result<Self> {
        let path = path.canonicalize()?;
        if !path.is_file() {
            return Err(fail("Preview needs a regular local source"));
        }
        let mut command = Command::new(process::executable()?);
        command
            .args(["worker", "preview-daemon", "--input"])
            .arg(&path)
            .arg("--data-root")
            .arg(&paths.root)
            .env("NVSF_RUNTIME_ROOT", &paths.runtimes)
            .env("OMP_NUM_THREADS", "2")
            .stdout(Stdio::piped());
        let mut child = process::Process::spawn(&mut command)?;
        let mut stdout = io::BufReader::new(child.stdout()?);
        let (send, replies) = mpsc::sync_channel(1);
        let reader = std::thread::Builder::new()
            .name("preview-daemon-replies".into())
            .spawn(move || {
                loop {
                    let packet = Packet::read(&mut stdout);
                    let failed = packet.is_err();
                    if send.send(packet).is_err() || failed {
                        break;
                    }
                }
            })?;
        Ok(Self {
            path,
            child,
            replies: Some(replies),
            reader: Some(reader),
        })
    }

    pub fn matches(&self, path: &Path) -> bool {
        self.path == path
    }

    pub fn request(
        &mut self,
        time_ms: u64,
        step: i8,
        fast: bool,
        cancel: &AtomicBool,
    ) -> Result<Packet> {
        let request = Request::new(time_ms, step, fast)?;
        if cancel.load(Ordering::Acquire) {
            return Err(AppError::Cancelled);
        }
        self.child.request_pipe()?.write_all(&request.bytes())?;
        let deadline = Instant::now() + Duration::from_secs(60);
        let replies = self
            .replies
            .as_ref()
            .ok_or_else(|| fail("Preview daemon has stopped"))?;
        loop {
            if cancel.load(Ordering::Acquire) {
                return Err(AppError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(fail("Preview daemon did not reply within 60 seconds"));
            }
            match replies.recv_timeout(Duration::from_millis(10)) {
                Ok(packet) => {
                    let packet = packet?;
                    if &packet.header[..8] != if fast { JPEG } else { PNG } {
                        return Err(fail("Preview daemon returned the wrong image format"));
                    }
                    return Ok(packet);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(fail("Preview daemon stopped without a reply"));
                }
            }
        }
    }
}

impl Drop for PreviewDaemon {
    fn drop(&mut self) {
        self.replies.take();
        if self.child.finish_preview().is_ok()
            && let Some(reader) = self.reader.take()
        {
            let _ = reader.join();
        }
    }
}

/// Hidden worker entry. The owner guard starts before FFmpeg is loaded.
pub fn run(path: &Path, data_root: Option<&Path>) -> Result<()> {
    process::guard_parent_pipe(true)?;
    let paths = Paths::new(data_root)?;
    let log = crate::logging::Log::new(&paths, "nvsf-preview-daemon");
    log.event("INFO", "PREVIEW", "Starting persistent preview worker");
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut work = || -> Result<()> {
        let runtime = MediaRuntime::load(&paths)?;
        let mut session = MediaSession::open(&runtime, path)?;
        while let Some(request) = Request::read(&mut input)? {
            let frame = session.frame(
                request.time_ms,
                request.step,
                if request.fast { 1280 } else { 0 },
            )?;
            let image = encode(&frame.image, request.fast)?;
            Packet::new(
                frame.time_ms,
                frame.estimated_duration_ms,
                request.fast,
                image,
            )
            .write(&mut output)?;
        }
        Ok(())
    };
    if let Err(error) = work() {
        log.event(
            "WARNING",
            "PREVIEW",
            "Preview worker stopped after a request failed",
        );
        return write_error(&mut output, &error.to_string());
    }
    log.event("INFO", "PREVIEW", "Preview worker stopped");
    Ok(())
}

fn encode(rgb: &RgbImage, fast: bool) -> Result<Vec<u8>> {
    let resized;
    let rgb = if fast && rgb.width() > 1280 {
        let height = (u64::from(rgb.height()) * 1280 / u64::from(rgb.width())).max(1) as u32;
        resized = image::imageops::resize(rgb, 1280, height, image::imageops::FilterType::Triangle);
        &resized
    } else {
        rgb
    };
    let mut bytes = Vec::new();
    let result = if fast {
        JpegEncoder::new_with_quality(&mut bytes, 84).encode(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            ExtendedColorType::Rgb8,
        )
    } else {
        PngEncoder::new(&mut bytes).write_image(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            ExtendedColorType::Rgb8,
        )
    };
    result.map_err(|error| fail(format!("Cannot encode preview: {error}")))?;
    if bytes.len() > PAYLOAD_LIMIT {
        return Err(fail("Preview exceeds 8MiB IPC bound"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn binary_requests_round_trip_and_reject_bad_flags() {
        let request = Request::new(1234, 2, true).unwrap();
        let mut bytes = request.bytes();
        assert_eq!(&bytes[..8], b"NVSFPQ1\0");
        assert_eq!(Request::read(&mut &bytes[..]).unwrap(), Some(request));
        assert!(Request::read(&mut &bytes[..17]).is_err());
        bytes[17] = 2;
        assert!(Request::read(&mut &bytes[..]).is_err());
        assert_eq!(Request::read(&mut &[][..]).unwrap(), None);
    }

    #[test]
    fn worker_and_ipc_use_different_success_headers() {
        for (fast, magic) in [(false, b"NVSFPRV1"), (true, b"NVSFPRV2")] {
            let original = Packet::new(300, 42, fast, vec![1, 2, 3]);
            let mut wire = Vec::new();
            original.write(&mut wire).unwrap();
            assert_eq!(&wire[..8], magic);
            assert_eq!(&wire[24..28], &3u32.to_le_bytes());
            let packet = Packet::read(&mut &wire[..]).unwrap();
            let ipc = packet.into_ipc();
            assert_eq!(&ipc[..24], &wire[..24]);
            assert_eq!(&ipc[24..], &[1, 2, 3]);
        }
    }

    #[test]
    fn error_frames_preserve_utf8_and_reject_oversized_replies() {
        let mut wire = Vec::new();
        write_error(&mut wire, "Cannot decode: فارسی").unwrap();
        assert_eq!(&wire[..8], b"NVSFERR1");
        assert!(
            Packet::read(&mut &wire[..])
                .unwrap_err()
                .to_string()
                .contains("فارسی")
        );
        wire[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(
            Packet::read(&mut &wire[..])
                .unwrap_err()
                .to_string()
                .contains("bound")
        );
    }

    #[test]
    fn jpeg_caps_width_without_upscaling_and_png_keeps_source_pixels() {
        for (width, target) in [(1920u32, 1280u32), (640, 640)] {
            let rgb = image::RgbImage::from_pixel(width, 60, image::Rgb([12, 34, 56]));
            let jpeg = encode(&rgb, true).unwrap();
            assert_eq!(
                image::guess_format(&jpeg).unwrap(),
                image::ImageFormat::Jpeg
            );
            let decoded = image::ImageReader::new(Cursor::new(jpeg))
                .with_guessed_format()
                .unwrap()
                .decode()
                .unwrap();
            assert_eq!(
                (decoded.width(), decoded.height()),
                (target, 60 * target / width)
            );
            let png = encode(&rgb, false).unwrap();
            assert_eq!(image::load_from_memory(&png).unwrap().into_rgb8(), rgb);
        }
    }
}
