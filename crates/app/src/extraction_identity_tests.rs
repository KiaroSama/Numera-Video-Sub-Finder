use super::SourceIdentity;
use nvsf_core::{ExtractConfig, MediaInfo, Project};
use sha2::{Digest, Sha256};

fn digest(config: &ExtractConfig) -> String {
    Sha256::digest(serde_json::to_vec(config).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn legacy(config: ExtractConfig) -> Project {
    Project {
        schema: 1,
        source_hash: "known source identity".into(),
        config_hash: digest(&config),
        model_hash: "known model identity".into(),
        config,
        media: MediaInfo {
            width: 640,
            height: 360,
            duration_ms: Some(1_000),
            codec: "fixture".into(),
            time_base_num: 1,
            time_base_den: 1_000,
            subtitle_streams: 0,
            rotation: 0,
        },
        cues: vec![],
        processed_ms: 0,
        active_appearances: vec![],
        completed: false,
        warnings: vec![],
    }
}

#[test]
fn source_is_canonical_before_hashing_and_legacy_resume_migrates_both_fields() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("video.mp4");
    std::fs::write(&source, b"source").unwrap();
    let source = source.canonicalize().unwrap();
    let config = ExtractConfig {
        input: "original-directory/video.mp4".into(),
        output: "subtitles.srt".into(),
        ..Default::default()
    };
    let mut saved = legacy(config.clone());
    let original_hash = saved.config_hash.clone();
    let bound = SourceIdentity::bind(config, &source).unwrap();
    assert_eq!(std::path::Path::new(&bound.config.input), source);
    assert_eq!(bound.hash, digest(&bound.config));
    assert_ne!(bound.hash, original_hash);
    bound
        .admit(&mut saved, "known source identity", "known model identity")
        .unwrap();
    assert_eq!(saved.config.input, bound.config.input);
    assert_eq!(saved.config_hash, digest(&saved.config));
    let resumed = SourceIdentity::bind(saved.config.clone(), &source).unwrap();
    resumed
        .admit(&mut saved, "known source identity", "known model identity")
        .unwrap();
}

#[test]
fn legacy_hash_migration_still_requires_exact_source_models_and_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("video.mp4");
    let config = ExtractConfig {
        input: "video.mp4".into(),
        output: "subtitles.srt".into(),
        ..Default::default()
    };
    let bound = SourceIdentity::bind(config.clone(), &source).unwrap();
    for changed in ["source", "model", "config"] {
        let mut saved = legacy(config.clone());
        match changed {
            "source" => saved.source_hash = "changed source".into(),
            "model" => saved.model_hash = "changed model".into(),
            _ => saved.config_hash = "unrelated configuration hash".into(),
        }
        let previous = serde_json::to_vec(&saved).unwrap();
        assert!(
            bound
                .admit(&mut saved, "known source identity", "known model identity")
                .is_err()
        );
        assert_eq!(serde_json::to_vec(&saved).unwrap(), previous);
    }
}
