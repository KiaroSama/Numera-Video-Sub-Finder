use std::{
    ffi::{CString, OsStr, c_char, c_int},
    fs::{File, OpenOptions},
    io,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        },
    },
    path::Path,
};

const O_CLOEXEC: c_int = 0x80000;
const O_NONBLOCK: c_int = 0x800;
const O_PATH: c_int = 0x200000;
#[cfg(target_arch = "x86_64")]
const O_NOFOLLOW: c_int = 0x20000;
#[cfg(target_arch = "aarch64")]
const O_NOFOLLOW: c_int = 0x8000;
#[cfg(target_arch = "x86_64")]
const O_DIRECTORY: c_int = 0x10000;
#[cfg(target_arch = "aarch64")]
const O_DIRECTORY: c_int = 0x4000;

unsafe extern "C" {
    fn openat(directory: c_int, path: *const c_char, flags: c_int, ...) -> c_int;
    fn renameat(old_dir: c_int, old: *const c_char, new_dir: c_int, new: *const c_char) -> c_int;
    fn renameat2(
        old_dir: c_int,
        old: *const c_char,
        new_dir: c_int,
        new: *const c_char,
        flags: u32,
    ) -> c_int;
    fn unlinkat(directory: c_int, path: *const c_char, flags: c_int) -> c_int;
    fn linkat(
        old_dir: c_int,
        old: *const c_char,
        new_dir: c_int,
        new: *const c_char,
        flags: c_int,
    ) -> c_int;
}

pub(super) fn validate_name(name: &OsStr) -> io::Result<()> {
    c_name(name).map(|_| ())
}

fn c_name(name: &OsStr) -> io::Result<CString> {
    if name.as_bytes().contains(&b'/') || name == "." || name == ".." || name.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Expected one output filename",
        ));
    }
    CString::new(name.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in output filename"))
}

pub(super) fn open_directory(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(O_DIRECTORY)
        .open(path)
}

fn open_relative(directory: &File, name: &OsStr, flags: c_int) -> io::Result<File> {
    let name = c_name(name)?;
    // SAFETY: the directory descriptor is retained by the caller; the single-component
    // filename is NUL-terminated and the returned descriptor acquires exactly one owner.
    let fd = unsafe {
        openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            flags | O_CLOEXEC | O_NOFOLLOW,
            0o600u32,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful openat returned a new descriptor owned by this File.
    Ok(unsafe { File::from_raw_fd(fd) })
}

pub(super) fn open_file(directory: &File, name: &OsStr) -> io::Result<File> {
    open_relative(directory, name, O_NONBLOCK)
}

pub(super) fn open_source_file(directory: &File, name: &OsStr) -> io::Result<Option<File>> {
    let inspection = open_relative(directory, name, O_PATH)?;
    let metadata = inspection.metadata()?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Source path changed to a symbolic link",
        ));
    }
    if !metadata.is_file() {
        return Ok(None);
    }
    drop(inspection);
    open_file(directory, name).map(Some)
}

pub(super) fn create_file(directory: &File, name: &OsStr) -> io::Result<File> {
    open_relative(directory, name, 1 | 0x40 | 0x80) // O_WRONLY | O_CREAT | O_EXCL
}

pub(super) fn publish(
    directory: &File,
    _file: &File,
    old: &OsStr,
    new: &OsStr,
    overwrite: bool,
) -> io::Result<()> {
    let old = c_name(old)?;
    let new = c_name(new)?;
    let fd = directory.as_raw_fd();
    if overwrite {
        // SAFETY: both single-component names use this retained directory; no ancestor is resolved.
        if unsafe { renameat(fd, old.as_ptr(), fd, new.as_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        return Ok(());
    }
    publish_noclobber_with(directory, &old, &new, || {
        // SAFETY: retained directory and bounded basenames; RENAME_NOREPLACE is atomic.
        if unsafe { renameat2(fd, old.as_ptr(), fd, new.as_ptr(), 1) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    })
}

fn publish_noclobber_with(
    directory: &File,
    old: &CString,
    new: &CString,
    rename: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    match rename() {
        Ok(()) => return Ok(()),
        // EINVAL (including Linux NFS), ENOSYS, EOPNOTSUPP: this exact operation
        // has fixed valid flags and same-directory regular-file names.
        Err(error) if matches!(error.raw_os_error(), Some(22 | 38 | 95)) => {}
        Err(error) => return Err(error),
    }
    let fd = directory.as_raw_fd();
    // SAFETY: both relative names stay under the retained parent. linkat never
    // replaces an existing destination, including one created after admission.
    if unsafe { linkat(fd, old.as_ptr(), fd, new.as_ptr(), 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: remove only the temporary name in that same directory after linking succeeds.
    if unsafe { unlinkat(fd, old.as_ptr(), 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) fn remove_file(directory: &File, _file: &File, name: &OsStr) -> io::Result<()> {
    let name = c_name(name)?;
    // SAFETY: cleanup uses the same retained directory and a single temporary basename.
    if unsafe { unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) fn sync_directory(directory: &File) -> io::Result<()> {
    directory.sync_all()
}

pub(super) fn same_directory(left: &File, right: &File) -> io::Result<bool> {
    let left = left.metadata()?;
    let right = right.metadata()?;
    Ok(left.dev() == right.dev() && left.ino() == right.ino())
}

pub(super) fn names_equal(left: &OsStr, right: &OsStr) -> io::Result<bool> {
    Ok(left == right)
}

pub(super) fn same_entry_name(left: &File, right: &File) -> io::Result<bool> {
    if !same_directory(left, right)? {
        return Ok(false);
    }
    // File identity alone would reject safe distinct hard links. /proc reports each
    // opened dentry name, including a case-folded filesystem's actual spelling.
    let left = std::fs::read_link(format!("/proc/self/fd/{}", left.as_raw_fd()))?;
    let right = std::fs::read_link(format!("/proc/self/fd/{}", right.as_raw_fd()))?;
    Ok(left.file_name() == right.file_name())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn unsupported_no_replace_uses_real_links_in_the_retained_directory() {
        for code in [22, 38, 95] {
            let root = tempfile::tempdir().unwrap();
            let selected = root.path().join("selected");
            let retained = root.path().join("retained");
            std::fs::create_dir(&selected).unwrap();
            let directory = open_directory(&selected).unwrap();
            let mut file = create_file(&directory, OsStr::new("staged")).unwrap();
            file.write_all(b"complete subtitles").unwrap();
            std::fs::rename(&selected, &retained).unwrap();
            std::fs::create_dir(&selected).unwrap();
            std::fs::write(selected.join("output.srt"), b"unrelated original").unwrap();
            publish_noclobber_with(
                &directory,
                &c_name(OsStr::new("staged")).unwrap(),
                &c_name(OsStr::new("output.srt")).unwrap(),
                || Err(io::Error::from_raw_os_error(code)),
            )
            .unwrap();
            assert_eq!(
                std::fs::read(retained.join("output.srt")).unwrap(),
                b"complete subtitles"
            );
            assert!(!retained.join("staged").exists());
            assert_eq!(
                std::fs::read(selected.join("output.srt")).unwrap(),
                b"unrelated original"
            );
        }
    }

    #[test]
    fn unsupported_no_replace_preserves_a_competitor_at_the_link_boundary() {
        let root = tempfile::tempdir().unwrap();
        let directory = open_directory(root.path()).unwrap();
        let mut file = create_file(&directory, OsStr::new("staged")).unwrap();
        file.write_all(b"complete subtitles").unwrap();
        let error = publish_noclobber_with(
            &directory,
            &c_name(OsStr::new("staged")).unwrap(),
            &c_name(OsStr::new("output.srt")).unwrap(),
            || {
                std::fs::write(root.path().join("output.srt"), b"late competitor")?;
                Err(io::Error::from_raw_os_error(22))
            },
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            std::fs::read(root.path().join("output.srt")).unwrap(),
            b"late competitor"
        );
        assert_eq!(
            std::fs::read(root.path().join("staged")).unwrap(),
            b"complete subtitles"
        );
        remove_file(&directory, &file, OsStr::new("staged")).unwrap();
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn no_replace_permission_and_io_errors_do_not_enter_the_link_fallback() {
        for code in [5, 13, 17] {
            let root = tempfile::tempdir().unwrap();
            let directory = open_directory(root.path()).unwrap();
            let mut file = create_file(&directory, OsStr::new("staged")).unwrap();
            file.write_all(b"complete subtitles").unwrap();
            let error = publish_noclobber_with(
                &directory,
                &c_name(OsStr::new("staged")).unwrap(),
                &c_name(OsStr::new("output.srt")).unwrap(),
                || Err(io::Error::from_raw_os_error(code)),
            )
            .unwrap_err();
            assert_eq!(error.raw_os_error(), Some(code));
            assert!(!root.path().join("output.srt").exists());
            assert_eq!(
                std::fs::read(root.path().join("staged")).unwrap(),
                b"complete subtitles"
            );
        }
    }
}
