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
    // Keep the qualified detector scale until its DB/crop consumer can change with it.
    let ratio = 960.0 / width.max(height) as f32;
    let aligned = |side: u32| ((side as f32 * ratio / 32.0).round().max(1.0) as u32) * 32;
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
    let boxes = crate::vision::db_regions(output, width, height, source_w, source_h)?;
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
    let resized = imageops::resize(image, width, height, imageops::FilterType::Triangle);
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
