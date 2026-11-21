use nvsf_app::{AppError, Paths, media::MediaRuntime};
use nvsf_core::media_abi::{
    ABI_VERSION, MAX_JSON_BYTES, MediaFrameV1, MediaHealth, MediaVersions, checked_rgb_len,
    validate_json,
};

#[test]
fn corrupt_truncated_and_empty_media_files_fail_safely_without_runtime_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let mut missing_paths = Paths::new(Some(dir.path())).unwrap();
    missing_paths.runtimes = dir.path().join("missing-runtimes");
    let missing_err = match MediaRuntime::load(&missing_paths) {
        Ok(_) => panic!("explicit missing runtime root must not load"),
        Err(err) => err,
    };
    assert!(matches!(missing_err, AppError::Message(_)));

    let paths = Paths::new(Some(dir.path())).unwrap();
    let loaded = MediaRuntime::load(&paths);
    for (name, bytes) in [
        ("empty.mp4", &[][..]),
        (
            "truncated-moov.mp4",
            b"\x00\x00\x00\x18ftypisom\x00\x00\x02\x00",
        ),
        (
            "corrupt-mkv.mkv",
            b"\x1a\x45\xdf\xa3\xff\xff\x00\x00garbage",
        ),
    ] {
        let file = dir.path().join(name);
        std::fs::write(&file, bytes).unwrap();
        if let Ok(runtime) = &loaded {
            assert!(
                runtime.probe(&file).is_err(),
                "corrupt media probe must fail for {name}"
            );
            assert!(
                runtime.decode(&file, 0, None, |_| Ok(true)).is_err(),
                "corrupt media decode must fail for {name}"
            );
        }
        assert_eq!(std::fs::read(&file).unwrap(), bytes);
    }
}

#[test]
fn media_abi_contracts_guard_stride_overflow_health_and_malformed_json() {
    let expected = 1920 * 1080 * 3;
    assert_eq!(checked_rgb_len(1920, 1080, expected).unwrap(), expected);
    assert!(checked_rgb_len(0, 1080, 0).is_err());
    assert!(checked_rgb_len(1920, 1080, expected - 1).is_err());
    assert!(checked_rgb_len(u32::MAX, u32::MAX, 0).is_err());

    let health = MediaHealth {
        abi_version: ABI_VERSION,
        frame_size: u32::try_from(std::mem::size_of::<MediaFrameV1>()).unwrap(),
        pointer_width: usize::BITS,
        architecture: std::env::consts::ARCH.into(),
        plugin_version: env!("CARGO_PKG_VERSION").into(),
        source_sha: nvsf_core::BUILD_SOURCE_SHA.map(str::to_owned),
        versions: MediaVersions {
            avcodec: 62 << 16,
            avformat: 62 << 16,
            avutil: 60 << 16,
            swscale: 9 << 16,
        },
        compiled_majors: MediaVersions {
            avcodec: 62,
            avformat: 62,
            avutil: 60,
            swscale: 9,
        },
    };
    assert!(health.validate().is_ok());
    let mut bad_abi = health.clone();
    bad_abi.abi_version = ABI_VERSION + 1;
    assert!(bad_abi.validate().is_err());

    let encoded = serde_json::to_vec(&health).unwrap();
    assert!(validate_json(&encoded).is_ok());
    assert!(validate_json(b"").is_err());
    assert!(validate_json(&[0xff, 0xfe]).is_err());
    assert!(validate_json(&vec![b' '; MAX_JSON_BYTES + 1]).is_err());
}
