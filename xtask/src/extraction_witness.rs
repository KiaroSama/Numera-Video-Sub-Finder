//! Hosted synthetic checkpoint/cancellation witnesses using the actual application pipeline.
use nvsf_app::{AppError, Control, Paths, extraction, storage};
use nvsf_core::{ExtractConfig, Rect};
use std::{
    error::Error,
    path::Path,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub fn verify(input: &Path, data: &Path, root: &Path) -> Result<()> {
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
        || std::env::var("RUNNER_ENVIRONMENT").as_deref() != Ok("github-hosted")
    {
        return Err("Extraction lifecycle witnesses require hosted CI".into());
    }
    let started = Instant::now();
    let root = root.join("extraction-lifecycle");
    std::fs::create_dir(&root)?;
    let paths = Paths::new(Some(data))?;
    let output = root.join("cancelled.srt");
    let config = ExtractConfig {
        input: input.to_str().ok_or("Invalid source path")?.into(),
        output: output.to_str().ok_or("Invalid output path")?.into(),
        data_root: Some(paths.root.to_str().ok_or("Invalid data path")?.into()),
        device: "cpu".into(),
        quality: "fast".into(),
        threads: 2,
        end_ms: Some(3500),
        ..Default::default()
    };
    let control = Control::default();
    let mut positions = Vec::new();
    let result = extraction::run(config.clone(), &control, |progress| {
        positions.push(progress.position_ms);
        control.cancel.store(true, Ordering::Release);
    });
    if !matches!(result, Err(AppError::Cancelled)) || positions.len() != 1 || output.exists() {
        return Err("Actual extraction did not cancel after its first committed frame".into());
    }
    let checkpoint = output.with_extension("nvsf.json");
    let mut saved = storage::load_project(&checkpoint)?;
    if saved.completed || saved.processed_ms == 0 || saved.active_appearances.is_empty() {
        return Err("Cancellation did not preserve a resumable active appearance".into());
    }
    let original = std::fs::read(&checkpoint)?;
    let mut incompatible = config.clone();
    incompatible.min_confidence = 0.8;
    let error = extraction::run(incompatible, &Control::default(), |_| {})
        .err()
        .ok_or("Changed configuration incorrectly resumed checkpoint")?;
    if !error
        .to_string()
        .contains("source/configuration/model mismatch")
        || std::fs::read(&checkpoint)? != original
        || output.exists()
    {
        return Err("Incompatible resume did not preserve the original checkpoint/output".into());
    }
    // A saved manual correction must survive both resumed work and the completed fast path.
    let mut correction = nvsf_core::Cue::new(9000, 0, 0, 1, "Original OCR".into())?;
    correction.text = "Saved فارسی 日本".into();
    correction.edited = true;
    saved.cues.push(correction.clone());
    storage::save_project(&saved, &checkpoint)?;
    let resumed = extraction::run(config.clone(), &Control::default(), |_| {})?;
    if !resumed.completed
        || !resumed.active_appearances.is_empty()
        || !resumed.cues.contains(&correction)
    {
        return Err(
            "Resumed extraction lost a correction or failed to finish active appearances".into(),
        );
    }
    let exported = std::fs::read(&output)?;
    if !nvsf_core::parse_srt(std::str::from_utf8(&exported)?)?
        .iter()
        .any(|cue| cue.text == correction.text)
    {
        return Err("Resumed correction absent from actual exported bytes".into());
    }
    let completed_checkpoint = std::fs::read(&checkpoint)?;
    let denied = extraction::run(config.clone(), &Control::default(), |_| {
        panic!("overwrite refused")
    });
    if !denied
        .err()
        .is_some_and(|e| e.to_string().contains("enable overwrite explicitly"))
        || std::fs::read(&output)? != exported
    {
        return Err(
            "Existing extraction output was not preserved without overwrite consent".into(),
        );
    }
    std::fs::remove_file(&output)?;
    let completed = extraction::run(config, &Control::default(), |_| {
        panic!("completed checkpoint decoded again")
    })?;
    if !completed.completed
        || std::fs::read(&output)? != exported
        || std::fs::read(&checkpoint)? != completed_checkpoint
    {
        return Err(
            "Completed checkpoint fast path changed saved corrections or checkpoint bytes".into(),
        );
    }
    let blank_output = root.join("excluded.srt");
    let blank = ExtractConfig {
        input: input.to_str().ok_or("Invalid source path")?.into(),
        output: blank_output.to_str().ok_or("Invalid output path")?.into(),
        data_root: Some(paths.root.to_str().ok_or("Invalid data path")?.into()),
        device: "cpu".into(),
        quality: "balanced".into(),
        end_ms: Some(500),
        exclusions: vec![Rect::new(0.0, 0.0, 1.0, 1.0)?],
        threads: 2,
        ..Default::default()
    };
    let excluded = extraction::run(blank, &Control::default(), |_| {})?;
    if !excluded.completed
        || !excluded.cues.is_empty()
        || std::fs::metadata(&blank_output)?.len() != 0
    {
        return Err("Full exclusion did not suppress synthetic text without inventing cues".into());
    }
    let (response, _) = nvsf_app::native_request::supervise(
        &paths,
        input,
        nvsf_app::native_request::Operation::Suggest {
            language: "en".into(),
        },
        &std::sync::atomic::AtomicBool::new(false),
    )?;
    let nvsf_app::native_request::Response::Suggest(suggestions) = response else {
        return Err("Native suggestion worker returned a different response".into());
    };
    if suggestions.is_empty() || suggestions.len() > 2 || suggestions == [Rect::default()] {
        return Err(
            "Actual region suggestion omitted subtitle bands or invented extra bands".into(),
        );
    }
    for region in &suggestions {
        region.validate()?;
    }
    if !suggestions
        .iter()
        .any(|region| region.y + region.height > 0.8)
    {
        return Err("Suggested bands omitted the known bottom subtitle".into());
    }
    let unreadable = nvsf_app::native_request::supervise(
        &paths,
        input,
        nvsf_app::native_request::Operation::Recognize {
            at_ms: 500,
            region: Rect::new(0.0, 0.0, 1.0, 0.1)?,
            language: "en".into(),
        },
        &std::sync::atomic::AtomicBool::new(false),
    )
    .err()
    .ok_or("Blank upper crop incorrectly produced a recognition candidate")?;
    if !unreadable.to_string().contains("Cue crop is unreadable") {
        return Err(format!("Blank-crop witness failed before recognition: {unreadable}").into());
    }
    for spec in nvsf_app::models::specs()?
        .into_iter()
        .filter(|spec| spec.id != "ja-specialist")
    {
        nvsf_app::models::verify(&paths, &spec.id)?;
    }
    for language in ["eng", "fas", "jpn"] {
        nvsf_app::languages::verify(&paths, language)?;
    }
    nvsf_app::models::install_defaults(&paths)?;
    let bundled = root.join("bundled");
    let bundled_models = bundled.join("data/models");
    std::fs::create_dir_all(&bundled_models)?;
    std::fs::copy(
        paths.models.join("en-ja.onnx"),
        bundled_models.join("en-ja.onnx"),
    )?;
    // Paths::new seeds from the inherited package before callers can override runtimes.
    // This witness needs an empty root and only its explicitly selected bundle.
    let seed_root = root.join("seeded-models");
    let seeded = Paths {
        models: seed_root.join("models"),
        projects: seed_root.join("projects"),
        logs: seed_root.join("logs"),
        cache: seed_root.join("cache"),
        root: seed_root,
        runtimes: bundled.join("runtimes"),
    };
    for directory in [
        &seeded.models,
        &seeded.projects,
        &seeded.logs,
        &seeded.cache,
    ] {
        std::fs::create_dir_all(directory)?;
    }
    if seeded.models.join("det.onnx").exists() {
        return Err("Bundled seed witness must start without a detector".into());
    }
    nvsf_app::models::seed_bundled(&seeded)?;
    nvsf_app::models::verify(&seeded, "en-ja")?;
    std::fs::write(
        bundled_models.join("det.onnx"),
        b"corrupted bundled candidate",
    )?;
    if !nvsf_app::models::seed_bundled(&seeded)
        .err()
        .is_some_and(|e| e.to_string().contains("Bundled model integrity failed"))
        || seeded.models.join("det.onnx").exists()
    {
        return Err("Corrupt bundled candidate was not rejected before activation".into());
    }
    nvsf_app::models::verify(&seeded, "en-ja")?;
    let exported_model = root.join("en-ja.onnx");
    nvsf_app::models::export_model(&paths, "en-ja", &exported_model)?;
    if nvsf_app::models::export_model(&paths, "en-ja", &exported_model).is_ok() {
        return Err("Model export replaced an existing destination".into());
    }
    let imported = Paths::new(Some(&root.join("imported-model")))?;
    nvsf_app::models::import_model(&imported, "en-ja", &exported_model)?;
    let admitted = nvsf_app::models::verify(&imported, "en-ja")?;
    let expected_model = storage::hash_file(&admitted)?;
    std::fs::write(&exported_model, b"corrupted import candidate")?;
    if nvsf_app::models::import_model(&imported, "en-ja", &exported_model).is_ok()
        || storage::hash_file(&admitted)? != expected_model
    {
        return Err("Corrupt model import replaced the verified installed model".into());
    }
    if started.elapsed() >= Duration::from_secs(60) {
        return Err("Extraction lifecycle witnesses exceeded60s budget".into());
    }
    storage::write_atomic(
        &root.join("report.json"),
        &serde_json::to_vec_pretty(&serde_json::json!({
            "source_sha":std::env::var("GITHUB_SHA")?, "passed":true, "synthetic":true,
            "proof":["cancel after committed frame", "incompatible resume preservation", "active appearance resume", "saved Unicode correction export", "overwrite refusal", "completed checkpoint export without decoding", "balanced full exclusion", "real subtitle-band suggestion", "blank cue rejection", "verified model export/import and corrupt replacement refusal", "installed default reuse and verified bundled seed with corrupt candidate refusal"],
            "not_proved":["real-language quality", "native pause UI", "power-loss durability"],
            "elapsed_ms":started.elapsed().as_millis()
        }))?,
        false,
    )?;
    Ok(())
}
