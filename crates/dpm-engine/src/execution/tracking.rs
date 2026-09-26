//! External tracking links: context attached to work that never changes its lifecycle,
//! evidence, dependencies or gates.

use crate::{EngineError, ExternalLinkRequest};
use chrono::{DateTime, Utc};
use dpm_model::{
    ActorId, ExternalLink, ExternalLinkRole, ExternalObservation, ExternalReference,
    ExternalReferenceId, Plan, WorkItemId,
};
use std::collections::BTreeSet;

pub(super) fn link(
    plan: &mut Plan,
    actor: &ActorId,
    request: &ExternalLinkRequest,
    at: DateTime<Utc>,
) -> Result<(), EngineError> {
    if !plan.work_items.contains_key(&request.work) {
        return Err(EngineError::MissingWorkItem(request.work));
    }
    resolve_identity(plan, request)?;
    let reference = plan
        .external_references
        .entry(request.reference)
        .or_insert_with(|| ExternalReference {
            id: request.reference,
            identity: request.identity.clone(),
            label: request.label.clone(),
            url: None,
            observation: None,
            links: BTreeSet::new(),
        });
    if reference.links.iter().any(|l| l.work == request.work) {
        return Err(EngineError::DuplicateExternalLink {
            work: request.work,
            reference: request.reference,
        });
    }
    if request.role == ExternalLinkRole::Tracks
        && let Some(owner) = reference.tracking_owner()
    {
        return Err(EngineError::TrackingOwned {
            identity: reference.identity.to_string(),
            owner: plan
                .work_items
                .get(&owner)
                .map(|w| w.key.clone())
                .ok_or(EngineError::MissingWorkItem(owner))?,
        });
    }
    reference.label.clone_from(&request.label);
    reference.url.clone_from(&request.url);
    if let Some(state) = request.observed {
        reference.observation = Some(ExternalObservation {
            state,
            observed_at: at,
            observed_by: actor.clone(),
        });
    }
    reference.links.insert(ExternalLink {
        work: request.work,
        role: request.role,
    });
    Ok(())
}

/// An identity maps to exactly one reference identifier, and identifiers are never reused.
fn resolve_identity(plan: &Plan, request: &ExternalLinkRequest) -> Result<(), EngineError> {
    let recorded = plan
        .external_references
        .values()
        .find(|r| r.identity == request.identity);
    match (recorded, plan.external_references.get(&request.reference)) {
        (Some(existing), _) if existing.id != request.reference => {
            Err(EngineError::InvalidCommand {
                entity: request.reference.to_string(),
                reason: format!(
                    "{} is already recorded as external reference {}",
                    request.identity, existing.id
                ),
            })
        }
        (None, Some(other)) => Err(EngineError::InvalidCommand {
            entity: request.reference.to_string(),
            reason: format!(
                "reference id already records {}; identities cannot be rewritten by linking",
                other.identity
            ),
        }),
        _ => Ok(()),
    }
}

pub(super) fn unlink(
    plan: &mut Plan,
    work: WorkItemId,
    reference: ExternalReferenceId,
) -> Result<(), EngineError> {
    if !plan.work_items.contains_key(&work) {
        return Err(EngineError::MissingWorkItem(work));
    }
    let record = plan
        .external_references
        .get_mut(&reference)
        .ok_or(EngineError::MissingExternalReference(reference))?;
    let link = record
        .links
        .iter()
        .find(|l| l.work == work)
        .copied()
        .ok_or(EngineError::MissingExternalLink { work, reference })?;
    record.links.remove(&link);
    if record.links.is_empty() {
        plan.external_references.remove(&reference);
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
