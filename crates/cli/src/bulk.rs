use crate::{BulkArgs, status, utf8_path};
use nvsf_app::{AppError, Control, Result};
use nvsf_core::{ExtractConfig, Project, Rect};
use serde::Serialize;
use serde_json::json;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Instant,
};

pub(crate) const MIN_CONCURRENCY: usize = 1;
pub(crate) const MAX_CONCURRENCY: usize = 8;
const MAX_INPUTS_FILE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct BulkItemResult {
    pub(crate) input: PathBuf,
    pub(crate) output: PathBuf,
    pub(crate) status: String,
    pub(crate) exit_code: i32,
    pub(crate) cues: usize,
    pub(crate) review_needed: bool,
    pub(crate) elapsed_ms: u128,
    pub(crate) error: Option<String>,
}

pub(crate) fn validate_concurrency(concurrency: usize) -> Result<()> {
    if !(MIN_CONCURRENCY..=MAX_CONCURRENCY).contains(&concurrency) {
        return Err(AppError::InvalidConfig(format!(
            "Concurrency must be between {MIN_CONCURRENCY} and {MAX_CONCURRENCY}"
        )));
    }
    Ok(())
}

pub(crate) fn per_job_threads(total_threads: usize, concurrency: usize) -> usize {
    (total_threads / concurrency.max(1)).clamp(1, 16)
}

pub(crate) fn parse_extensions(raw: &str) -> Result<Vec<String>> {
    let list: Vec<String> = raw
        .split(',')
        .map(|s| s.trim().trim_start_matches('.').to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    if list.is_empty() {
        return Err(AppError::InvalidConfig(
            "Bulk --extensions requires at least one file extension".into(),
        ));
    }
    Ok(list)
}

pub(crate) fn parse_inputs_text(text: &str) -> Result<Vec<PathBuf>> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if trimmed.starts_with('[') {
        let parsed: Vec<String> = serde_json::from_str(trimmed).map_err(|e| {
            AppError::InvalidConfig(format!("Invalid JSON array in --inputs-file: {e}"))
        })?;
        return Ok(parsed
            .into_iter()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect());
    }
    Ok(trimmed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(PathBuf::from)
        .collect())
}

pub(crate) fn discover_inputs(args: &BulkArgs) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    let mut push_unique = |path: PathBuf| {
        if seen.insert(path.clone()) {
            found.push(path);
        }
    };

    for input in &args.inputs {
        push_unique(input.clone());
    }

    if let Some(file_path) = &args.inputs_file {
        let meta = std::fs::metadata(file_path)
            .map_err(|e| AppError::InvalidConfig(format!("Unable to read --inputs-file: {e}")))?;
        if meta.len() > MAX_INPUTS_FILE_BYTES {
            return Err(AppError::InvalidConfig(
                "Bulk --inputs-file exceeds 4MiB limit".into(),
            ));
        }
        let text = std::fs::read_to_string(file_path)?;
        for path in parse_inputs_text(&text)? {
            push_unique(path);
        }
    }

    if let Some(dir) = &args.input_dir {
        if !dir.is_dir() {
            return Err(AppError::InvalidConfig(
                "Bulk --input-dir must be an existing directory".into(),
            ));
        }
        let exts = parse_extensions(&args.extensions)?;
        let mut dir_entries = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if !entry.file_type()?.is_file() {
                continue;
            }
            let ext_matches = path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                exts.iter()
                    .any(|allowed| allowed == &e.to_ascii_lowercase())
            });
            if ext_matches {
                dir_entries.push(path);
            }
        }
        dir_entries.sort();
        for path in dir_entries {
            push_unique(path);
        }
    }

    if found.is_empty() {
        return Err(AppError::InvalidConfig(
            "Bulk mode requires at least one input via --input, --inputs-file or --input-dir"
                .into(),
        ));
    }
    Ok(found)
}

pub(crate) fn plan_outputs(
    inputs: &[PathBuf],
    output_dir: Option<&Path>,
    format: &str,
) -> Result<Vec<PathBuf>> {
    if let Some(dir) = output_dir
        && !dir.is_dir()
    {
        return Err(AppError::InvalidConfig(
            "Bulk --output-dir must be an existing directory".into(),
        ));
    }
    let mut used = HashSet::new();
    let mut outputs = Vec::with_capacity(inputs.len());
    for (idx, input) in inputs.iter().enumerate() {
        let stem = input.file_stem().and_then(|s| s.to_str()).ok_or_else(|| {
            AppError::InvalidConfig("Bulk input requires a UTF-8 filename".into())
        })?;
        let base_dir = match output_dir {
            Some(dir) => dir,
            None => input
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        };
        let mut candidate = base_dir.join(format!("{stem}.{format}"));
        if !used.insert(candidate.clone()) {
            let mut suffix = idx + 1;
            loop {
                let numbered = base_dir.join(format!("{stem}-{suffix}.{format}"));
                if used.insert(numbered.clone()) {
                    candidate = numbered;
                    break;
                }
                suffix += 1;
            }
        }
        outputs.push(candidate);
    }
    Ok(outputs)
}

pub(crate) fn execute_pool<F>(
    jobs: Vec<(PathBuf, PathBuf, ExtractConfig)>,
    concurrency: usize,
    fail_fast: bool,
    control: &Control,
    runner: &F,
) -> Vec<BulkItemResult>
where
    F: Fn(ExtractConfig, &Control) -> Result<Project> + Sync,
{
    let total = jobs.len();
    if total == 0 {
        return Vec::new();
    }
    let worker_count = concurrency
        .clamp(MIN_CONCURRENCY, MAX_CONCURRENCY)
        .min(total);
    let cursor = AtomicUsize::new(0);
    let stop_early = Arc::new(AtomicBool::new(false));
    let slots: Mutex<Vec<Option<BulkItemResult>>> = Mutex::new(vec![None; total]);

    std::thread::scope(|scope| {
        for _ in 0..worker_count {
            let jobs_ref = &jobs;
            let cursor_ref = &cursor;
            let slots_ref = &slots;
            let stop_ref = Arc::clone(&stop_early);
            scope.spawn(move || {
                loop {
                    let idx = cursor_ref.fetch_add(1, Ordering::Relaxed);
                    if idx >= jobs_ref.len() {
                        break;
                    }
                    let (input, output, cfg) = jobs_ref[idx].clone();
                    let item_start = Instant::now();
                    let result = if control.cancel.load(Ordering::Acquire)
                        || (fail_fast && stop_ref.load(Ordering::Acquire))
                    {
                        BulkItemResult {
                            input,
                            output,
                            status: "cancelled".into(),
                            exit_code: 5,
                            cues: 0,
                            review_needed: false,
                            elapsed_ms: item_start.elapsed().as_millis(),
                            error: Some(if stop_ref.load(Ordering::Acquire) {
                                "Skipped due to --fail-fast after earlier failure".into()
                            } else {
                                "Job cancelled before start".into()
                            }),
                        }
                    } else {
                        match runner(cfg, control) {
                            Ok(project) => {
                                let code = status::project(&project);
                                let review_needed = code == 2;
                                BulkItemResult {
                                    input,
                                    output,
                                    status: if review_needed {
                                        "review_needed".into()
                                    } else {
                                        "ok".into()
                                    },
                                    exit_code: code,
                                    cues: project.cues.len(),
                                    review_needed,
                                    elapsed_ms: item_start.elapsed().as_millis(),
                                    error: None,
                                }
                            }
                            Err(err) => {
                                let code = status::failure(&err);
                                if fail_fast {
                                    stop_ref.store(true, Ordering::Release);
                                    control.cancel.store(true, Ordering::Release);
                                }
                                let is_cancelled = matches!(err, AppError::Cancelled);
                                BulkItemResult {
                                    input,
                                    output,
                                    status: if is_cancelled {
                                        "cancelled".into()
                                    } else {
                                        "failed".into()
                                    },
                                    exit_code: code,
                                    cues: 0,
                                    review_needed: false,
                                    elapsed_ms: item_start.elapsed().as_millis(),
                                    error: Some(err.to_string()),
                                }
                            }
                        }
                    };
                    if let Ok(mut guard) = slots_ref.lock() {
                        guard[idx] = Some(result);
                    }
                }
            });
        }
    });

    let taken = match slots.into_inner() {
        Ok(vec) => vec,
        Err(poisoned) => poisoned.into_inner(),
    };
    taken
        .into_iter()
        .enumerate()
        .map(|(i, slot)| {
            slot.unwrap_or_else(|| BulkItemResult {
                input: jobs[i].0.clone(),
                output: jobs[i].1.clone(),
                status: "failed".into(),
                exit_code: 4,
                cues: 0,
                review_needed: false,
                elapsed_ms: 0,
                error: Some("Worker thread terminated without reporting result".into()),
            })
        })
        .collect()
}

pub(crate) fn summarize_exit_code(items: &[BulkItemResult]) -> i32 {
    let mut any_review = false;
    for item in items {
        if item.exit_code != 0 && item.exit_code != 2 {
            return item.exit_code;
        }
        if item.exit_code == 2 {
            any_review = true;
        }
    }
    if any_review { 2 } else { 0 }
}

pub(crate) fn run_bulk(args: BulkArgs, data_root: Option<&Path>, control: &Control) -> Result<i32> {
    run_bulk_with(args, data_root, control, &|cfg, ctrl, options| {
        nvsf_app::worker::supervise_with_srt(
            cfg,
            ctrl,
            |p| eprintln!("Bulk {} {}ms cues={}", p.stage, p.position_ms, p.cues),
            options,
        )
    })
}

fn run_bulk_with<F>(
    args: BulkArgs,
    data_root: Option<&Path>,
    control: &Control,
    extract: &F,
) -> Result<i32>
where
    F: Fn(ExtractConfig, &Control, nvsf_app::extraction::SrtOptions) -> Result<Project> + Sync,
{
    validate_concurrency(args.concurrency)?;
    if !["srt", "vtt", "ass", "json"].contains(&args.format.as_str()) {
        return Err(AppError::InvalidConfig(
            "Choose srt, vtt, ass or json".into(),
        ));
    }
    let inputs = discover_inputs(&args)?;
    let outputs = plan_outputs(&inputs, args.output_dir.as_deref(), &args.format)?;
    let effective_concurrency = args.concurrency.min(inputs.len());
    let threads = per_job_threads(args.threads, effective_concurrency);
    let data_root_utf8 = data_root.map(utf8_path).transpose()?;
    let format = args.format.clone();
    let overwrite = args.overwrite;
    let bom = args.bom;
    let crlf = args.crlf;

    let mut jobs = Vec::with_capacity(inputs.len());
    for (input, target_output) in inputs.into_iter().zip(outputs) {
        let extract_output = if format == "srt" {
            target_output.clone()
        } else {
            target_output.with_extension("srt")
        };
        let cfg = ExtractConfig {
            input: utf8_path(&input)?,
            output: utf8_path(&extract_output)?,
            data_root: data_root_utf8.clone(),
            language: args.language.clone(),
            device: args.device.clone(),
            quality: args.quality.clone(),
            regions: if args.roi.is_empty() {
                vec![Rect::default()]
            } else {
                args.roi.clone()
            },
            exclusions: args.exclude.clone(),
            start_ms: args.start_ms,
            end_ms: args.end_ms,
            threads,
            overwrite,
            normalize_persian: args.normalize_persian,
            min_confidence: args.min_confidence,
            gpu_required: args.gpu_required,
            accelerator_required: args.accelerator_required,
        };
        cfg.validate()
            .map_err(|e| AppError::InvalidConfig(e.to_string()))?;
        jobs.push((input, target_output, cfg));
    }

    let started = Instant::now();
    let results = execute_pool(
        jobs,
        effective_concurrency,
        args.fail_fast,
        control,
        &|cfg, ctrl| {
            let final_format = format.as_str();
            let target_path = PathBuf::from(&cfg.output).with_extension(final_format);
            let options = if final_format == "srt" {
                nvsf_app::extraction::SrtOptions { bom, crlf }
            } else {
                nvsf_app::extraction::SrtOptions::default()
            };
            let project = extract(cfg, ctrl, options)?;
            if final_format != "srt" {
                nvsf_app::export_project(
                    &project,
                    &target_path,
                    final_format,
                    overwrite,
                    bom,
                    crlf,
                )?;
            }
            Ok(project)
        },
    );

    let succeeded = results
        .iter()
        .filter(|r| matches!(r.status.as_str(), "ok" | "review_needed"))
        .count();
    let review_needed = results.iter().filter(|r| r.review_needed).count();
    let failed = results.len() - succeeded;
    let exit_code = summarize_exit_code(&results);

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "mode": "bulk",
            "concurrency": effective_concurrency,
            "per_job_threads": threads,
            "resume": args.resume,
            "total": results.len(),
            "succeeded": succeeded,
            "review_needed": review_needed,
            "failed": failed,
            "elapsed_ms": started.elapsed().as_millis(),
            "items": results,
        }))?
    );
    Ok(exit_code)
}

pub(crate) fn run_batch_concurrent(
    inputs: &[PathBuf],
    output_dir: &Path,
    language: &str,
    concurrency: usize,
    data_root: Option<&Path>,
    control: &Control,
) -> Result<i32> {
    validate_concurrency(concurrency)?;
    let data_root_utf8 = data_root.map(utf8_path).transpose()?;
    let threads = per_job_threads(2, concurrency.min(inputs.len().max(1)));
    let mut jobs = Vec::with_capacity(inputs.len());
    for (i, input) in inputs.iter().enumerate() {
        let stem = input
            .file_stem()
            .ok_or_else(|| nvsf_app::fail("Input has no filename"))?
            .to_str()
            .ok_or_else(|| nvsf_app::fail("Batch filename must be UTF-8"))?;
        let output = output_dir.join(format!("{stem}-{}.srt", i + 1));
        let cfg = ExtractConfig {
            input: utf8_path(input)?,
            output: utf8_path(&output)?,
            language: language.to_owned(),
            data_root: data_root_utf8.clone(),
            threads,
            ..Default::default()
        };
        jobs.push((input.clone(), output, cfg));
    }

    let results = execute_pool(jobs, concurrency, false, control, &|cfg, ctrl| {
        nvsf_app::extract(cfg, ctrl, |p| {
            eprintln!("Batch {} {}ms", p.stage, p.position_ms)
        })
    });

    let mut exit_code = 0i32;
    for (i, item) in results.iter().enumerate() {
        let succeeded = matches!(item.status.as_str(), "ok" | "review_needed");
        println!(
            "{}",
            json!({
                "item": i + 1,
                "completed": succeeded,
                "cues": item.cues,
                "review_needed": item.review_needed,
                "status": item.status,
                "error": item.error,
            })
        );
        if item.exit_code == 2 && exit_code == 0 {
            exit_code = 2;
        } else if item.exit_code != 0 && item.exit_code != 2 && (exit_code == 0 || exit_code == 2) {
            exit_code = item.exit_code;
        }
    }
    Ok(exit_code)
}

#[cfg(test)]
#[path = "bulk_tests.rs"]
mod tests;
