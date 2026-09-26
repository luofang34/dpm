use super::{ValidationError, invalid};
use crate::*;
use std::{collections::BTreeSet, fmt::Display};

pub(super) fn validate(plan: &Plan) -> Result<(), ValidationError> {
    nonempty("workspace", plan.workspace.id, &plan.workspace.name)?;
    entries(
        "project",
        plan.projects.iter().map(|(id, p)| (id, &p.id, &p.key)),
    )?;
    entries(
        "work",
        plan.work_items.iter().map(|(id, w)| (id, &w.id, &w.key)),
    )?;
    entries(
        "requirement",
        plan.requirements.iter().map(|(id, r)| (id, &r.id, &r.key)),
    )?;
    entries(
        "decision",
        plan.decisions.iter().map(|(id, d)| (id, &d.id, &d.key)),
    )?;
    entries("risk", plan.risks.iter().map(|(id, r)| (id, &r.id, &r.key)))?;
    for project in plan.projects.values() {
        nonempty("project title", project.id, &project.title)?;
    }
    for work in plan.work_items.values() {
        validate_work(plan, work)?;
    }
    validate_context(plan)
}

fn entries<'a, I: Display + PartialEq + 'a>(
    entity: &'static str,
    entries: impl Iterator<Item = (&'a I, &'a I, &'a Key)>,
) -> Result<(), ValidationError> {
    let mut keys = BTreeSet::new();
    for (id, value_id, key) in entries {
        if id != value_id {
            return Err(invalid(
                entity,
                id,
                format!("map key differs from value id {value_id}"),
            ));
        }
        nonempty(entity, id, &key.0)?;
        if key.0.trim() != key.0 || !keys.insert(key.0.as_str()) {
            return Err(invalid(
                entity,
                id,
                format!("key {key} has whitespace or is duplicated"),
            ));
        }
    }
    Ok(())
}

pub(super) fn nonempty(
    entity: &'static str,
    id: impl Display,
    text: &str,
) -> Result<(), ValidationError> {
    if text.trim().is_empty() {
        return Err(invalid(entity, id, "text must not be empty"));
    }
    Ok(())
}

fn validate_work(plan: &Plan, work: &WorkItem) -> Result<(), ValidationError> {
    nonempty("work title", work.id, &work.title)?;
    super::instructions::validate(work)?;
    if let Some(review) = &work.last_rejection {
        nonempty("review reason", work.id, &review.reason)?;
        nonempty("review actor", work.id, &review.actor.name)?;
        if !work.is_executable()
            || work.owner.is_none()
            || work.owner.as_ref() == Some(&review.actor)
        {
            return Err(invalid(
                "review",
                work.id,
                "rejection requires an owned task and an independent reviewer",
            ));
        }
    }
    if work.reported_progress_percent > 100
        || (work.reported_progress_percent > 0 && (!work.is_executable() || work.owner.is_none()))
    {
        return Err(invalid(
            "work progress",
            work.id,
            "progress must be 0..100 and nonzero reports require an owned task",
        ));
    }
    project_reference(plan, work.project, work.id)?;
    if let Some(estimate) = work.estimate {
        estimate
            .validate()
            .map_err(|source| ValidationError::Estimate {
                work: work.id,
                source,
            })?;
    }
    for requirement in &work.requirement_ids {
        if !plan.requirements.contains_key(requirement) {
            return Err(invalid(
                "work",
                work.id,
                format!("missing requirement {requirement}"),
            ));
        }
    }
    for artifact in &work.artifact_ids {
        if !plan.artifacts.contains_key(artifact) {
            return Err(invalid(
                "work",
                work.id,
                format!("missing artifact {artifact}"),
            ));
        }
    }
    if let Some(owner) = &work.owner {
        nonempty("actor", work.id, &owner.name)?;
    }
    if !work.is_executable() {
        if work.owner.is_some()
            || work.block_reason.is_some()
            || work.estimate.is_some()
            || !matches!(work.status, WorkStatus::Planned | WorkStatus::Proposed)
        {
            return Err(invalid(
                "work",
                work.id,
                "aggregate work cannot carry task execution state",
            ));
        }
        return Ok(());
    }
    validate_lifecycle(work)
}

fn validate_lifecycle(work: &WorkItem) -> Result<(), ValidationError> {
    if work.status != WorkStatus::Proposed {
        nonempty("work objective", work.id, &work.objective)?;
        if work.acceptance.is_empty() || work.acceptance.iter().any(|c| c.text.trim().is_empty()) {
            return Err(invalid(
                "work",
                work.id,
                "executable work requires concrete acceptance criteria",
            ));
        }
    }
    if (work.status == WorkStatus::Blocked) != work.block_reason.is_some() {
        return Err(invalid(
            "work",
            work.id,
            "block reason and blocked lifecycle must agree",
        ));
    }
    if let Some(reason) = &work.block_reason {
        nonempty("block reason", work.id, reason)?;
    }
    let requires_owner = matches!(
        work.status,
        WorkStatus::Claimed
            | WorkStatus::InProgress
            | WorkStatus::Submitted
            | WorkStatus::Verified
            | WorkStatus::Done
    );
    let forbids_owner = matches!(work.status, WorkStatus::Proposed | WorkStatus::Planned);
    if (requires_owner && work.owner.is_none()) || (forbids_owner && work.owner.is_some()) {
        return Err(invalid(
            "work",
            work.id,
            "ownership does not match lifecycle",
        ));
    }
    Ok(())
}

fn project_reference(
    plan: &Plan,
    project: ProjectId,
    entity: impl Display,
) -> Result<(), ValidationError> {
    if !plan.projects.contains_key(&project) {
        return Err(invalid(
            "project reference",
            entity,
            format!("missing project {project}"),
        ));
    }
    Ok(())
}

fn validate_context(plan: &Plan) -> Result<(), ValidationError> {
    for requirement in plan.requirements.values() {
        project_reference(plan, requirement.project, requirement.id)?;
        nonempty("requirement", requirement.id, &requirement.statement)?;
        nonempty("requirement title", requirement.id, &requirement.title)?;
    }
    for (id, artifact) in &plan.artifacts {
        if id != &artifact.id {
            return Err(invalid("artifact", id, "map key differs from artifact id"));
        }
        nonempty("artifact URI", id, &artifact.uri)?;
        nonempty("artifact creator", id, &artifact.created_by.name)?;
    }
    for decision in plan.decisions.values() {
        validate_decision(plan, decision)?;
    }
    for risk in plan.risks.values() {
        project_reference(plan, risk.project, risk.id)?;
        nonempty("risk description", risk.id, &risk.description)?;
        if !risk.probability.is_finite() || !(0.0..=1.0).contains(&risk.probability) {
            return Err(invalid(
                "risk",
                risk.id,
                "probability must be finite and within [0, 1]",
            ));
        }
        for id in &risk.related_work {
            if !plan.work_items.contains_key(id) {
                return Err(invalid(
                    "risk",
                    risk.id,
                    format!("missing related work {id}"),
                ));
            }
        }
    }
    Ok(())
}

fn validate_decision(plan: &Plan, decision: &Decision) -> Result<(), ValidationError> {
    project_reference(plan, decision.project, decision.id)?;
    nonempty("decision", decision.id, &decision.question)?;
    if decision.status == DecisionStatus::Open && decision.outcome.is_some() {
        return Err(invalid(
            "decision",
            decision.id,
            "open decision cannot carry a resolution",
        ));
    }
    if decision.status == DecisionStatus::Decided
        && decision
            .outcome
            .as_ref()
            .is_none_or(|o| o.trim().is_empty())
    {
        return Err(invalid(
            "decision",
            decision.id,
            "resolved decision requires a non-empty outcome",
        ));
    }
    if let Some(rationale) = &decision.rationale {
        nonempty("decision rationale", decision.id, rationale)?;
    }
    for (relation, ids) in [
        ("blocked", &decision.blocks),
        ("related", &decision.related_work),
    ] {
        for id in ids {
            if !plan.work_items.contains_key(id) {
                return Err(invalid(
                    "decision",
                    decision.id,
                    format!("missing {relation} work {id}"),
                ));
            }
        }
    }
    for id in &decision.artifact_ids {
        if !plan.artifacts.contains_key(id) {
            return Err(invalid(
                "decision",
                decision.id,
                format!("missing source artifact {id}"),
            ));
        }
    }
    Ok(())
}
