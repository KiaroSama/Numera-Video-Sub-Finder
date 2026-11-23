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
    // SAFETY: both names are one component in this same retained directory. Neither
    // rename operation resolves an ancestor; RENAME_NOREPLACE enforces no-clobber atomically.
    let result = unsafe {
        if overwrite {
            renameat(fd, old.as_ptr(), fd, new.as_ptr())
        } else {
            renameat2(fd, old.as_ptr(), fd, new.as_ptr(), 1)
        }
    };
    if result != 0 {
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
