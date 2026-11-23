use crate::{
    AppError, Paths, Result, fail,
    inference::Ocr,
    logging::Log,
    media, models,
    providers::profiles::ProfileFiles,
    storage::{hash_file, save_project},
    tesseract::Tesseract,
    vision,
};

#[path = "extraction_checkpoint.rs"]
mod checkpoint;
#[path = "extraction_identity.rs"]
mod identity;
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

#[derive(Clone, Copy, Debug, Default)]
pub struct SrtOptions {
    pub bom: bool,
    pub crlf: bool,
}

pub fn extract(
    config: ExtractConfig,
    control: &Control,
    callback: impl FnMut(Progress),
) -> Result<Project> {
    crate::worker::supervise(config, control, callback)
}

fn line_confidence(scores: &[Option<f32>]) -> Option<f32> {
    if scores.is_empty() {
        return None;
    }
    scores
        .iter()
        .copied()
        .sum::<Option<f32>>()
        .map(|sum| sum / scores.len() as f32)
}

fn optional_verifier<T>(
    has_paddle: bool,
    required_acceleration: bool,
    initialize: impl FnOnce() -> Result<T>,
) -> (Option<T>, Option<&'static str>) {
    if !has_paddle {
        return (None, None);
    }
    if required_acceleration {
        return (
            None,
            Some(
                "Independent Tesseract verifier disabled by required acceleration; low-confidence observations require review",
            ),
        );
    }
    match initialize() {
        Ok(verifier) => (Some(verifier), None),
        Err(_) => (
            None,
            Some("Independent Tesseract verifier unavailable; primary ONNX remains active"),
        ),
    }
}

#[cfg(test)]
mod verifier_tests {
    use super::{line_confidence, optional_verifier};

    #[test]
    fn required_acceleration_never_initializes_a_cpu_verifier() {
        for required in [true, false] {
            let mut initialized = false;
            let (verifier, warning) = optional_verifier(true, required, || {
                initialized = true;
                Ok(17)
            });
            assert_eq!(initialized, !required);
            assert_eq!(verifier, (!required).then_some(17));
            if required {
                assert!(warning.unwrap().contains("required acceleration"));
            } else {
                assert!(warning.is_none());
            }
        }
        let (verifier, warning) = optional_verifier(false, false, || -> crate::Result<()> {
            panic!("Tesseract-primary recognition must not verify itself")
        });
        assert!(verifier.is_none() && warning.is_none());
    }

    #[test]
    fn one_unscored_line_cannot_be_averaged_into_confident_text() {
        assert_eq!(
            line_confidence(&[Some(0.99), Some(0.99), Some(0.99), None]),
            None
        );
        assert_eq!(line_confidence(&[None]), None);
        assert_eq!(line_confidence(&[]), None);
        assert_eq!(line_confidence(&[Some(0.5), Some(1.0)]), Some(0.75));
    }
}

/// Applies an independent observation without comparing uncalibrated engine scores.
pub fn adjudicate_verifier(primary: &mut Recognition, verified: Result<Recognition>) -> Result<()> {
    let verified = match verified {
        Ok(verified) => verified,
        Err(error) => {
            primary.confidence = None;
            return Err(error);
        }
    };
    if primary.text.trim().is_empty() && !verified.text.trim().is_empty() {
        primary.text = verified.text;
        primary.confidence = None;
    } else if primary.text.trim() != verified.text.trim() {
        primary.confidence = None;
    }
    Ok(())
}

pub fn run(
    config: ExtractConfig,
    control: &Control,
    callback: impl FnMut(Progress),
) -> Result<Project> {
    run_with_srt(config, control, callback, SrtOptions::default())
}

pub fn run_with_srt(
    config: ExtractConfig,
    control: &Control,
    callback: impl FnMut(Progress),
    options: SrtOptions,
) -> Result<Project> {
    run_inner(config, control, callback, None, None, options).map(|(project, _)| project)
}

/// Continues the explicitly selected checkpoint, retaining its location and output ownership.
pub fn run_resuming(
    config: ExtractConfig,
    control: &Control,
    callback: impl FnMut(Progress),
    checkpoint: &Path,
) -> Result<Project> {
    run_inner(
        config,
        control,
        callback,
        None,
        Some(checkpoint),
        SrtOptions::default(),
    )
    .map(|(project, _)| project)
}

/// Profiles the ordinary extraction sessions without changing the OCR or export policy.
pub fn run_profiled(
    config: ExtractConfig,
    control: &Control,
    callback: impl FnMut(Progress),
    profile_directory: &Path,
) -> Result<(Project, ProfileFiles)> {
    let (project, profiles) = run_inner(
        config,
        control,
        callback,
        Some(profile_directory),
        None,
        SrtOptions::default(),
    )?;
    Ok((
        project,
        profiles.ok_or_else(|| fail("Extraction produced no provider profiles"))?,
    ))
}

fn run_inner(
    config: ExtractConfig,
    control: &Control,
    mut callback: impl FnMut(Progress),
    profile_directory: Option<&Path>,
    selected_checkpoint: Option<&Path>,
    options: SrtOptions,
) -> Result<(Project, Option<ProfileFiles>)> {
    config.validate()?;
    let paths = Paths::new(config.data_root.as_deref().map(Path::new))?;
    let log = Log::new(&paths, "nvsf-worker");
    let input = Path::new(&config.input).canonicalize()?;
    let identity = identity::SourceIdentity::bind(config, &input)?;
    let config = &identity.config;
    let output = Path::new(&config.output);
    if output.exists() && !config.overwrite {
        return Err(fail("Output exists; enable overwrite explicitly"));
    }
    if output.exists() && output.canonicalize()? == input {
        return Err(fail("Output resolves to input media"));
    }
    let checkpoint_owner = checkpoint::Checkpoint::open(output, selected_checkpoint, &input)?;
    let checkpoint = checkpoint_owner.path.clone();
    let source_hash = hash_file(&input)?;
    let config_hash = &identity.hash;
    let model_hash = crate::languages::fingerprint_with_policy(
        &paths,
        &config.language,
        config.gpu_required || config.accelerator_required,
    )?;
    let runtime = media::MediaRuntime::load(&paths)?;
    let media = runtime.probe(&input)?;
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
    if let Some(mut saved) = checkpoint_owner.load()? {
        identity.admit(&mut saved, &source_hash, &model_hash)?;
        if saved.completed {
            if profile_directory.is_some() {
                return Err(fail(
                    "A completed checkpoint cannot establish fresh provider placement; choose a new output",
                ));
            }
            checkpoint_owner.export(&saved, output, config.overwrite, options.bom, options.crlf)?;
            save_project(&saved, &checkpoint)?;
            return Ok((saved, None));
        }
        start = start.max(saved.processed_ms);
        project = saved;
    }
    let mut ocr = if let Some(directory) = profile_directory {
        Ocr::new_profiled(
            &paths,
            &config.language,
            &config.device,
            config.threads,
            config.gpu_required || config.accelerator_required,
            directory,
        )?
    } else {
        Ocr::new(
            &paths,
            &config.language,
            &config.device,
            config.threads,
            config.gpu_required || config.accelerator_required,
        )?
    };
    let provider = ocr.provider.clone();
    let mut specialist = if nvsf_core::languages::find(&config.language)
        .is_some_and(|l| l.code == "jpn")
        && models::verify(&paths, "ja-specialist").is_ok()
    {
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
    let has_paddle = nvsf_core::languages::paddle_model(&config.language).is_some();
    let (mut fallback, verifier_warning) = optional_verifier(
        has_paddle,
        config.gpu_required || config.accelerator_required,
        || Tesseract::new(&paths, &config.language),
    );
    if let Some(warning) = verifier_warning {
        if !project.warnings.iter().any(|existing| existing == warning) {
            project.warnings.push(warning.into());
        }
        log.event("WARNING", "OCR", warning);
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
                    if r.confidence
                        .is_none_or(|score| score < config.min_confidence)
                    {
                        if let Some(s) = &mut specialist {
                            let candidate = s.recognize(&text_crop)?;
                            if r.text.trim().is_empty() {
                                r.text = candidate.text;
                                r.confidence = None;
                            } else if r.text.trim() != candidate.text.trim() {
                                r.confidence = None;
                            }
                        }
                        if let Some(t) = &mut fallback
                            && adjudicate_verifier(
                                &mut r,
                                t.recognize(&text_crop, bh > bw.saturating_mul(2)),
                            )
                            .is_err()
                        {
                            let warning = "Independent Tesseract verification failed; affected observations require review";
                            if !project.warnings.iter().any(|existing| existing == warning) {
                                project.warnings.push(warning.into());
                                log.event("WARNING", "OCR", warning);
                            }
                        }
                    }
                    if !r.text.trim().is_empty() {
                        lines.push(r.text);
                        scores.push(r.confidence);
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
                        confidence: line_confidence(&scores),
                        crop: Some(image_path.to_string_lossy().into_owned()),
                    })
                };
                previous[i] = signature;
                last_ocr[i] = frame.time_ms;
            }
            if let Some(mut cue) = trackers[i].observe(frame.time_ms, end, candidates[i].clone())? {
                if nvsf_core::languages::is_persian(&config.language) {
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
    let decoded = runtime.decode(&input, start, config.end_ms, |frame| {
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
    if decoded.is_ok()
        && let Some(last) = pending.take()
    {
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
    drop(process);
    // Flush both profiles even after decode/OCR failure; preserve the original job error below.
    let profiles = if profile_directory.is_some() {
        Some(ocr.finish_profiles())
    } else {
        None
    };
    project
        .warnings
        .push("Final frame end derived from the last observed frame interval".into());
    for tracker in &mut trackers {
        if decoded.is_err() {
            break;
        }
        if let Some(mut cue) = tracker.finish() {
            if nvsf_core::languages::is_persian(&config.language) {
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
    let profiles = profiles.transpose()?;
    project.cues.sort_by_key(|c| (c.start_ms, c.track));
    project.active_appearances.clear();
    checkpoint_owner.export(
        &project,
        output,
        config.overwrite,
        options.bom,
        options.crlf,
    )?;
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
    Ok((project, profiles))
}
