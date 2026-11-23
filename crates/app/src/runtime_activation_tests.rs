use super::{activate, activate_with_barrier, sync_directory};
use crate::{
    runtimes::{Entry, Pack},
    storage::write_atomic,
};
use std::path::Path;

fn staged_pack(root: &Path) -> (Pack, Vec<u8>, Vec<u8>) {
    let files = [
        crate::media::NATIVE_LIBRARY_NAME,
        crate::runtimes::TESSERACT_LIBRARY,
        crate::runtimes::ONNX_LIBRARY,
        "runtime-notices.tar.gz",
    ];
    let pack = Pack {
        schema: 1,
        version: env!("CARGO_PKG_VERSION").into(),
        target: crate::runtimes::target(),
        source_sha: nvsf_core::BUILD_SOURCE_SHA
            .unwrap_or("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .into(),
        files: files
            .into_iter()
            .map(|name| {
                std::fs::write(root.join(name), b"fixture").unwrap();
                Entry {
                    file: name.into(),
                    bytes: 7,
                    sha256: "f16d05ec6b29248d2c61adb1e9263f78e4f7bace1b955014a2d17872cfe4064d"
                        .into(),
                }
            })
            .collect(),
    };
    // This fixture exercises promotion after authentication; it never loads these inert bytes.
    let manifest = serde_json::to_vec(&pack).unwrap();
    let signature = b"already-authenticated signature fixture".to_vec();
    std::fs::write(root.join("runtime-pack.json"), &manifest).unwrap();
    std::fs::write(root.join("runtime-pack.sig"), &signature).unwrap();
    (pack, manifest, signature)
}

#[test]
fn a_corrupt_managed_directory_is_repaired_under_the_same_manifest_identity() {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir_in(root.path()).unwrap();
    let (pack, manifest, signature) = staged_pack(stage.path());
    let destination = root.path().join("selected-runtime");
    std::fs::create_dir(&destination).unwrap();
    std::fs::write(
        destination.join(crate::media::NATIVE_LIBRARY_NAME),
        b"corrupted",
    )
    .unwrap();
    let pointer = root.path().join("runtime-selection.json");
    std::fs::write(&pointer, b"previous selection").unwrap();
    activate(
        &pack,
        &manifest,
        &signature,
        stage.path(),
        &destination,
        &pointer,
        || write_atomic(&pointer, b"repaired selection", true),
    )
    .unwrap();
    pack.verify(&destination).unwrap();
    assert_eq!(std::fs::read(pointer).unwrap(), b"repaired selection");
    assert!(!stage.path().exists());
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 2);
}

#[test]
fn failed_selection_publication_restores_the_previous_directory_and_staged_pack() {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir_in(root.path()).unwrap();
    let (pack, manifest, signature) = staged_pack(stage.path());
    let destination = root.path().join("selected-runtime");
    std::fs::create_dir(&destination).unwrap();
    std::fs::write(destination.join("retained.bin"), b"previous runtime bytes").unwrap();
    let pointer = root.path().join("runtime-selection.json");
    std::fs::create_dir(&pointer).unwrap();
    assert!(
        activate(
            &pack,
            &manifest,
            &signature,
            stage.path(),
            &destination,
            &pointer,
            || { write_atomic(&pointer, b"cannot replace directory", true) }
        )
        .is_err()
    );
    assert_eq!(
        std::fs::read(destination.join("retained.bin")).unwrap(),
        b"previous runtime bytes"
    );
    pack.verify(stage.path()).unwrap();
    assert_eq!(std::fs::read_dir(&destination).unwrap().count(), 1);
    assert!(pointer.is_dir());
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 3);
}

#[test]
fn a_verified_existing_pack_is_reused_without_replacing_its_directory() {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir_in(root.path()).unwrap();
    let (pack, manifest, signature) = staged_pack(stage.path());
    let destination = root.path().join("selected-runtime");
    std::fs::create_dir(&destination).unwrap();
    staged_pack(&destination);
    let original = destination.join(crate::media::NATIVE_LIBRARY_NAME);
    let retained_link = root.path().join("original-inode");
    let pointer = root.path().join("runtime-selection.json");
    std::fs::hard_link(&original, &retained_link).unwrap();
    activate(
        &pack,
        &manifest,
        &signature,
        stage.path(),
        &destination,
        &pointer,
        || Ok(()),
    )
    .unwrap();
    std::fs::write(retained_link, b"same inode").unwrap();
    assert_eq!(std::fs::read(original).unwrap(), b"same inode");
    pack.verify(stage.path()).unwrap();
}

#[test]
fn failed_stage_promotion_restores_the_previous_runtime_before_returning() {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir_in(root.path()).unwrap();
    let (pack, manifest, signature) = staged_pack(stage.path());
    let stage_path = stage.path().to_path_buf();
    stage.close().unwrap();
    let destination = root.path().join("selected-runtime");
    std::fs::create_dir(&destination).unwrap();
    std::fs::write(destination.join("retained.bin"), b"previous runtime bytes").unwrap();
    let pointer = root.path().join("runtime-selection.json");
    assert!(
        activate(
            &pack,
            &manifest,
            &signature,
            &stage_path,
            &destination,
            &pointer,
            || { panic!("A failed promotion must not publish a selection") }
        )
        .is_err()
    );
    assert_eq!(
        std::fs::read(destination.join("retained.bin")).unwrap(),
        b"previous runtime bytes"
    );
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn a_failure_after_pointer_persistence_restores_selection_before_runtime_rollback() {
    for had_destination in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let stage = tempfile::tempdir_in(root.path()).unwrap();
        let (pack, manifest, signature) = staged_pack(stage.path());
        let previous_runtime = root.path().join("previous-runtime");
        std::fs::create_dir(&previous_runtime).unwrap();
        staged_pack(&previous_runtime);
        let destination = root.path().join("replacement-runtime");
        if had_destination {
            std::fs::create_dir(&destination).unwrap();
            std::fs::write(
                destination.join("retained.bin"),
                b"previous damaged runtime",
            )
            .unwrap();
        }
        let pointer = root.path().join("runtime-selection.json");
        std::fs::write(&pointer, b"\"previous-runtime\"").unwrap();
        let error = activate(
            &pack,
            &manifest,
            &signature,
            stage.path(),
            &destination,
            &pointer,
            || {
                write_atomic(&pointer, b"\"replacement-runtime\"", true)?;
                Err(crate::fail(
                    "Injected durability failure after pointer persistence",
                ))
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("after pointer persistence"));
        assert_eq!(std::fs::read(&pointer).unwrap(), b"\"previous-runtime\"");
        pack.verify(&previous_runtime).unwrap();
        pack.verify(stage.path()).unwrap();
        if had_destination {
            assert_eq!(
                std::fs::read(destination.join("retained.bin")).unwrap(),
                b"previous damaged runtime"
            );
        } else {
            assert!(!destination.exists());
        }
    }
}

#[test]
fn promoted_runtime_parent_is_synced_before_selection_publication() {
    let root = tempfile::tempdir().unwrap();
    let runtimes = root.path().join("runtimes");
    std::fs::create_dir(&runtimes).unwrap();
    let stage = tempfile::tempdir_in(&runtimes).unwrap();
    let (pack, manifest, signature) = staged_pack(stage.path());
    let destination = runtimes.join("selected-runtime");
    std::fs::create_dir(&destination).unwrap();
    std::fs::write(destination.join("retained.bin"), b"previous runtime bytes").unwrap();
    let pointer = root.path().join("runtime-selection.json");
    std::fs::write(&pointer, b"previous selection").unwrap();
    let order = std::cell::RefCell::new(Vec::new());
    activate_with_barrier(
        &pack,
        &manifest,
        &signature,
        stage.path(),
        &destination,
        &pointer,
        (
            |parent| {
                assert_eq!(parent, runtimes);
                assert_ne!(parent, pointer.parent().unwrap());
                pack.verify(&destination).unwrap();
                assert!(!stage.path().exists());
                assert_eq!(std::fs::read(&pointer).unwrap(), b"previous selection");
                sync_directory(parent)?;
                order.borrow_mut().push("runtime parent synced");
                Ok(())
            },
            || {
                assert_eq!(*order.borrow(), ["runtime parent synced"]);
                write_atomic(&pointer, b"repaired selection", true)?;
                order.borrow_mut().push("selection published");
                Ok(())
            },
        ),
    )
    .unwrap();
    assert_eq!(
        *order.borrow(),
        ["runtime parent synced", "selection published"]
    );
    pack.verify(&destination).unwrap();
    assert_eq!(std::fs::read(pointer).unwrap(), b"repaired selection");
    assert_eq!(std::fs::read_dir(runtimes).unwrap().count(), 1);
}

#[test]
fn a_runtime_parent_sync_failure_rolls_back_before_publishing_selection() {
    for had_destination in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let runtimes = root.path().join("runtimes");
        std::fs::create_dir(&runtimes).unwrap();
        let stage = tempfile::tempdir_in(&runtimes).unwrap();
        let (pack, manifest, signature) = staged_pack(stage.path());
        let destination = runtimes.join("selected-runtime");
        if had_destination {
            std::fs::create_dir(&destination).unwrap();
            std::fs::write(destination.join("retained.bin"), b"previous runtime bytes").unwrap();
        }
        let pointer = root.path().join("runtime-selection.json");
        std::fs::write(&pointer, b"previous selection").unwrap();
        let error = activate_with_barrier(
            &pack,
            &manifest,
            &signature,
            stage.path(),
            &destination,
            &pointer,
            (
                |parent| {
                    assert_eq!(parent, runtimes);
                    pack.verify(&destination).unwrap();
                    sync_directory(parent)?;
                    Err(crate::fail("Injected runtime parent durability failure"))
                },
                || panic!("A failed runtime directory sync must not publish selection"),
            ),
        )
        .unwrap_err();
        assert!(error.to_string().contains("runtime parent durability"));
        assert_eq!(std::fs::read(&pointer).unwrap(), b"previous selection");
        pack.verify(stage.path()).unwrap();
        if had_destination {
            assert_eq!(
                std::fs::read(destination.join("retained.bin")).unwrap(),
                b"previous runtime bytes"
            );
            assert_eq!(std::fs::read_dir(destination).unwrap().count(), 1);
        } else {
            assert!(!destination.exists());
        }
        assert_eq!(
            std::fs::read_dir(&runtimes).unwrap().count(),
            usize::from(had_destination) + 1
        );
    }
}
