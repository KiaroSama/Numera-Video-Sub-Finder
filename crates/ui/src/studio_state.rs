use crate::{
    ipc,
    roi_editor::Drag,
    state::Ui,
    undo::{ExtractEdit, History, RegionId},
};
use leptos::prelude::*;
use nvsf_core::{ExtractConfig, Project, Rect};
use serde_json::json;
use wasm_bindgen_futures::spawn_local;

#[derive(Clone, Copy)]
pub(crate) struct Studio {
    pub(crate) zoom: RwSignal<u32>,
    pub(crate) playing: RwSignal<bool>,
    pub(crate) initial: RwSignal<bool>,
    pub(crate) roi: RwSignal<Option<RegionId>>,
    pub(crate) roi_drag: RwSignal<Option<Drag>>,
    pub(crate) shortcuts: RwSignal<bool>,
    pub(crate) close_open: RwSignal<bool>,
    history: RwSignal<History<ExtractEdit>>,
    source_generation: RwSignal<u64>,
}

impl Studio {
    pub(crate) fn new() -> Self {
        Self {
            zoom: RwSignal::new(100),
            playing: RwSignal::new(false),
            initial: RwSignal::new(false),
            roi: RwSignal::new(None),
            roi_drag: RwSignal::new(None),
            shortcuts: RwSignal::new(false),
            close_open: RwSignal::new(false),
            history: RwSignal::new(History::default()),
            source_generation: RwSignal::new(0),
        }
    }

    pub(crate) fn reset_extract(self) {
        self.history.set(History::default());
        self.roi.set(None);
        self.roi_drag.set(None);
    }

    pub(crate) fn reset_source(self) {
        self.source_generation.update(|generation| *generation += 1);
        self.initial.set(false);
        self.playing.set(false);
        self.zoom.set(100);
        self.roi_drag.set(None);
    }
}

impl Ui {
    pub(crate) fn open_project(self) {
        self.request::<Option<Project>>("pick_project", json!({}), move |project| {
            if let Some(project) = project {
                self.load(project);
            }
        });
    }

    pub(crate) fn open_video(self) {
        self.pick_video(false);
    }

    pub(crate) fn admit_video(self, source: ipc::Source, initial: bool) {
        self.studio.reset_source();
        self.preview_generation
            .update(|generation| *generation += 1);
        let old = self.preview_url.get_untracked();
        self.preview_url.set(String::new());
        if !old.is_empty() {
            let _ = web_sys::Url::revoke_object_url(&old);
        }
        self.source.set(Some(source));
        self.time.set(0);
        if !initial || ipc::e2e_mode() {
            self.seek(0);
            return;
        }
        self.studio.initial.set(true);
        if !self.preview_busy.get_untracked() {
            self.request_initial_frame();
        }
    }

    pub(crate) fn request_initial_frame(self) {
        let Some(token) = self
            .source
            .with_untracked(|s| s.as_ref().map(|s| s.token.clone()))
        else {
            return;
        };
        let generation = self.studio.source_generation.get_untracked();
        self.preview_busy.set(true);
        spawn_local(async move {
            let time = ipc::call::<u64>("preview_initial_frame", json!({"token": token}))
                .await
                .unwrap_or(0);
            if self.preview_busy.is_disposed() {
                return;
            }
            self.preview_busy.set(false);
            if self.studio.source_generation.get_untracked() == generation {
                self.studio.initial.set(false);
                self.seek(time);
            } else if self.studio.initial.get_untracked() {
                self.request_initial_frame();
            } else {
                self.seek(self.time.get_untracked());
            }
        });
    }

    pub(crate) fn review_source_linked(self) -> bool {
        self.source.with(|source| {
            source.as_ref().is_some_and(|source| {
                self.project.with(|project| {
                    project
                        .as_ref()
                        .is_some_and(|project| project.source_hash == source.fingerprint)
                })
            })
        })
    }

    pub(crate) fn can_extract(self) -> bool {
        !self.busy.get()
            && !self.closing.get()
            && !self.active()
            && !self.review_loading.get()
            && self.source.with(Option::is_some)
            && self.output.with(Option::is_some)
    }

    pub(crate) fn start_extraction(self) {
        if let Some(queued) = self.current_queue() {
            self.begin(queued);
        }
    }

    pub(crate) fn edit_extract(self, edit: impl FnOnce(&mut ExtractConfig)) {
        if self.closing.get_untracked() {
            return;
        }
        let previous = self.config.with_untracked(ExtractEdit::snapshot);
        self.config.update(edit);
        if self.config.with_untracked(ExtractEdit::snapshot) != previous {
            self.studio.history.update(|history| history.push(previous));
        }
    }

    pub(crate) fn add_region(self, excluded: bool, rect: Rect) {
        let mut id = RegionId { excluded, index: 0 };
        id.index = self.config.with_untracked(|config| id.list(config).len());
        if id.index >= if excluded { 32 } else { 8 } {
            self.error.set(Some(
                "Region limit reached (8 subtitle regions or 32 exclusions)".into(),
            ));
            return;
        }
        if !self.closing.get_untracked() {
            self.edit_extract(|config| id.list_mut(config).push(rect));
            self.studio.roi.set(Some(id));
        }
    }

    pub(crate) fn update_region(self, id: RegionId, rect: Rect) {
        self.edit_extract(|config| {
            if let Some(old) = id.list_mut(config).get_mut(id.index) {
                *old = rect;
            }
        });
    }

    pub(crate) fn remove_region(self, id: RegionId) -> bool {
        if self.closing.get_untracked()
            || !self.config.with_untracked(|config| {
                let regions = id.list(config);
                id.index < regions.len() && (id.excluded || regions.len() > 1)
            })
        {
            return false;
        }
        self.studio.roi_drag.set(None);
        self.edit_extract(|config| {
            id.list_mut(config).remove(id.index);
        });
        self.studio.roi.set(None);
        true
    }

    pub(crate) fn remove_selected_region(self) -> bool {
        self.studio
            .roi
            .get_untracked()
            .is_some_and(|id| self.remove_region(id))
    }

    pub(crate) fn can_undo(self) -> bool {
        self.can_travel(false)
    }

    pub(crate) fn can_redo(self) -> bool {
        self.can_travel(true)
    }

    fn can_travel(self, redo: bool) -> bool {
        if self.busy.get() || self.closing.get() {
            return false;
        }
        match self.route.read().as_str() {
            "Extract" => self
                .studio
                .history
                .with(|h| if redo { h.can_redo() } else { h.can_undo() }),
            "Review" if self.review_editable() && !self.draft_dirty.get() => {
                self.revision.get();
                self.history
                    .with_value(|h| if redo { h.can_redo() } else { h.can_undo() })
            }
            _ => false,
        }
    }

    pub(crate) fn undo(self) {
        self.travel_workspace(false);
    }

    pub(crate) fn redo(self) {
        self.travel_workspace(true);
    }

    fn travel_workspace(self, redo: bool) {
        match self.route.get_untracked().as_str() {
            "Review" => self.travel(redo),
            "Extract" if self.can_travel(redo) => {
                let current = self.config.with_untracked(ExtractEdit::snapshot);
                let snapshot = self
                    .studio
                    .history
                    .try_update(|history| {
                        if redo {
                            history.redo(current)
                        } else {
                            history.undo(current)
                        }
                    })
                    .flatten();
                if let Some(snapshot) = snapshot {
                    self.studio.roi_drag.set(None);
                    self.studio.roi.set(None);
                    self.config.update(|config| snapshot.restore(config));
                }
            }
            _ => {}
        }
    }
}
