//! Evidence that is a local file: the file stays where it is, and the artifact records its location,
//! SHA-256 digest and size, so the reference can be checked after the file moves or changes.

use crate::{AppError, Application};
use chrono::Utc;
use dpm_model::{ActorId, Artifact, ArtifactId, ArtifactKind};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

/// What `attach-file` / `attach_file` is asked to record besides the file itself.
#[derive(Debug, Clone)]
pub struct FileEvidence {
    /// The file, absolute or relative to the caller's directory.
    pub path: PathBuf,
    /// Its domain category.
    pub kind: ArtifactKind,
    /// What it shows.
    pub label: String,
    /// Further attributes, such as `outcome`, `command` or `commit`.
    pub metadata: BTreeMap<String, String>,
}

impl Application {
    /// The artifact referencing a local file, fingerprinted now; the file is read, never copied.
    pub fn file_artifact_blocking(
        &self,
        actor: ActorId,
        evidence: FileEvidence,
    ) -> Result<Artifact, AppError> {
        self.ensure_writable()?;
        let unreadable = |source| AppError::EvidenceFile {
            path: evidence.path.clone(),
            source,
        };
        let path = evidence.path.canonicalize().map_err(unreadable)?;
        let (digest, bytes) = fingerprint_blocking(&path).map_err(unreadable)?;
        for key in [dpm_engine::DIGEST_KEY, dpm_engine::SIZE_KEY] {
            if evidence.metadata.contains_key(key) {
                return Err(AppError::InvalidRequest(format!(
                    "metadata {key} is measured from the file, not supplied"
                )));
            }
        }
        let mut metadata = evidence.metadata;
        metadata.insert(dpm_engine::DIGEST_KEY.into(), digest);
        metadata.insert(dpm_engine::SIZE_KEY.into(), bytes.to_string());
        Ok(Artifact {
            id: ArtifactId::new(),
            kind: evidence.kind,
            uri: file_uri(&path),
            label: evidence.label,
            metadata,
            created_by: actor,
            created_at: Utc::now(),
        })
    }
}

fn fingerprint_blocking(path: &Path) -> std::io::Result<(String, u64)> {
    let mut file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "evidence must be a regular file",
        ));
    }
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut bytes = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        let Some(chunk) = buffer.get(..read).filter(|chunk| !chunk.is_empty()) else {
            break;
        };
        hasher.update(chunk);
        bytes = bytes.saturating_add(read as u64);
    }
    let digest = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok((digest, bytes))
}

/// A `file://` URL of an absolute path, percent-encoding every byte outside the unreserved set and `/`.
fn file_uri(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let mut uri = String::from("file://");
    if !text.starts_with('/') {
        uri.push('/');
    }
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            uri.push(char::from(byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

#[cfg(test)]
mod tests;
