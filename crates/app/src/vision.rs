use crate::{Result, fail};
use image::RgbImage;
use ndarray::ArrayViewD;
use std::collections::VecDeque;

pub fn db_regions(
    output: ArrayViewD<'_, f32>,
    width: usize,
    height: usize,
    source_w: u32,
    source_h: u32,
) -> Result<Vec<(u32, u32, u32, u32)>> {
    if output.shape() != [1, 1, height, width]
        || output.iter().any(|v| !v.is_finite())
        || source_w == 0
        || source_h == 0
        || width == 0
        || height == 0
        || width.checked_mul(height).is_none_or(|n| n > 4_000_000)
    {
        return Err(fail("Detector output exceeds limit"));
    }
    let mut seen = vec![false; width * height];
    let mut boxes = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            if seen[index] || output[[0, 0, y, x]] < 0.3 {
                continue;
            }
            let mut q = VecDeque::from([(x, y)]);
            seen[index] = true;
            let (mut left, mut top, mut right, mut bottom) = (x, y, x, y);
            let mut sum = 0.0;
            let mut count = 0;
            while let Some((x, y)) = q.pop_front() {
                left = left.min(x);
                top = top.min(y);
                right = right.max(x);
                bottom = bottom.max(y);
                sum += output[[0, 0, y, x]];
                count += 1;
                for (nx, ny) in [
                    (x.saturating_sub(1), y),
                    (x + 1, y),
                    (x, y.saturating_sub(1)),
                    (x, y + 1),
                ] {
                    if nx >= width || ny >= height {
                        continue;
                    }
                    let i = ny * width + nx;
                    if !seen[i] && output[[0, 0, ny, nx]] >= 0.3 {
                        seen[i] = true;
                        q.push_back((nx, ny));
                    }
                }
            }
            if count < 4 || sum / (count as f32) < 0.6 {
                continue;
            }
            // ponytail: axis-aligned DB components; polygon expansion needed for strongly rotated lines.
            let pad = ((bottom - top + 1) as f64 * 0.35).ceil() as usize;
            left = left.saturating_sub(pad);
            top = top.saturating_sub(pad);
            right = (right + pad + 1).min(width);
            bottom = (bottom + pad + 1).min(height);
            let sx = (left as u64 * u64::from(source_w) / width as u64) as u32;
            let sy = (top as u64 * u64::from(source_h) / height as u64) as u32;
            let ex = (right as u64 * u64::from(source_w) / width as u64) as u32;
            let ey = (bottom as u64 * u64::from(source_h) / height as u64) as u32;
            if ex > sx && ey > sy {
                boxes.push((sx, sy, ex - sx, ey - sy));
            }
            if boxes.len() >= 1000 {
                return Err(fail("Too many detected text regions"));
            }
        }
    }
    boxes.sort_by_key(|b| (b.1, b.0));
    Ok(boxes)
}

pub fn signature(image: &RgbImage) -> Vec<u8> {
    image::imageops::resize(image, 64, 16, image::imageops::FilterType::Triangle).into_raw()
}
pub fn changed(a: &[u8], b: &[u8], threshold: f32) -> bool {
    if a.len() != b.len() || a.is_empty() {
        return true;
    }
    let delta = a
        .iter()
        .zip(b)
        .map(|(a, b)| f32::from(a.abs_diff(*b)))
        .sum::<f32>()
        / a.len() as f32;
    delta > threshold
}
