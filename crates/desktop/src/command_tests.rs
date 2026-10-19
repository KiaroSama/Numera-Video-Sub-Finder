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
