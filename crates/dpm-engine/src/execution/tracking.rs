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
    // A request resolving to an existing record is still checked, so no unknown kind or
    // non-canonical spelling can pass by matching a record.
    request.identity.validate(request.reference)?;
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
    // Only a tracking link restates the recorded kind, so context links cannot restate what the
    // tracked object is. Relinking with the same role may only restate the kind within the
    // provider table's kind-change rule; any other repeat is a duplicate.
    let restates = request.role == ExternalLinkRole::Tracks
        && request.identity.refines_kind_of(&reference.identity);
    if let Some(existing) = reference.links.iter().find(|l| l.work == request.work)
        && !(restates && existing.role == request.role)
    {
        return Err(EngineError::DuplicateExternalLink {
            work: request.work,
            reference: request.reference,
        });
    }
    if request.role == ExternalLinkRole::Tracks
        && let Some(owner) = reference.tracking_owner()
        && owner != request.work
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
    if let (Some(state), Some(previous)) = (request.observed, &reference.observation)
        && !previous.admits(state)
    {
        return Err(EngineError::InvalidCommand {
            entity: reference.id.to_string(),
            reason: format!(
                "{} was observed Merged by {} at {}; a merged pull request cannot become {state:?}",
                reference.identity, previous.observed_by, previous.observed_at
            ),
        });
    }
    if restates {
        reference.identity.kind = request.identity.kind.clone();
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
        .find(|r| r.identity.object_key() == request.identity.object_key());
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
mod tests;
