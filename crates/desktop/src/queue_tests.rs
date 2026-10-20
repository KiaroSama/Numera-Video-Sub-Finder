use crate::{
    queue::{MAX_ENTRIES, QueueFile, Record, Store, mark},
    state::{Host, lock},
};
use nvsf_core::ExtractConfig;
use std::path::{Path, PathBuf};

fn record(root: &Path, name: &str) -> Record {
    let source = root.join(format!("{name}.mp4"));
    let output = root.join(format!("{name}.srt"));
    Record {
        id: format!("queue-{name}"),
        source: source.clone(),
        output: output.clone(),
        source_hash: "a".repeat(64),
        config: ExtractConfig {
            input: source.to_str().unwrap().into(),
            output: output.to_str().unwrap().into(),
            ..ExtractConfig::default()
        },
        checkpoint: None,
        state: "queued".into(),
        error: None,
    }
}

fn host(root: &Path) -> Host {
    let paths = nvsf_app::Paths::new(Some(root)).unwrap();
    let log = nvsf_app::logging::Log::new(&paths, "nvsf-desktop");
    Host::new(paths, log)
}

#[test]
fn queue_registration_preserves_grants_and_rolls_back_failed_persistence() {
    use crate::queue::register_with;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let host = host(root);
    let source = root.join("granted source.mp4");
    let output = root.join("granted output.srt");
    std::fs::write(&source, b"original video identity").unwrap();
    let source = source.canonicalize().unwrap();
    lock(&host.0.sources)
        .unwrap()
        .insert("source".into(), source.clone());
    lock(&host.0.destinations)
        .unwrap()
        .insert("output".into(), output.clone());
    let media = nvsf_core::MediaInfo {
        width: 640,
        height: 360,
        duration_ms: Some(6000),
        codec: "h264".into(),
        time_base_num: 1,
        time_base_den: 1000,
        subtitle_streams: 0,
        rotation: 0,
    };
    let config = ExtractConfig::default();
    assert!(
        register_with(&host, "missing", "output", config.clone(), |_| panic!(
            "ungranted source"
        ))
        .is_err()
    );
    assert!(
        register_with(&host, "source", "missing", config.clone(), |_| panic!(
            "ungranted output"
        ))
        .is_err()
    );
    let alias = root.join("alias");
    std::fs::create_dir(&alias).unwrap();
    lock(&host.0.destinations)
        .unwrap()
        .insert("same".into(), alias.join("../granted source.mp4"));
    assert_eq!(
        register_with(&host, "source", "same", config.clone(), |_| panic!(
            "source overwrite"
        ))
        .unwrap_err(),
        "Queue output must not replace source media"
    );
    assert_eq!(
        register_with(&host, "source", "output", config.clone(), |_| Err(
            "controlled probe failure".into()
        ))
        .unwrap_err(),
        "controlled probe failure"
    );
    assert!(lock(&host.0.queue).unwrap().file.records.is_empty());
    let (id, admitted) = register_with(&host, "source", "output", config.clone(), |path| {
        assert_eq!(path, source);
        Ok(media.clone())
    })
    .unwrap();
    assert_eq!(Path::new(&admitted.input), source);
    assert_eq!(Path::new(&admitted.output), output);
    assert_eq!(admitted.data_root.as_deref(), host.0.paths.root.to_str());
    let original = std::fs::read(root.join("desktop-queue.json")).unwrap();
    {
        let store = lock(&host.0.queue).unwrap();
        assert_eq!(store.file.records.len(), 1);
        assert_eq!(store.file.records[0].id, id);
        assert_eq!(
            store.file.records[0].source_hash,
            nvsf_app::storage::hash_file(&source).unwrap()
        );
        assert_eq!(store.grants[&id].source.token, "source");
        assert_eq!(store.grants[&id].output.token, "output");
        assert!(!store.entries().unwrap()[0].needs_regrant);
    }
    assert_eq!(
        register_with(&host, "source", "output", config.clone(), |_| Ok(
            media.clone()
        ))
        .unwrap_err(),
        "This destination already has a saved queue record. Resume that record or remove it before starting new work"
    );
    assert_eq!(
        std::fs::read(root.join("desktop-queue.json")).unwrap(),
        original
    );
    let second = root.join("second.srt");
    lock(&host.0.destinations)
        .unwrap()
        .insert("second".into(), second.clone());
    let persisted = root.join("desktop-queue.json");
    let backup = root.join("preserved-queue.json");
    std::fs::rename(&persisted, &backup).unwrap();
    std::fs::create_dir(&persisted).unwrap();
    assert!(
        register_with(&host, "source", "second", config.clone(), |_| Ok(
            media.clone()
        ))
        .is_err()
    );
    assert_eq!(lock(&host.0.queue).unwrap().file.records.len(), 1);
    assert_eq!(lock(&host.0.queue).unwrap().grants.len(), 1);
    assert_eq!(std::fs::read(&backup).unwrap(), original);
    std::fs::remove_dir(&persisted).unwrap();
    std::fs::rename(&backup, &persisted).unwrap();
    {
        let mut store = lock(&host.0.queue).unwrap();
        store
            .file
            .records
            .extend((1..MAX_ENTRIES).map(|i| record(root, &format!("bulk-{i}"))));
        store.file.validate().unwrap();
    }
    assert_eq!(
        register_with(&host, "source", "second", config, |_| Ok(media)).unwrap_err(),
        "Queue is limited to 100 records; remove finished entries first"
    );
    assert_eq!(lock(&host.0.queue).unwrap().file.records.len(), MAX_ENTRIES);
    assert_eq!(lock(&host.0.queue).unwrap().grants.len(), 1);
    assert_eq!(std::fs::read(persisted).unwrap(), original);
    assert_eq!(std::fs::read(source).unwrap(), b"original video identity");
    assert!(!output.exists() && !second.exists());
}

#[test]
fn queue_regrant_rechecks_workspace_and_record_after_external_selection() {
    use crate::{
        queue::{Grant, regrant_queue_with},
        state::{Destination, Source},
    };
    use std::sync::atomic::Ordering;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let host = host(root);
    let row = record(root, "regrant");
    let id = row.id.clone();
    let grant = Grant {
        source: Source {
            token: "source-regrant".into(),
            name: "regrant.mp4".into(),
            fingerprint: row.source_hash.clone(),
            media: nvsf_core::MediaInfo {
                width: 640,
                height: 360,
                duration_ms: Some(1000),
                codec: "h264".into(),
                time_base_num: 1,
                time_base_den: 1000,
                subtitle_streams: 0,
                rotation: 0,
            },
        },
        output: Destination {
            token: "output-regrant".into(),
            name: "regrant.srt".into(),
        },
    };
    assert_eq!(
        regrant_queue_with(&host, &id, |_| panic!("missing record"))
            .err()
            .unwrap(),
        "Queue record not found"
    );
    {
        let mut store = lock(&host.0.queue).unwrap();
        store.file.records.push(row.clone());
        store.save(root).unwrap();
    }
    let original = std::fs::read(root.join("desktop-queue.json")).unwrap();
    host.0.unsaved.store(true, Ordering::Release);
    assert_eq!(
        regrant_queue_with(&host, &id, |_| panic!("unsaved edits"))
            .err()
            .unwrap(),
        "Save current corrections before regranting/resuming a queue entry"
    );
    host.0.unsaved.store(false, Ordering::Release);
    assert!(
        regrant_queue_with(&host, &id, |_| Ok(None))
            .unwrap()
            .is_none()
    );
    assert!(lock(&host.0.queue).unwrap().grants.is_empty());
    assert_eq!(
        regrant_queue_with(&host, &id, |_| {
            host.0.closing.store(true, Ordering::Release);
            Ok(Some(grant.clone()))
        })
        .err()
        .unwrap(),
        "Wait for source operations, update installation or application shutdown to finish"
    );
    host.0.closing.store(false, Ordering::Release);
    assert!(lock(&host.0.queue).unwrap().grants.is_empty());
    assert_eq!(
        regrant_queue_with(&host, &id, |_| {
            lock(&host.0.queue).unwrap().file.records.clear();
            Ok(Some(grant.clone()))
        })
        .err()
        .unwrap(),
        "Queue record was removed while its pickers were open"
    );
    {
        let mut store = lock(&host.0.queue).unwrap();
        assert!(store.grants.is_empty());
        store.file.records.push(row);
    }
    let entries = regrant_queue_with(&host, &id, |selected| {
        assert_eq!(selected.id, id);
        Ok(Some(grant))
    })
    .unwrap()
    .unwrap();
    assert_eq!(entries.len(), 1);
    assert!(!entries[0].needs_regrant);
    assert_eq!(entries[0].id, id);
    assert_eq!(
        std::fs::read(root.join("desktop-queue.json")).unwrap(),
        original
    );
}

#[test]
fn queue_round_trip_preserves_states_and_reports_regrant() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mut file = QueueFile {
        schema: 1,
        records: vec![record(root, "first"), record(root, "second")],
    };
    file.records[0].state = "running".into();
    file.records[1].state = "failed".into();
    file.records[1].error = Some("worker stopped".into());
    file.validate().unwrap();
    let mut store = Store::default();
    store.file = file;
    store.save(root).unwrap();

    let loaded = Store::load(root);
    loaded.check().unwrap();
    let records = &loaded.file.records;
    assert_eq!(records[0].state, "interrupted");
    assert_eq!(records[1].state, "failed");
    let entries = loaded.entries().unwrap();
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().all(|e| e.needs_regrant));
    assert_eq!(entries[0].name, "first.mp4");
    assert_eq!(entries[0].output_name, "first.srt");
    assert!(entries[0].resumable);
    assert_eq!(entries[1].error.as_deref(), Some("worker stopped"));
}

#[test]
fn queue_validation_rejects_ambiguous_identity_paths_and_states() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let invalid = |mutate: &dyn Fn(&mut Vec<Record>)| {
        let mut file = QueueFile {
            schema: 1,
            records: vec![record(root, "one"), record(root, "two")],
        };
        mutate(&mut file.records);
        file.validate()
    };
    assert!(invalid(&|records| records[1].output = records[0].output.clone()).is_err());
    assert!(invalid(&|records| records[0].id = records[1].id.clone()).is_err());
    assert!(invalid(&|records| records[0].source = PathBuf::from("relative.mp4")).is_err());
    assert!(invalid(&|records| records[0].source_hash = "abc".into()).is_err());
    assert!(invalid(&|records| records[0].state = "unknown".into()).is_err());
    assert!(invalid(&|records| records[0].config.input = "/tmp/other.mp4".into()).is_err());
    assert!(
        invalid(&|records| { records[0].checkpoint = Some(root.join("copied.nvsf.json")) })
            .is_err()
    );
    assert!(invalid(&|records| records[0].error = Some("x".repeat(4097))).is_err());
    assert!(
        invalid(&|records| {
            records.truncate(0);
            records.extend((0..=MAX_ENTRIES).map(|i| {
                let mut row = record(root, "bulk");
                row.id = format!("bulk-{i}");
                row.output = root.join(format!("bulk-{i}.srt"));
                row.config.output = row.output.to_str().unwrap().into();
                row
            }));
        })
        .is_err()
    );
}

#[test]
fn corrupt_queue_file_is_preserved_and_surfaced() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let path = root.join("desktop-queue.json");
    let original = b"{ not a queue";
    std::fs::write(&path, original).unwrap();
    let store = Store::load(root);
    let error = store.check().unwrap_err();
    assert!(error.contains("original file preserved"));
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

#[test]
fn failed_queue_persistence_rolls_back_state_and_preserves_the_checkpoint() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let host = host(root);
    let row = record(root, "rollback");
    let id = row.id.clone();
    let checkpoint = row.output.with_extension("nvsf.json");
    std::fs::write(&checkpoint, b"existing checkpoint").unwrap();
    {
        let mut store = lock(&host.0.queue).unwrap();
        store.file.records.push(row);
        store.save(root).unwrap();
    }
    let persisted = root.join("desktop-queue.json");
    let original = std::fs::read(&persisted).unwrap();
    std::fs::rename(&persisted, root.join("preserved-queue.json")).unwrap();
    std::fs::create_dir(&persisted).unwrap();
    assert!(mark(&host, &id, "failed", Some("new error".into())).is_err());
    let store = lock(&host.0.queue).unwrap();
    assert_eq!(store.file.records[0].state, "queued");
    assert!(store.file.records[0].error.is_none());
    assert!(store.file.records[0].checkpoint.is_none());
    assert_eq!(
        std::fs::read(root.join("preserved-queue.json")).unwrap(),
        original
    );
    assert_eq!(std::fs::read(checkpoint).unwrap(), b"existing checkpoint");
    assert!(persisted.is_dir());
}

#[test]
fn mark_updates_state_and_truncates_worker_errors() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    {
        let host = host(root);
        let row = record(root, "marked");
        let id = row.id.clone();
        {
            let mut store = lock(&host.0.queue).unwrap();
            store.file.records.push(row);
            store.save(root).unwrap();
        }
        mark(&host, &id, "failed", Some("x".repeat(2000))).unwrap();
        let store = lock(&host.0.queue).unwrap();
        let saved = &store.file.records[0];
        assert_eq!(saved.state, "failed");
        assert_eq!(saved.error.as_ref().unwrap().chars().count(), 1000);
        drop(store);
        assert!(mark(&host, "queue-missing", "failed", None).is_err());
        assert_eq!(lock(&host.0.queue).unwrap().file.records[0].state, "failed");
    }
    let reloaded = Store::load(root);
    reloaded.check().unwrap();
    assert_eq!(reloaded.file.records[0].state, "failed");
    assert_eq!(
        reloaded.file.records[0]
            .error
            .as_ref()
            .unwrap()
            .chars()
            .count(),
        1000
    );
}
