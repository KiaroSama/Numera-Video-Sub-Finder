use crate::{Paths, Result, fail};
use sha2::{Digest, Sha256};
use std::{io::Read, path::Path};

/// Reads only a content-bound PNG in this application's crop cache, never an arbitrary project path.
pub fn read(paths: &Paths, recorded: &str) -> Result<Vec<u8>> {
    let path = Path::new(recorded);
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| fail("Invalid crop evidence name"))?;
    let hash = name
        .strip_prefix("crop-")
        .and_then(|s| s.strip_suffix(".png"))
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| fail("Invalid crop evidence identity"))?;
    let cache = paths.cache.canonicalize()?;
    if path.is_symlink()
        || path
            .parent()
            .ok_or_else(|| fail("Missing crop parent"))?
            .canonicalize()?
            != cache
    {
        return Err(fail(
            "Crop evidence must remain inside the application cache",
        ));
    }
    if !std::fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(fail("Crop evidence must be a regular file"));
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Native O_NONBLOCK | O_NOFOLLOW: refuse swapped links and never wait on a FIFO open.
        #[cfg(target_os = "macos")]
        const FLAGS: i32 = 0x4 | 0x100;
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        const FLAGS: i32 = 2048 | 0x20000;
        #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
        const FLAGS: i32 = 2048 | 0x8000;
        options.custom_flags(FLAGS);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000).share_mode(1); // OPEN_REPARSE_POINT, FILE_SHARE_READ
    }
    let file = options.open(cache.join(name))?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if file.metadata()?.file_attributes() & 0x400 != 0 {
            return Err(fail("Crop evidence must not be a reparse point"));
        }
    }
    if !file.metadata()?.file_type().is_file() {
        return Err(fail("Crop evidence must be a regular file"));
    }
    let mut bytes = Vec::new();
    file.take(8 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(fail("Crop evidence exceeds 8MiB preview limit"));
    }
    let dimensions =
        image::ImageReader::with_format(std::io::Cursor::new(&bytes), image::ImageFormat::Png)
            .into_dimensions()
            .map_err(|e| fail(format!("Invalid crop evidence dimensions: {e}")))?;
    if dimensions.0 == 0
        || dimensions.1 == 0
        || dimensions.0 > 8192
        || dimensions.1 > 8192
        || u64::from(dimensions.0) * u64::from(dimensions.1) > 33_177_600
    {
        return Err(fail("Crop evidence exceeds pixel limit"));
    }
    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(&bytes), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    use image::ImageDecoder;
    let decoder = reader
        .into_decoder()
        .map_err(|e| fail(format!("Invalid crop evidence: {e}")))?;
    if decoder.color_type() != image::ColorType::Rgb8 {
        return Err(fail("Crop evidence must use the original RGB8 format"));
    }
    let image = image::DynamicImage::from_decoder(decoder)
        .map_err(|e| fail(format!("Invalid crop evidence: {e}")))?
        .into_rgb8();
    if u64::from(image.width()) * u64::from(image.height()) > 33_177_600 {
        return Err(fail("Crop evidence exceeds pixel limit"));
    }
    let mut digest = Sha256::new();
    digest.update(image.width().to_le_bytes());
    digest.update(image.height().to_le_bytes());
    digest.update(image.as_raw());
    let actual: String = digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if actual != hash {
        return Err(fail(
            "Crop evidence content differs from its recorded identity",
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn crop_preview_requires_contained_original_pixels() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::new(Some(root.path())).unwrap();
        let pixels = image::RgbImage::from_pixel(4, 2, image::Rgb([23, 42, 81]));
        let mut hash = Sha256::new();
        hash.update(4u32.to_le_bytes());
        hash.update(2u32.to_le_bytes());
        hash.update(pixels.as_raw());
        let name = format!("crop-{:x}.png", hash.finalize());
        let path = paths.cache.join(&name);
        pixels.save(&path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(read(&paths, path.to_str().unwrap()).unwrap(), bytes);
        let outside = root.path().join(&name);
        pixels.save(&outside).unwrap();
        assert!(read(&paths, outside.to_str().unwrap()).is_err());
        let fake = paths.cache.join(format!("crop-{}.png", "0".repeat(64)));
        pixels.save(&fake).unwrap();
        assert!(read(&paths, fake.to_str().unwrap()).is_err());
        image::RgbaImage::new(4, 2).save(&path).unwrap();
        assert!(
            read(&paths, path.to_str().unwrap())
                .unwrap_err()
                .to_string()
                .contains("RGB8")
        );
        let oversized = image::RgbImage::new(8193, 1);
        oversized.save(&path).unwrap();
        assert!(
            read(&paths, path.to_str().unwrap())
                .unwrap_err()
                .to_string()
                .contains("pixel limit")
        );
        std::fs::write(&path, b"not PNG").unwrap();
        assert!(read(&paths, path.to_str().unwrap()).is_err());
        #[cfg(unix)]
        {
            std::fs::remove_file(&path).unwrap();
            std::os::unix::fs::symlink(&outside, &path).unwrap();
            assert!(read(&paths, path.to_str().unwrap()).is_err());
            std::fs::remove_file(&path).unwrap();
            use std::os::unix::ffi::OsStrExt;
            #[cfg(target_os = "macos")]
            type Mode = u16;
            #[cfg(target_os = "linux")]
            type Mode = u32;
            unsafe extern "C" {
                fn mkfifo(path: *const std::ffi::c_char, mode: Mode) -> i32;
            }
            let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
            // SAFETY: valid terminated path in the owned fixture directory; no writer or process.
            assert_eq!(unsafe { mkfifo(name.as_ptr(), 0o600) }, 0);
            assert!(
                read(&paths, path.to_str().unwrap())
                    .unwrap_err()
                    .to_string()
                    .contains("regular file")
            );
        }
    }
}
