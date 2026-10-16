//! Content in a project's `.dpm` directory that is not project state.
//!
//! The directory is ignored by Git, which makes it an easy place to leave logs, screenshots and
//! build outputs; nothing removes them, so a workspace can grow without bound. Status names them so
//! the human or agent that left them sees it on its first query.

use crate::{ProjectLocation, ProjectSource};
use serde::Serialize;
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

/// Files a project keeps in its store directory besides the store.
const PROJECT_ENTRIES: [&str; 5] = [
    "project.toml",
    ".gitignore",
    "local.toml",
    "credentials.toml",
    ".dpm-claude",
];

/// Files SQLite and the run store keep beside a store, named by suffix.
const STORE_SUFFIXES: [&str; 8] = [
    "",
    "-wal",
    "-shm",
    "-journal",
    ".runs",
    ".runs-wal",
    ".runs-shm",
    ".runs-journal",
];

/// Files measured before the size is reported as a lower bound, so a status query stays cheap.
const SCAN_LIMIT: usize = 10_000;

const ADVICE: &str = "the store directory holds project state only; move these elsewhere or \
    delete them, and attach evidence as a link (commit, pull request, CI run)";

/// Entries of the store directory that are not project state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StrayStoreContent {
    /// The store directory.
    pub directory: PathBuf,
    /// Its top-level entries that are not project state, in name order.
    pub entries: Vec<String>,
    /// Their combined size in bytes; a lower bound when `complete` is false.
    pub bytes: u64,
    /// Whether every file was measured.
    pub complete: bool,
    /// What to do about them.
    pub advice: &'static str,
}

impl ProjectLocation {
    /// The store directory's content that is not project state; `None` when there is none or the
    /// directory cannot be read, which status must not fail over.
    pub(crate) fn stray_store_content_blocking(&self) -> Option<StrayStoreContent> {
        let directory = self.root.join(".dpm");
        let mut expected: Vec<OsString> = PROJECT_ENTRIES.iter().map(OsString::from).collect();
        if let ProjectSource::Database(store) = &self.source
            && same_directory_blocking(store.parent(), &directory)
            && let Some(name) = store.file_name()
        {
            expected.extend(STORE_SUFFIXES.iter().map(|suffix| {
                let mut side = name.to_os_string();
                side.push(suffix);
                side
            }));
        }
        let listing = match fs::read_dir(&directory) {
            Ok(listing) => listing,
            Err(error) => {
                tracing::debug!(directory = %directory.display(), %error, "store directory unreadable");
                return None;
            }
        };
        let mut stray: Vec<PathBuf> = listing
            .filter_map(Result::ok)
            .filter(|entry| !expected.contains(&entry.file_name()))
            .map(|entry| entry.path())
            .collect();
        if stray.is_empty() {
            return None;
        }
        stray.sort();
        let mut budget = SCAN_LIMIT;
        let bytes = stray
            .iter()
            .map(|path| size_blocking(path, &mut budget))
            .fold(0_u64, u64::saturating_add);
        Some(StrayStoreContent {
            entries: stray
                .iter()
                .filter_map(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .collect(),
            directory,
            bytes,
            complete: budget > 0,
            advice: ADVICE,
        })
    }
}

fn same_directory_blocking(parent: Option<&Path>, directory: &Path) -> bool {
    let canonical = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    parent.is_some_and(|parent| canonical(parent) == canonical(directory))
}

/// Size of a file, or of a directory's files without following links, spending one unit of
/// `budget` per entry; stops counting once the budget is spent.
fn size_blocking(path: &Path, budget: &mut usize) -> u64 {
    if *budget == 0 {
        return 0;
    }
    *budget -= 1;
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return 0;
    };
    if !metadata.is_dir() {
        return metadata.len();
    }
    fs::read_dir(path).map_or(0, |listing| {
        listing
            .filter_map(Result::ok)
            .map(|entry| size_blocking(&entry.path(), budget))
            .fold(0, u64::saturating_add)
    })
}

#[cfg(test)]
mod tests;
