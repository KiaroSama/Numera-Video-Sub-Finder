use crate::{QueueAddArgs, QueueCommand, utf8_path};
use nvsf_app::{AppError, Control, Paths, Result, storage};
use nvsf_core::{ExtractConfig, Project, Rect};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::HashSet,
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const MAX_ENTRIES: usize = 100;
const MAX_BYTES: u64 = 2 * 1024 * 1024;
static NEXT_QUEUE_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Record {
    pub(crate) id: String,
    pub(crate) source: PathBuf,
    pub(crate) output: PathBuf,
    pub(crate) source_hash: String,
    pub(crate) config: ExtractConfig,
    pub(crate) checkpoint: Option<PathBuf>,
    pub(crate) state: String,
    pub(crate) error: Option<String>,
}

#[derive(Default, Debug, Serialize, Deserialize)]
pub(crate) struct QueueFile {
    pub(crate) schema: u32,
    pub(crate) records: Vec<Record>,
}

impl QueueFile {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.schema != 1 || self.records.len() > MAX_ENTRIES {
            return Err(nvsf_app::fail("Invalid or oversized persisted queue"));
        }
        let mut ids = HashSet::new();
        let mut outputs = HashSet::new();
        for r in &self.records {
            if r.id.is_empty()
                || r.id.len() > 128
                || !ids.insert(&r.id)
                || !outputs.insert(&r.output)
                || !r.source.is_absolute()
                || !r.output.is_absolute()
                || r.source.as_os_str().len() > 32_768
                || r.output.as_os_str().len() > 32_768
                || r.source_hash.len() != 64
                || !r.source_hash.bytes().all(|b| b.is_ascii_hexdigit())
                || ![
                    "queued",
                    "interrupted",
                    "running",
                    "completed",
                    "cancelled",
                    "failed",
                ]
                .contains(&r.state.as_str())
                || r.error.as_ref().is_some_and(|e| e.len() > 4096)
                || Path::new(&r.config.input) != r.source
                || Path::new(&r.config.output) != r.output
                || r.checkpoint
                    .as_ref()
                    .is_some_and(|p| *p != r.output.with_extension("nvsf.json"))
            {
                return Err(nvsf_app::fail(
                    "Persisted queue identity/path/state validation failed",
                ));
            }
            r.config.validate()?;
        }
        Ok(())
    }
}

struct QueueStore {
    root: PathBuf,
    file: QueueFile,
    _lock: storage::JobLock,
}

impl QueueStore {
    fn open(root: &Path) -> Result<Self> {
        let lock = storage::JobLock::acquire(&root.join("desktop-queue.lock"))?;
        let path = root.join("desktop-queue.json");
        let file = if !path.exists() {
            QueueFile {
                schema: 1,
                records: Vec::new(),
            }
        } else {
            if path.is_symlink() {
                return Err(nvsf_app::fail("Persisted queue must not be a symlink"));
            }
            if path.metadata()?.len() > MAX_BYTES {
                return Err(nvsf_app::fail("Persisted queue exceeds 2MiB"));
            }
            let mut bytes = Vec::new();
            std::fs::File::open(&path)?
                .take(MAX_BYTES + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() as u64 > MAX_BYTES {
                return Err(nvsf_app::fail("Persisted queue exceeds 2MiB"));
            }
            let mut parsed: QueueFile = serde_json::from_slice(&bytes)?;
            parsed.validate()?;
            for record in &mut parsed.records {
                if record.state == "running" {
                    record.state = "interrupted".into();
                }
            }
            parsed
        };
        Ok(Self {
            root: root.to_path_buf(),
            file,
            _lock: lock,
        })
    }

    fn save(&self) -> Result<()> {
        self.file.validate()?;
        let bytes = serde_json::to_vec_pretty(&self.file)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(nvsf_app::fail("Queue exceeds 2MiB persistence bound"));
        }
        storage::write_atomic(&self.root.join("desktop-queue.json"), &bytes, true)
    }

    fn entry_views(&self) -> Vec<serde_json::Value> {
        self.file
            .records
            .iter()
            .map(|r| {
                let resumable = r.checkpoint.is_some()
                    || matches!(r.state.as_str(), "interrupted" | "cancelled" | "failed");
                json!({
                    "id": r.id,
                    "source": r.source,
                    "output": r.output,
                    "language": r.config.language,
                    "device": r.config.device,
                    "quality": r.config.quality,
                    "state": r.state,
                    "checkpoint": r.checkpoint,
                    "resumable": resumable,
                    "error": r.error,
                })
            })
            .collect()
    }
}

pub(crate) fn run(paths: &Paths, command: QueueCommand, control: &Control) -> Result<i32> {
    match command {
        QueueCommand::List => {
            let store = QueueStore::open(&paths.root)?;
            let entries = store.entry_views();
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "count": entries.len(),
                    "entries": entries,
                }))?
            );
            Ok(0)
        }
        QueueCommand::Add(args) => {
            let mut store = QueueStore::open(&paths.root)?;
            let added = add_record(&mut store, &paths.root, *args)?;
            let entries = store.entry_views();
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "added": added.id,
                    "count": entries.len(),
                    "entries": entries,
                }))?
            );
            Ok(0)
        }
        QueueCommand::Remove { id } => {
            let mut store = QueueStore::open(&paths.root)?;
            remove_record(&mut store, &id)?;
            let entries = store.entry_views();
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "removed": id,
                    "count": entries.len(),
                    "entries": entries,
                }))?
            );
            Ok(0)
        }
        QueueCommand::Clear { state } => {
            let mut store = QueueStore::open(&paths.root)?;
            let cleared = clear_records(&mut store, &state)?;
            let entries = store.entry_views();
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "cleared": cleared,
                    "remaining": entries.len(),
                    "entries": entries,
                }))?
            );
            Ok(0)
        }
        QueueCommand::Run {
            id,
            concurrency,
            fail_fast,
        } => {
            let mut store = QueueStore::open(&paths.root)?;
            let (summary, exit_code) = run_records(
                &mut store,
                id.as_deref(),
                concurrency,
                fail_fast,
                control,
                |cfg, ctrl| {
                    nvsf_app::extract(cfg, ctrl, |p| {
                        eprintln!("Queue {} {}ms cues={}", p.stage, p.position_ms, p.cues)
                    })
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&summary)?);
            Ok(exit_code)
        }
    }
}

fn absolutize_output(path: &Path) -> Result<PathBuf> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    if let (Some(parent), Some(name)) = (abs.parent(), abs.file_name())
        && parent.is_dir()
    {
        return Ok(parent.canonicalize()?.join(name));
    }
    Ok(abs)
}

fn generate_id(existing: &[Record]) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    loop {
        let seq = NEXT_QUEUE_ID.fetch_add(1, Ordering::Relaxed);
        let candidate = format!("cli-{millis}-{seq}");
        if !existing.iter().any(|r| r.id == candidate) {
            return candidate;
        }
    }
}

fn add_record(store: &mut QueueStore, data_root: &Path, args: QueueAddArgs) -> Result<Record> {
    if store.file.records.len() >= MAX_ENTRIES {
        return Err(nvsf_app::fail("Queue is limited to 100 entries"));
    }
    let source = args.input.canonicalize()?;
    if !source.is_file() {
        return Err(AppError::InvalidConfig(
            "Queue input must be an existing regular file".into(),
        ));
    }
    let output = absolutize_output(&args.output)?;
    let expected_checkpoint = output.with_extension("nvsf.json");
    let checkpoint = if let Some(cp) = args.checkpoint {
        let canonical_cp = cp.canonicalize()?;
        let expected_canon = expected_checkpoint
            .canonicalize()
            .unwrap_or_else(|_| expected_checkpoint.clone());
        if canonical_cp != expected_canon {
            return Err(AppError::InvalidConfig(
                "Checkpoint path must equal output path with .nvsf.json extension".into(),
            ));
        }
        let saved = storage::load_project(&canonical_cp)?;
        let hash = storage::hash_file(&source)?;
        if saved.source_hash != hash {
            return Err(nvsf_app::fail(
                "Checkpoint source fingerprint does not match input video",
            ));
        }
        Some(expected_checkpoint)
    } else if expected_checkpoint.is_file() {
        Some(expected_checkpoint)
    } else {
        None
    };

    if store.file.records.iter().any(|r| r.output == output) {
        return Err(AppError::InvalidConfig(
            "Queue already contains an entry with this output path".into(),
        ));
    }

    let source_hash = storage::hash_file(&source)?;
    let config = ExtractConfig {
        input: utf8_path(&source)?,
        output: utf8_path(&output)?,
        data_root: Some(utf8_path(data_root)?),
        language: args.language,
        device: args.device,
        quality: args.quality,
        regions: if args.roi.is_empty() {
            vec![Rect::default()]
        } else {
            args.roi
        },
        exclusions: args.exclude,
        start_ms: args.start_ms,
        end_ms: args.end_ms,
        threads: args.threads,
        overwrite: args.overwrite,
        normalize_persian: args.normalize_persian,
        min_confidence: args.min_confidence,
        gpu_required: args.gpu_required,
        accelerator_required: args.accelerator_required,
    };
    config
        .validate()
        .map_err(|e| AppError::InvalidConfig(e.to_string()))?;

    let record = Record {
        id: generate_id(&store.file.records),
        source,
        output,
        source_hash,
        config,
        checkpoint,
        state: "queued".into(),
        error: None,
    };
    store.file.records.push(record.clone());
    store.save()?;
    Ok(record)
}

fn remove_record(store: &mut QueueStore, id: &str) -> Result<()> {
    let idx = store
        .file
        .records
        .iter()
        .position(|r| r.id == id)
        .ok_or_else(|| nvsf_app::fail(format!("Queue entry {id} not found")))?;
    if store.file.records[idx].state == "running" {
        return Err(nvsf_app::fail("Cannot remove a running queue entry"));
    }
    store.file.records.remove(idx);
    store.save()
}

fn clear_records(store: &mut QueueStore, state: &str) -> Result<usize> {
    if !matches!(
        state,
        "all" | "queued" | "interrupted" | "completed" | "cancelled" | "failed"
    ) {
        return Err(AppError::InvalidConfig(
            "State filter must be all, queued, interrupted, completed, cancelled or failed".into(),
        ));
    }
    let before = store.file.records.len();
    store.file.records.retain(|r| {
        if r.state == "running" {
            true
        } else if state == "all" {
            false
        } else {
            r.state != state
        }
    });
    let cleared = before - store.file.records.len();
    store.save()?;
    Ok(cleared)
}

fn run_records<F>(
    store: &mut QueueStore,
    selected_id: Option<&str>,
    concurrency: usize,
    fail_fast: bool,
    control: &Control,
    runner: F,
) -> Result<(serde_json::Value, i32)>
where
    F: Fn(ExtractConfig, &Control) -> Result<Project> + Sync,
{
    if !(1..=8).contains(&concurrency) {
        return Err(AppError::InvalidConfig(
            "Queue concurrency must be between 1 and 8".into(),
        ));
    }
    let indices: Vec<usize> = if let Some(id) = selected_id {
        let idx = store
            .file
            .records
            .iter()
            .position(|r| r.id == id)
            .ok_or_else(|| nvsf_app::fail(format!("Queue entry {id} not found")))?;
        vec![idx]
    } else {
        store
            .file
            .records
            .iter()
            .enumerate()
            .filter(|(_, r)| r.state != "completed" && r.state != "running")
            .map(|(i, _)| i)
            .collect()
    };

    let started = Instant::now();
    if indices.is_empty() {
        return Ok((
            json!({
                "mode": "queue",
                "concurrency": concurrency,
                "total": 0,
                "succeeded": 0,
                "review_needed": 0,
                "failed": 0,
                "elapsed_ms": started.elapsed().as_millis(),
                "items": [],
            }),
            0,
        ));
    }

    let mut jobs = Vec::with_capacity(indices.len());
    for &idx in &indices {
        let record = &mut store.file.records[idx];
        let hash = storage::hash_file(&record.source)?;
        if hash != record.source_hash {
            return Err(nvsf_app::fail(format!(
                "Queue entry {} source file changed since it was queued",
                record.id
            )));
        }
        record.state = "running".into();
        record.error = None;
        let mut cfg = record.config.clone();
        if concurrency > 1 {
            cfg.threads = (cfg.threads / concurrency).max(1);
        }
        jobs.push((record.source.clone(), record.output.clone(), cfg));
    }
    store.save()?;

    let results = crate::bulk::execute_pool(jobs, concurrency, fail_fast, control, &runner);

    let mut succeeded = 0usize;
    let mut review_needed = 0usize;
    let mut failed = 0usize;
    let mut exit_code = 0i32;
    let mut items = Vec::with_capacity(indices.len());

    for (pos, &idx) in indices.iter().enumerate() {
        let item = &results[pos];
        let record = &mut store.file.records[idx];
        let checkpoint = record.output.with_extension("nvsf.json");
        record.checkpoint = checkpoint.is_file().then_some(checkpoint);
        match item.status.as_str() {
            "ok" => {
                record.state = "completed".into();
                record.error = None;
                succeeded += 1;
            }
            "review_needed" => {
                record.state = "completed".into();
                record.error = None;
                succeeded += 1;
                review_needed += 1;
                if exit_code == 0 {
                    exit_code = 2;
                }
            }
            "cancelled" => {
                record.state = "cancelled".into();
                record.error = item.error.clone().map(|e| truncate_error(&e));
                failed += 1;
                exit_code = combine_failure(exit_code, item.exit_code);
            }
            _ => {
                record.state = "failed".into();
                record.error = item.error.clone().map(|e| truncate_error(&e));
                failed += 1;
                exit_code = combine_failure(exit_code, item.exit_code);
            }
        }
        items.push(json!({
            "id": record.id,
            "input": item.input,
            "output": item.output,
            "status": item.status,
            "exit_code": item.exit_code,
            "cues": item.cues,
            "review_needed": item.review_needed,
            "elapsed_ms": item.elapsed_ms,
            "error": item.error,
        }));
    }
    store.save()?;

    Ok((
        json!({
            "mode": "queue",
            "concurrency": concurrency,
            "total": indices.len(),
            "succeeded": succeeded,
            "review_needed": review_needed,
            "failed": failed,
            "elapsed_ms": started.elapsed().as_millis(),
            "items": items,
        }),
        exit_code,
    ))
}

fn combine_failure(current: i32, next: i32) -> i32 {
    if current == 0 || current == 2 {
        next
    } else {
        current
    }
}

fn truncate_error(text: &str) -> String {
    if text.len() <= 4096 {
        return text.to_owned();
    }
    let mut end = 4096;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

#[cfg(test)]
#[path = "queue_cmd_tests.rs"]
mod tests;
