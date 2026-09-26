use super::{ValidationError, invalid};
use crate::{ActorKind, Dependency, DependencyPolicy, Plan};
use std::collections::BTreeSet;

/// Edge identity, relation uniqueness, waiver and link invariants; dependency endpoints and cycles
/// are checked with the execution graph.
pub(super) fn validate(plan: &Plan) -> Result<(), ValidationError> {
    let mut ids = BTreeSet::new();
    let mut relations = BTreeSet::new();
    for edge in &plan.dependencies {
        if !ids.insert(edge.id) {
            return Err(invalid(
                "dependency",
                edge.id,
                "duplicate dependency identity",
            ));
        }
        if !relations.insert((edge.predecessor, edge.successor, edge.kind)) {
            return Err(invalid(
                "dependency",
                edge.id,
                format!(
                    "duplicate {} relation for the same ordered pair; edit the existing edge",
                    edge.kind.abbreviation()
                ),
            ));
        }
        policy(edge)?;
    }
    links(plan)
}

fn policy(edge: &Dependency) -> Result<(), ValidationError> {
    if edge.rationale.as_ref().is_some_and(|r| r.trim().is_empty()) {
        return Err(invalid(
            "dependency",
            edge.id,
            "rationale must not be empty",
        ));
    }
    let Some(waiver) = &edge.waiver else {
        return Ok(());
    };
    if edge.policy == DependencyPolicy::Hard {
        return Err(invalid(
            "dependency",
            edge.id,
            "hard constraints cannot be waived",
        ));
    }
    if waiver.actor.kind == ActorKind::Agent
        || waiver.actor.name.trim().is_empty()
        || waiver.reason.trim().is_empty()
    {
        return Err(invalid(
            "dependency",
            edge.id,
            "a waiver requires a named human or service actor and a reason",
        ));
    }
    Ok(())
}

fn links(plan: &Plan) -> Result<(), ValidationError> {
    let mut seen = BTreeSet::new();
    for link in &plan.links {
        let label = format!("{:?} {} -> {}", link.kind, link.source, link.target);
        for id in [link.source, link.target] {
            if !plan.work_items.contains_key(&id) {
                return Err(invalid("work link", &label, format!("missing work {id}")));
            }
        }
        if link.source == link.target {
            return Err(invalid(
                "work link",
                &label,
                "a work item cannot link to itself",
            ));
        }
        if link.note.as_ref().is_some_and(|n| n.trim().is_empty()) {
            return Err(invalid("work link", &label, "note must not be empty"));
        }
        // A reversed copy would repeat the relationship or, for directed kinds, contradict it.
        let pair = (
            link.kind,
            link.source.min(link.target),
            link.source.max(link.target),
        );
        if !seen.insert(pair) {
            return Err(invalid(
                "work link",
                &label,
                "duplicate link of this kind between the same work items",
            ));
        }
    }
    Ok(())
}
