use crate::state::{Destination, Host, Result, Source, blocking, display_name, lock, path_text};
use nvsf_core::ExtractConfig;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
};

pub(crate) const MAX_ENTRIES: usize = 100;
const MAX_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    pub source: PathBuf,
    pub output: PathBuf,
    pub source_hash: String,
    pub config: ExtractConfig,
    pub checkpoint: Option<PathBuf>,
    pub state: String,
    pub error: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
pub struct QueueFile {
    pub(crate) schema: u32,
    pub(crate) records: Vec<Record>,
}

impl QueueFile {
    pub fn validate(&self) -> Result<()> {
        if self.schema != 1 || self.records.len() > MAX_ENTRIES {
            return Err("Invalid or oversized persisted queue".into());
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
                return Err("Persisted queue identity/path/state validation failed".into());
            }
            r.config.validate().map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct Store {
    pub(crate) file: QueueFile,
    pub(crate) grants: HashMap<String, Grant>,
    error: Option<String>,
    _owner: Option<nvsf_app::storage::JobLock>,
}
#[derive(Clone)]
pub(crate) struct Grant {
    pub source: Source,
    pub output: Destination,
}

#[derive(Clone, Serialize)]
pub struct QueueEntry {
    pub id: String,
    pub name: String,
    pub output_name: String,
    pub language: String,
    pub device: String,
    pub quality: String,
    pub state: String,
    pub needs_regrant: bool,
    pub resumable: bool,
    pub error: Option<String>,
}

impl Store {
    pub fn load(root: &Path) -> Self {
        let owner = match nvsf_app::storage::JobLock::acquire(&root.join("desktop-queue.lock")) {
            Ok(owner) => owner,
            Err(e) => {
                return Self {
                    error: Some(format!(
                        "Durable queue is owned by another application instance: {e}"
                    )),
                    ..Self::default()
                };
            }
        };
        let path = root.join("desktop-queue.json");
        let result = (|| -> Result<QueueFile> {
            if !path.exists() {
                return Ok(QueueFile {
                    schema: 1,
                    records: vec![],
                });
            }
            if path.is_symlink() {
                return Err("Persisted queue must not be a symlink".into());
            }
            if path.metadata().map_err(|e| e.to_string())?.len() > MAX_BYTES {
                return Err("Persisted queue exceeds 2MiB".into());
            }
            let mut bytes = Vec::new();
            std::fs::File::open(&path)
                .map_err(|e| e.to_string())?
                .take(MAX_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() as u64 > MAX_BYTES {
                return Err("Persisted queue exceeds 2MiB".into());
            }
            let file: QueueFile = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            file.validate()?;
            Ok(file)
        })();
        match result {
            Ok(mut file) => {
                for r in &mut file.records {
                    if r.state == "running" {
                        r.state = "interrupted".into();
                    }
                }
                Self {
                    file,
                    grants: HashMap::new(),
                    error: None,
                    _owner: Some(owner),
                }
            }
            Err(e) => Self {
                error: Some(format!(
                    "Queue could not be restored; original file preserved: {e}"
                )),
                _owner: Some(owner),
                ..Self::default()
            },
        }
    }
    pub(crate) fn check(&self) -> Result<()> {
        if let Some(e) = &self.error {
            Err(e.clone())
        } else {
            Ok(())
        }
    }
    pub(crate) fn save(&self, root: &Path) -> Result<()> {
        self.check()?;
        self.file.validate()?;
        let bytes = serde_json::to_vec_pretty(&self.file).map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err("Queue exceeds 2MiB persistence bound".into());
        }
        nvsf_app::storage::write_atomic(&root.join("desktop-queue.json"), &bytes, true)
            .map_err(|e| e.to_string())
    }
    pub(crate) fn entries(&self) -> Result<Vec<QueueEntry>> {
        self.check()?;
        Ok(self
            .file
            .records
            .iter()
            .map(|r| QueueEntry {
                id: r.id.clone(),
                name: display_name(&r.source),
                output_name: display_name(&r.output),
                language: r.config.language.clone(),
                device: r.config.device.clone(),
                quality: r.config.quality.clone(),
                state: r.state.clone(),
                needs_regrant: !self.grants.contains_key(&r.id),
                resumable: r.checkpoint.is_some()
                    || matches!(r.state.as_str(), "interrupted" | "cancelled" | "failed"),
                error: r.error.clone(),
            })
            .collect())
    }
}

fn config_for(
    host: &Host,
    source_token: &str,
    output_token: &str,
    mut config: ExtractConfig,
) -> Result<ExtractConfig> {
    config.input = path_text(&host.source(source_token)?)?;
    config.output = path_text(&host.destination(output_token)?)?;
    config.data_root = Some(path_text(&host.0.paths.root)?);
    config.validate().map_err(|e| e.to_string())?;
    Ok(config)
}

pub fn register(
    host: &Host,
    source_token: &str,
    output_token: &str,
    config: ExtractConfig,
) -> Result<(String, ExtractConfig)> {
    register_with(host, source_token, output_token, config, |source| {
        crate::files::probe_source(host, source)
    })
}

pub(crate) fn register_with(
    host: &Host,
    source_token: &str,
    output_token: &str,
    config: ExtractConfig,
    probe: impl FnOnce(&Path) -> Result<nvsf_core::MediaInfo>,
) -> Result<(String, ExtractConfig)> {
    let config = config_for(host, source_token, output_token, config)?;
    let source = host.source(source_token)?;
    let output = host.destination(output_token)?;
    if source == output || output.canonicalize().is_ok_and(|p| p == source) {
        return Err("Queue output must not replace source media".into());
    }
    let source_hash = nvsf_app::storage::hash_file(&source).map_err(|e| e.to_string())?;
    let media = probe(&source)?;
    let id = host.token("queue");
    let record = Record {
        id: id.clone(),
        source: source.clone(),
        output: output.clone(),
        source_hash: source_hash.clone(),
        config: config.clone(),
        checkpoint: None,
        state: "queued".into(),
        error: None,
    };
    let mut store = lock(&host.0.queue)?;
    store.check()?;
    if store.file.records.iter().any(|r| r.output == output) {
        return Err("This destination already has a saved queue record. Resume that record or remove it before starting new work".into());
    }
    if store.file.records.len() >= MAX_ENTRIES {
        return Err("Queue is limited to 100 records; remove finished entries first".into());
    }
    store.file.records.push(record);
    if let Err(e) = store.save(&host.0.paths.root) {
        store.file.records.pop();
        return Err(e);
    }
    store.grants.insert(
        id.clone(),
        Grant {
            source: Source {
                token: source_token.into(),
                name: display_name(&source),
                fingerprint: source_hash,
                media,
            },
            output: Destination {
                token: output_token.into(),
                name: display_name(&output),
            },
        },
    );
    Ok((id, config))
}

pub fn mark(host: &Host, id: &str, state: &str, error: Option<String>) -> Result<()> {
    let mut store = lock(&host.0.queue)?;
    store.check()?;
    let before = store.file.records.clone();
    let record = store
        .file
        .records
        .iter_mut()
        .find(|r| r.id == id)
        .ok_or_else(|| "Queue record disappeared".to_string())?;
    record.state = state.into();
    record.error = error.map(|e| e.chars().take(1000).collect());
    if record.output.with_extension("nvsf.json").is_file() {
        record.checkpoint = Some(record.output.with_extension("nvsf.json"));
    }
    if let Err(e) = store.save(&host.0.paths.root) {
        store.file.records = before;
        return Err(e);
    }
    Ok(())
}

#[tauri::command]
pub async fn queue_list(host: tauri::State<'_, Host>) -> Result<Vec<QueueEntry>> {
    let host = host.inner().clone();
    blocking(move || lock(&host.0.queue)?.entries()).await
}

#[tauri::command]
pub async fn queue_add(
    host: tauri::State<'_, Host>,
    source_token: String,
    output_token: String,
    config: ExtractConfig,
) -> Result<Vec<QueueEntry>> {
    let host = host.inner().clone();
    blocking(move || {
        let _operation = lock(&host.0.operation)?;
        register(&host, &source_token, &output_token, config)?;
        lock(&host.0.queue)?.entries()
    })
    .await
}

#[tauri::command]
pub async fn queue_remove(host: tauri::State<'_, Host>, id: String) -> Result<Vec<QueueEntry>> {
    let host = host.inner().clone();
    blocking(move || {
        let _operation = lock(&host.0.operation)?;
        let mut store = lock(&host.0.queue)?;
        store.check()?;
        let i = store
            .file
            .records
            .iter()
            .position(|r| r.id == id)
            .ok_or_else(|| "Queue record not found".to_string())?;
        if store.file.records[i].state == "running" {
            return Err("Cancel the active job before removing its queue record".into());
        }
        let removed = store.file.records.remove(i);
        if let Err(e) = store.save(&host.0.paths.root) {
            store.file.records.insert(i, removed);
            return Err(e);
        }
        store.grants.remove(&id);
        store.entries()
    })
    .await
}

#[tauri::command]
pub async fn queue_regrant(
    app: tauri::AppHandle,
    host: tauri::State<'_, Host>,
    id: String,
) -> Result<Option<Vec<QueueEntry>>> {
    let host = host.inner().clone();
    blocking(move || {
        regrant_queue_with(&host, &id, |record| {
            crate::resume::regrant(&app, &host, record)
        })
    })
    .await
}

pub(crate) fn regrant_queue_with(
    host: &Host,
    id: &str,
    regrant: impl FnOnce(&Record) -> Result<Option<Grant>>,
) -> Result<Option<Vec<QueueEntry>>> {
    host.ensure_idle()?;
    if host.0.unsaved.load(std::sync::atomic::Ordering::Acquire) {
        return Err("Save current corrections before regranting/resuming a queue entry".into());
    }
    let record = {
        let store = lock(&host.0.queue)?;
        store.check()?;
        store
            .file
            .records
            .iter()
            .find(|r| r.id == id)
            .cloned()
            .ok_or_else(|| "Queue record not found".to_string())?
    };
    let Some(grant) = regrant(&record)? else {
        return Ok(None);
    };
    let _operation = lock(&host.0.operation)?;
    host.ensure_idle()?;
    let mut store = lock(&host.0.queue)?;
    store.check()?;
    if !store.file.records.iter().any(|r| r.id == id) {
        return Err("Queue record was removed while its pickers were open".into());
    }
    store.grants.insert(id.into(), grant);
    Ok(Some(store.entries()?))
}

#[derive(Serialize)]
pub struct Started {
    pub id: String,
    pub source: Source,
    pub output: Destination,
}

#[tauri::command]
pub async fn queue_start(host: tauri::State<'_, Host>, id: String) -> Result<Started> {
    let host = host.inner().clone();
    blocking(move ||{
        let _operation=lock(&host.0.operation)?;host.ensure_idle()?;
        if host.0.unsaved.load(std::sync::atomic::Ordering::Acquire) { return Err("Save current corrections before starting/resuming a queue entry".into()); }
        let (record,grant)={let store=lock(&host.0.queue)?;store.check()?;
            (store.file.records.iter().find(|r|r.id==id).cloned().ok_or_else(||"Queue record not found".to_string())?,
            store.grants.get(&id).cloned().ok_or_else(||"Regrant the source and output with native pickers before starting this restored job".to_string())?)};
        let source=host.source(&grant.source.token)?;host.destination(&grant.output.token)?;
        if record.state=="completed" {return Err("Saved job is complete; open its project in History/Review instead".into());}
        if record.checkpoint.as_ref().is_some_and(|p|!p.is_file()) {return Err("Saved checkpoint is missing; restore it before resuming. Queue/source/output remain unchanged".into());}
        if nvsf_app::storage::hash_file(&source).map_err(|e|e.to_string())?!=record.source_hash{return Err("Queued source content changed; existing state preserved".into());}
        let root=path_text(&host.0.paths.root)?;
        if record.config.data_root.as_deref().is_some_and(|saved|saved!=root.as_str()){return Err("Application data root changed; restore the original root before resuming this queue record".into());}
        let checkpoint=record.output.with_extension("nvsf.json");
        if checkpoint.is_file() {
            let checkpoint=&checkpoint;
            let project=nvsf_app::storage::load_project(checkpoint).map_err(|e|e.to_string())?;
            crate::resume::check_checkpoint(&project,checkpoint)?;
            if project.source_hash!=record.source_hash || serde_json::to_vec(&project.config).map_err(|e|e.to_string())?!=serde_json::to_vec(&record.config).map_err(|e|e.to_string())? {
                return Err("Checkpoint no longer matches queued identity/configuration".into());
            }
        }
        let job_id=crate::jobs::launch(&host,record.config,Some(id))?;
        Ok(Started {id:job_id,source:grant.source,output:grant.output})
    }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    fn record() -> Record {
        let root = std::env::current_dir().unwrap();
        let source = root.join("video.mp4");
        let output = root.join("video.srt");
        let config = ExtractConfig {
            input: source.to_str().unwrap().into(),
            output: output.to_str().unwrap().into(),
            ..ExtractConfig::default()
        };
        Record {
            id: "q1".into(),
            source,
            output,
            source_hash: "a".repeat(64),
            config,
            checkpoint: None,
            state: "queued".into(),
            error: None,
        }
    }
    #[test]
    fn durable_queue_validates_identity_and_never_restores_grants() {
        let file = QueueFile {
            schema: 1,
            records: vec![record()],
        };
        file.validate().unwrap();
        let json = serde_json::to_vec(&file).unwrap();
        let restored: QueueFile = serde_json::from_slice(&json).unwrap();
        let store = Store {
            file: restored,
            ..Store::default()
        };
        assert!(store.entries().unwrap()[0].needs_regrant);
        assert!(!String::from_utf8(json).unwrap().contains("token"));
        let mut duplicate = file;
        duplicate.records.push(record());
        assert!(duplicate.validate().is_err());
        duplicate.records.pop();
        duplicate.records[0].source_hash = "bad".into();
        assert!(duplicate.validate().is_err());
        let corrupt = Store {
            error: Some("corrupt persisted queue".into()),
            ..Store::default()
        };
        assert!(corrupt.entries().is_err());
    }
    #[test]
    fn restart_and_corruption_preserve_durable_user_state() {
        let root = tempfile::tempdir().unwrap();
        {
            let mut store = Store::load(root.path());
            let mut pending = record();
            pending.state = "running".into();
            store.file.records.push(pending);
            store.save(root.path()).unwrap();
            assert!(
                Store::load(root.path()).entries().is_err(),
                "second writer must be blocked"
            );
        }
        {
            let restored = Store::load(root.path());
            let entries = restored.entries().unwrap();
            assert_eq!(entries[0].state, "interrupted");
            assert!(entries[0].needs_regrant);
            assert!(restored.grants.is_empty());
        }
        let path = root.path().join("desktop-queue.json");
        std::fs::write(&path, b"invalid UTF-8 JSON \xff").unwrap();
        let corrupted = Store::load(root.path());
        assert!(corrupted.entries().is_err());
        assert!(corrupted.save(root.path()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"invalid UTF-8 JSON \xff");
    }
}
