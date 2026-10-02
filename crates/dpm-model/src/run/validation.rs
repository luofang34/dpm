//! Structural validation of caller-supplied run data, applied before anything is recorded.

use super::{ActivityInput, RunProvenance, RunSession, RunSource, RunStart, RunTransition};
use crate::{ValidationError, validation::invalid};

/// Longest identifier, key or word a run record accepts.
const MAX_IDENTIFIER_BYTES: usize = 256;

/// Longest detail text a lifecycle transition accepts.
pub const MAX_DETAIL_BYTES: usize = 2048;

/// Most exact source references one run may carry.
pub const MAX_SOURCES: usize = 8;

/// Longest model or version a run's provenance accepts.
pub const MAX_PROVENANCE_BYTES: usize = 128;

fn identifier(
    entity: &'static str,
    id: &str,
    field: &str,
    value: &str,
) -> Result<(), ValidationError> {
    if value.trim().is_empty() || value.len() > MAX_IDENTIFIER_BYTES {
        return Err(invalid(
            entity,
            id,
            format!("{field} must be 1..={MAX_IDENTIFIER_BYTES} bytes and not blank"),
        ));
    }
    if value.chars().any(char::is_control) {
        return Err(invalid(
            entity,
            id,
            format!("{field} must not contain control characters"),
        ));
    }
    Ok(())
}

impl RunSession {
    /// Check the provider word and the opaque identifiers.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let word = !self.provider.is_empty()
            && self.provider.len() <= 32
            && self.provider.starts_with(|c: char| c.is_ascii_lowercase())
            && self
                .provider
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
        if !word {
            return Err(invalid(
                "run session",
                &self.provider,
                "provider must be a lowercase word of up to 32 letters, digits, - or _",
            ));
        }
        identifier("run session", &self.provider, "session", &self.session)?;
        if let Some(turn) = &self.turn {
            identifier("run session", &self.provider, "turn", turn)?;
        }
        self.provenance
            .as_ref()
            .map_or(Ok(()), |provenance| provenance.validate(&self.provider))
    }
}

impl RunProvenance {
    /// Check the bounds of each attested fact.
    pub fn validate(&self, provider: &str) -> Result<(), ValidationError> {
        for (name, value) in [
            ("requested_model", &self.requested_model),
            ("observed_model", &self.observed_model),
            ("runtime_version", &self.runtime_version),
        ] {
            if let Some(value) = value {
                let bad = value.trim().is_empty()
                    || value.len() > MAX_PROVENANCE_BYTES
                    || value.chars().any(char::is_control);
                if bad {
                    return Err(invalid(
                        "run provenance",
                        provider,
                        format!(
                            "{name} must be 1..={MAX_PROVENANCE_BYTES} bytes, not blank, without control characters"
                        ),
                    ));
                }
            }
        }
        match &self.configuration_digest {
            Some(digest)
                if !(digest.len() == 64
                    && digest
                        .chars()
                        .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))) =>
            {
                Err(invalid(
                    "run provenance",
                    provider,
                    "configuration_digest must be 64 lowercase hexadecimal digits",
                ))
            }
            _ => Ok(()),
        }
    }
}

/// Whether `commit` is a full lowercase hexadecimal object name of 40 or 64 digits.
#[must_use]
pub fn is_exact_commit(commit: &str) -> bool {
    matches!(commit.len(), 40 | 64)
        && commit
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

impl RunSource {
    /// Check the form of the reference; whether it names something real is the plan's to say.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let Self::GitCommit { commit, .. } = self else {
            return Ok(());
        };
        if is_exact_commit(commit) {
            Ok(())
        } else {
            Err(invalid(
                "run source",
                commit,
                "a commit must be a full lowercase hexadecimal object name of 40 or 64 digits; a branch or abbreviated name is not an exact source",
            ))
        }
    }
}

impl RunStart {
    /// Check the request's own form, without the plan.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if !self.id.is_time_ordered() {
            return Err(invalid("run", self.id, "run id must be a version 7 UUID"));
        }
        if self.executor.name.trim().is_empty() {
            return Err(invalid("run", self.id, "executor name must not be empty"));
        }
        if self.parent == Some(self.id) {
            return Err(invalid("run", self.id, "a run cannot be its own parent"));
        }
        if let Some(session) = &self.session {
            session.validate()?;
        }
        if self.sources.len() > MAX_SOURCES {
            return Err(invalid(
                "run",
                self.id,
                format!("at most {MAX_SOURCES} source references"),
            ));
        }
        self.sources.iter().try_for_each(RunSource::validate)
    }
}

impl RunTransition {
    /// Check the request's own form.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if !self.id.is_time_ordered() {
            return Err(invalid(
                "run transition",
                self.id,
                "event id must be a version 7 UUID",
            ));
        }
        match &self.detail {
            Some(detail) if detail.trim().is_empty() || detail.len() > MAX_DETAIL_BYTES => {
                Err(invalid(
                    "run transition",
                    self.id,
                    format!("detail must be 1..={MAX_DETAIL_BYTES} bytes and not blank when given"),
                ))
            }
            _ => Ok(()),
        }
    }
}

impl ActivityInput {
    /// Check the request's own form; over-long text is truncated, not refused.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.source_sequence == 0 {
            return Err(invalid(
                "run activity",
                self.run,
                "source_sequence starts at 1; zero means no activity yet",
            ));
        }
        Ok(())
    }
}
