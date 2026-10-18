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
