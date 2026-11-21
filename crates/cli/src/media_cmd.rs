use crate::{
    PreviewCommand, RegionsCommand,
    frame_overlay::{
        PixelBox, box_color, draw_coordinate_grid, draw_rect_overlay, format_region_arg,
        pixel_box_to_rect, rect_to_pixel_box, sample_timestamps_ms,
    },
};
use nvsf_app::{
    AppError, Control, Paths, Result, media,
    native_request::{self, Operation, Response},
    storage,
};
use nvsf_core::Rect;
use std::{path::Path, sync::atomic::Ordering};

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

struct RenderOptions<'a> {
    crop: Option<Rect>,
    grid: bool,
    draw_boxes: &'a [Rect],
    pixel_boxes: &'a [PixelBox],
    overwrite: bool,
}

pub(crate) fn run_preview(paths: &Paths, command: PreviewCommand, control: &Control) -> Result<()> {
    match command {
        PreviewCommand::Frame {
            input,
            at_ms,
            output,
            crop,
            grid,
            draw_box,
            pixel_box,
            overwrite,
        } => {
            if crop.is_none() && !grid && draw_box.is_empty() && pixel_box.is_empty() {
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
                        "frame_width": width,
                        "frame_height": height,
                        "width": width,
                        "height": height,
                        "bytes": bytes.len(),
                        "cropped": false,
                        "grid": false,
                        "boxes": [],
                    }))?
                );
                Ok(())
            } else {
                let runtime = media::MediaRuntime::load(paths)?;
                let opts = RenderOptions {
                    crop,
                    grid,
                    draw_boxes: &draw_box,
                    pixel_boxes: &pixel_box,
                    overwrite,
                };
                let report = render_frame_to_png(&runtime, &input, at_ms, &output, &opts)?;
                println!("{}", serde_json::to_string_pretty(&report)?);
                Ok(())
            }
        }
        PreviewCommand::Strip {
            input,
            output_dir,
            count,
            start_ms,
            end_ms,
            crop,
            grid,
            draw_box,
            pixel_box,
            overwrite,
        } => {
            let runtime = media::MediaRuntime::load(paths)?;
            let media_info = runtime.probe(&input)?;
            let duration_ms = media_info.duration_ms.unwrap_or(start_ms);
            let timestamps = sample_timestamps_ms(duration_ms, start_ms, end_ms, count)?;
            std::fs::create_dir_all(&output_dir)?;
            let opts = RenderOptions {
                crop,
                grid,
                draw_boxes: &draw_box,
                pixel_boxes: &pixel_box,
                overwrite,
            };
            let mut frames = Vec::with_capacity(timestamps.len());
            for (idx, at_ms) in timestamps.into_iter().enumerate() {
                if control.cancel.load(Ordering::Acquire) {
                    return Err(AppError::Cancelled);
                }
                let file = output_dir.join(format!("frame-{:03}-{at_ms}ms.png", idx + 1));
                frames.push(render_frame_to_png(&runtime, &input, at_ms, &file, &opts)?);
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "input": input,
                    "output_dir": output_dir,
                    "duration_ms": media_info.duration_ms,
                    "frame_width": media_info.width,
                    "frame_height": media_info.height,
                    "count": frames.len(),
                    "grid": grid,
                    "frames": frames,
                }))?
            );
            Ok(())
        }
    }
}

fn render_frame_to_png(
    runtime: &media::MediaRuntime,
    input: &Path,
    at_ms: u64,
    output: &Path,
    opts: &RenderOptions<'_>,
) -> Result<serde_json::Value> {
    let frame = runtime.preview(input, at_ms)?;
    let frame_w = frame.width();
    let frame_h = frame.height();
    render_rgb_to_png(
        input,
        at_ms,
        frame_w,
        frame_h,
        frame.into_raw(),
        output,
        opts,
    )
}

fn render_rgb_to_png(
    input: &Path,
    at_ms: u64,
    frame_w: u32,
    frame_h: u32,
    mut rgb: Vec<u8>,
    output: &Path,
    opts: &RenderOptions<'_>,
) -> Result<serde_json::Value> {
    if let Some(rect) = opts.crop {
        rect.validate()
            .map_err(|e| AppError::InvalidConfig(e.to_string()))?;
    }

    if opts.grid {
        draw_coordinate_grid(&mut rgb, frame_w, frame_h);
    }

    let mut resolved_boxes = Vec::with_capacity(opts.draw_boxes.len() + opts.pixel_boxes.len());
    for &rect in opts.draw_boxes {
        rect.validate()
            .map_err(|e| AppError::InvalidConfig(e.to_string()))?;
        resolved_boxes.push(rect);
    }
    for &px in opts.pixel_boxes {
        resolved_boxes.push(pixel_box_to_rect(px, frame_w, frame_h)?);
    }

    let mut box_reports = Vec::with_capacity(resolved_boxes.len());
    for (idx, rect) in resolved_boxes.into_iter().enumerate() {
        let px = draw_rect_overlay(&mut rgb, frame_w, frame_h, rect, box_color(idx))?;
        box_reports.push(serde_json::json!({
            "index": idx,
            "normalized": rect,
            "region_arg": format_region_arg(rect),
            "pixels": px,
        }));
    }

    let (out_w, out_h, out_rgb, crop_pixels) = if let Some(rect) = opts.crop {
        let px = rect_to_pixel_box(rect, frame_w, frame_h)?;
        let cropped = crop_rgb_buffer(&rgb, frame_w, px)?;
        (px.width, px.height, cropped, Some(px))
    } else {
        (frame_w, frame_h, rgb, None)
    };

    let png = encode_rgb_png(out_w, out_h, &out_rgb)?;
    storage::write_atomic(output, &png, opts.overwrite)?;
    Ok(serde_json::json!({
        "input": input,
        "at_ms": at_ms,
        "output": output,
        "frame_width": frame_w,
        "frame_height": frame_h,
        "width": out_w,
        "height": out_h,
        "crop": opts.crop,
        "crop_pixels": crop_pixels,
        "cropped": opts.crop.is_some(),
        "grid": opts.grid,
        "boxes": box_reports,
        "bytes": png.len(),
    }))
}

fn crop_rgb_buffer(raw: &[u8], frame_w: u32, px: PixelBox) -> Result<Vec<u8>> {
    let stride = (frame_w as usize)
        .checked_mul(3)
        .ok_or_else(|| nvsf_app::fail("Preview frame stride overflow"))?;
    let mut cropped = Vec::with_capacity(
        (px.width as usize)
            .saturating_mul(px.height as usize)
            .saturating_mul(3),
    );
    for row in 0..px.height as usize {
        let row_start = (px.y as usize + row)
            .checked_mul(stride)
            .and_then(|base| base.checked_add((px.x as usize).checked_mul(3)?))
            .ok_or_else(|| nvsf_app::fail("Preview crop offset overflow"))?;
        let row_end = row_start
            .checked_add((px.width as usize).saturating_mul(3))
            .ok_or_else(|| nvsf_app::fail("Preview crop row overflow"))?;
        let slice = raw
            .get(row_start..row_end)
            .ok_or_else(|| nvsf_app::fail("Preview crop exceeds frame bounds"))?;
        cropped.extend_from_slice(slice);
    }
    Ok(cropped)
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

    #[test]
    fn render_rgb_to_png_draws_grid_boxes_crops_and_honors_overwrite() {
        let dir = std::env::temp_dir().join(format!("nvsf-media-cmd-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let out = dir.join("annotated.png");
        let input = dir.join("sample.mp4");
        let rgb = vec![24u8; 100 * 80 * 3];
        let draw = [Rect::new(0.1, 0.7, 0.8, 0.2).expect("rect")];
        let px_box = [PixelBox {
            x: 10,
            y: 10,
            width: 30,
            height: 20,
        }];
        let crop = Rect::new(0.0, 0.5, 1.0, 0.5).expect("crop");
        let opts = RenderOptions {
            crop: Some(crop),
            grid: true,
            draw_boxes: &draw,
            pixel_boxes: &px_box,
            overwrite: true,
        };
        let report = render_rgb_to_png(&input, 5000, 100, 80, rgb.clone(), &out, &opts)
            .expect("render annotated crop");
        assert_eq!(report["frame_width"], 100);
        assert_eq!(report["frame_height"], 80);
        assert_eq!(report["width"], 100);
        assert_eq!(report["height"], 40);
        assert_eq!(report["cropped"], true);
        assert_eq!(report["grid"], true);
        assert_eq!(report["boxes"].as_array().expect("boxes").len(), 2);
        assert_eq!(
            report["boxes"][0]["region_arg"],
            "0.1000,0.7000,0.8000,0.2000"
        );

        // overwrite=false must refuse clobbering existing PNG.
        let no_overwrite = RenderOptions {
            crop: None,
            grid: false,
            draw_boxes: &[],
            pixel_boxes: &[],
            overwrite: false,
        };
        assert!(
            render_rgb_to_png(&input, 5000, 100, 80, rgb.clone(), &out, &no_overwrite).is_err()
        );

        // Full-frame uncropped render succeeds with overwrite=true.
        let full_opts = RenderOptions {
            crop: None,
            grid: true,
            draw_boxes: &draw,
            pixel_boxes: &[],
            overwrite: true,
        };
        let full = render_rgb_to_png(&input, 0, 100, 80, rgb, &out, &full_opts).expect("full");
        assert_eq!(full["cropped"], false);
        assert_eq!(full["width"], 100);
        assert_eq!(full["height"], 80);

        // Out-of-bounds crop fails cleanly.
        assert!(
            crop_rgb_buffer(
                &[0u8; 12],
                2,
                PixelBox {
                    x: 1,
                    y: 1,
                    width: 4,
                    height: 4
                }
            )
            .is_err()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
