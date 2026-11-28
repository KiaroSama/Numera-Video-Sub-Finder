use crate::state::Ui;
use leptos::prelude::*;
use nvsf_core::{EditHistory, ExtractConfig, Project};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SetupSnapshot(ExtractConfig);

impl SetupSnapshot {
    pub(crate) fn capture(config: &ExtractConfig) -> Self {
        let mut settings = config.clone();
        // Native grants own locations; every durable extraction option remains significant.
        settings.input.clear();
        settings.output.clear();
        settings.data_root = None;
        Self(settings)
    }

    pub(crate) fn is_dirty(&self, config: &ExtractConfig) -> bool {
        *self != Self::capture(config)
    }
}

struct PendingSave {
    revision: u64,
    review_generation: u64,
    source_token: Option<String>,
    setup: Option<Project>,
}

impl PendingSave {
    fn new(ui: Ui, project: &Project, has_source_token: bool) -> Self {
        Self {
            revision: ui.revision.get_untracked(),
            review_generation: ui.review_generation.get_untracked(),
            source_token: ui
                .source
                .with_untracked(|source| source.as_ref().map(|source| source.token.clone())),
            setup: (has_source_token && project.is_setup()).then(|| project.clone()),
        }
    }

    fn current(&self, ui: Ui) -> bool {
        !ui.closing.get_untracked()
            && !ui.draft_dirty.get_untracked()
            && self.revision == ui.revision.get_untracked()
            && self.review_generation == ui.review_generation.get_untracked()
            && self.source_token
                == ui
                    .source
                    .with_untracked(|source| source.as_ref().map(|source| source.token.clone()))
    }

    fn apply(self, ui: Ui, result: &Result<Option<String>, String>) -> bool {
        if !matches!(result, Ok(Some(_))) || !self.current(ui) {
            return false;
        }
        if let Some(setup) = self.setup {
            #[cfg(target_arch = "wasm32")]
            ui.studio
                .extract_baseline
                .set(Some(SetupSnapshot::capture(&setup.config)));
            ui.project.set(Some(setup));
            ui.selected.set(None);
            ui.history.set_value(EditHistory::default());
            ui.revision.update(|revision| *revision += 1);
        }
        ui.dirty.set(false);
        true
    }
}

impl Ui {
    pub(crate) fn can_save_project(self) -> bool {
        !self.closing.get()
            && !self.busy.get()
            && !self.preview_busy.get()
            && !self.active()
            && !self.review_loading.get()
            && (self.project.with(Option::is_some) || self.source.with(Option::is_some))
    }
}

#[cfg(target_arch = "wasm32")]
mod browser {
    use super::*;
    use crate::ipc::{self, HistoryEntry};
    use serde_json::json;
    use wasm_bindgen_futures::spawn_local;

    impl Ui {
        pub(crate) fn capture_extract_baseline(self) {
            self.studio
                .extract_baseline
                .set(Some(self.config.with_untracked(SetupSnapshot::capture)));
        }

        pub(crate) fn extract_dirty(self) -> bool {
            self.source.with(Option::is_some)
                && self.studio.extract_baseline.with(|baseline| {
                    baseline.as_ref().is_some_and(|baseline| {
                        self.config.with(|config| baseline.is_dirty(config))
                    })
                })
        }

        pub(crate) fn workspace_project(self) -> Option<(Project, Option<String>)> {
            let source = self.source.get_untracked();
            if let Some(project) = self.project.get_untracked()
                && (source.is_none()
                    || self.dirty.get_untracked()
                    || (!untrack(|| self.extract_dirty())
                        && source
                            .as_ref()
                            .is_some_and(|source| source.fingerprint == project.source_hash)
                        && (project.completed || !project.cues.is_empty())))
            {
                let token = source
                    .filter(|source| source.fingerprint == project.source_hash)
                    .map(|source| source.token);
                return Some((project, token));
            }
            let source = source?;
            let config = ExtractConfig {
                input: source.name,
                ..self.config.get_untracked()
            };
            Some((
                Project {
                    schema: 1,
                    source_hash: source.fingerprint,
                    config_hash: String::new(),
                    model_hash: String::new(),
                    config,
                    media: source.media,
                    cues: Vec::new(),
                    processed_ms: 0,
                    active_appearances: Vec::new(),
                    completed: false,
                    warnings: Vec::new(),
                },
                Some(source.token),
            ))
        }

        pub(crate) fn can_save(self) -> bool {
            self.review_editable() && !self.preview_busy.get() && !self.draft_dirty.get()
        }

        pub(crate) fn save(self, as_new: bool) {
            self.save_workspace(as_new);
        }

        pub(crate) fn save_workspace(self, as_new: bool) {
            if self.can_save_project() {
                self.persist_project(None, as_new);
            }
        }

        pub fn persist(self, format: Option<String>, bom: bool, crlf: bool) {
            self.persist_project(format.map(|format| (format, bom, crlf)), false);
        }

        fn persist_project(self, export: Option<(String, bool, bool)>, as_new: bool) {
            let save = export.is_none();
            let Some((project, source_token)) = (if save {
                self.workspace_project()
            } else {
                if !self.review_editable() || self.preview_busy.get_untracked() {
                    return;
                }
                self.project.get_untracked().map(|project| (project, None))
            }) else {
                return;
            };
            if self.draft_dirty.get_untracked() {
                self.error.set(Some(
                    "Apply the current cue correction before saving or exporting".into(),
                ));
                return;
            }
            let pending = PendingSave::new(self, &project, source_token.is_some());
            self.busy.set(true);
            self.error.set(None);
            spawn_local(async move {
                let result = async {
                    if let Some((format, bom, crlf)) = export {
                        ipc::call::<()>("replace_project", json!({"project": project})).await?;
                        ipc::call::<Option<String>>(
                            "export_project",
                            json!({"format": format, "bom": bom, "crlf": crlf}),
                        )
                        .await
                    } else {
                        let command = if as_new {
                            "save_project_as"
                        } else {
                            "save_project"
                        };
                        ipc::call::<Option<String>>(
                            command,
                            json!({"project": project, "sourceToken": source_token}),
                        )
                        .await
                    }
                }
                .await;
                if self.busy.is_disposed() {
                    return;
                }
                let accepted = save && pending.apply(self, &result);
                match result {
                    Ok(Some(name)) => {
                        let review_dirty =
                            self.dirty.get_untracked() || self.draft_dirty.get_untracked();
                        let dirty = review_dirty || untrack(|| self.extract_dirty());
                        self.push_notice(if save && !accepted {
                            format!("Saved {name} from the earlier workspace snapshot. The current workspace was kept; review it before saving again.")
                        } else if save && dirty {
                            format!("Saved {name}. Other workspace changes are still unsaved.")
                        } else {
                            format!("Saved {name}")
                        });
                        // A completed native write clears its flags; restore any newer UI edits.
                        if save
                            && let Err(error) = ipc::call::<()>(
                                "set_unsaved",
                                json!({
                                    "dirty": dirty, "reviewDirty": review_dirty,
                                }),
                            )
                            .await
                        {
                            self.error.set(Some(error));
                        }
                        if save
                            && let Ok(entries) =
                                ipc::call::<Vec<HistoryEntry>>("list_history", json!({})).await
                        {
                            self.recent.set(entries);
                        }
                    }
                    Ok(None) => {}
                    Err(error) => self.error.set(Some(error)),
                }
                self.busy.set(false);
            });
        }
    }
}

#[cfg(test)]
#[path = "project_save_tests.rs"]
mod tests;
