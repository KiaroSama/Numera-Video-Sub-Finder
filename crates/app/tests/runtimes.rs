use nvsf_app::{Paths, runtimes};
use std::sync::atomic::AtomicBool;

#[test]
fn repair_denial_and_unsigned_inventory_cannot_activate_native_code() {
    let directory = tempfile::tempdir().unwrap();
    let paths = Paths::new(Some(directory.path())).unwrap();
    assert!(
        runtimes::repair(
            &paths,
            b"{}",
            b"bad signature",
            "https://invalid.example",
            false,
            &AtomicBool::new(false)
        )
        .is_err()
    );
    assert!(runtimes::authenticate(b"{}", b"bad signature").is_err());
    assert!(!paths.root.join("runtime-selection.json").exists());
    assert!(runtimes::managed(&paths).unwrap().is_none());
    std::fs::write(paths.root.join("runtime-selection.json"), b"\"../escape\"").unwrap();
    assert!(runtimes::managed(&paths).is_err());
    assert!(
        Paths::new(Some(directory.path())).is_ok(),
        "corrupt managed selection must not prevent repair UI startup"
    );
}

#[test]
fn explicit_runtime_paths_are_preserved_and_native_pack_requires_all_engines() {
    let directory = tempfile::tempdir().unwrap();
    let paths = Paths::new(Some(directory.path())).unwrap();
    let mut selected = paths.clone();
    selected.runtimes = directory.path().join("explicit runtime root");
    let pack = runtimes::Pack {
        schema: 1,
        version: env!("CARGO_PKG_VERSION").into(),
        target: runtimes::target(),
        source_sha: nvsf_core::BUILD_SOURCE_SHA
            .unwrap_or("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .into(),
        files: vec![runtimes::Entry {
            file: nvsf_app::media::NATIVE_LIBRARY_NAME.into(),
            bytes: 1,
            sha256: "b".repeat(64),
        }],
    };
    assert!(
        pack.validate().is_err(),
        "media-only inventory must not claim complete OCR repair"
    );
    assert!(
        runtimes::diagnose(&selected)
            .iter()
            .all(|row| row.state != "verified")
    );
    assert!(!selected.runtimes.exists());
}

#[cfg(unix)]
#[test]
fn managed_runtime_inventory_requires_a_private_executable_helper() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let names = [
        nvsf_app::media::NATIVE_LIBRARY_NAME,
        runtimes::TESSERACT_LIBRARY,
        runtimes::ONNX_LIBRARY,
        "runtime-notices.tar.gz",
        "llama-mtmd-cli",
    ];
    let pack = runtimes::Pack {
        schema: 1,
        version: env!("CARGO_PKG_VERSION").into(),
        target: runtimes::target(),
        source_sha: nvsf_core::BUILD_SOURCE_SHA
            .unwrap_or("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .into(),
        files: names
            .iter()
            .map(|name| {
                let path = directory.path().join(name);
                std::fs::write(&path, b"fixture").unwrap();
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
                runtimes::Entry {
                    file: (*name).into(),
                    bytes: 7,
                    sha256: nvsf_app::storage::hash_file(&path).unwrap(),
                }
            })
            .collect(),
    };
    assert!(
        pack.verify(directory.path()).is_err(),
        "non-executable helper admitted"
    );
    let helper = directory.path().join("llama-mtmd-cli");
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
    pack.verify(directory.path()).unwrap();
    std::fs::set_permissions(helper, std::fs::Permissions::from_mode(0o777)).unwrap();
    assert!(
        pack.verify(directory.path()).is_err(),
        "shared-writable helper admitted"
    );
}

#[test]
fn managed_selection_rejects_oversized_malformed_and_escaping_pointers() {
    let directory = tempfile::tempdir().unwrap();
    let paths = Paths::new(Some(&directory.path().join("data"))).unwrap();
    let pointer = paths.root.join("runtime-selection.json");

    std::fs::write(&pointer, serde_json::to_vec(&"a".repeat(300)).unwrap()).unwrap();
    assert!(runtimes::managed(&paths).is_err());

    std::fs::write(&pointer, br#""not-a-hex-identity""#).unwrap();
    assert!(runtimes::managed(&paths).is_err());

    let valid = "a".repeat(64);
    std::fs::write(&pointer, serde_json::to_vec(&valid).unwrap()).unwrap();
    assert!(
        runtimes::managed(&paths).is_err(),
        "no pack directory exists"
    );

    std::fs::create_dir_all(paths.root.join("runtimes").join(&valid)).unwrap();
    assert!(
        runtimes::managed(&paths).is_err(),
        "no signed inventory exists"
    );
    assert!(
        !paths
            .root
            .join("runtimes")
            .join(&valid)
            .join("runtime-pack.json")
            .exists()
    );
}

#[test]
fn runtime_inventory_rejects_unsafe_entries_and_preserves_verified_files() {
    let directory = tempfile::tempdir().unwrap();
    let names = [
        nvsf_app::media::NATIVE_LIBRARY_NAME,
        runtimes::TESSERACT_LIBRARY,
        runtimes::ONNX_LIBRARY,
        "runtime-notices.tar.gz",
    ];
    let pack = runtimes::Pack {
        schema: 1,
        version: env!("CARGO_PKG_VERSION").into(),
        target: runtimes::target(),
        source_sha: nvsf_core::BUILD_SOURCE_SHA
            .unwrap_or("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .into(),
        files: names
            .iter()
            .map(|name| {
                let path = directory.path().join(name);
                std::fs::write(&path, b"fixture").unwrap();
                runtimes::Entry {
                    file: (*name).into(),
                    bytes: 7,
                    sha256: "f16d05ec6b29248d2c61adb1e9263f78e4f7bace1b955014a2d17872cfe4064d"
                        .into(),
                }
            })
            .collect(),
    };
    pack.validate().unwrap();
    pack.verify(directory.path()).unwrap();
    for name in [
        "",
        ".hidden",
        "../outside",
        "nested/file",
        "nested\\file",
        "C:drive",
        "two..dots",
        "trailing.",
        "space name",
        "é.dll",
    ] {
        let mut invalid = pack.clone();
        invalid.files[0].file = name.into();
        assert_eq!(
            invalid.validate().unwrap_err().to_string(),
            "Operation failed: Unsafe or invalid runtime inventory entry",
            "{name}"
        );
    }
    for (bytes, hash) in [
        (0, "a".repeat(64)),
        (512 * 1024 * 1024 + 1, "a".repeat(64)),
        (7, "a".repeat(63)),
        (7, "z".repeat(64)),
    ] {
        let mut invalid = pack.clone();
        invalid.files[0].bytes = bytes;
        invalid.files[0].sha256 = hash;
        assert_eq!(
            invalid.validate().unwrap_err().to_string(),
            "Operation failed: Unsafe or invalid runtime inventory entry"
        );
    }
    let mut duplicate = pack.clone();
    duplicate.files.push(pack.files[0].clone());
    assert_eq!(
        duplicate.validate().unwrap_err().to_string(),
        "Operation failed: Unsafe or invalid runtime inventory entry"
    );
    for field in ["schema", "version", "target", "source"] {
        let mut invalid = pack.clone();
        match field {
            "schema" => invalid.schema = 2,
            "version" => invalid.version = "999.0.0".into(),
            "target" => invalid.target = "wrong-target".into(),
            _ => invalid.source_sha = "not-a-source-sha".into(),
        }
        assert_eq!(
            invalid.validate().unwrap_err().to_string(),
            "Operation failed: Runtime pack version, target or inventory is incompatible"
        );
    }
    let extra = directory.path().join("uninventoried.txt");
    std::fs::write(&extra, b"preserve extra").unwrap();
    assert_eq!(
        pack.verify(directory.path()).unwrap_err().to_string(),
        "Operation failed: Uninventoried file in native runtime closure"
    );
    assert_eq!(std::fs::read(&extra).unwrap(), b"preserve extra");
    std::fs::remove_file(&extra).unwrap();
    let native = directory.path().join(names[0]);
    std::fs::write(&native, b"changed").unwrap();
    assert_eq!(
        pack.verify(directory.path()).unwrap_err().to_string(),
        format!("Operation failed: Runtime integrity failed: {}", names[0])
    );
    assert_eq!(std::fs::read(native).unwrap(), b"changed");
    for name in &names[1..] {
        assert_eq!(
            std::fs::read(directory.path().join(name)).unwrap(),
            b"fixture"
        );
    }
}

#[test]
fn signature_authentication_bounds_inputs_before_verification() {
    for (manifest, signature) in [
        (vec![b' '; 1024 * 1024 + 1], b"bad signature".to_vec()),
        (b"{}".to_vec(), vec![b' '; 4097]),
    ] {
        let error = runtimes::authenticate(&manifest, &signature).unwrap_err();
        assert!(matches!(error, nvsf_app::AppError::Message(message)
            if message == "Runtime signature or manifest exceeds limits"));
    }
}
