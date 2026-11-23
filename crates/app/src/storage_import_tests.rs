use super::*;
use std::io::Seek;

#[test]
fn staged_import_is_readable_and_callback_failure_cleans_the_original_directory() {
    let directory = tempfile::tempdir().unwrap();
    let selected = directory.path().join("selected");
    let admitted = directory.path().join("admitted");
    std::fs::create_dir(&selected).unwrap();
    std::fs::write(selected.join("pack.bin"), b"previous pack").unwrap();
    let output = AtomicOutput::new(&selected.join("pack.bin"), true).unwrap();
    std::fs::rename(&selected, &admitted).unwrap();
    std::fs::create_dir(&selected).unwrap();
    std::fs::write(selected.join("pack.bin"), b"unrelated replacement root").unwrap();
    let result = output.write_with(|file| {
        file.write_all(b"staged pack")?;
        file.sync_all()?;
        file.rewind()?;
        let mut verified = Vec::new();
        file.read_to_end(&mut verified)?;
        assert_eq!(verified, b"staged pack");
        Err(fail("injected verification failure"))
    });
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("injected verification failure")
    );
    assert_eq!(
        std::fs::read(admitted.join("pack.bin")).unwrap(),
        b"previous pack"
    );
    assert_eq!(
        std::fs::read(selected.join("pack.bin")).unwrap(),
        b"unrelated replacement root"
    );
    assert_eq!(std::fs::read_dir(admitted).unwrap().count(), 1);
    assert_eq!(std::fs::read_dir(selected).unwrap().count(), 1);
}

#[test]
fn bound_import_lock_keeps_its_parent_and_releases_without_path_cleanup() {
    let directory = tempfile::tempdir().unwrap();
    let selected = directory.path().join("selected");
    let admitted = directory.path().join("admitted");
    std::fs::create_dir(&selected).unwrap();
    let output = AtomicOutput::new(&selected.join("pack.bin"), true).unwrap();
    let guard = output
        .lock_sibling(OsStr::new("pack.download.lock"))
        .unwrap();
    assert!(
        output
            .lock_sibling(OsStr::new("pack.download.lock"))
            .is_err()
    );
    std::fs::rename(&selected, &admitted).unwrap();
    std::fs::create_dir(&selected).unwrap();
    std::fs::write(selected.join("pack.download.lock"), b"unrelated lock bytes").unwrap();
    assert!(
        output
            .lock_sibling(OsStr::new("pack.download.lock"))
            .is_err()
    );
    drop(guard);
    assert!(
        output
            .lock_sibling(OsStr::new("pack.download.lock"))
            .is_ok()
    );
    assert_eq!(
        std::fs::read(selected.join("pack.download.lock")).unwrap(),
        b"unrelated lock bytes"
    );
    assert_eq!(std::fs::read_dir(admitted).unwrap().count(), 1);
}
