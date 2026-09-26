use crate::{AppError, Application};
use dpm_engine::{Command, ExternalLinkRequest};
use dpm_model::{
    ExternalIdentity, ExternalLinkRole, ExternalReference, ExternalReferenceId, ExternalState, Plan,
};
use serde::{Deserialize, Serialize};

/// Adapter-neutral link input; CLI flags and MCP arguments both build this value.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalLinkInput {
    /// Provider identity; equivalent spellings are canonicalized before lookup.
    pub identity: ExternalIdentity,
    /// Display label; defaults to the recorded label, or the identity for a new reference.
    #[serde(default)]
    pub label: Option<String>,
    /// Optional display URL on the identity's instance.
    #[serde(default)]
    pub url: Option<String>,
    /// Relationship of the work to the external object.
    pub role: ExternalLinkRole,
    /// Optional reported external state, recorded as an observation only.
    #[serde(default)]
    pub observed: Option<ExternalState>,
}

impl Application {
    /// Build a link command for a work key, reusing the reference already recorded for the identity.
    pub fn external_link_command_blocking(
        &self,
        key: &str,
        input: ExternalLinkInput,
    ) -> Result<Command, AppError> {
        self.ensure_writable()?;
        let plan = self.plan_blocking()?;
        let work = work_id(&plan, key)?;
        let identity = input.identity.canonical();
        let recorded = recorded(&plan, &identity);
        let label = input
            .label
            .or_else(|| recorded.map(|r| r.label.clone()))
            .unwrap_or_else(|| identity.to_string());
        Ok(Command::LinkExternal(ExternalLinkRequest {
            work,
            reference: recorded.map_or_else(ExternalReferenceId::new, |r| r.id),
            identity,
            label,
            url: input.url.or_else(|| recorded.and_then(|r| r.url.clone())),
            role: input.role,
            observed: input.observed,
        }))
    }

    /// Build an unlink command; an identity that was never recorded is reported as not found.
    pub fn external_unlink_command_blocking(
        &self,
        key: &str,
        identity: &ExternalIdentity,
    ) -> Result<Command, AppError> {
        self.ensure_writable()?;
        let plan = self.plan_blocking()?;
        let work = work_id(&plan, key)?;
        let identity = identity.canonical();
        let reference = recorded(&plan, &identity)
            .ok_or_else(|| AppError::UnknownExternalReference(identity.to_string()))?
            .id;
        Ok(Command::UnlinkExternal { work, reference })
    }
}

fn work_id(plan: &Plan, key: &str) -> Result<dpm_model::WorkItemId, AppError> {
    plan.find_work_by_key(key)
        .map(|w| w.id)
        .ok_or_else(|| AppError::UnknownWork(key.into()))
}

fn recorded<'a>(plan: &'a Plan, identity: &ExternalIdentity) -> Option<&'a ExternalReference> {
    plan.external_references
        .values()
        .find(|r| r.identity == *identity)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
