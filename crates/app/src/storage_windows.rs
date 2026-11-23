use std::{
    ffi::{OsStr, OsString, c_void},
    fs::{File, OpenOptions},
    io,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::{MetadataExt, OpenOptionsExt},
        io::{AsRawHandle, FromRawHandle},
    },
    path::Path,
};

type Handle = *mut c_void;
const SYNCHRONIZE: u32 = 0x00100000;
const FILE_READ_ATTRIBUTES: u32 = 0x80;
const FILE_TRAVERSE: u32 = 0x20;
const DELETE: u32 = 0x00010000;

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *mut u16,
}

#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root_directory: Handle,
    name: *mut UnicodeString,
    attributes: u32,
    security_descriptor: *mut c_void,
    security_quality: *mut c_void,
}

#[repr(C)]
struct IoStatus {
    status_or_pointer: usize,
    information: usize,
}

#[repr(C)]
struct RenameInfo {
    replace_or_flags: u32,
    root_directory: Handle,
    file_name_length: u32,
    file_name: [u16; 1],
}

#[repr(C)]
#[derive(PartialEq, Eq)]
struct FileId {
    volume: u64,
    identifier: [u8; 16],
}

#[repr(C)]
struct LegacyFileInformation {
    attributes: u32,
    creation: [u32; 2],
    access: [u32; 2],
    write: [u32; 2],
    volume: u32,
    size_high: u32,
    size_low: u32,
    links: u32,
    index_high: u32,
    index_low: u32,
}

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtCreateFile(
        file: *mut Handle,
        access: u32,
        attributes: *const ObjectAttributes,
        status: *mut IoStatus,
        allocation_size: *const i64,
        file_attributes: u32,
        share_access: u32,
        disposition: u32,
        options: u32,
        ea_buffer: *const c_void,
        ea_length: u32,
    ) -> i32;
    fn NtSetInformationFile(
        file: Handle,
        status: *mut IoStatus,
        info: *const c_void,
        len: u32,
        class: i32,
    ) -> i32;
    fn RtlNtStatusToDosError(status: i32) -> u32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetFileInformationByHandle(file: Handle, class: i32, info: *const c_void, len: u32) -> i32;
    fn GetFileInformationByHandleEx(file: Handle, class: i32, info: *mut c_void, len: u32) -> i32;
    fn GetFileInformationByHandle(file: Handle, info: *mut LegacyFileInformation) -> i32;
    fn GetFinalPathNameByHandleW(file: Handle, path: *mut u16, len: u32, flags: u32) -> u32;
    fn CompareStringOrdinal(
        left: *const u16,
        left_len: i32,
        right: *const u16,
        right_len: i32,
        ignore_case: i32,
    ) -> i32;
}

pub(super) fn validate_name(name: &OsStr) -> io::Result<()> {
    let name: Vec<_> = name.encode_wide().collect();
    if name.is_empty()
        || name.len() > (u16::MAX as usize / 2)
        || name
            .iter()
            .any(|&c| [0, b'/' as u16, b'\\' as u16, b':' as u16].contains(&c))
        || matches!(name.last(), Some(0x20 | 0x2e))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Expected one ordinary output filename",
        ));
    }
    Ok(())
}

pub(super) fn open_directory(path: &Path) -> io::Result<File> {
    let directory = OpenOptions::new()
        .read(true)
        .access_mode(SYNCHRONIZE | FILE_READ_ATTRIBUTES | FILE_TRAVERSE)
        .share_mode(1 | 2 | 4)
        .custom_flags(0x02000000) // FILE_FLAG_BACKUP_SEMANTICS
        .open(path)?;
    if !directory.metadata()?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "Output parent is not a directory",
        ));
    }
    Ok(directory)
}

enum OpenMode {
    Read,
    Create,
    Inspect,
    Lock,
}

fn open_relative(directory: &File, name: &OsStr, mode: OpenMode) -> io::Result<File> {
    let (create, inspect, lock) = match mode {
        OpenMode::Read => (false, false, false),
        OpenMode::Create => (true, false, false),
        OpenMode::Inspect => (false, true, false),
        OpenMode::Lock => (false, false, true),
    };
    validate_name(name)?;
    let mut name: Vec<_> = name.encode_wide().collect();
    let mut unicode = UnicodeString {
        length: (name.len() * 2) as u16,
        maximum_length: (name.len() * 2) as u16,
        buffer: name.as_mut_ptr(),
    };
    let attributes = ObjectAttributes {
        length: std::mem::size_of::<ObjectAttributes>() as u32,
        root_directory: directory.as_raw_handle(),
        name: &mut unicode,
        attributes: 0x40, // OBJ_CASE_INSENSITIVE, matching ordinary Windows paths.
        security_descriptor: std::ptr::null_mut(),
        security_quality: std::ptr::null_mut(),
    };
    let mut io_status = IoStatus {
        status_or_pointer: 0,
        information: 0,
    };
    let mut handle = std::ptr::null_mut();
    let access = if inspect {
        SYNCHRONIZE | FILE_READ_ATTRIBUTES
    } else if create || lock {
        0x0012019f | if create { DELETE } else { 0 } // FILE_GENERIC_READ | FILE_GENERIC_WRITE; temporary publication also needs DELETE.
    } else {
        SYNCHRONIZE | FILE_READ_ATTRIBUTES | 1 // FILE_READ_DATA
    };
    // SAFETY: these repr(C) layouts match the Win64 ABI on x64/ARM64. Every pointer
    // remains live for this synchronous call; RootDirectory pins this one relative name.
    let status = unsafe {
        NtCreateFile(
            &mut handle,
            access,
            &attributes,
            &mut io_status,
            std::ptr::null(),
            0x80, // FILE_ATTRIBUTE_NORMAL
            if create { 1 } else { 1 | 2 | 4 },
            if lock {
                3
            } else if create {
                2
            } else {
                1
            }, // FILE_OPEN_IF (no truncation), FILE_CREATE (exclusive), or FILE_OPEN.
            0x20 | 0x00200000 | if inspect { 0 } else { 0x40 }, // SYNCHRONOUS_IO_NONALERT | OPEN_REPARSE_POINT, optionally NON_DIRECTORY_FILE.
            std::ptr::null(),
            0,
        )
    };
    if status < 0 {
        // SAFETY: this converts the returned NTSTATUS, not a stale thread last-error value.
        return Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(status) } as i32,
        ));
    }
    // SAFETY: NtCreateFile returned a new successful handle, transferred to this sole owner.
    let file = unsafe { File::from_raw_handle(handle) };
    if !create && file.metadata()?.file_attributes() & 0x400 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Output destination must not be a reparse point",
        ));
    }
    Ok(file)
}

pub(super) fn open_file(directory: &File, name: &OsStr) -> io::Result<File> {
    open_relative(directory, name, OpenMode::Read)
}

pub(super) fn create_file(directory: &File, name: &OsStr) -> io::Result<File> {
    open_relative(directory, name, OpenMode::Create)
}

pub(super) fn open_lock_file(directory: &File, name: &OsStr) -> io::Result<File> {
    open_relative(directory, name, OpenMode::Lock)
}

pub(super) fn open_source_file(directory: &File, name: &OsStr) -> io::Result<Option<File>> {
    let inspection = open_relative(directory, name, OpenMode::Inspect)?;
    if !inspection.metadata()?.is_file() {
        return Ok(None);
    }
    drop(inspection);
    open_file(directory, name).map(Some)
}

pub(super) fn publish(
    directory: &File,
    file: &File,
    _old: &OsStr,
    new: &OsStr,
    overwrite: bool,
) -> io::Result<()> {
    validate_name(new)?;
    let name: Vec<_> = new.encode_wide().collect();
    let bytes = std::mem::size_of::<RenameInfo>() + (name.len() + 1) * 2;
    let mut storage = vec![0usize; bytes.div_ceil(std::mem::size_of::<usize>())];
    let mut io_status = IoStatus {
        status_or_pointer: 0,
        information: 0,
    };
    // SAFETY: the usize buffer is aligned for the Win64 structure and large enough
    // for its flexible UTF-16 name plus NUL. The source handle owns the temporary;
    // the native FileRenameInformation class preserves RootDirectory plus basename,
    // so no ancestor pathname is resolved. The synchronous handle keeps buffers live.
    let status = unsafe {
        let info = storage.as_mut_ptr().cast::<RenameInfo>();
        (*info).replace_or_flags = u32::from(overwrite);
        (*info).root_directory = directory.as_raw_handle();
        (*info).file_name_length = (name.len() * 2) as u32;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            std::ptr::addr_of_mut!((*info).file_name).cast::<u16>(),
            name.len(),
        );
        NtSetInformationFile(
            file.as_raw_handle(),
            &mut io_status,
            info.cast(),
            bytes as u32,
            10, // Native FileRenameInformation; Win32 FileRenameInfo uses a different class.
        )
    };
    if status < 0 {
        // SAFETY: convert this returned NTSTATUS, not an unrelated Win32 last error.
        return Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(status) } as i32,
        ));
    }
    Ok(())
}

pub(super) fn remove_file(_directory: &File, file: &File, _name: &OsStr) -> io::Result<()> {
    let delete = 1u8;
    // SAFETY: FileDispositionInfo contains one BOOLEAN. DELETE access belongs to
    // this temporary handle; no cleanup path can follow a substituted ancestor.
    if unsafe {
        SetFileInformationByHandle(file.as_raw_handle(), 4, (&delete as *const u8).cast(), 1)
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) fn sync_directory(_directory: &File) -> io::Result<()> {
    Ok(())
}

fn file_id(file: &File) -> io::Result<FileId> {
    let mut id = FileId {
        volume: 0,
        identifier: [0; 16],
    };
    // SAFETY: FileIdInfo (class 18) writes the 24-byte FILE_ID_INFO layout. Its
    // full 128-bit identifier also distinguishes directories on ReFS.
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            18,
            (&mut id as *mut FileId).cast(),
            std::mem::size_of::<FileId>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(id)
}

pub(super) fn same_directory(left: &File, right: &File) -> io::Result<bool> {
    same_directory_with(left, right, file_id)
}

fn same_directory_with(
    left: &File,
    right: &File,
    identify: impl Fn(&File) -> io::Result<FileId>,
) -> io::Result<bool> {
    let identify_supported = |file| match identify(file) {
        Ok(id) => Ok(Some(id)),
        // FAT and older SMB implementations may not support FileIdInfo. Preserve
        // permission, I/O, and all other errors instead of masking them with fallback.
        Err(error) if matches!(error.raw_os_error(), Some(1 | 50 | 87)) => Ok(None),
        Err(error) => Err(error),
    };
    let left_id = identify_supported(left)?;
    let right_id = identify_supported(right)?;
    match (left_id, right_id) {
        (Some(left), Some(right)) => Ok(left == right),
        _ => Ok(legacy_file_id(left)? == legacy_file_id(right)?),
    }
}

fn legacy_file_id(file: &File) -> io::Result<(u32, u64)> {
    // SAFETY: all fields are integer values, so zero initialization is valid.
    let mut info: LegacyFileInformation = unsafe { std::mem::zeroed() };
    // SAFETY: the 52-byte repr(C) layout is BY_HANDLE_FILE_INFORMATION. The retained
    // handle supplies both volume and file ID; no pathname is reopened for fallback.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((
        info.volume,
        (u64::from(info.index_high) << 32) | u64::from(info.index_low),
    ))
}

pub(super) fn names_equal(left: &OsStr, right: &OsStr) -> io::Result<bool> {
    let left: Vec<_> = left.encode_wide().collect();
    let right: Vec<_> = right.encode_wide().collect();
    // SAFETY: both live UTF-16 slices use explicit bounded lengths, without NUL reliance.
    let result = unsafe {
        CompareStringOrdinal(
            left.as_ptr(),
            left.len() as i32,
            right.as_ptr(),
            right.len() as i32,
            1,
        )
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(result == 2) // CSTR_EQUAL
}

fn actual_name(file: &File) -> io::Result<OsString> {
    let mut buffer = vec![0u16; 32768];
    // SAFETY: the buffer has the advertised WCHAR capacity. VOLUME_NAME_NT avoids
    // drive-letter aliases; FILE_NAME_NORMALIZED returns the actual entry spelling.
    let len = unsafe {
        GetFinalPathNameByHandleW(
            file.as_raw_handle(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            2,
        )
    };
    if len == 0 {
        return Err(io::Error::last_os_error());
    }
    if len as usize >= buffer.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Resolved filename exceeds the Windows path limit",
        ));
    }
    let path = OsString::from_wide(&buffer[..len as usize]);
    Path::new(&path)
        .file_name()
        .map(OsStr::to_os_string)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Missing resolved filename"))
}

pub(super) fn same_entry_name(left: &File, right: &File) -> io::Result<bool> {
    if !same_directory(left, right)? {
        return Ok(false);
    }
    names_equal(&actual_name(left)?, &actual_name(right)?)
}

#[cfg(test)]
#[path = "storage_windows_publication_tests.rs"]
mod publication_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_extended_identity_uses_real_legacy_directory_handles() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        let left = open_directory(&first).unwrap();
        let alias = open_directory(&first).unwrap();
        let right = open_directory(&second).unwrap();
        for code in [1, 50, 87] {
            assert!(
                same_directory_with(&left, &alias, |_| Err(io::Error::from_raw_os_error(code)))
                    .unwrap()
            );
            assert!(
                !same_directory_with(&left, &right, |_| Err(io::Error::from_raw_os_error(code)))
                    .unwrap()
            );
        }
    }

    #[test]
    fn extended_identity_access_errors_are_never_relabelled_as_unsupported() {
        let directory = tempfile::tempdir().unwrap();
        let handle = open_directory(directory.path()).unwrap();
        let error = same_directory_with(&handle, &handle, |_| Err(io::Error::from_raw_os_error(5)))
            .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(5));
    }
}
