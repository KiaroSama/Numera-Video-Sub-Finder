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
