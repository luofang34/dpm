//! What newly attached evidence may be: a reference to where it lives, a fingerprint that keeps the
//! reference checkable, and a small structured result; never the content itself.
//!
//! The project store and its directory hold plan state only. Copying build outputs, screenshots or
//! logs beside it lets a workspace grow without bound, so evidence stays wherever it is kept (a
//! commit, a CI run, a document system, a shared drive) and dpm records how to find and check it.

use crate::EngineError;
use dpm_model::{Artifact, ArtifactKind};

/// Schemes whose locator carries the content itself rather than naming where it lives.
const CONTENT_SCHEMES: [&str; 4] = ["data", "blob", "javascript", "about"];

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

/// Extensions of screenshots, recordings, binaries, archives and databases, refused only as files
/// in a repository: committing them as evidence bloats every clone. Elsewhere a photo or drawing
/// is legitimate evidence.
const CONTENT_EXTENSIONS: [&str; 37] = [
    "png", "jpg", "jpeg", "gif", "webp", "heic", "heif", "bmp", "tif", "tiff", "mp4", "mov",
    "webm", "mkv", "avi", "wasm", "a", "o", "so", "dylib", "dll", "exe", "bin", "rlib", "zip",
    "tar", "gz", "tgz", "xz", "7z", "dmg", "pkg", "iso", "sqlite", "db", "gputrace", "trace",
];

/// Metadata naming a local file's SHA-256 digest, lowercase hex.
pub const DIGEST_KEY: &str = "sha256";
/// Metadata naming a local file's size in bytes.
pub const SIZE_KEY: &str = "bytes";
/// Metadata stating a check's or build's result.
pub const OUTCOME_KEY: &str = "outcome";
/// Results a check or build may state.
pub const OUTCOMES: [&str; 3] = ["passed", "failed", "partial"];

const GUIDANCE: &str = "evidence names where it lives: attach_git_head for a commit, a URL for a \
    CI run, pull request or document, or attach_file for a local file (it records the digest)";

/// Reject evidence that carries content or cannot be checked later.
pub(crate) fn check(artifact: &Artifact) -> Result<(), EngineError> {
    let reject = |reason: String| EngineError::InvalidCommand {
        entity: artifact.id.to_string(),
        reason: format!("{reason}; {GUIDANCE}"),
    };
    check_sizes(artifact).map_err(reject)?;
    check_locator(artifact).map_err(reject)?;
    check_outcome(artifact).map_err(reject)
}

fn check_sizes(artifact: &Artifact) -> Result<(), String> {
    if artifact.uri.len() > MAX_EVIDENCE_URI_BYTES {
        return Err(format!(
            "artifact uri exceeds {MAX_EVIDENCE_URI_BYTES} bytes"
        ));
    }
    if artifact.label.len() > MAX_EVIDENCE_LABEL_BYTES {
        return Err(format!(
            "artifact label exceeds {MAX_EVIDENCE_LABEL_BYTES} bytes"
        ));
    }
    if artifact.metadata.len() > MAX_EVIDENCE_METADATA_ENTRIES {
        return Err(format!(
            "artifact metadata exceeds {MAX_EVIDENCE_METADATA_ENTRIES} entries"
        ));
    }
    if artifact.metadata.iter().any(|(k, v)| {
        k.len() > MAX_EVIDENCE_METADATA_BYTES || v.len() > MAX_EVIDENCE_METADATA_BYTES
    }) {
        return Err(format!(
            "artifact metadata entries are at most {MAX_EVIDENCE_METADATA_BYTES} bytes"
        ));
    }
    Ok(())
}

fn check_locator(artifact: &Artifact) -> Result<(), String> {
    let uri = &artifact.uri;
    let scheme = uri
        .split_once(':')
        .map(|(scheme, _)| scheme)
        .filter(|s| is_scheme(s))
        .ok_or_else(|| format!("artifact uri {uri:?} names no scheme"))?;
    let rest = uri
        .get(scheme.len().saturating_add(1)..)
        .unwrap_or_default();
    let scheme = scheme.to_ascii_lowercase();
    if CONTENT_SCHEMES.contains(&scheme.as_str()) {
        return Err(format!(
            "a {scheme}: uri carries content instead of naming where it lives"
        ));
    }
    match scheme.as_str() {
        "file" => {
            check_segments(rest, &['/', '\\'])?;
            check_fingerprint(artifact)
        }
        "repo" => {
            check_repository_path(rest, &['/', '\\'])?;
            check_fingerprint(artifact)
        }
        // A committed file is `git:<revision>:<path>`; commit forms (`git:local@<sha>`,
        // `git:<remote>@<sha>`) end in a revision, so every component is checked alike.
        "git" => check_repository_path(rest, &['/', '\\', ':', '@']),
        _ => Ok(()),
    }
}

/// An RFC 3986 scheme of at least two characters, so a Windows drive letter is not one.
fn is_scheme(scheme: &str) -> bool {
    let mut characters = scheme.chars();
    characters.next().is_some_and(|c| c.is_ascii_alphabetic())
        && scheme.len() >= 2
        && characters.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

fn check_segments(path: &str, separators: &[char]) -> Result<(), String> {
    if path
        .split(separators)
        .any(|segment| segment == STORE_DIRECTORY)
    {
        return Err(format!(
            "evidence path {path:?} lies in a project store directory {STORE_DIRECTORY}"
        ));
    }
    Ok(())
}

fn check_repository_path(path: &str, separators: &[char]) -> Result<(), String> {
    let segments: Vec<&str> = path.split(separators).collect();
    if path.starts_with(['/', '\\']) || segments.contains(&"..") {
        return Err(format!("evidence path {path:?} leaves the repository"));
    }
    check_segments(path, separators)?;
    let extension = segments
        .last()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, extension)| extension.to_ascii_lowercase());
    if extension.is_some_and(|e| CONTENT_EXTENSIONS.contains(&e.as_str())) {
        return Err(format!(
            "evidence path {path:?} commits a screenshot, recording, binary or archive to the \
             repository; link it where it is hosted instead"
        ));
    }
    Ok(())
}

/// A local file can change or vanish, so its reference records what it held.
fn check_fingerprint(artifact: &Artifact) -> Result<(), String> {
    let digest = artifact.metadata.get(DIGEST_KEY);
    let size = artifact.metadata.get(SIZE_KEY);
    let digest_ok = digest
        .is_some_and(|d| d.len() == 64 && d.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));
    if !digest_ok || !size.is_some_and(|s| s.parse::<u64>().is_ok()) {
        return Err(format!(
            "a local file needs metadata {DIGEST_KEY} (64 lowercase hex) and {SIZE_KEY} (an \
             integer) so the reference stays checkable"
        ));
    }
    Ok(())
}

/// A check or build states its result, so a verifier reads it without opening the logs.
fn check_outcome(artifact: &Artifact) -> Result<(), String> {
    if !matches!(
        artifact.kind,
        ArtifactKind::TestResult | ArtifactKind::Build
    ) {
        return Ok(());
    }
    match artifact.metadata.get(OUTCOME_KEY) {
        Some(outcome) if OUTCOMES.contains(&outcome.as_str()) => Ok(()),
        _ => Err(format!(
            "a test result or build needs metadata {OUTCOME_KEY}: one of {}",
            OUTCOMES.join(", ")
        )),
    }
}

#[cfg(test)]
mod tests;
