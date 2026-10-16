use crate::{
    helpers,
    ipc::{self, Destination, Doctor, HistoryEntry, JobStatus, Model, QueueEntry, Source, Started},
};
use leptos::prelude::*;
use nvsf_core::{Cue, EditHistory, ExtractConfig, Project};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use wasm_bindgen_futures::spawn_local;

#[derive(Clone)]
pub struct Queued {
    pub source: Source,
    pub output: Destination,
    pub config: ExtractConfig,
}

#[derive(Clone, Copy)]
pub struct Ui {
    pub route: RwSignal<String>,
    pub source: RwSignal<Option<Source>>,
    pub output: RwSignal<Option<Destination>>,
    pub config: RwSignal<ExtractConfig>,
    pub project: RwSignal<Option<Project>>,
    pub selected: RwSignal<Option<u64>>,
    pub history: StoredValue<EditHistory>,
    pub revision: RwSignal<u64>,
    pub dirty: RwSignal<bool>,
    pub draft_dirty: RwSignal<bool>,
    pub busy: RwSignal<bool>,
    pub error: RwSignal<Option<String>>,
    pub notice: RwSignal<String>,
    pub status: RwSignal<JobStatus>,
    pub queue: RwSignal<Vec<QueueEntry>>,
    pub models: RwSignal<Vec<Model>>,
    pub recent: RwSignal<Vec<HistoryEntry>>,
    pub diagnostics: RwSignal<Option<Doctor>>,
    pub maintenance: RwSignal<crate::maintenance::Status>,
    pub theme: RwSignal<String>,
    pub preview_url: RwSignal<String>,
    pub preview_busy: RwSignal<bool>,
    pub time: RwSignal<u64>,
    pub preview_generation: RwSignal<u64>,
}

impl Ui {
    pub fn new(theme: String) -> Self {
        Self {
            route: RwSignal::new("Extract".into()),
            source: RwSignal::new(None),
            output: RwSignal::new(None),
            config: RwSignal::new(ExtractConfig::default()),
            project: RwSignal::new(None),
            selected: RwSignal::new(None),
            history: StoredValue::new(EditHistory::default()),
            revision: RwSignal::new(0),
            dirty: RwSignal::new(false),
            draft_dirty: RwSignal::new(false),
            busy: RwSignal::new(false),
            error: RwSignal::new(None),
            notice: RwSignal::new(String::new()),
            status: RwSignal::new(JobStatus {
                state: "idle".into(),
                ..JobStatus::default()
            }),
            queue: RwSignal::new(vec![]),
            models: RwSignal::new(vec![]),
            recent: RwSignal::new(vec![]),
            diagnostics: RwSignal::new(None),
            maintenance: RwSignal::new(crate::maintenance::Status::default()),
            theme: RwSignal::new(theme),
            preview_url: RwSignal::new(String::new()),
            preview_busy: RwSignal::new(false),
            time: RwSignal::new(0),
            preview_generation: RwSignal::new(0),
        }
    }
    pub fn active(self) -> bool {
        matches!(
            self.status.read().state.as_str(),
            "running" | "paused" | "cancelling"
        )
    }
    pub fn request<T: DeserializeOwned + 'static>(
        self,
        command: &'static str,
        args: Value,
        success: impl FnOnce(T) + 'static,
    ) {
        if self.busy.get_untracked() {
            return;
        }
        if matches!(
            command,
            "pick_project" | "open_history" | "start_job" | "queue_start" | "pick_checkpoint"
        ) && (self.dirty.get_untracked() || self.draft_dirty.get_untracked())
        {
            self.error.set(Some(
                "Save your current review project before replacing it or starting another job"
                    .into(),
            ));
            return;
        }
        self.busy.set(true);
        self.error.set(None);
        spawn_local(async move {
            match ipc::call::<T>(command, args).await {
                Ok(value) => success(value),
                Err(e) => self.error.set(Some(e)),
            }
            self.busy.set(false);
        });
    }
    pub fn load(self, project: Project) {
        if !self.source.with_untracked(|s| {
            s.as_ref()
                .is_some_and(|s| s.fingerprint == project.source_hash)
        }) {
            self.source.set(None);
            let old = self.preview_url.get_untracked();
            self.preview_url.set(String::new());
            if !old.is_empty() {
                let _ = web_sys::Url::revoke_object_url(&old);
            }
        }
        self.config.set(project.config.clone());
        self.selected.set(project.cues.first().map(|c| c.id));
        self.project.set(Some(project));
        self.history.set_value(EditHistory::default());
        self.dirty.set(false);
        self.draft_dirty.set(false);
        self.revision.update(|v| *v += 1);
        self.route.set("Review".into());
    }
    pub fn pick_source(self) {
        let expected_hash = if self.route.get_untracked() == "Review" {
            self.project
                .with_untracked(|p| p.as_ref().map(|p| p.source_hash.clone()))
        } else {
            None
        };
        self.request::<Option<Source>>(
            "pick_source",
            json!({"expectedHash": expected_hash}),
            move |source| {
                if let Some(source) = source {
                    self.source.set(Some(source));
                    self.time.set(0);
                    self.seek(0);
                }
            },
        );
    }
    pub fn seek(self, time: u64) {
        self.time.set(time);
        let Some(source) = self.source.get_untracked() else {
            return;
        };
        let time = source
            .media
            .duration_ms
            .map(|duration| time.min(duration.saturating_sub(1)))
            .unwrap_or(time);
        self.time.set(time);
        if self.preview_busy.get_untracked() {
            return;
        }
        self.preview_busy.set(true);
        self.preview_generation.update(|n| *n += 1);
        let generation = self.preview_generation.get_untracked();
        spawn_local(async move {
            match ipc::preview(&source.token, time).await {
                Ok(url) => {
                    if self.preview_url.is_disposed() {
                        let _ = web_sys::Url::revoke_object_url(&url);
                        return;
                    }
                    if self.preview_generation.get_untracked() == generation
                        && self.source.with_untracked(|s| {
                            s.as_ref()
                                .map(|s| s.token.as_str() == source.token.as_str())
                                .unwrap_or(false)
                        })
                    {
                        let old = self.preview_url.get_untracked();
                        self.preview_url.set(url);
                        if !old.is_empty() {
                            let _ = web_sys::Url::revoke_object_url(&old);
                        }
                    } else {
                        let _ = web_sys::Url::revoke_object_url(&url);
                    }
                }
                Err(e) => self.error.set(Some(e)),
            }
            self.preview_busy.set(false);
            if self.time.get_untracked() != time
                || self.source.with_untracked(|s| {
                    s.as_ref().map(|s| s.token != source.token).unwrap_or(false)
                })
            {
                self.seek(self.time.get_untracked());
            }
        });
    }
    pub fn selected_cue(self) -> Option<Cue> {
        let id = self.selected.get()?;
        self.project
            .with(|p| p.as_ref()?.cues.iter().find(|c| c.id == id).cloned())
    }
    pub fn edit(self, change: impl FnOnce(&mut Vec<Cue>) -> nvsf_core::Result<()>) {
        let mut error = None;
        self.project.update(|project| {
            if let Some(project) = project {
                self.history.update_value(|history| {
                    if let Err(e) = history.apply(&mut project.cues, change) {
                        error = Some(e.to_string());
                    }
                });
            }
        });
        if let Some(e) = error {
            self.error.set(Some(e));
        } else {
            self.dirty.set(true);
            self.draft_dirty.set(false);
            self.revision.update(|v| *v += 1);
        }
    }
    pub fn travel(self, redo: bool) {
        if self.draft_dirty.get_untracked() {
            self.error.set(Some(
                "Apply or discard the current cue draft before undo/redo".into(),
            ));
            return;
        }
        self.project.update(|p| {
            if let Some(p) = p {
                self.history.update_value(|h| {
                    if redo {
                        h.redo(&mut p.cues);
                    } else {
                        h.undo(&mut p.cues);
                    }
                });
            }
        });
        self.dirty.set(true);
        self.revision.update(|v| *v += 1);
    }
    pub fn begin(self, queued: Queued) {
        if self.dirty.get_untracked() || self.draft_dirty.get_untracked() {
            self.error.set(Some(
                "Save current corrections before starting another job".into(),
            ));
            return;
        }
        if self.active() {
            self.error.set(Some(
                "Wait for the active job before starting another".into(),
            ));
            return;
        }
        self.source.set(Some(queued.source.clone()));
        self.request::<String>("start_job", json!({"sourceToken": queued.source.token, "outputToken": queued.output.token, "config": queued.config}), move |id| self.watch_job(id));
    }
    pub fn watch_job(self, id: String) {
        self.status.set(JobStatus {
            id,
            state: "running".into(),
            ..JobStatus::default()
        });
        self.route.set("Queue".into());
        spawn_local(async move {
            match ipc::call::<Vec<QueueEntry>>("queue_list", json!({})).await {
                Ok(entries) => self.queue.set(entries),
                Err(e) => self.error.set(Some(e)),
            }
            loop {
                gloo_timers::future::TimeoutFuture::new(600).await;
                if self.status.is_disposed() {
                    break;
                }
                match ipc::call::<JobStatus>("job_status", json!({})).await {
                    Ok(status) => {
                        let active =
                            matches!(status.state.as_str(), "running" | "paused" | "cancelling");
                        if let Some(e) = status.error.clone() {
                            self.error.set(Some(e));
                        }
                        let ready = status.project_ready;
                        self.status.set(status);
                        if !active {
                            match ipc::call::<Vec<QueueEntry>>("queue_list", json!({})).await {
                                Ok(entries) => self.queue.set(entries),
                                Err(e) => self.error.set(Some(e)),
                            }
                            if ready {
                                match ipc::call::<Option<Project>>("current_project", json!({}))
                                    .await
                                {
                                    Ok(Some(p)) => self.load(p),
                                    Ok(None) => self.error.set(Some(
                                        "Completed job did not return a review project".into(),
                                    )),
                                    Err(e) => self.error.set(Some(e)),
                                }
                            }
                            break;
                        }
                    }
                    Err(e) => {
                        self.error.set(Some(e));
                        break;
                    }
                }
            }
        });
    }
    pub fn add_queue(self) {
        if let Some(q) = self.current_queue() {
            self.request::<Vec<QueueEntry>>("queue_add",json!({"sourceToken":q.source.token,"outputToken":q.output.token,"config":q.config}),move |entries|{
                self.queue.set(entries);self.notice.set("Saved to the durable queue. Restored entries require source/output regrant; nothing starts automatically.".into());
            });
        }
    }
    pub fn start_saved(self, id: String) {
        self.request::<Started>("queue_start", json!({"id":id}), move |started| {
            self.source.set(Some(started.source));
            self.output.set(Some(started.output));
            self.watch_job(started.id);
        });
    }
    pub fn resume_picker(self) {
        self.request::<Option<Vec<QueueEntry>>>("pick_checkpoint",json!({}),move |entries|if let Some(entries)=entries{
            self.queue.set(entries);self.route.set("Queue".into());self.notice.set("Checkpoint registered with fresh native grants. Review its configuration/preservation warning, then explicitly resume the queue entry.".into());
        });
    }
    pub fn current_queue(self) -> Option<Queued> {
        Some(Queued {
            source: self.source.get_untracked()?,
            output: self.output.get_untracked()?,
            config: self.config.get_untracked(),
        })
    }
    pub fn persist(self, format: Option<String>, bom: bool, crlf: bool) {
        let Some(project) = self.project.get_untracked() else {
            return;
        };
        if self.busy.get_untracked() {
            return;
        }
        if self.draft_dirty.get_untracked() {
            self.error.set(Some(
                "Apply the current cue correction before saving or exporting".into(),
            ));
            return;
        }
        let revision = self.revision.get_untracked();
        self.busy.set(true);
        self.error.set(None);
        spawn_local(async move {
            let result = async {
                ipc::call::<()>("replace_project", json!({"project": project})).await?;
                if let Some(format) = format {
                    ipc::call::<Option<String>>(
                        "export_project",
                        json!({"format": format, "bom": bom, "crlf": crlf}),
                    )
                    .await
                } else {
                    ipc::call::<Option<String>>("save_project", json!({})).await
                }
            }
            .await;
            match result {
                Ok(Some(name)) => {
                    self.notice.set(format!("Saved {name}"));
                    if self.revision.get_untracked() == revision {
                        self.dirty.set(false);
                    }
                }
                Ok(None) => {}
                Err(e) => self.error.set(Some(e)),
            }
            self.busy.set(false);
        });
    }
    pub fn set_theme(self, value: String) {
        let value = helpers::theme_value(&value).to_string();
        if let Some(window) = web_sys::window() {
            if let Some(document) = window.document() {
                if let Some(root) = document.document_element() {
                    let _ = root.set_attribute("data-theme", &value);
                }
            }
            match window.local_storage() {
                Ok(Some(storage)) => {
                    if let Err(e) = storage.set_item("nvsf-theme", &value) {
                        self.error
                            .set(Some(format!("Theme could not be persisted: {e:?}")));
                    }
                }
                _ => self.error.set(Some(
                    "Theme storage unavailable; preference applies for this session only".into(),
                )),
            }
        }
        self.theme.set(value.clone());
        spawn_local(async move {
            if let Err(e) = ipc::call::<()>("ui_ready", json!({"theme": value})).await {
                self.error.set(Some(e));
            }
        });
    }
}
