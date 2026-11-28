use super::*;
use crate::state::{Job, JobStatus};
use nvsf_core::{Cue, ExtractConfig, MediaInfo};
use std::{
    cell::RefCell,
    sync::{Arc, Mutex, mpsc},
};

struct Workspace {
    directory: tempfile::TempDir,
    host: Host,
    files: ProjectFiles,
    source: PathBuf,
    review_path: PathBuf,
    destination: PathBuf,
    setup: Project,
}

impl Workspace {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let paths = nvsf_app::Paths::new(Some(directory.path())).unwrap();
        let log = nvsf_app::logging::Log::new(&paths, "nvsf-save-contract");
        let host = Host::new(paths, log);
        let files = ProjectFiles::default();
        let source = directory.path().join("source.mp4");
        std::fs::write(&source, b"original video bytes").unwrap();
        let source = source.canonicalize().unwrap();
        lock(&host.0.sources)
            .unwrap()
            .insert("source-token".into(), source.clone());
        let review = Project {
            schema: 1,
            source_hash: nvsf_app::storage::hash_file(&source).unwrap(),
            config_hash: "b".repeat(64),
            model_hash: "c".repeat(64),
            config: ExtractConfig {
                input: path_text(&source).unwrap(),
                output: path_text(&directory.path().join("subtitles.srt")).unwrap(),
                data_root: Some(path_text(directory.path()).unwrap()),
                ..Default::default()
            },
            media: MediaInfo {
                width: 640,
                height: 360,
                duration_ms: Some(10_000),
                codec: "h264".into(),
                time_base_num: 1,
                time_base_den: 1000,
                subtitle_streams: 0,
                rotation: 0,
            },
            cues: vec![Cue::new(1, 0, 0, 1000, "Saved فارسی 日本".into()).unwrap()],
            processed_ms: 10_000,
            active_appearances: Vec::new(),
            completed: true,
            warnings: vec!["retained extraction warning".into()],
        };
        let review_path = directory.path().join("completed.nvsf");
        nvsf_app::storage::save_project(&review, &review_path).unwrap();
        *lock(&host.0.project).unwrap() = Some(review.clone());
        lock(&files.0).unwrap().active = Some((None, review_path.clone()));
        host.0.unsaved.store(true, Ordering::Release);
        host.0.extract_only_dirty.store(true, Ordering::Release);
        let mut setup = review;
        setup.config.input = "source.mp4".into();
        setup.config.output.clear();
        setup.config.data_root = None;
        setup.config.language = "fa".into();
        setup.config.overwrite = true;
        setup.config.normalize_persian = true;
        setup.config.gpu_required = true;
        setup.config.min_confidence = 0.8;
        setup.config.threads = 3;
        setup.config_hash.clear();
        setup.model_hash.clear();
        setup.cues.clear();
        setup.processed_ms = 0;
        setup.completed = false;
        let destination = directory.path().join("new setup.nvsf");
        std::fs::write(&destination, b"existing destination").unwrap();
        Self {
            directory,
            host,
            files,
            source,
            review_path,
            destination,
            setup,
        }
    }

    fn request(&self) -> SaveRequest {
        SaveRequest {
            project: Some(self.setup.clone()),
            source_token: Some("source-token".into()),
            as_new: false,
        }
    }

    fn state(&self) -> Vec<u8> {
        let paths = lock(&self.files.0).unwrap();
        serde_json::to_vec(&serde_json::json!({
            "project": lock(&self.host.0.project).unwrap().as_ref(),
            "active": paths.active,
            "generation": paths.generation,
            "recent": paths.recent,
            "grants": paths.recent_grants,
            "job": job_id(&self.host).unwrap(),
            "dirty": self.host.0.unsaved.load(Ordering::Acquire),
            "extract_only_dirty": self.host.0.extract_only_dirty.load(Ordering::Acquire),
        }))
        .unwrap()
    }

    fn unchanged(&self, state: &[u8], review: &[u8]) {
        assert_eq!(self.state(), state);
        assert_eq!(std::fs::read(&self.review_path).unwrap(), review);
        assert_eq!(
            std::fs::read(&self.destination).unwrap(),
            b"existing destination"
        );
    }
}

#[test]
fn cancelled_setup_save_preserves_active_review_paths_and_dirty_state() {
    for cancel_picker in [true, false] {
        let workspace = Workspace::new();
        let state = workspace.state();
        let review = std::fs::read(&workspace.review_path).unwrap();
        let result = save(
            &workspace.host,
            &workspace.files,
            workspace.request(),
            None,
            |_| Ok((!cancel_picker).then(|| workspace.destination.clone())),
            || Ok(false),
            |_, _| panic!("Cancellation must not publish"),
        );
        assert_eq!(result.unwrap(), None);
        workspace.unchanged(&state, &review);
    }
}

#[test]
fn failed_picker_confirmation_or_write_preserves_the_existing_workspace() {
    for failure in ["picker", "confirmation", "write"] {
        let workspace = Workspace::new();
        let state = workspace.state();
        let review = std::fs::read(&workspace.review_path).unwrap();
        let result = save(
            &workspace.host,
            &workspace.files,
            workspace.request(),
            None,
            |_| {
                if failure == "picker" {
                    Err("controlled picker failure".into())
                } else {
                    Ok(Some(workspace.destination.clone()))
                }
            },
            || {
                if failure == "confirmation" {
                    Err("controlled confirmation failure".into())
                } else {
                    Ok(true)
                }
            },
            |_, _| Err("controlled write failure before publication".into()),
        );
        assert!(result.unwrap_err().contains("controlled"));
        workspace.unchanged(&state, &review);
    }
}

#[test]
fn first_setup_save_keeps_an_empty_workspace_on_cancel_then_commits() {
    let workspace = Workspace::new();
    *lock(&workspace.host.0.project).unwrap() = None;
    lock(&workspace.files.0).unwrap().active = None;
    let state = workspace.state();
    let review = std::fs::read(&workspace.review_path).unwrap();
    assert!(
        save(
            &workspace.host,
            &workspace.files,
            workspace.request(),
            None,
            |_| Ok(None),
            || panic!("No destination was chosen"),
            super::super::write_project
        )
        .unwrap()
        .is_none()
    );
    workspace.unchanged(&state, &review);
    let destination = workspace.directory.path().join("first.nvsf");
    assert!(
        save(
            &workspace.host,
            &workspace.files,
            workspace.request(),
            None,
            |_| Ok(Some(destination.clone())),
            || panic!("A new path needs no replacement consent"),
            super::super::write_project
        )
        .unwrap()
        .is_some()
    );
    assert!(
        nvsf_app::storage::load_project(&destination)
            .unwrap()
            .is_setup()
    );
    assert!(
        lock(&workspace.host.0.project)
            .unwrap()
            .as_ref()
            .unwrap()
            .is_setup()
    );
    assert_eq!(std::fs::read(&workspace.review_path).unwrap(), review);
}

#[test]
fn save_without_a_project_rejects_invalid_optional_arguments() {
    let workspace = Workspace::new();
    *lock(&workspace.host.0.project).unwrap() = None;
    for token in [None, Some("source-token".to_string())] {
        let state = workspace.state();
        let result = save(
            &workspace.host,
            &workspace.files,
            SaveRequest {
                project: None,
                source_token: token,
                as_new: false,
            },
            None,
            |_| panic!("Missing project must not reach picker"),
            || panic!("Missing project must not confirm"),
            super::super::write_project,
        );
        assert!(result.is_err());
        assert_eq!(workspace.state(), state);
    }
}

#[test]
fn successful_setup_save_publishes_exact_configuration_only_after_writing() {
    let workspace = Workspace::new();
    let review = std::fs::read(&workspace.review_path).unwrap();
    let result = save(
        &workspace.host,
        &workspace.files,
        workspace.request(),
        None,
        |_| Ok(Some(workspace.destination.clone())),
        || Ok(true),
        super::super::write_project,
    );
    assert_eq!(result.unwrap().as_deref(), Some("new setup.nvsf"));
    let saved = nvsf_app::storage::load_project(&workspace.destination).unwrap();
    let mut expected = workspace.setup.config.clone();
    expected.input = path_text(&workspace.source).unwrap();
    assert_eq!(saved.config, expected);
    assert!(saved.is_setup());
    assert_eq!(
        serde_json::to_vec(&saved).unwrap(),
        serde_json::to_vec(lock(&workspace.host.0.project).unwrap().as_ref().unwrap()).unwrap()
    );
    assert_eq!(std::fs::read(&workspace.review_path).unwrap(), review);
    assert_eq!(
        lock(&workspace.files.0).unwrap().active,
        Some((None, workspace.destination.clone()))
    );
    assert!(!workspace.host.0.unsaved.load(Ordering::Acquire));
    assert!(!workspace.host.0.extract_only_dirty.load(Ordering::Acquire));
    let mut next = saved;
    next.config.quality = "accurate".into();
    let result = save(
        &workspace.host,
        &workspace.files,
        SaveRequest {
            project: Some(next.clone()),
            source_token: Some("source-token".into()),
            as_new: false,
        },
        None,
        |_| panic!("A saved setup must retain its own Save destination"),
        || Ok(true),
        super::super::write_project,
    );
    assert!(result.unwrap().is_some());
    assert_eq!(
        nvsf_app::storage::load_project(&workspace.destination)
            .unwrap()
            .config,
        next.config
    );
    assert_eq!(lock(&workspace.files.0).unwrap().recent.len(), 1);
}

#[test]
fn stale_save_rejects_without_rolling_back_a_newer_workspace() {
    for change in ["generation", "project", "job", "review_dirty"] {
        let workspace = Workspace::new();
        let review = std::fs::read(&workspace.review_path).unwrap();
        let changed = RefCell::new(Vec::new());
        let result = save(
            &workspace.host,
            &workspace.files,
            workspace.request(),
            None,
            |_| {
                match change {
                    "generation" => lock(&workspace.files.0).unwrap().generation += 1,
                    "project" => lock(&workspace.host.0.project)
                        .unwrap()
                        .as_mut()
                        .unwrap()
                        .warnings
                        .push("newer native project state".into()),
                    "job" => {
                        let (_send, done) = mpsc::channel();
                        *lock(&workspace.host.0.job).unwrap() = Some(Job {
                            id: "new-job".into(),
                            control: nvsf_app::Control::default(),
                            status: Arc::new(Mutex::new(JobStatus::default())),
                            done,
                            thread: None,
                        });
                    }
                    _ => workspace
                        .host
                        .0
                        .extract_only_dirty
                        .store(false, Ordering::Release),
                }
                *changed.borrow_mut() = workspace.state();
                Ok(Some(workspace.destination.clone()))
            },
            || Ok(true),
            |_, _| panic!("Stale state must be rejected before publication"),
        );
        assert!(result.unwrap_err().contains("workspace changed"));
        workspace.unchanged(&changed.borrow(), &review);
    }
}

#[test]
fn setup_save_rejects_invalid_identity_and_unsaved_review_before_the_picker() {
    for invalid in ["source", "review_dirty", "configuration"] {
        let mut workspace = Workspace::new();
        match invalid {
            "source" => workspace.setup.source_hash = "d".repeat(64),
            "review_dirty" => workspace
                .host
                .0
                .extract_only_dirty
                .store(false, Ordering::Release),
            _ => workspace.setup.config.threads = 0,
        }
        let state = workspace.state();
        let review = std::fs::read(&workspace.review_path).unwrap();
        assert!(
            save(
                &workspace.host,
                &workspace.files,
                workspace.request(),
                None,
                |_| panic!("Invalid candidate must not open a picker"),
                || panic!("Invalid candidate must not confirm"),
                super::super::write_project
            )
            .is_err()
        );
        workspace.unchanged(&state, &review);
    }
}

#[test]
fn review_save_restores_native_paths_and_preserves_extraction_metadata() {
    let workspace = Workspace::new();
    let original = lock(&workspace.host.0.project).unwrap().clone().unwrap();
    let mut correction = project_staging::for_webview(original.clone());
    correction.cues[0].text = "Corrected فارسی 日本".into();
    correction.cues[0].edited = true;
    correction.processed_ms = 9000;
    correction.completed = false;
    let result = save(
        &workspace.host,
        &workspace.files,
        SaveRequest {
            project: Some(correction.clone()),
            source_token: None,
            as_new: false,
        },
        None,
        |_| panic!("Review Save must keep its existing destination"),
        || Ok(true),
        super::super::write_project,
    );
    assert_eq!(result.unwrap().as_deref(), Some("completed.nvsf"));
    let saved = nvsf_app::storage::load_project(&workspace.review_path).unwrap();
    assert_eq!(saved.config, original.config);
    assert_eq!(saved.cues, correction.cues);
    assert_eq!(saved.processed_ms, original.processed_ms);
    assert_eq!(saved.completed, original.completed);
    assert_eq!(saved.model_hash, original.model_hash);
}

#[test]
fn project_source_cannot_become_a_save_destination() {
    let workspace = Workspace::new();
    let state = workspace.state();
    let review = std::fs::read(&workspace.review_path).unwrap();
    let result = save(
        &workspace.host,
        &workspace.files,
        workspace.request(),
        None,
        |_| Ok(Some(workspace.source.clone())),
        || panic!("Source protection must run before overwrite confirmation"),
        super::super::write_project,
    );
    assert!(result.unwrap_err().contains("source media"));
    assert_eq!(
        std::fs::read(&workspace.source).unwrap(),
        b"original video bytes"
    );
    workspace.unchanged(&state, &review);
}

#[test]
fn native_close_verification_failure_keeps_the_active_project() {
    let workspace = Workspace::new();
    workspace.host.0.closing.store(true, Ordering::Release);
    let state = workspace.state();
    let review = std::fs::read(&workspace.review_path).unwrap();
    let verify = || Err("controlled close snapshot failure".into());
    let result = save(
        &workspace.host,
        &workspace.files,
        SaveRequest {
            project: None,
            source_token: None,
            as_new: true,
        },
        Some(&verify),
        |_| Ok(Some(workspace.destination.clone())),
        || Ok(true),
        |_, _| panic!("A changed close snapshot must not publish"),
    );
    assert_eq!(result.unwrap_err(), "controlled close snapshot failure");
    workspace.unchanged(&state, &review);
    assert!(workspace.directory.path().exists());
    let verify = || Ok(());
    assert!(
        save(
            &workspace.host,
            &workspace.files,
            SaveRequest {
                project: None,
                source_token: None,
                as_new: true,
            },
            Some(&verify),
            |_| Ok(Some(workspace.destination.clone())),
            || Ok(true),
            super::super::write_project
        )
        .unwrap()
        .is_some()
    );
    assert_eq!(
        nvsf_app::storage::load_project(&workspace.destination)
            .unwrap()
            .cues,
        lock(&workspace.host.0.project)
            .unwrap()
            .as_ref()
            .unwrap()
            .cues
    );
    assert!(workspace.host.0.closing.load(Ordering::Acquire));
}
