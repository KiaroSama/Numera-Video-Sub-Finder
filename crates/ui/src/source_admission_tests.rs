use crate::{
    ipc::{Destination, Source, Started},
    state::Ui,
};
use leptos::prelude::*;
use nvsf_core::{ExtractConfig, MediaInfo, Project};

fn source(token: &str, fingerprint: &str) -> Source {
    Source {
        token: token.into(),
        name: format!("{fingerprint}.mp4"),
        fingerprint: fingerprint.into(),
        media: MediaInfo {
            width: 1280,
            height: 720,
            duration_ms: Some(4000),
            codec: "h264".into(),
            time_base_num: 1,
            time_base_den: 1000,
            subtitle_streams: 0,
            rotation: 0,
        },
    }
}

fn project(fingerprint: &str) -> Project {
    Project {
        schema: 1,
        source_hash: fingerprint.into(),
        config_hash: "config".into(),
        model_hash: "model".into(),
        config: ExtractConfig::default(),
        media: source("grant", fingerprint).media,
        cues: Vec::new(),
        processed_ms: 4000,
        active_appearances: Vec::new(),
        completed: true,
        warnings: Vec::new(),
    }
}

#[test]
fn starting_queued_video_retires_the_previous_source_before_loading_its_review() {
    Owner::new().with(|| {
        let ui = Ui::new("dark".into());
        ui.admit_video(source("grant-a", "video-a"), false);
        ui.preview_url.set("blob:video-a".into());
        ui.time.set(2300);
        ui.preview_busy.set(true);
        let a_generation = ui.preview_generation.get_untracked();

        let id = ui.accept_started_source(Started {
            id: "job-b".into(),
            source: source("grant-b", "video-b"),
            output: Destination {
                token: "output-b".into(),
                name: "b.srt".into(),
            },
        });
        assert_eq!(id, "job-b");
        assert_eq!(ui.source.get_untracked().unwrap().token, "grant-b");
        assert_eq!(ui.output.get_untracked().unwrap().token, "output-b");
        assert!(ui.preview_url.get_untracked().is_empty());
        assert_eq!(ui.time.get_untracked(), 0);
        assert_ne!(ui.preview_generation.get_untracked(), a_generation);
        assert!(
            ui.preview_busy.get_untracked(),
            "The outstanding decode retains ownership until completion"
        );

        ui.load(project("video-b"));
        assert_eq!(ui.route.get_untracked(), "Review");
        assert_eq!(ui.source.get_untracked().unwrap().fingerprint, "video-b");
        assert!(
            ui.preview_url.get_untracked().is_empty(),
            "Video A must never be presented as evidence for B"
        );
    });
}

#[test]
fn a_regranted_same_video_still_invalidates_the_old_request_generation() {
    Owner::new().with(|| {
        let ui = Ui::new("dark".into());
        ui.admit_video(source("old-grant", "same-video"), false);
        ui.preview_url.set("blob:old-grant".into());
        let old_generation = ui.preview_generation.get_untracked();
        ui.admit_video(source("new-grant", "same-video"), false);
        assert_ne!(ui.preview_generation.get_untracked(), old_generation);
        assert_eq!(ui.source.get_untracked().unwrap().token, "new-grant");
        assert!(ui.preview_url.get_untracked().is_empty());
    });
}

#[test]
fn loading_an_unlinked_project_invalidates_inflight_source_work_and_position() {
    Owner::new().with(|| {
        let ui = Ui::new("dark".into());
        ui.admit_video(source("grant-a", "video-a"), false);
        ui.preview_url.set("blob:a".into());
        ui.time.set(2300);
        ui.preview_busy.set(true);
        let old_generation = ui.preview_generation.get_untracked();
        ui.load(project("video-b"));
        assert!(ui.source.get_untracked().is_none());
        assert!(ui.preview_url.get_untracked().is_empty());
        assert_ne!(ui.preview_generation.get_untracked(), old_generation);
        assert_eq!(ui.time.get_untracked(), 0);
        assert!(ui.preview_busy.get_untracked());
    });
}

#[test]
fn loading_a_matching_project_preserves_its_current_valid_source_frame() {
    Owner::new().with(|| {
        let ui = Ui::new("dark".into());
        ui.admit_video(source("grant-a", "video-a"), false);
        ui.preview_url.set("blob:a".into());
        ui.time.set(2300);
        let generation = ui.preview_generation.get_untracked();
        ui.load(project("video-a"));
        assert_eq!(ui.preview_url.get_untracked(), "blob:a");
        assert_eq!(ui.time.get_untracked(), 2300);
        assert_eq!(ui.preview_generation.get_untracked(), generation);
    });
}

#[test]
fn source_replacement_retires_each_owned_url_once_and_never_releases_busy_ownership() {
    Owner::new().with(|| {
        let ui = Ui::new("dark".into());
        ui.preview_url.set("blob:a".into());
        ui.preview_busy.set(true);
        let mut retired = Vec::new();
        ui.replace_source_with(Some(source("b", "b")), |url| retired.push(url.to_owned()));
        ui.replace_source_with(None, |url| retired.push(url.to_owned()));
        assert_eq!(retired, ["blob:a"]);
        assert!(ui.preview_busy.get_untracked());
    });
}

#[test]
fn loading_an_incomplete_setup_project_routes_to_extract() {
    Owner::new().with(|| {
        let ui = Ui::new("dark".into());
        let mut setup = project("video-setup");
        setup.completed = false;
        setup.processed_ms = 0;
        ui.load(setup);
        assert_eq!(ui.route.get_untracked(), "Extract");
    });
}
