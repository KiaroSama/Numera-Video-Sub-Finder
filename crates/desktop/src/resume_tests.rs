use crate::{
    queue::{Grant, Record},
    resume::{check_checkpoint, open_checkpoint_with, regrant_with, restore_record},
    state::{Destination, Host, Source, lock},
};
use nvsf_core::{Cue, ExtractConfig, MediaInfo, Project};
use std::path::Path;

#[test]
fn checkpoint_admission_preserves_exact_config_and_saved_corrections() {
    let root = std::env::current_dir().unwrap();
    let config = ExtractConfig {
        input: root.join("video.mp4").to_str().unwrap().into(),
        output: root.join("video.srt").to_str().unwrap().into(),
        ..ExtractConfig::default()
    };
    let mut cue = Cue::new(1, 0, 0, 1000, "Original OCR".into()).unwrap();
    cue.text = "Corrected فارسی 日本".into();
    cue.edited = true;
    let mut project = Project {
        schema: 1,
        source_hash: "a".repeat(64),
        config_hash: "b".repeat(64),
        model_hash: "c".repeat(64),
        config,
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
        cues: vec![cue],
        processed_ms: 1500,
        active_appearances: vec![],
        completed: false,
        warnings: vec![],
    };
    let checkpoint = Path::new(&project.config.output).with_extension("nvsf.json");
    let before = serde_json::to_vec(&project).unwrap();
    check_checkpoint(&project, &checkpoint).unwrap();
    assert_eq!(
        serde_json::to_vec(&project).unwrap(),
        before,
        "admission must not rewrite config/corrections"
    );
    assert!(check_checkpoint(&project, &root.join("copied.json")).is_err());
    project.completed = true;
    assert!(check_checkpoint(&project, &checkpoint).is_err());
    project.completed = false;
    project.config.input = "relative.mp4".into();
    assert!(check_checkpoint(&project, &checkpoint).is_err());
}

fn workspace() -> (tempfile::TempDir, Host, Record, Grant) {
    let directory = tempfile::tempdir().unwrap();
    let paths = nvsf_app::Paths::new(Some(directory.path())).unwrap();
    let log = nvsf_app::logging::Log::new(&paths, "nvsf-desktop");
    let host = Host::new(paths, log);
    let source = directory.path().join("video.mp4");
    let output = directory.path().join("video.srt");
    std::fs::write(&source, b"original video identity").unwrap();
    let source_hash = nvsf_app::storage::hash_file(&source).unwrap();
    let record = Record {
        id: "saved".into(),
        source: source.clone(),
        output: output.clone(),
        source_hash: source_hash.clone(),
        config: ExtractConfig {
            input: source.to_str().unwrap().into(),
            output: output.to_str().unwrap().into(),
            ..Default::default()
        },
        checkpoint: Some(output.with_extension("nvsf.json")),
        state: "interrupted".into(),
        error: None,
    };
    let grant = Grant {
        source: Source {
            token: "source-grant".into(),
            name: "video.mp4".into(),
            fingerprint: source_hash,
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
        },
        output: Destination {
            token: "output-grant".into(),
            name: "video.srt".into(),
        },
    };
    (directory, host, record, grant)
}

#[test]
fn checkpoint_open_preserves_corrections_and_requires_compatible_location_and_root() {
    let (directory, host, record, grant) = workspace();
    let mut cue = Cue::new(1, 0, 0, 1000, "Original".into()).unwrap();
    cue.text = "Corrected فارسی 日本".into();
    cue.edited = true;
    let mut project = Project {
        schema: 1,
        source_hash: record.source_hash.clone(),
        config_hash: "b".repeat(64),
        model_hash: "c".repeat(64),
        config: record.config.clone(),
        media: grant.source.media.clone(),
        cues: vec![cue],
        processed_ms: 1500,
        active_appearances: vec![],
        completed: false,
        warnings: vec![],
    };
    let path = record.checkpoint.as_ref().unwrap();
    nvsf_app::storage::save_project(&project, path).unwrap();
    let before = std::fs::read(path).unwrap();
    assert!(
        open_checkpoint_with(&host, path, |_| Ok(false), |_| panic!("consent denied"))
            .unwrap()
            .is_none()
    );
    assert!(
        open_checkpoint_with(&host, path, |_| Ok(true), |_| Ok(None))
            .unwrap()
            .is_none()
    );
    assert!(lock(&host.0.queue).unwrap().file.records.is_empty());
    let copied = directory.path().join("copied.json");
    std::fs::write(&copied, &before).unwrap();
    assert_eq!(
        open_checkpoint_with(
            &host,
            &copied,
            |_| panic!("wrong location"),
            |_| panic!("wrong location")
        )
        .err()
        .unwrap(),
        "Checkpoint is not at its original output-associated location; original data preserved"
    );
    project.config.data_root = Some("different-root".into());
    nvsf_app::storage::save_project(&project, path).unwrap();
    assert_eq!(
        open_checkpoint_with(
            &host,
            path,
            |_| panic!("wrong data root"),
            |_| panic!("wrong data root")
        )
        .err()
        .unwrap(),
        "Checkpoint uses a different data root. Restore that root before resuming; models and existing state are preserved."
    );
    project.config.data_root = Some(host.0.paths.root.to_str().unwrap().into());
    nvsf_app::storage::save_project(&project, path).unwrap();
    let original = std::fs::read(path).unwrap();
    let entries = open_checkpoint_with(
        &host,
        path,
        |message| {
            assert!(message.contains("1500 ms") && message.contains("1 completed cues"));
            assert!(message.contains("Corrections stored in this checkpoint will be kept"));
            Ok(true)
        },
        |selected| {
            assert_eq!(selected.source_hash, record.source_hash);
            assert_eq!(selected.checkpoint.as_ref(), Some(path));
            Ok(Some(grant))
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(entries.len(), 1);
    assert!(!entries[0].needs_regrant);
    assert_eq!(std::fs::read(path).unwrap(), original);
    let restored = nvsf_app::storage::load_project(path).unwrap();
    assert_eq!(restored.cues, project.cues);
    assert!(!record.output.exists());
}

#[test]
fn regrant_cancellation_and_mismatched_identity_never_publish_grants() {
    let (directory, host, record, _) = workspace();
    assert!(
        regrant_with(
            &host,
            &record,
            || Ok(None),
            || panic!("source cancelled"),
            |_| panic!("source cancelled")
        )
        .unwrap()
        .is_none()
    );
    assert!(
        regrant_with(
            &host,
            &record,
            || Ok(Some(record.source.clone())),
            || Ok(None),
            |_| panic!("output cancelled")
        )
        .unwrap()
        .is_none()
    );
    let other = directory.path().join("other.mp4");
    std::fs::write(&other, b"original video identity").unwrap();
    assert!(
        regrant_with(
            &host,
            &record,
            || Ok(Some(other)),
            || panic!("wrong source"),
            |_| panic!("wrong source")
        )
        .err()
        .unwrap()
        .contains("saved path/fingerprint")
    );
    std::fs::write(&record.source, b"changed video identity").unwrap();
    assert!(
        regrant_with(
            &host,
            &record,
            || Ok(Some(record.source.clone())),
            || panic!("wrong hash"),
            |_| panic!("wrong hash")
        )
        .err()
        .unwrap()
        .contains("saved path/fingerprint")
    );
    std::fs::write(&record.source, b"original video identity").unwrap();
    assert!(
        regrant_with(
            &host,
            &record,
            || Ok(Some(record.source.clone())),
            || Ok(Some(directory.path().join("different.srt"))),
            |_| panic!("wrong destination")
        )
        .err()
        .unwrap()
        .contains("original output location")
    );
    assert!(lock(&host.0.sources).unwrap().is_empty());
    assert!(lock(&host.0.destinations).unwrap().is_empty());
    assert!(!record.output.exists());
    assert_eq!(
        regrant_with(
            &host,
            &record,
            || Err("controlled source picker failure".into()),
            || panic!("failed source picker"),
            |_| panic!("failed source picker")
        )
        .err()
        .unwrap(),
        "controlled source picker failure"
    );
    assert_eq!(
        regrant_with(
            &host,
            &record,
            || Ok(Some(record.source.clone())),
            || Err("controlled output picker failure".into()),
            |_| panic!("failed output picker")
        )
        .err()
        .unwrap(),
        "controlled output picker failure"
    );
    assert!(lock(&host.0.sources).unwrap().is_empty());
    assert!(lock(&host.0.destinations).unwrap().is_empty());
}

#[test]
fn compatible_regrant_publishes_tokens_only_after_probe_and_checks_limits() {
    let (_directory, host, record, grant) = workspace();
    assert_eq!(
        regrant_with(
            &host,
            &record,
            || Ok(Some(record.source.clone())),
            || Ok(Some(record.output.clone())),
            |_| Err("controlled probe failure".into())
        )
        .err()
        .unwrap(),
        "controlled probe failure"
    );
    assert!(lock(&host.0.sources).unwrap().is_empty());
    assert!(lock(&host.0.destinations).unwrap().is_empty());
    let expected = grant.source.media;
    let canonical_source = record.source.canonicalize().unwrap();
    let selected = regrant_with(
        &host,
        &record,
        || Ok(Some(record.source.clone())),
        || Ok(Some(record.output.clone())),
        |path| {
            assert_eq!(path, canonical_source.as_path());
            Ok(expected.clone())
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(selected.source.fingerprint, record.source_hash);
    assert_eq!(selected.source.media.width, 1920);
    assert_eq!(
        host.source(&selected.source.token).unwrap(),
        record.source.canonicalize().unwrap()
    );
    assert_eq!(
        host.destination(&selected.output.token).unwrap(),
        record.output
    );
    for index in 0..255 {
        lock(&host.0.destinations)
            .unwrap()
            .insert(format!("filled-{index}"), record.output.clone());
    }
    let sources = lock(&host.0.sources).unwrap().len();
    assert!(
        regrant_with(
            &host,
            &record,
            || Ok(Some(record.source.clone())),
            || Ok(Some(record.output.clone())),
            |_| Ok(expected)
        )
        .err()
        .unwrap()
        .contains("Session grant limit")
    );
    assert_eq!(lock(&host.0.sources).unwrap().len(), sources);
    lock(&host.0.destinations).unwrap().clear();
    lock(&host.0.sources)
        .unwrap()
        .extend((0..1023).map(|index| (format!("filled-{index}"), canonical_source.clone())));
    assert_eq!(lock(&host.0.sources).unwrap().len(), 1024);
    assert!(
        regrant_with(
            &host,
            &record,
            || Ok(Some(record.source.clone())),
            || Ok(Some(record.output.clone())),
            |_| Ok(selected.source.media.clone())
        )
        .err()
        .unwrap()
        .contains("Session grant limit")
    );
    assert_eq!(lock(&host.0.sources).unwrap().len(), 1024);
    assert!(lock(&host.0.destinations).unwrap().is_empty());
}

#[test]
fn checkpoint_restore_refuses_busy_and_full_queues_without_publishing_grants() {
    let (directory, host, record, grant) = workspace();
    host.0
        .closing
        .store(true, std::sync::atomic::Ordering::Release);
    assert_eq!(
        restore_record(&host, record.clone(), grant.clone())
            .err()
            .unwrap(),
        "Wait for source operations, update installation or application shutdown to finish"
    );
    host.0
        .closing
        .store(false, std::sync::atomic::Ordering::Release);
    let before = {
        let mut store = lock(&host.0.queue).unwrap();
        for index in 0..crate::queue::MAX_ENTRIES {
            let mut saved = record.clone();
            saved.id = format!("saved-{index}");
            saved.output = directory.path().join(format!("saved-{index}.srt"));
            saved.config.output = saved.output.to_str().unwrap().into();
            saved.checkpoint = Some(saved.output.with_extension("nvsf.json"));
            store.file.records.push(saved);
        }
        store.save(&host.0.paths.root).unwrap();
        std::fs::read(host.0.paths.root.join("desktop-queue.json")).unwrap()
    };
    assert_eq!(
        restore_record(&host, record, grant).err().unwrap(),
        "Queue limit reached; remove a finished record"
    );
    let store = lock(&host.0.queue).unwrap();
    assert_eq!(store.file.records.len(), crate::queue::MAX_ENTRIES);
    assert!(store.grants.is_empty());
    assert_eq!(
        std::fs::read(host.0.paths.root.join("desktop-queue.json")).unwrap(),
        before
    );
}

#[test]
fn checkpoint_restore_reuses_identity_and_rolls_back_failed_persistence() {
    let (directory, host, mut record, grant) = workspace();
    std::fs::write(
        record.checkpoint.as_ref().unwrap(),
        b"preserved checkpoint bytes",
    )
    .unwrap();
    let entries = restore_record(&host, record.clone(), grant.clone()).unwrap();
    assert_eq!(entries.len(), 1);
    assert!(!entries[0].needs_regrant);
    assert_eq!(entries[0].id, "saved");
    let persisted_path = directory.path().join("desktop-queue.json");
    let before = std::fs::read(&persisted_path).unwrap();
    record.id = "new-session-id".into();
    let entries = restore_record(&host, record.clone(), grant.clone()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, "saved");
    let mut conflict = record.clone();
    conflict.source_hash = "d".repeat(64);
    assert!(
        restore_record(&host, conflict, grant.clone())
            .err()
            .unwrap()
            .contains("conflicts")
    );
    let mut incompatible = record.clone();
    incompatible.config.language = "fa".into();
    assert_eq!(
        restore_record(&host, incompatible, grant.clone())
            .err()
            .unwrap(),
        "Existing queue record conflicts with this checkpoint; original record preserved"
    );
    assert_eq!(std::fs::read(&persisted_path).unwrap(), before);
    let backup = directory.path().join("queue-backup.json");
    std::fs::rename(&persisted_path, &backup).unwrap();
    std::fs::create_dir(&persisted_path).unwrap();
    let mut replacement = record.clone();
    replacement.state = "failed".into();
    replacement.error = Some("must roll back".into());
    let before_record = {
        let store = lock(&host.0.queue).unwrap();
        store.check().unwrap();
        store.file.validate().unwrap();
        serde_json::to_vec(&store.file.records[0]).unwrap()
    };
    let mut replacement_grant = grant.clone();
    replacement_grant.source.token = "replacement-grant".into();
    assert!(restore_record(&host, replacement, replacement_grant).is_err());
    {
        let store = lock(&host.0.queue).unwrap();
        assert_eq!(
            serde_json::to_vec(&store.file.records[0]).unwrap(),
            before_record
        );
        assert_eq!(store.grants["saved"].source.token, "source-grant");
    }
    let mut additional = record.clone();
    additional.id = "additional".into();
    additional.output = directory.path().join("additional.srt");
    additional.config.output = additional.output.to_str().unwrap().into();
    additional.checkpoint = Some(additional.output.with_extension("nvsf.json"));
    {
        let mut candidate = crate::queue::QueueFile {
            schema: 1,
            records: lock(&host.0.queue).unwrap().file.records.clone(),
        };
        candidate.records.push(additional.clone());
        candidate.validate().unwrap();
    }
    assert!(restore_record(&host, additional, grant).is_err());
    assert_eq!(lock(&host.0.queue).unwrap().file.records.len(), 1);
    assert!(
        !lock(&host.0.queue)
            .unwrap()
            .grants
            .contains_key("additional")
    );
    assert_eq!(std::fs::read(&backup).unwrap(), before);
    assert_eq!(
        std::fs::read(record.checkpoint.unwrap()).unwrap(),
        b"preserved checkpoint bytes"
    );
}
