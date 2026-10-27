use nvsf_app::{Paths, media::MediaRuntime};
use std::{io::Write, path::Path};

#[path = "support/media_sanitizer_admission.rs"]
mod sanitizer_admission;

fn runtime(root: &Path) -> MediaRuntime {
    let paths = Paths {
        root: root.into(),
        models: root.join("models"),
        projects: root.join("projects"),
        logs: root.join("logs"),
        cache: root.join("cache"),
        runtimes: sanitizer_admission::selected_root().unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.ci-work/runtimes")
        }),
    };
    let runtime = MediaRuntime::load(&paths)
        .expect("hosted native media adapter must be staged before contract suites");
    sanitizer_admission::verify_loaded(&paths.runtimes);
    runtime
}

fn anamorphic_fixture(path: &Path) {
    let mut file = std::fs::File::create(path).unwrap();
    // Lossless, self-contained source bytes: 4x2 coded pixels displayed as8x2, three25fps frames.
    file.write_all(b"YUV4MPEG2 W4 H2 F25:1 Ip A2:1 C444\n")
        .unwrap();
    for luma in [16, 128, 235] {
        file.write_all(b"FRAME\n").unwrap();
        file.write_all(&[luma; 8]).unwrap();
        file.write_all(&[128; 16]).unwrap();
    }
}

#[test]
fn native_probe_decode_and_seek_share_display_aspect_and_source_pts_in_unicode_paths() {
    let directory = tempfile::tempdir().unwrap();
    let video = directory.path().join("display geometry 日本語.y4m");
    anamorphic_fixture(&video);
    let original = std::fs::read(&video).unwrap();
    let runtime = runtime(directory.path());
    let info = runtime.probe(&video).unwrap();
    assert_eq!((info.width, info.height), (8, 2));
    let mut frames = Vec::new();
    runtime
        .decode(&video, 0, None, |frame| {
            assert_eq!(frame.image.dimensions(), (info.width, info.height));
            assert_eq!(frame.estimated_duration_ms, 40);
            frames.push((frame.time_ms, frame.image.get_pixel(0, 0)[0]));
            Ok(true)
        })
        .unwrap();
    assert_eq!(frames.iter().map(|f| f.0).collect::<Vec<_>>(), [0, 40, 80]);
    assert!(frames[0].1 < 10 && frames[2].1 > 240, "{frames:?}");
    assert!(frames[1].1 > frames[0].1 && frames[1].1 < frames[2].1);
    let mut ranged = Vec::new();
    runtime
        .decode(&video, 40, Some(80), |frame| {
            ranged.push(frame.time_ms);
            Ok(true)
        })
        .unwrap();
    assert_eq!(ranged, [40]);
    assert_eq!(runtime.preview(&video, 40).unwrap().dimensions(), (8, 2));
    assert_eq!(std::fs::read(&video).unwrap(), original);
}

#[test]
fn native_consumer_stop_and_malformed_input_preserve_source_and_allow_reopening() {
    let directory = tempfile::tempdir().unwrap();
    let video = directory.path().join("cancel fixture.y4m");
    anamorphic_fixture(&video);
    let runtime = runtime(directory.path());
    let mut delivered = 0;
    runtime
        .decode(&video, 0, None, |_| {
            delivered += 1;
            Ok(false)
        })
        .unwrap();
    assert_eq!(delivered, 1);
    assert_eq!(runtime.preview(&video, 80).unwrap().dimensions(), (8, 2));
    let malformed = directory.path().join("invalid media.bin");
    std::fs::write(&malformed, b"not a video").unwrap();
    assert!(runtime.probe(&malformed).is_err());
    assert!(runtime.decode(&malformed, 0, None, |_| Ok(true)).is_err());
    assert_eq!(std::fs::read(&malformed).unwrap(), b"not a video");
    assert!(runtime.decode(&video, 80, Some(40), |_| Ok(true)).is_err());
}
