use super::*;
use nvsf_core::{Cue, MediaInfo};
use std::sync::atomic::AtomicU32;

static TEST_SEQ: AtomicU32 = AtomicU32::new(0);

fn temp_root() -> PathBuf {
    let id = TEST_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("nvsf-qtest-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn make_args(input: PathBuf, output: PathBuf) -> QueueAddArgs {
    QueueAddArgs {
        input,
        output,
        language: "en".into(),
        device: "auto".into(),
        quality: "balanced".into(),
        roi: vec![],
        exclude: vec![],
        start_ms: 0,
        end_ms: None,
        threads: 2,
        overwrite: false,
        normalize_persian: false,
        min_confidence: 0.7,
        gpu_required: false,
        accelerator_required: false,
        checkpoint: None,
    }
}

fn completed_project(cfg: &ExtractConfig, reviewed: bool) -> Project {
    let mut cue = Cue::new(1, 0, 0, 1000, "Line".into()).expect("cue");
    cue.reviewed = reviewed;
    cue.confidence = Some(if reviewed { 0.95 } else { 0.4 });
    Project {
        schema: 1,
        source_hash: "a".repeat(64),
        config_hash: "b".repeat(64),
        model_hash: "c".repeat(64),
        config: cfg.clone(),
        media: MediaInfo {
            width: 1280,
            height: 720,
            duration_ms: Some(1000),
            codec: "h264".into(),
            time_base_num: 1,
            time_base_den: 1000,
            subtitle_streams: 0,
            rotation: 0,
        },
        cues: vec![cue],
        processed_ms: 1000,
        active_appearances: vec![],
        completed: true,
        warnings: vec![],
    }
}

#[test]
fn queue_crud_and_run_preserve_desktop_schema() {
    let dir = temp_root();
    let video = dir.join("clip.mp4");
    std::fs::write(&video, b"fake video bytes").expect("write clip");
    let out = dir.join("clip.srt");

    let mut store = QueueStore::open(&dir).expect("open queue");
    let rec =
        add_record(&mut store, &dir, make_args(video.clone(), out.clone())).expect("add record");
    assert!(!rec.id.is_empty());
    assert_eq!(store.file.schema, 1);
    assert_eq!(store.file.records.len(), 1);
    assert!(add_record(&mut store, &dir, make_args(video.clone(), out)).is_err());

    let out2 = dir.join("clip2.srt");
    let rec2 = add_record(&mut store, &dir, make_args(video, out2)).expect("add second");
    remove_record(&mut store, &rec2.id).expect("remove second");
    assert_eq!(store.file.records.len(), 1);

    let control = Control::default();
    let (summary, code) = run_records(&mut store, None, 1, false, &control, |cfg, _| {
        Ok(completed_project(&cfg, true))
    })
    .expect("run queue");
    assert_eq!(code, 0);
    assert_eq!(summary["succeeded"], 1);
    assert_eq!(store.file.records[0].state, "completed");

    let cleared = clear_records(&mut store, "completed").expect("clear completed");
    assert_eq!(cleared, 1);
    assert!(store.file.records.is_empty());
    drop(store);
    let reloaded = QueueStore::open(&dir).expect("reopen queue");
    assert_eq!(reloaded.file.schema, 1);
    assert!(reloaded.file.records.is_empty());
    drop(reloaded);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn queue_validation_error_truncation_and_failure_isolation() {
    let dir = temp_root();
    let v1 = dir.join("v1.mp4");
    let v2 = dir.join("v2.mp4");
    std::fs::write(&v1, b"v1").expect("v1");
    std::fs::write(&v2, b"v2").expect("v2");

    let mut store = QueueStore::open(&dir).expect("open");
    let r1 = add_record(&mut store, &dir, make_args(v1.clone(), dir.join("o1.srt"))).expect("r1");
    let r2 = add_record(&mut store, &dir, make_args(v2.clone(), dir.join("o2.srt"))).expect("r2");

    // Duplicate output destination rejected.
    assert!(add_record(&mut store, &dir, make_args(v1.clone(), dir.join("o1.srt"))).is_err());
    // Missing remove ID rejected.
    assert!(remove_record(&mut store, "no-such-id").is_err());
    // Invalid clear state rejected.
    assert!(clear_records(&mut store, "bogus").is_err());

    // Failure isolation and fail-fast cancellation.
    let control = Control::default();
    let (code, summary) = run_records(&mut store, None, 1, true, &control, &|cfg, _| {
        if cfg.input.ends_with("v1.mp4") {
            Err(AppError::Provider("gpu unavailable".into()))
        } else {
            Ok(completed_project(&cfg, false))
        }
    })
    .expect("run fail-fast");
    assert_eq!(code, 6);
    assert_eq!(summary["failed"], 1);
    assert_eq!(store.file.records[0].state, "failed");
    assert_eq!(r1.id, store.file.records[0].id);
    assert_eq!(r2.id, store.file.records[1].id);

    // Schema/field validation errors.
    let bad = QueueFile {
        schema: 2,
        records: store.file.records.clone(),
    };
    assert!(bad.validate().is_err());
    let mut bad_state = QueueFile {
        schema: 1,
        records: store.file.records.clone(),
    };
    bad_state.records[0].state = "unknown-state".into();
    assert!(bad_state.validate().is_err());
    let mut dup_id = QueueFile {
        schema: 1,
        records: store.file.records.clone(),
    };
    dup_id.records[1].id = dup_id.records[0].id.clone();
    assert!(dup_id.validate().is_err());

    assert_eq!(combine_failure(0, 6), 6);
    assert_eq!(combine_failure(4, 6), 4);
    assert_eq!(truncate_error(&"x".repeat(1200)).chars().count(), 1000);

    let cleared = clear_records(&mut store, "all").expect("clear all");
    assert_eq!(cleared, 2);
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}
