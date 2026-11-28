use super::*;
use crate::ipc::Source;
use nvsf_core::{Cue, MediaInfo, Rect};

fn media() -> MediaInfo {
    MediaInfo {
        width: 640,
        height: 360,
        duration_ms: Some(5000),
        codec: "h264".into(),
        time_base_num: 1,
        time_base_den: 1000,
        subtitle_streams: 0,
        rotation: 0,
    }
}

fn setup() -> Project {
    Project {
        schema: 1,
        source_hash: "video".into(),
        config_hash: String::new(),
        model_hash: String::new(),
        config: ExtractConfig {
            input: "video.mp4".into(),
            ..ExtractConfig::default()
        },
        media: media(),
        cues: Vec::new(),
        processed_ms: 0,
        active_appearances: Vec::new(),
        completed: false,
        warnings: Vec::new(),
    }
}

fn review() -> Ui {
    let ui = Ui::new("dark".into());
    let mut project = setup();
    project.completed = true;
    project.processed_ms = 5000;
    project.cues = vec![Cue::new(1, 0, 100, 900, "original".into()).unwrap()];
    let mut history = EditHistory::default();
    for text in ["corrected", "later"] {
        history
            .apply(&mut project.cues, |cues| {
                cues[0].text = text.into();
                Ok(())
            })
            .unwrap();
    }
    assert!(history.undo(&mut project.cues));
    ui.source.set(Some(Source {
        token: "video-grant".into(),
        name: "video.mp4".into(),
        fingerprint: "video".into(),
        media: media(),
    }));
    ui.config.set(project.config.clone());
    ui.project.set(Some(project));
    ui.selected.set(Some(1));
    ui.history.set_value(history);
    ui.revision.set(17);
    ui.dirty.set(true);
    ui
}

fn project_json(ui: Ui) -> serde_json::Value {
    serde_json::to_value(ui.project.get_untracked()).unwrap()
}

#[test]
fn every_persisted_extraction_option_changes_dirty_state_and_reverting_cleans_it() {
    let original = ExtractConfig::default();
    let baseline = SetupSnapshot::capture(&original);
    let changes: [(&str, fn(&mut ExtractConfig)); 13] = [
        ("language", |config| config.language = "fa".into()),
        ("device", |config| config.device = "cuda".into()),
        ("quality", |config| config.quality = "accurate".into()),
        ("regions", |config| config.regions.clear()),
        ("exclusions", |config| {
            config.exclusions.push(Rect::default())
        }),
        ("start", |config| config.start_ms = 100),
        ("end", |config| config.end_ms = Some(1000)),
        ("threads", |config| config.threads = 4),
        ("confidence", |config| config.min_confidence = 0.8),
        ("overwrite", |config| config.overwrite = true),
        ("Persian normalization", |config| {
            config.normalize_persian = true
        }),
        ("GPU required", |config| config.gpu_required = true),
        ("accelerator required", |config| {
            config.accelerator_required = true
        }),
    ];
    assert!(!baseline.is_dirty(&original));
    for (name, change) in changes {
        let mut current = original.clone();
        change(&mut current);
        assert!(baseline.is_dirty(&current), "Missed durable option: {name}");
        current.clone_from(&original);
        assert!(!baseline.is_dirty(&current), "Revert stayed dirty: {name}");
    }
}

#[test]
fn native_location_rebinding_does_not_change_semantic_setup_identity() {
    let original = ExtractConfig::default();
    let baseline = SetupSnapshot::capture(&original);
    let current = ExtractConfig {
        input: "regranted/video.mp4".into(),
        output: "new-grant/output.srt".into(),
        data_root: Some("relocated/data".into()),
        ..original
    };
    assert!(!baseline.is_dirty(&current));
}

#[test]
fn cancelled_and_failed_save_preserve_review_cues_selection_history_and_dirty_state() {
    for outcome in [Ok(None), Err("Atomic project write failed".into())] {
        for review_dirty in [false, true] {
            for new_draft in [false, true] {
                Owner::new().with(|| {
                    let ui = review();
                    ui.dirty.set(review_dirty);
                    ui.config.update(|config| config.gpu_required = true);
                    let candidate = if review_dirty {
                        ui.project.get_untracked().unwrap()
                    } else {
                        let mut candidate = setup();
                        candidate.config = ui.config.get_untracked();
                        candidate
                    };
                    let pending = PendingSave::new(ui, &candidate, true);
                    ui.draft_dirty.set(new_draft);
                    let before = project_json(ui);
                    let settings = ui.config.get_untracked();
                    assert!(!pending.apply(ui, &outcome));
                    assert_eq!(project_json(ui), before);
                    assert_eq!(ui.config.get_untracked(), settings);
                    assert_eq!(ui.selected.get_untracked(), Some(1));
                    assert_eq!(ui.revision.get_untracked(), 17);
                    assert_eq!(ui.dirty.get_untracked(), review_dirty);
                    assert_eq!(ui.draft_dirty.get_untracked(), new_draft);
                    assert!(ui.history.with_value(EditHistory::can_undo));
                    assert!(ui.history.with_value(EditHistory::can_redo));
                });
            }
        }
    }
}

#[test]
fn successful_setup_save_publishes_only_after_acceptance_and_preserves_later_options() {
    Owner::new().with(|| {
        let ui = review();
        ui.dirty.set(false);
        let candidate = setup();
        let saved = SetupSnapshot::capture(&candidate.config);
        let pending = PendingSave::new(ui, &candidate, true);
        assert_eq!(ui.project.get_untracked().unwrap().cues.len(), 1);
        ui.config.update(|config| config.gpu_required = true);
        assert!(pending.apply(ui, &Ok(Some("setup.nvsf".into()))));
        assert!(ui.project.get_untracked().unwrap().is_setup());
        assert_eq!(ui.selected.get_untracked(), None);
        assert!(!ui.history.with_value(EditHistory::can_undo));
        assert!(!ui.history.with_value(EditHistory::can_redo));
        assert!(!ui.dirty.get_untracked());
        assert_eq!(ui.revision.get_untracked(), 18);
        assert!(ui.config.with_untracked(|config| config.gpu_required));
        assert!(ui.config.with_untracked(|config| saved.is_dirty(config)));
    });
}

#[test]
fn successful_review_save_keeps_cues_selection_and_undo_redo_history() {
    Owner::new().with(|| {
        let ui = review();
        let candidate = ui.project.get_untracked().unwrap();
        let before = project_json(ui);
        let pending = PendingSave::new(ui, &candidate, true);
        assert!(pending.apply(ui, &Ok(Some("review.nvsf".into()))));
        assert_eq!(project_json(ui), before);
        assert_eq!(ui.selected.get_untracked(), Some(1));
        assert_eq!(ui.revision.get_untracked(), 17);
        assert!(!ui.dirty.get_untracked());
        assert!(ui.history.with_value(EditHistory::can_undo));
        assert!(ui.history.with_value(EditHistory::can_redo));
    });
}

#[test]
fn a_late_save_cannot_replace_a_changed_review_source_or_close_session() {
    let changes: [fn(Ui); 5] = [
        |ui| ui.revision.update(|revision| *revision += 1),
        |ui| ui.review_generation.update(|generation| *generation += 1),
        |ui| {
            ui.source
                .update(|source| source.as_mut().unwrap().token = "new-grant".into())
        },
        |ui| ui.closing.set(true),
        |ui| ui.draft_dirty.set(true),
    ];
    for change in changes {
        Owner::new().with(|| {
            let ui = review();
            let pending = PendingSave::new(ui, &setup(), true);
            change(ui);
            let before = project_json(ui);
            let revision = ui.revision.get_untracked();
            assert!(!pending.apply(ui, &Ok(Some("previous-setup.nvsf".into()))));
            assert_eq!(project_json(ui), before);
            assert_eq!(ui.revision.get_untracked(), revision);
            assert_eq!(ui.selected.get_untracked(), Some(1));
            assert!(ui.dirty.get_untracked());
            assert!(ui.history.with_value(EditHistory::can_undo));
            assert!(ui.history.with_value(EditHistory::can_redo));
        });
    }
}

#[test]
fn a_busy_close_refusal_does_not_strand_an_already_committed_setup_save() {
    for unlock_before_response in [false, true] {
        Owner::new().with(|| {
            let ui = review();
            ui.dirty.set(false);
            let candidate = setup();
            let pending = PendingSave::new(ui, &candidate, true);
            ui.busy.set(true);
            let committed = Ok(Some("setup.nvsf".into()));

            // The native write succeeded, but its IPC response has not reached the UI yet.
            let acknowledgement = ui.close_snapshot(true);
            assert_eq!(acknowledgement["busy"], true);
            assert_eq!(acknowledgement["revision"], 17);
            assert!(!ui.closing.get_untracked());
            assert_eq!(ui.project.get_untracked().unwrap().cues.len(), 1);
            if unlock_before_response {
                ui.close_snapshot(false);
            }
            assert!(pending.apply(ui, &committed));
            assert!(ui.project.get_untracked().unwrap().is_setup());
            assert_eq!(ui.selected.get_untracked(), None);
            assert_eq!(ui.revision.get_untracked(), 18);
            assert!(!ui.history.with_value(EditHistory::can_undo));
            ui.busy.set(false);
            ui.close_snapshot(false);
            assert!(!ui.closing.get_untracked());
            assert!(ui.can_save_project());
        });
    }
}

#[test]
fn an_idle_close_still_freezes_publication_until_native_unlock() {
    Owner::new().with(|| {
        let ui = review();
        let pending = PendingSave::new(ui, &setup(), true);
        let before = project_json(ui);
        let acknowledgement = ui.close_snapshot(true);
        assert_eq!(acknowledgement["busy"], false);
        assert_eq!(acknowledgement["dirty"], true);
        assert_eq!(acknowledgement["draft"], false);
        assert_eq!(acknowledgement["revision"], 17);
        assert!(ui.closing.get_untracked());
        assert!(!pending.apply(ui, &Ok(Some("earlier-setup.nvsf".into()))));
        assert_eq!(project_json(ui), before);
        assert!(ui.dirty.get_untracked());
        assert!(ui.history.with_value(EditHistory::can_undo));
        ui.close_snapshot(false);
        assert!(!ui.closing.get_untracked());
        assert!(ui.can_save_project());
    });
}

#[test]
fn save_controls_share_the_handler_admission_for_active_and_pending_work() {
    Owner::new().with(|| {
        let ui = Ui::new("dark".into());
        assert!(!ui.can_save_project());
        ui.project.set(Some(setup()));
        assert!(ui.can_save_project());
        for state in ["running", "paused", "cancelling"] {
            ui.status.update(|status| status.state = state.into());
            assert!(!ui.can_save_project(), "Save admitted active state {state}");
        }
        ui.status.update(|status| status.state = "idle".into());
        ui.busy.set(true);
        assert!(!ui.can_save_project());
        ui.busy.set(false);
        ui.preview_busy.set(true);
        assert!(!ui.can_save_project());
        ui.preview_busy.set(false);
        ui.review_loading.set(true);
        assert!(!ui.can_save_project());
        ui.review_loading.set(false);
        ui.closing.set(true);
        assert!(!ui.can_save_project());
        ui.closing.set(false);
        assert!(ui.can_save_project());
    });
}
