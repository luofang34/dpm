use super::{ExternalLinkRole, ExternalObjectKind, ExternalReference, ExternalState, credentials};
use crate::{Plan, ValidationError, validation::invalid};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn validate(plan: &Plan) -> Result<(), ValidationError> {
    let mut identities = BTreeMap::new();
    for (id, reference) in &plan.external_references {
        if *id != reference.id {
            return Err(invalid(
                "external reference",
                id,
                format!("map key differs from value id {}", reference.id),
            ));
        }
        reference.identity.validate(*id)?;
        if let Some(previous) = identities.insert(reference.identity.object_key(), id) {
            return Err(invalid(
                "external reference",
                id,
                format!(
                    "identity {} is already recorded as {previous}",
                    reference.identity
                ),
            ));
        }
        validate_display(reference)?;
        validate_links(plan, reference)?;
    }
    Ok(())
}

fn validate_display(reference: &ExternalReference) -> Result<(), ValidationError> {
    let id = reference.id;
    if reference.label.trim().is_empty() {
        return Err(invalid("external reference", id, "label must not be empty"));
    }
    credentials::check_text(&reference.label)
        .map_err(|reason| invalid("external reference label", id, reason))?;
    if let Some(url) = &reference.url {
        credentials::check_url(url, &reference.identity.instance)
            .map_err(|reason| invalid("external reference url", id, reason))?;
    }
    if let Some(observation) = &reference.observation {
        if observation.observed_by.name.trim().is_empty() {
            return Err(invalid("external observation", id, "actor is required"));
        }
        if observation.state == ExternalState::Merged
            && reference.identity.kind != ExternalObjectKind::PullRequest
        {
            return Err(invalid(
                "external observation",
                id,
                "only pull requests can be merged",
            ));
        }
    }
    Ok(())
}

fn validate_links(plan: &Plan, reference: &ExternalReference) -> Result<(), ValidationError> {
    let id = reference.id;
    if reference.links.is_empty() {
        return Err(invalid(
            "external reference",
            id,
            "a reference needs at least one work link; unlinking the last link removes it",
        ));
    }
    let mut works = BTreeSet::new();
    for link in &reference.links {
        if !plan.work_items.contains_key(&link.work) {
            return Err(invalid(
                "external link",
                id,
                format!("missing work {}", link.work),
            ));
        }
        if !works.insert(link.work) {
            return Err(invalid(
                "external link",
                id,
                format!("work {} is linked more than once", link.work),
            ));
        }
    }
    let trackers = reference
        .links
        .iter()
        .filter(|link| link.role == ExternalLinkRole::Tracks)
        .count();
    if trackers > 1 {
        return Err(invalid(
            "external link",
            id,
            format!(
                "{} has {trackers} tracking owners; one work item may track it",
                reference.identity
            ),
        ));
    }
    Ok(())
}
