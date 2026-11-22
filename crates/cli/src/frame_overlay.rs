use nvsf_app::{AppError, Result};
use nvsf_core::Rect;
use std::str::FromStr;

/// Pixel-space rectangle (`x,y,width,height`) against full decoded frame dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub(crate) struct PixelBox {
    pub(crate) x: u32,
    pub(crate) y: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl FromStr for PixelBox {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(',').map(str::trim).collect();
        if parts.len() != 4 {
            return Err("Pixel box must use x,y,width,height integers".into());
        }
        let parse = |raw: &str| {
            raw.parse::<u32>()
                .map_err(|_| format!("Invalid non-negative pixel integer: {raw}"))
        };
        let x = parse(parts[0])?;
        let y = parse(parts[1])?;
        let width = parse(parts[2])?;
        let height = parse(parts[3])?;
        if width == 0 || height == 0 {
            return Err("Pixel box width and height must be positive".into());
        }
        Ok(Self {
            x,
            y,
            width,
            height,
        })
    }
}

pub(crate) fn pixel_box_to_rect(px: PixelBox, frame_w: u32, frame_h: u32) -> Result<Rect> {
    if frame_w == 0 || frame_h == 0 {
        return Err(AppError::InvalidConfig(
            "Frame dimensions must be positive".into(),
        ));
    }
    let right =
        px.x.checked_add(px.width)
            .ok_or_else(|| AppError::InvalidConfig("Pixel box X extent overflow".into()))?;
    let bottom =
        px.y.checked_add(px.height)
            .ok_or_else(|| AppError::InvalidConfig("Pixel box Y extent overflow".into()))?;
    if px.width == 0 || px.height == 0 || right > frame_w || bottom > frame_h {
        return Err(AppError::InvalidConfig(format!(
            "Pixel box {},{},{},{} exceeds frame {}x{}",
            px.x, px.y, px.width, px.height, frame_w, frame_h
        )));
    }
    Rect::new(
        f64::from(px.x) / f64::from(frame_w),
        f64::from(px.y) / f64::from(frame_h),
        f64::from(px.width) / f64::from(frame_w),
        f64::from(px.height) / f64::from(frame_h),
    )
    .map_err(|e| AppError::InvalidConfig(e.to_string()))
}

pub(crate) fn rect_to_pixel_box(rect: Rect, frame_w: u32, frame_h: u32) -> Result<PixelBox> {
    let (x, y, width, height) = rect
        .pixels(frame_w, frame_h)
        .map_err(|e| AppError::InvalidConfig(e.to_string()))?;
    Ok(PixelBox {
        x,
        y,
        width,
        height,
    })
}

pub(crate) fn format_region_arg(rect: Rect) -> String {
    format!(
        "{:.4},{:.4},{:.4},{:.4}",
        rect.x, rect.y, rect.width, rect.height
    )
}

pub(crate) fn sample_timestamps_ms(
    duration_ms: u64,
    start_ms: u64,
    end_ms: Option<u64>,
    count: usize,
) -> Result<Vec<u64>> {
    if count == 0 || count > 24 {
        return Err(AppError::InvalidConfig(
            "Strip frame count must be between 1 and 24".into(),
        ));
    }
    let effective_end = match end_ms {
        Some(end) if end < start_ms => {
            return Err(AppError::InvalidConfig(
                "Strip end_ms must be greater than or equal to start_ms".into(),
            ));
        }
        Some(end) => end.min(duration_ms),
        None => duration_ms,
    };
    if start_ms > effective_end {
        return Err(AppError::InvalidConfig(
            "Strip start_ms exceeds available media duration".into(),
        ));
    }
    if count == 1 || effective_end == start_ms {
        return Ok(vec![start_ms]);
    }
    let span = u128::from(effective_end - start_ms);
    let two_count = (count as u128) * 2;
    Ok((0..count)
        .map(|idx| {
            let offset = (span * (2 * (idx as u128) + 1)) / two_count;
            start_ms + offset as u64
        })
        .collect())
}

const BOX_COLORS: [[u8; 3]; 4] = [
    [0, 229, 255],  // cyan
    [255, 221, 0],  // yellow
    [255, 64, 192], // magenta
    [64, 255, 128], // lime
];

pub(crate) fn box_color(index: usize) -> [u8; 3] {
    BOX_COLORS[index % BOX_COLORS.len()]
}

fn blend_pixel(rgb: &mut [u8], width: u32, height: u32, x: u32, y: u32, color: [u8; 3], alpha: u8) {
    if x >= width || y >= height {
        return;
    }
    let idx = ((y as usize) * (width as usize) + (x as usize)) * 3;
    if idx + 2 >= rgb.len() {
        return;
    }
    let a = u16::from(alpha);
    let inv = 255u16 - a;
    for ch in 0..3 {
        rgb[idx + ch] = ((u16::from(rgb[idx + ch]) * inv + u16::from(color[ch]) * a) / 255) as u8;
    }
}

fn set_pixel(rgb: &mut [u8], width: u32, height: u32, x: u32, y: u32, color: [u8; 3]) {
    blend_pixel(rgb, width, height, x, y, color, 255);
}

// 3x5 bitmap glyphs for digits '1'..'9' (used for 0.1..0.9 normalized grid ticks).
const DIGIT_GLYPHS: [[u8; 5]; 9] = [
    [0b010, 0b110, 0b010, 0b010, 0b111], // 1
    [0b111, 0b001, 0b111, 0b100, 0b111], // 2
    [0b111, 0b001, 0b111, 0b001, 0b111], // 3
    [0b101, 0b101, 0b111, 0b001, 0b001], // 4
    [0b111, 0b100, 0b111, 0b001, 0b111], // 5
    [0b111, 0b100, 0b111, 0b101, 0b111], // 6
    [0b111, 0b001, 0b010, 0b010, 0b010], // 7
    [0b111, 0b101, 0b111, 0b101, 0b111], // 8
    [0b111, 0b101, 0b111, 0b001, 0b111], // 9
];

fn draw_tick_digit(rgb: &mut [u8], width: u32, height: u32, x0: u32, y0: u32, digit: usize) {
    let Some(rows) = DIGIT_GLYPHS.get(digit.saturating_sub(1)) else {
        return;
    };
    for dy in 0..7u32 {
        for dx in 0..5u32 {
            blend_pixel(rgb, width, height, x0 + dx, y0 + dy, [10, 10, 14], 200);
        }
    }
    for (row_idx, bits) in rows.iter().enumerate() {
        for col_idx in 0..3u32 {
            if (bits >> (2 - col_idx)) & 1 == 1 {
                set_pixel(
                    rgb,
                    width,
                    height,
                    x0 + 1 + col_idx,
                    y0 + 1 + row_idx as u32,
                    [245, 245, 250],
                );
            }
        }
    }
}

/// Overlays a 10x10 normalized coordinate grid (0.1..0.9) with readable edge digit ticks.
pub(crate) fn draw_coordinate_grid(rgb: &mut [u8], width: u32, height: u32) {
    if width < 20 || height < 20 {
        return;
    }
    for step in 1..10u32 {
        let gx = (u64::from(width) * u64::from(step) / 10) as u32;
        let gy = (u64::from(height) * u64::from(step) / 10) as u32;
        let alpha = if step == 5 { 180 } else { 115 };
        for y in 0..height {
            if y % 4 < 3 {
                blend_pixel(rgb, width, height, gx, y, [255, 255, 255], alpha);
                if gx + 1 < width {
                    blend_pixel(rgb, width, height, gx + 1, y, [16, 16, 20], alpha / 2);
                }
            }
        }
        for x in 0..width {
            if x % 4 < 3 {
                blend_pixel(rgb, width, height, x, gy, [255, 255, 255], alpha);
                if gy + 1 < height {
                    blend_pixel(rgb, width, height, x, gy + 1, [16, 16, 20], alpha / 2);
                }
            }
        }
        draw_tick_digit(rgb, width, height, gx.saturating_add(2), 2, step as usize);
        draw_tick_digit(rgb, width, height, 2, gy.saturating_add(2), step as usize);
    }
}

/// Draws a high-contrast 2px rectangle with a 1px dark outer halo onto an RGB24 frame.
pub(crate) fn draw_rect_overlay(
    rgb: &mut [u8],
    width: u32,
    height: u32,
    rect: Rect,
    color: [u8; 3],
) -> Result<PixelBox> {
    let px = rect_to_pixel_box(rect, width, height)?;
    let x0 = px.x;
    let y0 = px.y;
    let x1 = px.x.saturating_add(px.width).saturating_sub(1);
    let y1 = px.y.saturating_add(px.height).saturating_sub(1);
    let halo = [12, 12, 16];

    for x in x0.saturating_sub(1)..=x1.saturating_add(1).min(width - 1) {
        if y0 > 0 {
            set_pixel(rgb, width, height, x, y0 - 1, halo);
        }
        if y1 + 1 < height {
            set_pixel(rgb, width, height, x, y1 + 1, halo);
        }
    }
    for y in y0.saturating_sub(1)..=y1.saturating_add(1).min(height - 1) {
        if x0 > 0 {
            set_pixel(rgb, width, height, x0 - 1, y, halo);
        }
        if x1 + 1 < width {
            set_pixel(rgb, width, height, x1 + 1, y, halo);
        }
    }
    for thickness in 0..2u32 {
        let top = (y0 + thickness).min(y1);
        let bottom = y1.saturating_sub(thickness).max(y0);
        for x in x0..=x1 {
            set_pixel(rgb, width, height, x, top, color);
            set_pixel(rgb, width, height, x, bottom, color);
        }
        let left = (x0 + thickness).min(x1);
        let right = x1.saturating_sub(thickness).max(x0);
        for y in y0..=y1 {
            set_pixel(rgb, width, height, left, y, color);
            set_pixel(rgb, width, height, right, y, color);
        }
    }
    Ok(px)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_box_parsing_conversion_and_formatting_round_trip() {
        let px: PixelBox = "192,864,1536,162".parse().unwrap();
        assert_eq!(
            px,
            PixelBox {
                x: 192,
                y: 864,
                width: 1536,
                height: 162
            }
        );
        let rect = pixel_box_to_rect(px, 1920, 1080).unwrap();
        assert!((rect.x - 0.1).abs() < 1e-6);
        assert!((rect.y - 0.8).abs() < 1e-6);
        assert!((rect.width - 0.8).abs() < 1e-6);
        assert!((rect.height - 0.15).abs() < 1e-6);
        assert_eq!(format_region_arg(rect), "0.1000,0.8000,0.8000,0.1500");
        let back = rect_to_pixel_box(rect, 1920, 1080).unwrap();
        assert_eq!(back, px);

        assert!("1,2,3".parse::<PixelBox>().is_err());
        assert!("1,2,0,4".parse::<PixelBox>().is_err());
        assert!("a,2,3,4".parse::<PixelBox>().is_err());
        assert!(pixel_box_to_rect(px, 0, 1080).is_err());
        assert!(pixel_box_to_rect(px, 1000, 1080).is_err());
    }

    #[test]
    fn grid_and_box_overlays_modify_target_pixels_and_timestamps_bound_ranges() {
        let (w, h) = (100u32, 80u32);
        let mut rgb = vec![32u8; (w * h * 3) as usize];
        draw_coordinate_grid(&mut rgb, w, h);
        // Vertical gridline at x = 50 should be brightened.
        let mid = ((40 * w + 50) * 3) as usize;
        assert!(rgb[mid] > 100);

        let rect = Rect::new(0.1, 0.7, 0.8, 0.2).unwrap();
        let color = box_color(0);
        let px = draw_rect_overlay(&mut rgb, w, h, rect, color).unwrap();
        assert_eq!((px.x, px.y, px.width, px.height), (10, 56, 80, 16));
        let top_left = ((px.y * w + px.x) * 3) as usize;
        assert_eq!(&rgb[top_left..top_left + 3], &color);

        let mut tiny = vec![0u8; 10 * 10 * 3];
        draw_coordinate_grid(&mut tiny, 10, 10);
        assert!(tiny.iter().all(|&b| b == 0));

        assert_eq!(
            sample_timestamps_ms(10_000, 1_000, Some(9_000), 4).unwrap(),
            vec![2_000, 4_000, 6_000, 8_000]
        );
        assert_eq!(
            sample_timestamps_ms(2_000, 0, None, 3).unwrap(),
            vec![333, 1_000, 1_666]
        );
        assert_eq!(
            sample_timestamps_ms(10_000, 500, None, 1).unwrap(),
            vec![500]
        );
        assert!(sample_timestamps_ms(10_000, 0, None, 0).is_err());
        assert!(sample_timestamps_ms(10_000, 0, None, 25).is_err());
        assert!(sample_timestamps_ms(10_000, 5_000, Some(4_000), 3).is_err());
        assert!(sample_timestamps_ms(5_000, 6_000, None, 2).is_err());
    }
}
