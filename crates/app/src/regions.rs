use crate::{Paths, Result, fail, inference::Ocr, media};
use nvsf_core::{Recognition, Rect};
use std::path::Path;

#[cfg(test)]
#[path = "region_tests.rs"]
mod tests;

struct Proposal {
    seed: Rect,
    bounds: Rect,
    moments: u8,
}

fn spatial_order(a: &Rect, b: &Rect) -> std::cmp::Ordering {
    a.y.total_cmp(&b.y)
        .then(a.x.total_cmp(&b.x))
        .then(a.width.total_cmp(&b.width))
        .then(a.height.total_cmp(&b.height))
}

fn overlap(a: Rect, b: Rect) -> f64 {
    let width = (a.x + a.width).min(b.x + b.width) - a.x.max(b.x);
    let height = (a.y + a.height).min(b.y + b.height) - a.y.max(b.y);
    width.max(0.0) * height.max(0.0) / (a.width * a.height).min(b.width * b.height)
}

/// Groups spatial proposals only; repetition is not evidence that text is dialogue.
fn proposals(moments: &[Vec<Rect>]) -> Result<Vec<Rect>> {
    if moments.is_empty() || moments.len() > 5 || moments.iter().any(|m| m.len() > 1000) {
        return Err(fail("Region proposal sample limit exceeded"));
    }
    let mut detections = Vec::new();
    for (moment, regions) in moments.iter().enumerate() {
        for &region in regions {
            detections.push((region.validate()?, moment));
        }
    }
    detections.sort_by(|a, b| spatial_order(&a.0, &b.0).then(a.1.cmp(&b.1)));
    let mut groups = Vec::<Proposal>::new();
    for (region, moment) in detections {
        // ponytail: fixed seeds prevent envelope-chain merging; at most 5000 boxes, O(n^2).
        let matching = groups
            .iter()
            .enumerate()
            .map(|(index, group)| (index, overlap(group.seed, region)))
            .filter(|(_, score)| *score >= 0.6)
            .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)))
            .map(|(index, _)| index);
        if let Some(index) = matching {
            let group = &mut groups[index];
            let left = group.bounds.x.min(region.x);
            let top = group.bounds.y.min(region.y);
            let right = (group.bounds.x + group.bounds.width).max(region.x + region.width);
            let bottom = (group.bounds.y + group.bounds.height).max(region.y + region.height);
            group.bounds = Rect::new(left, top, right - left, bottom - top)?;
            group.moments |= 1 << moment;
        } else {
            groups.push(Proposal {
                seed: region,
                bounds: region,
                moments: 1 << moment,
            });
        }
    }
    groups.sort_by(|a, b| {
        b.moments
            .count_ones()
            .cmp(&a.moments.count_ones())
            .then(spatial_order(&a.seed, &b.seed))
    });
    let mut result = groups
        .into_iter()
        .take(8)
        .map(|group| {
            let r = group.bounds;
            let left = (r.x - 0.01).max(0.0);
            let top = (r.y - 0.02).max(0.0);
            let right = (r.x + r.width + 0.01).min(1.0);
            let bottom = (r.y + r.height + 0.02).min(1.0);
            Rect::new(left, top, right - left, bottom - top)
        })
        .collect::<nvsf_core::Result<Vec<_>>>()?;
    if result.is_empty() {
        return Err(fail("No text region suggestions; select regions manually"));
    }
    result.sort_by(spatial_order);
    Ok(result)
}

pub fn suggest(paths: &Paths, input: &Path, language: &str) -> Result<Vec<Rect>> {
    let runtime = media::MediaRuntime::load(paths)?;
    let info = runtime.probe(input)?;
    let duration = info.duration_ms.unwrap_or(10_000);
    let mut ocr = Ocr::new(paths, language, "auto", 2, false)?;
    let mut moments = Vec::new();
    let mut sampled = std::collections::BTreeSet::new();
    for fraction in [1u64, 3, 5, 7, 9] {
        let at = duration.saturating_mul(fraction) / 10;
        if !sampled.insert(at) {
            continue;
        }
        let frame = match runtime.preview(input, at) {
            Ok(frame) => frame,
            Err(_) => continue,
        };
        let mut boxes = Vec::new();
        for (x, y, w, h) in ocr.detect(&frame)? {
            boxes.push(Rect::new(
                x as f64 / frame.width() as f64,
                y as f64 / frame.height() as f64,
                w as f64 / frame.width() as f64,
                h as f64 / frame.height() as f64,
            )?);
        }
        moments.push(boxes);
    }
    if moments.is_empty() {
        return Err(fail(
            "No source frames available for region suggestions; select regions manually",
        ));
    }
    proposals(&moments)
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
