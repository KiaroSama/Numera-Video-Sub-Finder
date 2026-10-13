use crate::{
    AppError, Paths, Result, fail,
    inference::Ocr,
    logging::Log,
    media, models,
    storage::{JobLock, hash_file, load_project, save_project},
    tesseract::Tesseract,
    vision,
};
use nvsf_core::{
    AppearanceTracker, ExtractConfig, Progress, Project, Recognition, normalize_persian,
};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Default)]
pub struct Control {
    pub cancel: Arc<AtomicBool>,
    pub pause: Arc<AtomicBool>,
}

pub fn extract(
    config: ExtractConfig,
    control: &Control,
    callback: impl FnMut(Progress),
) -> Result<Project> {
    crate::worker::supervise(config, control, callback)
}

pub fn run(
    config: ExtractConfig,
    control: &Control,
    mut callback: impl FnMut(Progress),
) -> Result<Project> {
    config.validate()?;
    let paths = Paths::new(config.data_root.as_deref().map(Path::new))?;
    let log = Log::new(&paths, "nvsf-worker");
    let input = Path::new(&config.input).canonicalize()?;
    let output = Path::new(&config.output);
    if output.exists() && !config.overwrite {
        return Err(fail("Output exists; enable overwrite explicitly"));
    }
    if output.exists() && output.canonicalize()? == input {
        return Err(fail("Output resolves to input media"));
    }
    let checkpoint = output.with_extension("nvsf.json");
    let _lock = JobLock::acquire(&checkpoint.with_extension("lock"))?;
    let source_hash = hash_file(&input)?;
    let config_hash = Sha256::digest(serde_json::to_vec(&config)?)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let model_hash = models::specs()?
        .iter()
        .map(|m| m.sha256.as_str())
        .collect::<Vec<_>>()
        .join(":");
    let media = media::probe(&input)?;
    let mut project = Project {
        schema: 1,
        source_hash: source_hash.clone(),
        config_hash: config_hash.clone(),
        model_hash: model_hash.clone(),
        config: config.clone(),
        media: media.clone(),
        cues: vec![],
        processed_ms: config.start_ms,
        active_appearances: vec![],
        completed: false,
        warnings: vec![],
    };
    let mut start = config.start_ms;
    if checkpoint.is_file() {
        let saved = load_project(&checkpoint)?;
        if saved.source_hash != source_hash
            || saved.config_hash != config_hash
            || saved.model_hash != model_hash
        {
            return Err(fail(
                "Checkpoint source/configuration/model mismatch; choose a new output or restore compatible configuration",
            ));
        }
        if saved.completed {
            crate::export_project(&saved, output, "srt", config.overwrite, false, false)?;
            return Ok(saved);
        }
        start = start.max(saved.processed_ms);
        project = saved;
    }
    let mut ocr = Ocr::new(
        &paths,
        &config.language,
        &config.device,
        config.threads,
        config.gpu_required || config.accelerator_required,
    )?;
    let provider = ocr.provider.clone();
    let mut specialist =
        if config.language == "ja" && models::verify(&paths, "ja-specialist").is_ok() {
            Some(Ocr::new(
                &paths,
                "ja-specialist",
                &config.device,
                config.threads,
                config.gpu_required || config.accelerator_required,
            )?)
        } else {
            None
        };
    let mut fallback = Tesseract::new(&paths, &config.language).ok();
    if fallback.is_none() {
        project
            .warnings
            .push("Independent Tesseract verifier unavailable; primary ONNX remains active".into());
        log.event("WARNING", "OCR", "Tesseract verifier unavailable");
    }
    log.event(
        "INFO",
        "OCR",
        &format!(
            "Begin language={} quality={} provider={provider} threads={}",
            config.language, config.quality, config.threads
        ),
    );
    let mut trackers: Vec<_> = (0..config.regions.len())
        .map(|i| AppearanceTracker::new(i as u32))
        .collect();
    for cue in &project.active_appearances {
        trackers[cue.track as usize] = AppearanceTracker::restore(cue.track, cue.clone())?;
    }
    save_project(&project, &checkpoint)?;
    let mut next_id = project.cues.iter().map(|c| c.id).max().unwrap_or(0);
    let mut previous = vec![Vec::<u8>::new(); config.regions.len()];
    let mut candidates: Vec<Option<Recognition>> = trackers
        .iter()
        .map(|t| {
            t.active().map(|c| Recognition {
                text: c.raw_text.clone(),
                confidence: c.confidence,
                crop: c.crop.clone(),
            })
        })
        .collect();
    let mut last_ocr = vec![0u64; config.regions.len()];
    let mut pending: Option<media::DecodedFrame> = None;
    let mut committed_count = project.cues.len();
    let mut committed_appearances = project.active_appearances.clone();
    let mut committed_ms = project.processed_ms;
    let mut frames = 0u64;
    let mut last_saved = Instant::now();
    let started = Instant::now();
    let mut process = |frame: media::DecodedFrame, end: u64| -> Result<()> {
        if control.cancel.load(Ordering::Relaxed) {
            return Err(AppError::Cancelled);
        }
        let pause_deadline = Instant::now() + Duration::from_secs(24 * 3600);
        while control.pause.load(Ordering::Relaxed) {
            if control.cancel.load(Ordering::Relaxed) {
                return Err(AppError::Cancelled);
            }
            if Instant::now() >= pause_deadline {
                return Err(fail("Paused job exceeded lifetime limit"));
            }
            std::thread::park_timeout(Duration::from_millis(50));
        }
        for (i, region) in config.regions.iter().enumerate() {
            let (x, y, w, h) = region.pixels(frame.image.width(), frame.image.height())?;
            let mut crop = image::imageops::crop_imm(&frame.image, x, y, w, h).to_image();
            for exclusion in &config.exclusions {
                let (ex, ey, ew, eh) =
                    exclusion.pixels(frame.image.width(), frame.image.height())?;
                for py in ey.max(y)..(ey + eh).min(y + h) {
                    for px in ex.max(x)..(ex + ew).min(x + w) {
                        crop.put_pixel(px - x, py - y, image::Rgb([0, 0, 0]));
                    }
                }
            }
            let signature = vision::signature(&crop);
            let refresh = if config.quality == "fast" { 500 } else { 200 };
            let threshold = if config.quality == "accurate" {
                0.3
            } else {
                3.0
            };
            if config.quality == "accurate"
                || vision::changed(&previous[i], &signature, threshold)
                || frame.time_ms.saturating_sub(last_ocr[i]) >= refresh
            {
                let boxes = ocr.detect(&crop)?;
                let mut lines = Vec::new();
                let mut scores = Vec::new();
                for (bx, by, bw, bh) in boxes {
                    let text_crop = image::imageops::crop_imm(&crop, bx, by, bw, bh).to_image();
                    let mut r = ocr.recognize(&text_crop)?;
                    if r.confidence.unwrap_or(0.0) < config.min_confidence {
                        if let Some(s) = &mut specialist {
                            let candidate = s.recognize(&text_crop)?;
                            if r.text.trim().is_empty() {
                                r.text = candidate.text;
                                r.confidence = None;
                            } else if r.text.trim() != candidate.text.trim() {
                                r.confidence = None;
                            }
                        }
                        if let Some(t) = &mut fallback {
                            if let Ok(v) = t.recognize(&text_crop, bh > bw.saturating_mul(2)) {
                                // Scores from different engines are not calibrated against each other.
                                // A disagreement remains a primary candidate requiring manual review.
                                if !r.text.trim().is_empty() && v.text.trim() != r.text.trim() {
                                    r.confidence = None;
                                } else if r.text.trim().is_empty() && !v.text.trim().is_empty() {
                                    r.text = v.text;
                                    r.confidence = None;
                                }
                            }
                        }
                    }
                    if !r.text.trim().is_empty() {
                        lines.push(r.text);
                        scores.push(r.confidence.unwrap_or(0.0));
                    }
                }
                candidates[i] = if lines.is_empty() {
                    None
                } else {
                    let text = lines.join("\n");
                    let mut crop_digest = Sha256::new();
                    crop_digest.update(w.to_le_bytes());
                    crop_digest.update(h.to_le_bytes());
                    crop_digest.update(crop.as_raw());
                    let hash = crop_digest
                        .finalize()
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>();
                    let image_path = paths.cache.join(format!("crop-{hash}.png"));
                    if !image_path.is_file() {
                        crop.save(&image_path).map_err(|e| fail(e.to_string()))?;
                    }
                    Some(Recognition {
                        text,
                        confidence: Some(scores.iter().sum::<f32>() / scores.len() as f32),
                        crop: Some(image_path.to_string_lossy().into_owned()),
                    })
                };
                previous[i] = signature;
                last_ocr[i] = frame.time_ms;
            }
            if let Some(mut cue) = trackers[i].observe(frame.time_ms, end, candidates[i].clone())? {
                if config.language == "fa" {
                    cue.text = normalize_persian(&cue.text, config.normalize_persian);
                }
                next_id = next_id
                    .checked_add(1)
                    .ok_or_else(|| fail("Cue identity overflow"))?;
                cue.id = next_id;
                project.cues.push(cue);
                if project.cues.len() > 100_000 {
                    return Err(fail("Cue count limit reached"));
                }
            }
        }
        frames += 1;
        project.active_appearances = trackers
            .iter()
            .filter_map(|t| t.active().cloned())
            .collect();
        project.processed_ms = end;
        committed_count = project.cues.len();
        committed_appearances = project.active_appearances.clone();
        committed_ms = project.processed_ms;
        if last_saved.elapsed() >= Duration::from_secs(2) {
            save_project(&project, &checkpoint)?;
            last_saved = Instant::now();
        }
        callback(Progress {
            stage: "OCR".into(),
            frames,
            position_ms: frame.time_ms,
            duration_ms: media.duration_ms,
            cues: project.cues.len(),
            provider: provider.clone(),
            message: String::new(),
        });
        Ok(())
    };
    let decoded = media::decode(&input, start, config.end_ms, |frame| {
        if let Some(previous) = pending.take() {
            let minimum_end = previous
                .time_ms
                .checked_add(1)
                .ok_or_else(|| fail("Frame timestamp overflow"))?;
            let mut end = frame.time_ms.max(minimum_end);
            if let Some(limit) = config.end_ms {
                end = end.min(limit);
            }
            process(previous, end)?;
        }
        pending = Some(frame);
        Ok(true)
    });
    let mut decoded = decoded;
    if decoded.is_ok() {
        if let Some(last) = pending.take() {
            let mut end = last
                .time_ms
                .saturating_add(last.estimated_duration_ms.max(1));
            if let Some(limit) = config.end_ms.or(media.duration_ms) {
                end = end.min(limit);
            }
            decoded = if end <= last.time_ms {
                Err(fail("No positive final frame duration"))
            } else {
                process(last, end)
            };
        }
    }
    drop(process);
    project
        .warnings
        .push("Final frame end derived from the last observed frame interval".into());
    for tracker in &mut trackers {
        if decoded.is_err() {
            break;
        }
        if let Some(mut cue) = tracker.finish() {
            if config.language == "fa" {
                cue.text = normalize_persian(&cue.text, config.normalize_persian);
            }
            next_id = next_id
                .checked_add(1)
                .ok_or_else(|| fail("Cue identity overflow"))?;
            cue.id = next_id;
            project.cues.push(cue);
        }
    }
    if let Err(error) = decoded {
        // The last atomic checkpoint represents a fully processed frame; partial native
        // failures must not publish mutations from an incomplete multi-region frame.
        project.cues.truncate(committed_count);
        project.active_appearances = committed_appearances;
        project.processed_ms = committed_ms;
        save_project(&project, &checkpoint)?;
        log.event("ERROR", "JOB", &error.to_string());
        return Err(error);
    }
    project.cues.sort_by_key(|c| (c.start_ms, c.track));
    project.active_appearances.clear();
    crate::export_project(&project, output, "srt", config.overwrite, false, false)?;
    project.completed = true;
    save_project(&project, &checkpoint)?;
    log.event(
        "INFO",
        "JOB",
        &format!(
            "Completed frames={frames} cues={} elapsed_ms={}",
            project.cues.len(),
            started.elapsed().as_millis()
        ),
    );
    Ok(project)
}
