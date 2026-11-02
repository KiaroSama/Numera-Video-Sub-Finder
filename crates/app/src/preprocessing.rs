use crate::{Result, fail};
use image::{RgbImage, imageops};
use ndarray::Array4;

#[cfg(test)]
#[path = "preprocessing_tests.rs"]
mod tests;

fn validate(width: u32, height: u32) -> Result<()> {
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 33_177_600 {
        return Err(fail("Invalid or oversized OCR image dimensions"));
    }
    Ok(())
}

fn detector_size(width: u32, height: u32) -> Result<(u32, u32)> {
    validate(width, height)?;
    let (width, height) = if u64::from(width) + u64::from(height) < 64 {
        (width.max(32), height.max(32))
    } else {
        (width, height)
    };
    let ratio = (960.0 / f64::from(width.max(height))).min(1.0);
    // PaddleX 3.7's PP-OCRv5 consumer truncates before Python's ties-even stride rounding.
    let aligned = |side: u32| {
        (((f64::from(side) * ratio).trunc() / 32.0).round_ties_even() as u32 * 32).max(32)
    };
    Ok((aligned(width), aligned(height)))
}

fn recognition_size(width: u32, height: u32) -> Result<(u32, u32)> {
    validate(width, height)?;
    let scaled = 48.0 * f64::from(width) / f64::from(height);
    let padded = scaled.max(320.0).floor().min(3200.0) as u32;
    Ok(((scaled.ceil() as u32).min(padded), padded))
}

pub(crate) fn detected_regions(
    output: ndarray::ArrayViewD<'_, f32>,
    width: usize,
    height: usize,
    source_w: u32,
    source_h: u32,
) -> Result<Vec<(u32, u32, u32, u32)>> {
    validate(source_w, source_h)?;
    let tiny = u64::from(source_w) + u64::from(source_h) < 64;
    let padded_w = if tiny { source_w.max(32) } else { source_w };
    let padded_h = if tiny { source_h.max(32) } else { source_h };
    let boxes = crate::vision::db_regions(output, width, height, padded_w, padded_h)?;
    Ok(boxes
        .into_iter()
        .filter_map(|(x, y, w, h)| {
            let w = w.min(source_w.saturating_sub(x));
            let h = h.min(source_h.saturating_sub(y));
            (w > 0 && h > 0).then_some((x, y, w, h))
        })
        .collect())
}

pub(crate) fn detector(image: &RgbImage) -> Result<Array4<f32>> {
    let (width, height) = detector_size(image.width(), image.height())?;
    let padded;
    let source = if u64::from(image.width()) + u64::from(image.height()) < 64 {
        let mut image_with_border = RgbImage::new(image.width().max(32), image.height().max(32));
        imageops::replace(&mut image_with_border, image, 0, 0);
        padded = image_with_border;
        &padded
    } else {
        image
    };
    let resized = imageops::resize(source, width, height, imageops::FilterType::Triangle);
    let mut input = Array4::<f32>::zeros((1, 3, height as usize, width as usize));
    let mean = [0.485, 0.456, 0.406];
    let std = [0.229, 0.224, 0.225];
    for (x, y, p) in resized.enumerate_pixels() {
        for c in 0..3 {
            input[[0, c, y as usize, x as usize]] =
                (f32::from(p[2 - c]) / 255.0 - mean[c]) / std[c];
        }
    }
    Ok(input)
}

pub(crate) fn recognition(crop: &RgbImage) -> Result<Array4<f32>> {
    let (width, padded) = recognition_size(crop.width(), crop.height())?;
    // Linear filter family; byte parity with OpenCV INTER_LINEAR remains separately qualified.
    let resized = imageops::resize(crop, width, 48, imageops::FilterType::Triangle);
    let mut input = Array4::<f32>::zeros((1, 3, 48, padded as usize));
    for (x, y, p) in resized.enumerate_pixels() {
        for c in 0..3 {
            input[[0, c, y as usize, x as usize]] = (f32::from(p[2 - c]) / 255.0 - 0.5) / 0.5;
        }
    }
    Ok(input)
}
