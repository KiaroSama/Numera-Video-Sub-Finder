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

#[cfg(target_arch = "wasm32")]
#[derive(Clone)]
pub struct NotificationItem {
    pub id: u64,
    pub time: String,
    pub message: String,
    pub read: bool,
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
    pub review_loading: RwSignal<bool>,
    pub review_generation: RwSignal<u64>,
    pub closing: RwSignal<bool>,
    pub pending_project: RwSignal<Option<Project>>,
    pub error: RwSignal<Option<String>>,
    pub notice: RwSignal<String>,
    #[cfg(target_arch = "wasm32")]
    pub notifications: RwSignal<Vec<NotificationItem>>,
    #[cfg(target_arch = "wasm32")]
    pub notifications_open: RwSignal<bool>,
    #[cfg(target_arch = "wasm32")]
    pub toast_visible: RwSignal<bool>,
    #[cfg(target_arch = "wasm32")]
    notification_id: RwSignal<u64>,
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
    pub dragging: RwSignal<bool>,
    #[cfg(target_arch = "wasm32")]
    pub(crate) studio: crate::studio_state::Studio,
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
            review_loading: RwSignal::new(false),
            review_generation: RwSignal::new(0),
            closing: RwSignal::new(false),
            pending_project: RwSignal::new(None),
            error: RwSignal::new(None),
            notice: RwSignal::new(String::new()),
            #[cfg(target_arch = "wasm32")]
            notifications: RwSignal::new(Vec::new()),
            #[cfg(target_arch = "wasm32")]
            notifications_open: RwSignal::new(false),
            #[cfg(target_arch = "wasm32")]
            toast_visible: RwSignal::new(false),
            #[cfg(target_arch = "wasm32")]
            notification_id: RwSignal::new(0),
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
            dragging: RwSignal::new(false),
            #[cfg(target_arch = "wasm32")]
            studio: crate::studio_state::Studio::new(),
        }
    }
    pub fn push_notice(self, message: impl Into<String>) {
        let message = message.into();
        #[cfg(target_arch = "wasm32")]
        self.record_notice(&message);
        self.notice.set(message);
    }
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn record_notice(self, message: &str) {
        if message.is_empty() {
            return;
        }
        let id = self.notification_id.get_untracked().saturating_add(1);
        self.notification_id.set(id);
        let now = js_sys::Date::new_0();
        let time = format!(
            "{:02}:{:02}:{:02}",
            now.get_hours(),
            now.get_minutes(),
            now.get_seconds()
        );
        self.notifications.update(|items| {
            items.insert(
                0,
                NotificationItem {
                    id,
                    time,
                    message: message.into(),
                    read: false,
                },
            );
            items.truncate(40);
        });
        self.toast_visible.set(true);
    }
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn clear_notifications(self) {
        self.notice.set(String::new());
        self.toast_visible.set(false);
        self.notifications.set(Vec::new());
        crate::menubar::focus("#notifications-panel");
    }
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn close_notifications(self, restore_focus: bool) {
        self.notifications_open.set(false);
        if restore_focus {
            crate::menubar::focus("#header-notifications");
        }
    }
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn toggle_notifications(self) {
        if self.notifications_open.get_untracked() {
            self.close_notifications(true);
        } else {
            self.notifications_open.set(true);
        }
    }
    pub fn active(self) -> bool {
        matches!(
            self.status.read().state.as_str(),
            "running" | "paused" | "cancelling"
        )
    }
    pub fn review_editable(self) -> bool {
        helpers::review_mutation_allowed(
            self.project.with(Option::is_some),
            &self.status.read().state,
            self.busy.get() || self.review_loading.get(),
            self.closing.get(),
        )
    }
    pub fn begin_draft(self) -> bool {
        if !self.review_editable() {
            return false;
        }
        self.draft_dirty.set(true);
        true
    }
    pub(crate) fn close_snapshot(self, requested: bool) -> Value {
        let busy = self.busy.get_untracked();
        // Native close rejects busy acknowledgements; keep an in-flight save publishable.
        if !requested || !busy {
            self.closing.set(requested);
            #[cfg(target_arch = "wasm32")]
            if requested {
                self.studio.playing.set(false);
                self.studio.roi_drag.set(None);
            } else {
                self.studio.close_open.set(false);
            }
        }
        let dirty = self.dirty.get_untracked();
        #[cfg(target_arch = "wasm32")]
        let dirty = dirty || untrack(|| self.extract_dirty());
        json!({
            "dirty": dirty,
            "draft": self.draft_dirty.get_untracked(),
            "revision": self.revision.get_untracked(),
            "busy": busy,
        })
    }
    pub(crate) fn request_allowed(self, command: &str) -> bool {
        if self.closing.get_untracked() || self.busy.get_untracked() {
            return false;
        }
        let replaces_review = matches!(
            command,
            "pick_project" | "open_history" | "start_job" | "queue_start" | "pick_checkpoint"
        );
        if replaces_review && (self.active() || self.review_loading.get_untracked()) {
            self.error.set(Some(
                "Wait for the current extraction and its review result before opening another project"
                    .into(),
            ));
            return false;
        }
        #[cfg(target_arch = "wasm32")]
        if matches!(command, "pick_source" | "pick_project" | "open_history")
            && untrack(|| self.extract_dirty())
        {
            self.error.set(Some(
                "Save the current video setup before opening another file, or undo its changes."
                    .into(),
            ));
            return false;
        }
        if replaces_review && (self.dirty.get_untracked() || self.draft_dirty.get_untracked()) {
            self.error.set(Some(
                "Save your current review project before replacing it or starting another job"
                    .into(),
            ));
            return false;
        }
        true
    }
    pub fn request<T: DeserializeOwned + 'static>(
        self,
        command: &'static str,
        args: Value,
        success: impl FnOnce(T) + 'static,
    ) {
        if !self.request_allowed(command) {
            return;
        }
        self.busy.set(true);
        self.error.set(None);
        spawn_local(async move {
            let result = ipc::call::<T>(command, args).await;
            if self.busy.is_disposed() {
                return;
            }
            self.busy.set(false);
            match result {
                Ok(value) => success(value),
                Err(e) => self.error.set(Some(e)),
            }
        });
    }
    pub fn load(self, project: Project) {
        if self.closing.get_untracked() {
            self.pending_project.set(Some(project));
            return;
        }
        let linked = self.source.with_untracked(|s| {
            s.as_ref()
                .is_some_and(|s| s.fingerprint == project.source_hash)
        });
        if !linked {
            self.replace_source(None);
        }
        #[cfg(target_arch = "wasm32")]
        self.studio.reset_extract();
        self.config.set(project.config.clone());
        self.selected.set(project.cues.first().map(|c| c.id));
        let route = if project.cues.is_empty() && !project.completed {
            "Extract"
        } else {
            "Review"
        };
        self.project.set(Some(project));
        self.history.set_value(EditHistory::default());
        self.dirty.set(false);
        self.draft_dirty.set(false);
        self.revision.update(|v| *v += 1);
        self.route.set(route.into());
        #[cfg(target_arch = "wasm32")]
        {
            self.capture_extract_baseline();
            if !linked {
                self.bind_project_source();
            } else if self.preview_url.get_untracked().is_empty()
                && !self.studio.initial.get_untracked()
            {
                self.seek_project_frame();
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    fn bind_project_source(self) {
        let revision = self.revision.get_untracked();
        let generation = self.preview_generation.get_untracked();
        self.studio.loading_video.set(true);
        self.busy.set(true);
        spawn_local(async move {
            let result = ipc::call::<Option<Source>>("bind_project_source", json!({})).await;
            if self.revision.is_disposed() {
                return;
            }
            self.busy.set(false);
            if self.revision.get_untracked() != revision
                || self.preview_generation.get_untracked() != generation
                || self.closing.get_untracked()
            {
                self.studio.loading_video.set(false);
                return;
            }
            match result {
                Ok(Some(source))
                    if self.project.with_untracked(|p| {
                        p.as_ref()
                            .is_some_and(|p| p.source_hash == source.fingerprint)
                    }) =>
                {
                    self.replace_source(Some(source));
                    self.seek_project_frame();
                }
                Ok(_) => self.studio.loading_video.set(false),
                Err(error) => {
                    self.studio.loading_video.set(false);
                    self.error.set(Some(error));
                }
            }
        });
    }
    #[cfg(target_arch = "wasm32")]
    fn seek_project_frame(self) {
        self.seek(self.selected_cue().map_or_else(
            || self.config.with_untracked(|config| config.start_ms),
            |cue| cue.start_ms,
        ));
    }
    #[cfg(target_arch = "wasm32")]
    fn admit_source(self, source: Source) {
        self.studio.reset_extract();
        self.capture_extract_baseline();
        self.studio.loading_video.set(true);
        self.admit_video(source, true);
    }
    #[cfg(target_arch = "wasm32")]
    pub fn pick_source(self) {
        self.pick_video(self.route.get_untracked() == "Review");
    }
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn pick_video(self, link_to_review: bool) {
        let expected_hash = link_to_review
            .then(|| {
                self.project
                    .with_untracked(|p| p.as_ref().map(|p| p.source_hash.clone()))
            })
            .flatten();
        self.request_video::<Option<Source>>(
            "pick_source",
            json!({"expectedHash": expected_hash}),
            move |source| {
                let Some(source) = source else {
                    return false;
                };
                if !link_to_review {
                    self.route.set("Extract".into());
                }
                self.admit_source(source);
                true
            },
        );
    }
    #[cfg(target_arch = "wasm32")]
    fn request_video<T: DeserializeOwned + 'static>(
        self,
        command: &'static str,
        args: Value,
        accept: impl FnOnce(T) -> bool + 'static,
    ) {
        if !self.request_allowed(command) {
            return;
        }
        self.studio.loading_video.set(true);
        self.busy.set(true);
        self.error.set(None);
        spawn_local(async move {
            let result = ipc::call::<T>(command, args).await;
            if self.busy.is_disposed() {
                return;
            }
            self.busy.set(false);
            let admitted = match result {
                Ok(response) => accept(response),
                Err(error) => {
                    self.error.set(Some(error));
                    false
                }
            };
            if !admitted {
                self.studio.loading_video.set(false);
            }
        });
    }
    #[cfg(target_arch = "wasm32")]
    pub fn accept_drop(self, token: String) {
        if token.is_empty() {
            return;
        }
        let config = self.config.get_untracked();
        self.request_video::<crate::ipc::DropOutcome>(
            "accept_dropped_paths",
            json!({"token": token, "config": config}),
            move |outcome| {
                if outcome.kind == "single"
                    && let Some(source) = outcome.source
                {
                    if untrack(|| self.extract_dirty()) {
                        self.error.set(Some("Save the current video setup before opening a dropped video, or undo its changes.".into()));
                        return false;
                    }
                    let name = source.name.clone();
                    self.route.set("Extract".into());
                    self.admit_source(source);
                    let suffix = if outcome.truncated {
                        " Some dropped items exceeded the intake limit; drop smaller batches."
                    } else {
                        ""
                    };
                    let mut note = format!("Loaded dropped video: {name}{suffix}");
                    if let Some(error) = outcome.errors.first() {
                        note.push_str(&format!(" {} drop issue(s): {error}", outcome.errors.len()));
                    }
                    self.push_notice(note);
                    return true;
                }
                self.queue.set(outcome.queued);
                self.route.set("Queue".into());
                let mut note = format!(
                    "Added {} dropped video(s) to the queue.",
                    outcome.added_count
                );
                if outcome.skipped_unsupported > 0 {
                    note.push_str(&format!(
                        " Skipped {} unsupported item(s).",
                        outcome.skipped_unsupported
                    ));
                }
                if outcome.truncated {
                    note.push_str(" Reached the drop intake or 100-video queue limit.");
                }
                if let Some(error) = outcome.errors.first() {
                    note.push_str(&format!(" {} drop issue(s): {error}", outcome.errors.len()));
                }
                self.push_notice(note);
                false
            },
        );
    }
    pub fn selected_cue(self) -> Option<Cue> {
        let id = self.selected.get()?;
        self.project
            .with(|p| p.as_ref()?.cues.iter().find(|c| c.id == id).cloned())
    }
    pub fn edit(self, change: impl FnOnce(&mut Vec<Cue>) -> nvsf_core::Result<()>) {
        if !self.review_editable() {
            return;
        }
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
        if !self.review_editable() {
            return;
        }
        if self.draft_dirty.get_untracked() {
            self.error.set(Some(
                "Apply or discard the current cue draft before undo/redo".into(),
            ));
            return;
        }
        if !self.history.with_value(|history| {
            if redo {
                history.can_redo()
            } else {
                history.can_undo()
            }
        }) {
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
        if !self.request_allowed("start_job") {
            return;
        }
        self.request::<String>("start_job", json!({"sourceToken": queued.source.token, "outputToken": queued.output.token, "config": queued.config}), move |id| self.watch_job(id));
    }
    pub fn watch_job(self, id: String) {
        let generation = self.begin_review_watch(id);
        spawn_local(async move {
            let mut io = crate::job_watch::NativeJobWatch;
            self.watch_job_with(generation, &mut io).await;
        });
    }
    pub fn add_queue(self) {
        if let Some(q) = self.current_queue() {
            self.request::<Vec<QueueEntry>>("queue_add",json!({"sourceToken":q.source.token,"outputToken":q.output.token,"config":q.config}),move |entries|{
                self.queue.set(entries);self.push_notice("Saved to the durable queue. Restored entries require source/output regrant; nothing starts automatically.");
            });
        }
    }
    pub fn start_saved(self, id: String) {
        self.request::<Started>("queue_start", json!({"id":id}), move |started| {
            let id = self.accept_started_source(started);
            self.watch_job(id);
        });
    }
    pub fn resume_picker(self) {
        self.request::<Option<Vec<QueueEntry>>>("pick_checkpoint",json!({}),move |entries|if let Some(entries)=entries{
            self.queue.set(entries);self.route.set("Queue".into());self.push_notice("Checkpoint registered with fresh native grants. Review its configuration warning, then resume the queue entry.");
        });
    }
    pub fn current_queue(self) -> Option<Queued> {
        Some(Queued {
            source: self.source.get_untracked()?,
            output: self.output.get_untracked()?,
            config: self.config.get_untracked(),
        })
    }
    pub fn toggle_theme(self) {
        let next = helpers::opposite_theme(&self.theme.get_untracked()).to_string();
        self.set_theme(next);
    }
    pub fn set_theme(self, value: String) {
        let value = helpers::theme_value(&value).to_string();
        #[cfg(target_arch = "wasm32")]
        let previous = self.theme.get_untracked();
        if let Some(window) = web_sys::window() {
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
        #[cfg(target_arch = "wasm32")]
        {
            let window_theme = value.clone();
            spawn_local(async move {
                if let Err(error) =
                    ipc::call::<()>("set_window_theme", json!({"theme": window_theme})).await
                    && !self.error.is_disposed()
                {
                    self.error.set(Some(error));
                }
            });
            crate::glimm::sweep_theme(&previous, &value);
        }
    }
}
