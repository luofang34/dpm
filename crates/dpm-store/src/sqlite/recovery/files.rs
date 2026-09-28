//! File handling for recovery: target validation, SQLite side files and read-only durable data.

use crate::{StoreError, error::database_error};
use rusqlite::{Connection, OpenFlags};
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const SIDE_FILE_SUFFIXES: [&str; 3] = ["-wal", "-shm", "-journal"];

/// How persistent database and WAL contents are opened read-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::sqlite) enum ReadMode {
    /// No side file exists, so the main file holds every committed page; SQLite is told the file
    /// cannot change, so it creates no `-shm` or `-wal` and takes no locks.
    Immutable,
    /// Side files exist, so committed pages may live in the WAL; SQLite reads through the side
    /// files. SQLite may create or refresh the transient shared-memory index.
    Shared,
}

/// The observable state of a store file, compared around an immutable read to detect a writer
/// that started meanwhile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::sqlite) struct Fingerprint {
    length: u64,
    modified: Option<SystemTime>,
    side_files: Vec<PathBuf>,
}

pub(in crate::sqlite) fn side_file(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn existing_side_files_blocking(path: &Path) -> Result<Vec<PathBuf>, StoreError> {
    let mut found = Vec::new();
    for suffix in SIDE_FILE_SUFFIXES {
        let side = side_file(path, suffix);
        if side.try_exists().map_err(io_error("inspect", &side))? {
            found.push(side);
        }
    }
    Ok(found)
}

pub(in crate::sqlite) fn fingerprint_blocking(path: &Path) -> Result<Fingerprint, StoreError> {
    let metadata = fs::metadata(path).map_err(io_error("inspect", path))?;
    Ok(Fingerprint {
        length: metadata.len(),
        modified: metadata.modified().ok(),
        side_files: existing_side_files_blocking(path)?,
    })
}

pub(in crate::sqlite) fn read_mode_blocking(path: &Path) -> Result<ReadMode, StoreError> {
    if cfg!(unix) && existing_side_files_blocking(path)?.is_empty() {
        Ok(ReadMode::Immutable)
    } else {
        Ok(ReadMode::Shared)
    }
}

pub(in crate::sqlite) fn open_read_only_blocking(
    path: &Path,
    mode: ReadMode,
) -> Result<Connection, StoreError> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let connection = match mode {
        ReadMode::Shared => Connection::open_with_flags(path, flags),
        ReadMode::Immutable => Connection::open_with_flags(
            format!("file:{}?immutable=1", uri_path(path)),
            flags | OpenFlags::SQLITE_OPEN_URI,
        ),
    }
    .map_err(database_error(path, "open database read-only"))?;
    connection
        .busy_timeout(BUSY_TIMEOUT)
        .map_err(database_error(path, "set busy timeout"))?;
    Ok(connection)
}

/// Percent-encode every byte a URI filename could misread, such as `?`, `#` and `%`.
fn uri_path(path: &Path) -> String {
    path.as_os_str()
        .as_encoded_bytes()
        .iter()
        .map(|&byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'.' | b'-' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// Resolve an existing store to the canonical absolute path reported back to the operator.
pub(in crate::sqlite) fn canonical_blocking(path: &Path) -> Result<PathBuf, StoreError> {
    fs::canonicalize(path).map_err(io_error("resolve", path))
}

/// Resolve a target that does not exist yet through its canonical parent directory, refusing
/// names SQLite reserves for side files: SQLite would read such a file as part of, or delete it
/// together with, the database it belongs to.
pub(in crate::sqlite) fn target_path_blocking(to: &Path) -> Result<PathBuf, StoreError> {
    let Some(name) = to.file_name() else {
        return Err(StoreError::InvalidTarget {
            path: to.to_path_buf(),
            reason: "the path does not name a file",
        });
    };
    let parent = match to.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let target = canonical_blocking(parent)?.join(name);
    let lowercase = name.to_string_lossy().to_lowercase();
    if SIDE_FILE_SUFFIXES
        .iter()
        .any(|suffix| lowercase.ends_with(suffix))
    {
        return Err(StoreError::InvalidTarget {
            path: target,
            reason: "names ending in -wal, -shm or -journal belong to SQLite's side files",
        });
    }
    Ok(target)
}

/// Create the target exclusively, after checking that no stale side file would be read as part of
/// the new database.
pub(in crate::sqlite) fn create_target_blocking(to: &Path) -> Result<(), StoreError> {
    if let Some(side) = existing_side_files_blocking(to)?.into_iter().next() {
        return Err(StoreError::TargetExists { path: side });
    }
    // Exclusive creation closes the race between checking for and writing the target.
    match fs::OpenOptions::new().write(true).create_new(true).open(to) {
        Ok(_) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
            Err(StoreError::TargetExists {
                path: to.to_path_buf(),
            })
        }
        Err(source) => Err(io_error("create", to)(source)),
    }
}

/// Remove a target this command created after a failed copy; the copy failure is the actionable
/// error, and a leftover file is reported as `TargetExists` on the next attempt instead.
pub(in crate::sqlite) fn discard_blocking(to: &Path) {
    for path in SIDE_FILE_SUFFIXES
        .iter()
        .map(|suffix| side_file(to, suffix))
        .chain([to.to_path_buf()])
    {
        fs::remove_file(path).ok();
    }
}

fn io_error(action: &'static str, path: &Path) -> impl FnOnce(io::Error) -> StoreError {
    let path = path.to_path_buf();
    move |source| StoreError::Io {
        action,
        path,
        source,
    }
}
