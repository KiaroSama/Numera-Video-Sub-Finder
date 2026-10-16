use nvsf_app::{Paths, media::MediaRuntime};
use nvsf_core::media_abi::{MediaHealth, MediaVersions, checked_rgb_len, validate_json};

#[test]
fn missing_native_media_is_a_recoverable_error_before_media_calls() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().to_path_buf();
    let paths = Paths {
        models: root.join("models"),
        projects: root.join("projects"),
        logs: root.join("logs"),
        runtimes: root.join("missing runtimes"),
        cache: root.join("cache"),
        root,
    };
    let error = match MediaRuntime::load(&paths) {
        Ok(_) => panic!("A missing media runtime must not load"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("Missing packaged runtime"));
    assert!(!paths.runtimes.exists());
}

#[test]
fn borrowed_rgb_and_json_boundaries_reject_invalid_sizes_before_copying() {
    assert_eq!(checked_rgb_len(2, 1, 6).unwrap(), 6);
    for (width, height, len) in [(0, 1, 0), (2, 1, 5), (2, 1, 7), (u32::MAX, u32::MAX, 1)] {
        assert!(checked_rgb_len(width, height, len).is_err());
    }
    assert!(validate_json(b"{}").is_ok());
    assert!(validate_json(b"").is_err());
    assert!(validate_json(&vec![b' '; 1_048_577]).is_err());
}

#[test]
fn health_rejects_incompatible_abi_layout_architecture_and_native_versions() {
    let health = MediaHealth {
        abi_version: 1,
        frame_size: std::mem::size_of::<nvsf_core::media_abi::MediaFrameV1>() as u32,
        pointer_width: usize::BITS,
        architecture: std::env::consts::ARCH.into(),
        plugin_version: env!("CARGO_PKG_VERSION").into(),
        source_sha: nvsf_core::BUILD_SOURCE_SHA.map(str::to_owned),
        versions: MediaVersions {
            avcodec: 63 << 16,
            avformat: 63 << 16,
            avutil: 61 << 16,
            swscale: 10 << 16,
        },
        compiled_majors: MediaVersions {
            avcodec: 63,
            avformat: 63,
            avutil: 61,
            swscale: 10,
        },
    };
    assert!(health.validate().is_ok());
    let mut wrong = health.clone();
    wrong.abi_version = 2;
    assert!(wrong.validate().is_err());
    wrong = health.clone();
    wrong.frame_size += 1;
    assert!(wrong.validate().is_err());
    wrong = health.clone();
    wrong.pointer_width = 16;
    assert!(wrong.validate().is_err());
    wrong = health.clone();
    wrong.architecture = "unknown".into();
    assert!(wrong.validate().is_err());
    wrong = health.clone();
    wrong.plugin_version = "0.0.0".into();
    assert!(wrong.validate().is_err());
    wrong = health.clone();
    wrong.versions.avcodec = 62 << 16;
    assert!(wrong.validate().is_err());
    wrong = health;
    wrong.versions.avutil = 0;
    assert!(wrong.validate().is_err());
}

#[cfg(unix)]
#[test]
fn native_media_symlink_cannot_escape_the_selected_runtime_root() {
    use nvsf_app::media::NATIVE_LIBRARY_NAME;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().to_path_buf();
    let runtimes = root.join("runtimes");
    std::fs::create_dir(&runtimes).unwrap();
    let outside = root.join("unowned-library");
    std::fs::write(&outside, b"not executable").unwrap();
    std::os::unix::fs::symlink(outside, runtimes.join(NATIVE_LIBRARY_NAME)).unwrap();
    let paths = Paths {
        models: root.join("models"),
        projects: root.join("projects"),
        logs: root.join("logs"),
        cache: root.join("cache"),
        root,
        runtimes,
    };
    let error = match MediaRuntime::load(&paths) {
        Ok(_) => panic!("An escaping runtime must not load"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("escapes package"));
}
