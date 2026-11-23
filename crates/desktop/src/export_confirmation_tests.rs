use super::project;
use crate::files;
use nvsf_core::{Cue, Project};
use std::path::{Path, PathBuf};

const OUTPUTS: [&str; 3] = ["review.srt", "review.track-0.srt", "review.track-1.srt"];

fn setup() -> (tempfile::TempDir, Project, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let selected = directory.path().join("selected");
    std::fs::create_dir(&selected).unwrap();
    let source = directory.path().join("media.mp4");
    std::fs::write(&source, b"original source media").unwrap();
    let output = selected.join(OUTPUTS[0]);
    let mut corrected = project();
    corrected.config.input = source.to_string_lossy().into_owned();
    corrected.config.output = output.to_string_lossy().into_owned();
    corrected.source_hash = nvsf_app::storage::hash_file(&source).unwrap();
    corrected.cues = vec![
        Cue::new(1, 0, 0, 1000, "Corrected English".into()).unwrap(),
        Cue::new(2, 1, 0, 1000, "Corrected 日本語".into()).unwrap(),
    ];
    (directory, corrected, output)
}

fn write_existing(directory: &Path, prefix: &str) {
    for name in OUTPUTS {
        std::fs::write(directory.join(name), format!("{prefix}: {name}")).unwrap();
    }
}

fn assert_existing(directory: &Path, prefix: &str) {
    for name in OUTPUTS {
        assert_eq!(
            std::fs::read_to_string(directory.join(name)).unwrap(),
            format!("{prefix}: {name}")
        );
    }
    assert_eq!(std::fs::read_dir(directory).unwrap().count(), 3);
}

fn assert_exported(directory: &Path) {
    for (name, expected) in
        OUTPUTS
            .into_iter()
            .zip(["Corrected English", "Corrected English", "Corrected 日本語"])
    {
        let text = std::fs::read_to_string(directory.join(name)).unwrap();
        assert!(text.contains(expected));
        assert!(text.starts_with('\u{feff}'));
        assert!(text.contains("\r\n"));
    }
    assert_eq!(std::fs::read_dir(directory).unwrap().count(), 3);
}

#[test]
fn review_export_retains_every_admitted_track_during_overwrite_confirmation() {
    let (directory, corrected, output) = setup();
    let selected = output.parent().unwrap();
    let admitted = directory.path().join("admitted");
    write_existing(selected, "approved previous output");
    let mut prompted = false;
    let result = files::export_project_to(&corrected, &output, "srt", true, true, || {
        prompted = true;
        std::fs::rename(selected, &admitted).unwrap();
        std::fs::create_dir(selected).unwrap();
        write_existing(selected, "unapproved replacement directory");
        Ok(true)
    })
    .unwrap();
    assert!(prompted);
    assert_eq!(result.as_deref(), Some("review.srt"));
    assert_exported(&admitted);
    assert_existing(selected, "unapproved replacement directory");
    assert_eq!(
        std::fs::read(&corrected.config.input).unwrap(),
        b"original source media"
    );
}

#[test]
fn review_export_denial_or_confirmation_error_preserves_every_admitted_track() {
    for confirmation_error in [false, true] {
        let (directory, corrected, output) = setup();
        let selected = output.parent().unwrap();
        let admitted = directory.path().join("admitted");
        write_existing(selected, "previous corrections");
        let mut prompted = false;
        let result = files::export_project_to(&corrected, &output, "srt", true, true, || {
            prompted = true;
            std::fs::rename(selected, &admitted).unwrap();
            std::fs::create_dir(selected).unwrap();
            write_existing(selected, "replacement directory");
            if confirmation_error {
                Err("confirmation failed after directory replacement".into())
            } else {
                Ok(false)
            }
        });
        assert!(prompted);
        if confirmation_error {
            assert_eq!(
                result.unwrap_err(),
                "confirmation failed after directory replacement"
            );
        } else {
            assert!(result.unwrap().is_none());
        }
        assert_existing(&admitted, "previous corrections");
        assert_existing(selected, "replacement directory");
    }
}

#[test]
fn review_export_confirms_existing_tracks_even_when_the_primary_is_absent() {
    for approve in [false, true] {
        let (_directory, corrected, output) = setup();
        let selected = output.parent().unwrap();
        let existing_track = selected.join(OUTPUTS[2]);
        std::fs::write(&existing_track, b"previous track corrections").unwrap();
        let mut prompted = false;
        let result = files::export_project_to(&corrected, &output, "srt", true, true, || {
            prompted = true;
            Ok(approve)
        })
        .unwrap();
        assert!(prompted, "Consent covers the entire actual output set");
        if approve {
            assert_eq!(result.as_deref(), Some("review.srt"));
            assert_exported(selected);
        } else {
            assert!(result.is_none());
            assert!(!output.exists());
            assert_eq!(
                std::fs::read(&existing_track).unwrap(),
                b"previous track corrections"
            );
            assert_eq!(std::fs::read_dir(selected).unwrap().count(), 1);
        }
    }
}

#[test]
fn review_export_creates_an_absent_output_set_without_overwrite_confirmation() {
    let (_directory, corrected, output) = setup();
    assert_eq!(
        files::export_project_to(&corrected, &output, "srt", true, true, || {
            panic!("An entirely absent output set needs no replacement consent")
        })
        .unwrap()
        .as_deref(),
        Some("review.srt")
    );
    assert_exported(output.parent().unwrap());
}

#[test]
fn review_export_preserves_a_late_competitor_at_an_initially_absent_track() {
    let (_directory, corrected, output) = setup();
    let selected = output.parent().unwrap();
    let first_track = selected.join(OUTPUTS[1]);
    let last_track = selected.join(OUTPUTS[2]);
    std::fs::write(&output, b"previous primary corrections").unwrap();
    std::fs::write(&last_track, b"previous last-track corrections").unwrap();
    let mut prompted = false;
    let result = files::export_project_to(&corrected, &output, "srt", true, true, || {
        prompted = true;
        std::fs::write(&first_track, b"late competing track").unwrap();
        Ok(true)
    });
    assert!(prompted);
    assert!(result.is_err());
    assert_eq!(
        std::fs::read(&first_track).unwrap(),
        b"late competing track"
    );
    assert_eq!(
        std::fs::read(&output).unwrap(),
        b"previous primary corrections"
    );
    assert_eq!(
        std::fs::read(&last_track).unwrap(),
        b"previous last-track corrections"
    );
    assert_eq!(std::fs::read_dir(selected).unwrap().count(), 3);
}

#[test]
fn review_export_rejects_a_source_track_before_confirmation_or_any_write() {
    let (_directory, mut corrected, output) = setup();
    let selected = output.parent().unwrap();
    write_existing(selected, "preserved output");
    let source = selected.join(OUTPUTS[2]);
    corrected.config.input = source.to_string_lossy().into_owned();
    corrected.source_hash = nvsf_app::storage::hash_file(&source).unwrap();
    let error = files::export_project_to(&corrected, &output, "srt", true, true, || {
        panic!("Every source-media collision must fail before confirmation")
    })
    .unwrap_err();
    assert!(error.contains("Destination resolves to source media"));
    assert_existing(selected, "preserved output");
}
