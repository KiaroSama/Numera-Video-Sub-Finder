use crate::{
    files, language_manager, queue, recognition, runtime_manager,
    state::{Host, lock},
    updates, visual_retry,
};
use nvsf_core::{Cue, ExtractConfig, MediaInfo, Project, Rect};
use std::sync::atomic::Ordering;
use tauri::Manager;

fn workspace() -> (tempfile::TempDir, tauri::App<tauri::test::MockRuntime>) {
    let directory = tempfile::tempdir().unwrap();
    let paths = nvsf_app::Paths::new(Some(directory.path())).unwrap();
    let log = nvsf_app::logging::Log::new(&paths, "nvsf-desktop");
    let app = tauri::test::mock_builder()
        .manage(Host::new(paths, log))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    (directory, app)
}

fn project() -> Project {
    Project {
        schema: 1,
        source_hash: "a".repeat(64),
        config_hash: "b".repeat(64),
        model_hash: "c".repeat(64),
        config: ExtractConfig {
            input: "/media/input.mp4".into(),
            output: "/media/output.srt".into(),
            ..Default::default()
        },
        media: MediaInfo {
            width: 1920,
            height: 1080,
            duration_ms: Some(10_000),
            codec: "h264".into(),
            time_base_num: 1,
            time_base_den: 1000,
            subtitle_streams: 0,
            rotation: 0,
        },
        cues: vec![Cue::new(1, 0, 0, 1000, "Original".into()).unwrap()],
        processed_ms: 1500,
        active_appearances: vec![],
        completed: false,
        warnings: vec![],
    }
}

#[test]
fn cue_corrections_cannot_change_project_identity_or_extraction_state() {
    let (_directory, app) = workspace();
    let host = app.state::<Host>();
    let initial = project();
    assert!(files::current_project(host.clone()).unwrap().is_none());
    assert!(files::replace_project(host.clone(), initial.clone()).is_err());
    *lock(&host.inner().0.project).unwrap() = Some(initial.clone());
    let mut corrected = initial.clone();
    corrected.cues[0].text = "Corrected فارسی 日本".into();
    corrected.cues[0].edited = true;
    corrected.processed_ms = 9000;
    corrected.completed = true;
    files::replace_project(host.clone(), corrected).unwrap();
    let current = files::current_project(host.clone()).unwrap().unwrap();
    assert_eq!(current.cues[0].text, "Corrected فارسی 日本");
    assert!(current.cues[0].edited);
    assert_eq!(current.processed_ms, initial.processed_ms);
    assert!(!current.completed);
    let before = serde_json::to_vec(&current).unwrap();
    for field in ["source", "config", "model", "settings", "cue"] {
        let mut invalid = current.clone();
        match field {
            "source" => invalid.source_hash = "d".repeat(64),
            "config" => invalid.config_hash = "d".repeat(64),
            "model" => invalid.model_hash = "d".repeat(64),
            "settings" => invalid.config.language = "fa".into(),
            _ => invalid.cues[0].end_ms = 0,
        }
        assert!(files::replace_project(host.clone(), invalid).is_err());
        assert_eq!(
            serde_json::to_vec(&files::current_project(host.clone()).unwrap().unwrap()).unwrap(),
            before
        );
    }
}

#[test]
fn history_grants_expire_on_refresh_and_open_preserves_saved_corrections() {
    let (_directory, app) = workspace();
    let host = app.state::<Host>();
    let mut saved = project();
    saved.cues[0].text = "Saved فارسی 日本".into();
    nvsf_app::storage::save_project(&saved, &host.inner().0.paths.projects.join("review.json"))
        .unwrap();
    std::fs::write(
        host.inner().0.paths.projects.join("ignored.srt"),
        b"not a project",
    )
    .unwrap();
    std::fs::create_dir(host.inner().0.paths.projects.join("directory.json")).unwrap();
    let entries = tauri::async_runtime::block_on(files::list_history(host.clone())).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "review.json");
    let old = entries[0].token.clone();
    let entries = tauri::async_runtime::block_on(files::list_history(host.clone())).unwrap();
    assert_ne!(entries[0].token, old);
    assert!(tauri::async_runtime::block_on(files::open_history(host.clone(), old)).is_err());
    let opened =
        tauri::async_runtime::block_on(files::open_history(host.clone(), entries[0].token.clone()))
            .unwrap();
    assert_eq!(opened.cues, saved.cues);
    assert_eq!(
        files::current_project(host.clone()).unwrap().unwrap().cues,
        saved.cues
    );
    host.inner().0.source_busy.store(true, Ordering::Release);
    assert!(
        tauri::async_runtime::block_on(files::open_history(host.clone(), entries[0].token.clone()))
            .is_err()
    );
    host.inner().0.source_busy.store(false, Ordering::Release);
}

#[test]
fn queue_removal_persists_and_refuses_running_or_unknown_records() {
    let (directory, app) = workspace();
    let host = app.state::<Host>();
    let source = directory.path().join("source.mp4");
    let output = directory.path().join("result.srt");
    let config = ExtractConfig {
        input: source.to_str().unwrap().into(),
        output: output.to_str().unwrap().into(),
        ..Default::default()
    };
    {
        let mut store = lock(&host.inner().0.queue).unwrap();
        store.file.records.push(queue::Record {
            id: "saved".into(),
            source,
            output,
            source_hash: "a".repeat(64),
            config,
            checkpoint: None,
            state: "running".into(),
            error: None,
        });
        store.save(&host.inner().0.paths.root).unwrap();
    }
    assert!(
        tauri::async_runtime::block_on(queue::queue_remove(host.clone(), "saved".into())).is_err()
    );
    assert_eq!(
        tauri::async_runtime::block_on(queue::queue_list(host.clone()))
            .unwrap()
            .len(),
        1
    );
    queue::mark(host.inner(), "saved", "cancelled", None).unwrap();
    assert!(
        tauri::async_runtime::block_on(queue::queue_remove(host.clone(), "unknown".into()))
            .is_err()
    );
    let queue_path = host.inner().0.paths.root.join("desktop-queue.json");
    let original = std::fs::read(&queue_path).unwrap();
    let backup = directory.path().join("preserved-queue.json");
    std::fs::rename(&queue_path, &backup).unwrap();
    std::fs::create_dir(&queue_path).unwrap();
    assert!(
        tauri::async_runtime::block_on(queue::queue_remove(host.clone(), "saved".into())).is_err()
    );
    let retained = tauri::async_runtime::block_on(queue::queue_list(host.clone())).unwrap();
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].id, "saved");
    assert_eq!(retained[0].state, "cancelled");
    assert_eq!(std::fs::read(&backup).unwrap(), original);
    std::fs::remove_dir(&queue_path).unwrap();
    std::fs::rename(&backup, &queue_path).unwrap();
    assert!(
        tauri::async_runtime::block_on(queue::queue_remove(host.clone(), "saved".into()))
            .unwrap()
            .is_empty()
    );
    let persisted: serde_json::Value = serde_json::from_slice(
        &std::fs::read(host.inner().0.paths.root.join("desktop-queue.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(persisted["records"], serde_json::json!([]));
}

#[test]
fn restored_queue_start_checks_grants_checkpoint_and_identity_before_launch() {
    let (directory, app) = workspace();
    let host = app.state::<Host>();
    let source = directory.path().join("saved source.mp4");
    let output = directory.path().join("saved output.srt");
    std::fs::write(&source, b"saved source bytes").unwrap();
    let fingerprint = nvsf_app::storage::hash_file(&source).unwrap();
    let config = ExtractConfig {
        input: source.to_str().unwrap().into(),
        output: output.to_str().unwrap().into(),
        ..Default::default()
    };
    let record = queue::Record {
        id: "restored".into(),
        source: source.clone(),
        output: output.clone(),
        source_hash: fingerprint.clone(),
        config: config.clone(),
        checkpoint: None,
        state: "interrupted".into(),
        error: None,
    };
    let start_error = || {
        let result =
            tauri::async_runtime::block_on(queue::queue_start(host.clone(), "restored".into()));
        assert!(
            lock(&host.inner().0.job).unwrap().is_none(),
            "rejected admission must not launch a worker"
        );
        result.err().unwrap()
    };
    assert_eq!(start_error(), "Queue record not found");
    {
        let mut store = lock(&host.inner().0.queue).unwrap();
        store.file.records.push(record);
        store.save(&host.inner().0.paths.root).unwrap();
    }
    assert_eq!(
        start_error(),
        "Regrant the source and output with native pickers before starting this restored job"
    );
    lock(&host.inner().0.sources)
        .unwrap()
        .insert("source".into(), source.clone());
    lock(&host.inner().0.destinations)
        .unwrap()
        .insert("output".into(), output.clone());
    lock(&host.inner().0.queue).unwrap().grants.insert(
        "restored".into(),
        queue::Grant {
            source: crate::state::Source {
                token: "source".into(),
                name: "saved source.mp4".into(),
                fingerprint,
                media: project().media,
            },
            output: crate::state::Destination {
                token: "output".into(),
                name: "saved output.srt".into(),
            },
        },
    );
    host.inner().0.unsaved.store(true, Ordering::Release);
    assert_eq!(
        start_error(),
        "Save current corrections before starting/resuming a queue entry"
    );
    host.inner().0.unsaved.store(false, Ordering::Release);
    {
        let mut store = lock(&host.inner().0.queue).unwrap();
        store.file.records[0].state = "completed".into();
    }
    assert_eq!(
        start_error(),
        "Saved job is complete; open its project in History/Review instead"
    );
    let checkpoint = output.with_extension("nvsf.json");
    {
        let mut store = lock(&host.inner().0.queue).unwrap();
        store.file.records[0].state = "interrupted".into();
        store.file.records[0].checkpoint = Some(checkpoint.clone());
    }
    assert_eq!(
        start_error(),
        "Saved checkpoint is missing; restore it before resuming. Queue/source/output remain unchanged"
    );
    lock(&host.inner().0.queue).unwrap().file.records[0].checkpoint = None;
    std::fs::write(&source, b"changed source").unwrap();
    assert_eq!(
        start_error(),
        "Queued source content changed; existing state preserved"
    );
    std::fs::write(&source, b"saved source bytes").unwrap();
    lock(&host.inner().0.queue).unwrap().file.records[0]
        .config
        .data_root = Some("different root".into());
    assert_eq!(
        start_error(),
        "Application data root changed; restore the original root before resuming this queue record"
    );
    lock(&host.inner().0.queue).unwrap().file.records[0].config = config.clone();
    let mut saved = project();
    saved.config = config;
    saved.source_hash = "d".repeat(64);
    nvsf_app::storage::save_project(&saved, &checkpoint).unwrap();
    let checkpoint_before = std::fs::read(&checkpoint).unwrap();
    assert_eq!(
        start_error(),
        "Checkpoint no longer matches queued identity/configuration"
    );
    assert_eq!(std::fs::read(checkpoint).unwrap(), checkpoint_before);
    assert_eq!(std::fs::read(source).unwrap(), b"saved source bytes");
    assert!(!output.exists());
    assert!(lock(&host.inner().0.job).unwrap().is_none());
    let persisted: serde_json::Value = serde_json::from_slice(
        &std::fs::read(host.inner().0.paths.root.join("desktop-queue.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(persisted["records"][0]["state"], "interrupted");
    assert_eq!(
        persisted["records"][0]["source_hash"],
        nvsf_app::storage::hash_file(&directory.path().join("saved source.mp4")).unwrap()
    );
}

#[test]
fn review_commands_reject_invalid_identity_without_loading_native_resources() {
    let (directory, app) = workspace();
    let host = app.state::<Host>();
    let source = directory.path().join("source.mp4");
    std::fs::write(&source, b"original source").unwrap();
    lock(&host.inner().0.sources)
        .unwrap()
        .insert("granted".into(), source.clone());
    let recognize = |cue_id, at_ms| {
        tauri::async_runtime::block_on(recognition::recognize_cue(
            host.clone(),
            "granted".into(),
            cue_id,
            "en".into(),
            Rect::default(),
            at_ms,
        ))
        .err()
        .unwrap()
    };
    assert_eq!(recognize(1, 0), "No review project is open");
    let saved = project();
    let before = serde_json::to_vec(&saved).unwrap();
    *lock(&host.inner().0.project).unwrap() = Some(saved);
    assert_eq!(recognize(0, 0), "Invalid cue or preview timestamp");
    assert_eq!(
        recognize(1, 360_000_000_000),
        "Invalid cue or preview timestamp"
    );
    assert_eq!(
        recognize(1, 10_000),
        "Cue preview time is outside the source duration"
    );
    assert_eq!(
        recognize(1, 0),
        "Linked source does not match the review project's fingerprint"
    );
    tauri::async_runtime::block_on(async {
        assert_eq!(
            visual_retry::propose_visual_retry(
                host.clone(),
                "granted".into(),
                0,
                0,
                Rect::default(),
                true
            )
            .await
            .err()
            .unwrap(),
            "Invalid cue/source preview timestamp"
        );
        assert_eq!(
            visual_retry::propose_visual_retry(
                host.clone(),
                "granted".into(),
                1,
                10_000,
                Rect::default(),
                true
            )
            .await
            .err()
            .unwrap(),
            "Invalid cue/source preview timestamp"
        );
        assert_eq!(
            visual_retry::propose_visual_retry(
                host.clone(),
                "granted".into(),
                1,
                0,
                Rect::default(),
                true
            )
            .await
            .err()
            .unwrap(),
            "Linked video does not match the project's source fingerprint"
        );
    });
    assert!(!host.inner().0.model_busy.load(Ordering::Acquire));
    assert_eq!(
        serde_json::to_vec(&files::current_project(host.clone()).unwrap().unwrap()).unwrap(),
        before
    );
    assert_eq!(std::fs::read(source).unwrap(), b"original source");
    assert_eq!(
        std::fs::read_dir(&host.inner().0.paths.cache)
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn denied_resource_commands_do_not_start_network_or_native_inference() {
    let (_directory, app) = workspace();
    let host = app.state::<Host>();
    tauri::async_runtime::block_on(async {
        assert!(
            runtime_manager::repair_runtimes(host.clone(), false)
                .await
                .unwrap_err()
                .contains("approval")
        );
        assert!(
            language_manager::install_language(host.clone(), "fra".into(), false)
                .await
                .is_err()
        );
        assert!(
            language_manager::install_language(host.clone(), "../eng".into(), true)
                .await
                .is_err()
        );
        assert!(
            visual_retry::install_visual_retry(host.clone(), false)
                .await
                .is_err()
        );
        assert_eq!(
            recognition::suggest_regions(host.clone(), "missing".into(), "invalid-language".into())
                .await
                .err()
                .unwrap(),
            "Choose a language from the official catalog"
        );
        assert_eq!(
            recognition::recognize_cue(
                host.clone(),
                "missing".into(),
                1,
                "en".into(),
                Rect {
                    x: -1.0,
                    ..Rect::default()
                },
                0
            )
            .await
            .err()
            .unwrap(),
            "Invalid input: region must be finite, nonempty and inside the image"
        );
    });
    assert!(!host.inner().0.model_busy.load(Ordering::Acquire));
    assert_eq!(
        std::fs::read_dir(&host.inner().0.paths.cache)
            .unwrap()
            .count(),
        0
    );
    let status = updates::set_update_checks(host.clone(), false).unwrap();
    assert!(!status.enabled);
    assert!(
        !nvsf_app::updates::preferences(&host.inner().0.paths)
            .unwrap()
            .enabled
    );
    updates::cancel_update(host.clone());
    crate::set_unsaved(host.clone(), true).unwrap();
    assert!(host.inner().0.unsaved.load(Ordering::Acquire));
    crate::set_unsaved(host.clone(), false).unwrap();
    assert!(!host.inner().0.unsaved.load(Ordering::Acquire));
}
