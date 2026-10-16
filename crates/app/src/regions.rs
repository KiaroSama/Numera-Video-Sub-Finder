use crate::{Paths, Result, fail, inference::Ocr, media};
use nvsf_core::{Recognition, Rect};
use std::path::Path;

pub fn suggest(paths: &Paths, input: &Path, language: &str) -> Result<Vec<Rect>> {
    let runtime = media::MediaRuntime::load(paths)?;
    let info = runtime.probe(input)?;
    let duration = info.duration_ms.unwrap_or(10_000);
    let mut ocr = Ocr::new(paths, language, "auto", 2, false)?;
    let mut boxes = Vec::new();
    for fraction in [1u64, 3, 5, 7, 9] {
        let at = duration.saturating_mul(fraction) / 10;
        let frame = match runtime.preview(input, at) {
            Ok(frame) => frame,
            Err(_) => continue,
        };
        for (x, y, w, h) in ocr.detect(&frame)? {
            let center = (y as f64 + h as f64 / 2.0) / frame.height() as f64;
            if !(0.0..=0.28).contains(&center) && center < 0.65 {
                continue;
            }
            let r = Rect::new(
                x as f64 / frame.width() as f64,
                y as f64 / frame.height() as f64,
                w as f64 / frame.width() as f64,
                h as f64 / frame.height() as f64,
            )?;
            boxes.push(r);
        }
    }
    if boxes.is_empty() {
        return Ok(vec![Rect::default()]);
    }
    let mut result = Vec::<Rect>::new();
    for group in [true, false] {
        let selected: Vec<_> = boxes.iter().filter(|r| (r.y < 0.4) == group).collect();
        if selected.is_empty() {
            continue;
        }
        let left = selected.iter().map(|r| r.x).fold(1.0, f64::min).max(0.0);
        let top = (selected.iter().map(|r| r.y).fold(1.0, f64::min) - 0.02).max(0.0);
        let right = selected
            .iter()
            .map(|r| r.x + r.width)
            .fold(0.0, f64::max)
            .min(1.0);
        let bottom = (selected.iter().map(|r| r.y + r.height).fold(0.0, f64::max) + 0.02).min(1.0);
        result.push(Rect::new(left, top, right - left, bottom - top)?);
    }
    Ok(result)
}

pub fn recognize_cue(
    paths: &Paths,
    input: &Path,
    at_ms: u64,
    region: Rect,
    language: &str,
) -> Result<Recognition> {
    let frame = media::MediaRuntime::load(paths)?.preview(input, at_ms)?;
    let (x, y, w, h) = region.pixels(frame.width(), frame.height())?;
    let crop = image::imageops::crop_imm(&frame, x, y, w, h).to_image();
    let mut ocr = Ocr::new(paths, language, "auto", 2, false)?;
    let boxes = ocr.detect(&crop)?;
    let mut text = Vec::new();
    let mut confidence = 0.0;
    for (x, y, w, h) in boxes {
        let r = ocr.recognize(&image::imageops::crop_imm(&crop, x, y, w, h).to_image())?;
        if !r.text.trim().is_empty() {
            text.push(r.text);
            confidence += r.confidence.unwrap_or(0.0);
        }
    }
    if text.is_empty() {
        return Err(fail("Cue crop is unreadable; existing user text preserved"));
    }
    Ok(Recognition {
        confidence: Some(confidence / text.len() as f32),
        text: text.join("\n"),
        crop: None,
    })
}
