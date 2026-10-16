//! What attached evidence may be: a reference to where the evidence lives, never the evidence.
//!
//! The project store and its directory hold plan state only. Copying build outputs, screenshots
//! or logs beside it and linking them by path lets a workspace grow without bound and ties the
//! evidence to one machine, so evidence names a commit, a hosted page or a committed text file.

use crate::EngineError;
use dpm_model::Artifact;

/// Schemes that reference evidence held elsewhere: a Git commit or committed file, a path in the
/// project's repository, a hosted page, or an external identifier.
const EVIDENCE_SCHEMES: [&str; 6] = ["git", "repo", "https", "http", "urn", "doi"];

/// Longest accepted locator, in bytes; long enough for any URL, too short to carry content.
const MAX_EVIDENCE_URI_BYTES: usize = 2048;
/// Longest accepted label, in bytes.
const MAX_EVIDENCE_LABEL_BYTES: usize = 500;
/// Most metadata attributes one artifact may carry.
const MAX_EVIDENCE_METADATA_ENTRIES: usize = 32;
/// Longest accepted metadata key or value, in bytes.
const MAX_EVIDENCE_METADATA_BYTES: usize = 1024;

/// Directory that holds a project's store; evidence never lives inside it.
const STORE_DIRECTORY: &str = ".dpm";

/// Extensions of screenshots, recordings, binaries, archives and databases: content a link should
/// point at where it is hosted rather than a file committed or copied as evidence.
const CONTENT_EXTENSIONS: [&str; 37] = [
    "png", "jpg", "jpeg", "gif", "webp", "heic", "heif", "bmp", "tif", "tiff", "mp4", "mov",
    "webm", "mkv", "avi", "wasm", "a", "o", "so", "dylib", "dll", "exe", "bin", "rlib", "zip",
    "tar", "gz", "tgz", "xz", "7z", "dmg", "pkg", "iso", "sqlite", "db", "gputrace", "trace",
];

const GUIDANCE: &str = "evidence is a link, not content: commit the change and attach_git_head, \
    or link a hosted page (pull request, CI run, report)";

/// Reject evidence that is a local copy or embedded content rather than a reference.
pub(crate) fn check(artifact: &Artifact) -> Result<(), EngineError> {
    let reject = |reason: String| EngineError::InvalidCommand {
        entity: artifact.id.to_string(),
        reason: format!("{reason}; {GUIDANCE}"),
    };
    if artifact.uri.len() > MAX_EVIDENCE_URI_BYTES {
        return Err(reject(format!(
            "artifact uri exceeds {MAX_EVIDENCE_URI_BYTES} bytes"
        )));
    }
    if artifact.label.len() > MAX_EVIDENCE_LABEL_BYTES {
        return Err(reject(format!(
            "artifact label exceeds {MAX_EVIDENCE_LABEL_BYTES} bytes"
        )));
    }
    if artifact.metadata.len() > MAX_EVIDENCE_METADATA_ENTRIES {
        return Err(reject(format!(
            "artifact metadata exceeds {MAX_EVIDENCE_METADATA_ENTRIES} entries"
        )));
    }
    if artifact.metadata.iter().any(|(k, v)| {
        k.len() > MAX_EVIDENCE_METADATA_BYTES || v.len() > MAX_EVIDENCE_METADATA_BYTES
    }) {
        return Err(reject(format!(
            "artifact metadata entries are at most {MAX_EVIDENCE_METADATA_BYTES} bytes"
        )));
    }
    let Some((scheme, rest)) = artifact.uri.split_once(':') else {
        return Err(reject(format!(
            "artifact uri {:?} names no scheme",
            artifact.uri
        )));
    };
    if !EVIDENCE_SCHEMES.contains(&scheme) {
        return Err(reject(format!(
            "artifact uri scheme {scheme:?} is not one of {}",
            EVIDENCE_SCHEMES.join(", ")
        )));
    }
    match scheme {
        "repo" => check_path(rest, &['/', '\\']).map_err(reject),
        // A committed file is `git:<revision>:<path>`; commit forms (`git:local@<sha>`,
        // `git:<remote>@<sha>`) end in a revision, so every component is checked alike.
        "git" => check_path(rest, &['/', '\\', ':', '@']).map_err(reject),
        _ => Ok(()),
    }
}

fn check_path(path: &str, separators: &[char]) -> Result<(), String> {
    let segments: Vec<&str> = path.split(separators).collect();
    if path.starts_with(['/', '\\']) || segments.contains(&"..") {
        return Err(format!("evidence path {path:?} leaves the repository"));
    }
    if segments.contains(&STORE_DIRECTORY) {
        return Err(format!(
            "evidence path {path:?} lies in the project store directory {STORE_DIRECTORY}"
        ));
    }
    let extension = segments
        .last()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, extension)| extension.to_ascii_lowercase());
    if extension.is_some_and(|e| CONTENT_EXTENSIONS.contains(&e.as_str())) {
        return Err(format!(
            "evidence path {path:?} is a screenshot, recording, binary or archive"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
