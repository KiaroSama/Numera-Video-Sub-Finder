use super::*;
use std::io::Write;

#[test]
fn native_publication_stages_preserve_parent_binding_and_no_clobber() {
    assert_eq!(std::mem::size_of::<RenameInfo>(), 24, "Win64 rename layout");
    assert_eq!(std::mem::offset_of!(RenameInfo, root_directory), 8);
    assert_eq!(std::mem::offset_of!(RenameInfo, file_name_length), 16);
    assert_eq!(std::mem::offset_of!(RenameInfo, file_name), 20);

    let directory = tempfile::tempdir().unwrap();
    let selected = directory.path().join("selected");
    let admitted = directory.path().join("admitted");
    std::fs::create_dir(&selected).unwrap();
    let parent = open_directory(&selected).expect("open retained parent stage");
    std::fs::rename(&selected, &admitted).expect("substitute ancestor stage");
    std::fs::create_dir(&selected).unwrap();
    let name = OsStr::new("日本 retained.txt");
    std::fs::write(selected.join(name), b"unrelated replacement parent").unwrap();

    let first_name = OsStr::new(".stage-first.tmp");
    let mut first = create_file(&parent, first_name).expect("first native temporary create stage");
    first
        .write_all(b"first publication")
        .expect("first temporary write stage");
    first.sync_all().expect("first temporary sync stage");
    publish(&parent, &first, first_name, name, false)
        .expect("first directory-relative publication stage after successful create/write/sync");
    drop(first);
    assert_eq!(
        std::fs::read(admitted.join(name)).unwrap(),
        b"first publication"
    );
    assert!(!admitted.join(first_name).exists());

    let second_name = OsStr::new(".stage-second.tmp");
    let mut second =
        create_file(&parent, second_name).expect("second native temporary create stage");
    second
        .write_all(b"explicit replacement")
        .expect("second temporary write stage");
    second.sync_all().expect("second temporary sync stage");
    let error = publish(&parent, &second, second_name, name, false)
        .expect_err("no-clobber publication must reject the existing destination");
    assert_eq!(
        error.kind(),
        io::ErrorKind::AlreadyExists,
        "specific no-clobber result: {error}"
    );
    assert_eq!(
        std::fs::read(admitted.join(name)).unwrap(),
        b"first publication"
    );
    publish(&parent, &second, second_name, name, true)
        .expect("explicit replacement publication stage");
    drop(second);
    assert_eq!(
        std::fs::read(admitted.join(name)).unwrap(),
        b"explicit replacement"
    );
    assert!(!admitted.join(second_name).exists());
    assert_eq!(
        std::fs::read(selected.join(name)).unwrap(),
        b"unrelated replacement parent"
    );
    assert_eq!(std::fs::read_dir(admitted).unwrap().count(), 1);
    assert_eq!(std::fs::read_dir(selected).unwrap().count(), 1);
}
