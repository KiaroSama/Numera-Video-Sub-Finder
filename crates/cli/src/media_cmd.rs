use crate::{PreviewCommand, RegionsCommand};
use nvsf_app::{
    AppError, Control, Paths, Result, media,
    native_request::{self, Operation, Response},
    storage,
};
use nvsf_core::Rect;
use std::path::Path;

pub(crate) fn run_regions(paths: &Paths, command: RegionsCommand, control: &Control) -> Result<()> {
    match command {
        RegionsCommand::Suggest { input, language } => {
            let (response, _) = native_request::supervise(
                paths,
                &input,
                Operation::Suggest {
                    language: language.clone(),
                },
                &control.cancel,
            )?;
            let regions = match response {
                Response::Suggest(regions) => regions,
                _ => return Err(nvsf_app::fail("Unexpected region suggestion response")),
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "input": input,
                    "language": language,
                    "regions": regions,
                }))?
            );
            Ok(())
        }
    }
}

pub(crate) fn run_preview(paths: &Paths, command: PreviewCommand, control: &Control) -> Result<()> {
    match command {
        PreviewCommand::Frame {
            input,
            at_ms,
            output,
            crop,
            overwrite,
        } => {
            if let Some(rect) = crop {
                rect.validate()
                    .map_err(|e| AppError::InvalidConfig(e.to_string()))?;
                save_cropped_frame(paths, &input, at_ms, &output, rect, overwrite)
            } else {
                let (response, bytes) = native_request::supervise(
                    paths,
                    &input,
                    Operation::Preview { at_ms },
                    &control.cancel,
                )?;
                let (width, height) = match response {
                    Response::Preview { width, height, .. } => (width, height),
                    _ => return Err(nvsf_app::fail("Unexpected preview worker response")),
                };
                storage::write_atomic(&output, &bytes, overwrite)?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "input": input,
                        "at_ms": at_ms,
                        "output": output,
                        "width": width,
                        "height": height,
                        "bytes": bytes.len(),
                        "cropped": false,
                    }))?
                );
                Ok(())
            }
        }
    }
}

fn save_cropped_frame(
    paths: &Paths,
    input: &Path,
    at_ms: u64,
    output: &Path,
    rect: Rect,
    overwrite: bool,
) -> Result<()> {
    let frame = media::MediaRuntime::load(paths)?.preview(input, at_ms)?;
    let frame_w = frame.width();
    let frame_h = frame.height();
    let (x, y, w, h) = rect.pixels(frame_w, frame_h)?;
    let raw = frame.as_raw();
    let stride = (frame_w as usize)
        .checked_mul(3)
        .ok_or_else(|| nvsf_app::fail("Preview frame stride overflow"))?;
    let mut cropped = Vec::with_capacity((w as usize).saturating_mul(h as usize).saturating_mul(3));
    for row in 0..h as usize {
        let row_start = (y as usize + row)
            .checked_mul(stride)
            .and_then(|base| base.checked_add((x as usize).checked_mul(3)?))
            .ok_or_else(|| nvsf_app::fail("Preview crop offset overflow"))?;
        let row_end = row_start
            .checked_add((w as usize).saturating_mul(3))
            .ok_or_else(|| nvsf_app::fail("Preview crop row overflow"))?;
        let slice = raw
            .get(row_start..row_end)
            .ok_or_else(|| nvsf_app::fail("Preview crop exceeds frame bounds"))?;
        cropped.extend_from_slice(slice);
    }
    let png = encode_rgb_png(w, h, &cropped)?;
    storage::write_atomic(output, &png, overwrite)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "input": input,
            "at_ms": at_ms,
            "output": output,
            "width": w,
            "height": h,
            "crop": rect,
            "bytes": png.len(),
            "cropped": true,
        }))?
    );
    Ok(())
}

/// Encodes 8-bit RGB24 pixels as a spec-compliant PNG using stored DEFLATE blocks (std-only).
pub(crate) fn encode_rgb_png(width: u32, height: u32, rgb: &[u8]) -> Result<Vec<u8>> {
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|px| px.checked_mul(3))
        .ok_or_else(|| nvsf_app::fail("PNG image dimensions overflow"))?;
    if width == 0 || height == 0 || rgb.len() != expected {
        return Err(nvsf_app::fail("Invalid RGB buffer dimensions for PNG"));
    }
    let row_bytes = (width as usize) * 3;
    let mut raw_scanlines = Vec::with_capacity((row_bytes + 1) * height as usize);
    for row in rgb.chunks_exact(row_bytes) {
        raw_scanlines.push(0u8);
        raw_scanlines.extend_from_slice(row);
    }
    let zlib = zlib_stored(&raw_scanlines);
    let mut out = Vec::with_capacity(zlib.len() + 64);
    out.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);
    let mut ihdr = [0u8; 13];
    ihdr[0..4].copy_from_slice(&width.to_be_bytes());
    ihdr[4..8].copy_from_slice(&height.to_be_bytes());
    ihdr[8] = 8; // bit depth
    ihdr[9] = 2; // RGB color type
    push_png_chunk(&mut out, *b"IHDR", &ihdr)?;
    push_png_chunk(&mut out, *b"IDAT", &zlib)?;
    push_png_chunk(&mut out, *b"IEND", &[])?;
    Ok(out)
}

fn zlib_stored(data: &[u8]) -> Vec<u8> {
    const MAX_BLOCK: usize = 65_535;
    let block_count = data.len().div_ceil(MAX_BLOCK).max(1);
    let mut out = Vec::with_capacity(2 + data.len() + block_count * 5 + 4);
    out.extend_from_slice(&[0x78, 0x01]);
    if data.is_empty() {
        out.extend_from_slice(&[0x01, 0x00, 0x00, 0xff, 0xff]);
    } else {
        for (idx, chunk) in data.chunks(MAX_BLOCK).enumerate() {
            let is_last = idx + 1 == block_count;
            let len = chunk.len() as u16;
            out.push(u8::from(is_last));
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&(!len).to_le_bytes());
            out.extend_from_slice(chunk);
        }
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn push_png_chunk(out: &mut Vec<u8>, kind: [u8; 4], payload: &[u8]) -> Result<()> {
    let len =
        u32::try_from(payload.len()).map_err(|_| nvsf_app::fail("PNG chunk exceeds 4GiB limit"))?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&kind);
    out.extend_from_slice(payload);
    let crc = crc32(&kind, payload);
    out.extend_from_slice(&crc.to_be_bytes());
    Ok(())
}

fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    let mut a = 1u32;
    let mut b = 0u32;
    for &byte in data {
        a = (a + u32::from(byte)) % MOD;
        b = (b + a) % MOD;
    }
    (b << 16) | a
}

fn crc32(kind: &[u8; 4], payload: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in kind.iter().chain(payload.iter()) {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_png_encoder_writes_valid_signature_and_chunks() {
        let rgb = [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255];
        let png = encode_rgb_png(2, 2, &rgb).expect("encode 2x2 rgb");
        assert_eq!(&png[0..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(u32::from_be_bytes([png[16], png[17], png[18], png[19]]), 2);
        assert_eq!(u32::from_be_bytes([png[20], png[21], png[22], png[23]]), 2);
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
        let iend_crc = u32::from_be_bytes([
            png[png.len() - 4],
            png[png.len() - 3],
            png[png.len() - 2],
            png[png.len() - 1],
        ]);
        assert_eq!(iend_crc, 0xae42_6082);
    }

    #[test]
    fn rgb_png_encoder_rejects_mismatched_buffer_len() {
        assert!(encode_rgb_png(2, 2, &[0u8; 3]).is_err());
        assert!(encode_rgb_png(0, 2, &[]).is_err());
    }
}
