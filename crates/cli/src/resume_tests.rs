use super::*;
use nvsf_core::{Cue, MediaInfo};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

static NEXT: AtomicU32 = AtomicU32::new(0);

fn temp_dir() -> PathBuf {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("nvsf-resume-test-{}-{id}", std::process::id()));
    std::fs::create_dir(&path).expect("resume fixture directory");
    path
}

fn project(root: &Path, text: &str) -> Project {
    Project {
        schema: 1,
        source_hash: "a".repeat(64),
        config_hash: "b".repeat(64),
        model_hash: "c".repeat(64),
        config: ExtractConfig {
            input: root.join("input.mp4").to_str().unwrap().into(),
            output: root.join("output.srt").to_str().unwrap().into(),
            ..Default::default()
        },
        media: MediaInfo {
            width: 640,
            height: 360,
            duration_ms: Some(1000),
            codec: "fixture".into(),
            time_base_num: 1,
            time_base_den: 1000,
            subtitle_streams: 0,
            rotation: 0,
        },
        cues: vec![Cue::new(1, 0, 0, 500, text.into()).unwrap()],
        processed_ms: 500,
        active_appearances: vec![],
        completed: false,
        warnings: vec![],
    }
}

#[test]
fn resume_forwards_the_selected_checkpoint_instead_of_the_output_sibling() {
    let root = temp_dir();
    let selected = root.join("selected.nvsf.json");
    let sibling = root.join("output.nvsf.json");
    storage::save_project(&project(&root, "Selected correction"), &selected).unwrap();
    storage::save_project(&project(&root, "Unrelated correction"), &sibling).unwrap();
    let selected_bytes = std::fs::read(&selected).unwrap();
    let sibling_bytes = std::fs::read(&sibling).unwrap();

    let resumed = run_with(&selected, &Control::default(), |config, _, checkpoint| {
        assert_eq!(checkpoint, selected.canonicalize().unwrap());
        let loaded = storage::load_project(checkpoint)?;
        assert_eq!(config.output, loaded.config.output);
        Ok(loaded)
    })
    .expect("selected checkpoint admitted");

    assert_eq!(resumed.cues[0].text, "Selected correction");
    assert_eq!(std::fs::read(&selected).unwrap(), selected_bytes);
    assert_eq!(std::fs::read(&sibling).unwrap(), sibling_bytes);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn resume_load_failure_never_launches_a_worker_or_falls_back_to_a_sibling() {
    let root = temp_dir();
    let sibling = root.join("output.nvsf.json");
    storage::save_project(&project(&root, "Unrelated correction"), &sibling).unwrap();
    let sibling_bytes = std::fs::read(&sibling).unwrap();
    let malformed = root.join("malformed.nvsf.json");
    std::fs::write(&malformed, b"not a project").unwrap();
    for selected in [&malformed, &root.join("missing.nvsf.json")] {
        assert!(
            run_with(selected, &Control::default(), |_, _, _| {
                panic!("invalid selected checkpoint must not launch extraction")
            })
            .is_err()
        );
    }
    assert_eq!(std::fs::read(&sibling).unwrap(), sibling_bytes);
    std::fs::remove_dir_all(root).unwrap();
}
