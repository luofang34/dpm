use super::invalid;
use crate::EngineError;
use crate::context::{applicable_decisions, lineage};
use dpm_model::{Decision, DecisionStatus, Key, Plan, WorkItemId, WorkKind, WorkStatus};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Work whose execution context contains a replaced decision or its replacement.
///
/// Listing it is a prompt for review, not a gate or a lifecycle change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AffectedWork {
    /// Decision being superseded.
    pub decision: Key,
    /// Decision recording the new choice.
    pub replacement: Key,
    /// Stable identity of the affected work.
    pub work: WorkItemId,
    /// Human-readable key of the affected work.
    pub key: Key,
    /// Kind of the affected work.
    pub kind: WorkKind,
    /// Current lifecycle, so reviewers see started work that may need reassessment.
    pub status: WorkStatus,
}

/// Existing decisions change only by superseding a Decided choice with a linked replacement, or by
/// refining an Open question without loosening it.
///
/// An Open gate is resolved only by `decide`; superseding it here would bypass that gate.
pub(super) fn validate(
    current: &Plan,
    proposed: &Plan,
    locked: &BTreeSet<WorkItemId>,
) -> Result<(), EngineError> {
    for decision in current.decisions.values() {
        match proposed.decisions.get(&decision.id) {
            Some(next) if next == decision => {}
            Some(next) if refines_open(decision, next) => {
                let added: BTreeSet<_> = next.blocks.difference(&decision.blocks).collect();
                if added.iter().any(|id| locked.contains(id)) {
                    return Err(invalid(
                        &decision.key,
                        "cannot add a gate to the basis of existing execution",
                    ));
                }
            }
            Some(next) if supersedes_only(decision, next) => {
                if !proposed
                    .decisions
                    .values()
                    .any(|d| d.supersedes == Some(decision.id))
                {
                    return Err(invalid(
                        &decision.key,
                        "a superseded decision needs a linked replacement",
                    ));
                }
            }
            _ => {
                return Err(invalid(
                    &decision.key,
                    "existing decisions change only when a linked replacement supersedes a Decided choice; use decide for an open outcome",
                ));
            }
        }
    }
    for decision in proposed
        .decisions
        .values()
        .filter(|d| !current.decisions.contains_key(&d.id))
    {
        match decision.supersedes {
            None => new_question(decision, locked)?,
            Some(target) => replacement(current, decision, target)?,
        }
    }
    Ok(())
}

/// A sharper question, more context or more gates on a still-Open decision; outcome, options and
/// existing gates stay as they are, so nothing a reviewer or executor relies on is loosened.
fn refines_open(before: &Decision, after: &Decision) -> bool {
    before.status == DecisionStatus::Open
        && after.blocks.is_superset(&before.blocks)
        && Decision {
            question: before.question.clone(),
            related_work: before.related_work.clone(),
            blocks: before.blocks.clone(),
            ..after.clone()
        } == *before
}

fn supersedes_only(before: &Decision, after: &Decision) -> bool {
    before.status == DecisionStatus::Decided
        && after.status == DecisionStatus::Superseded
        && Decision {
            status: before.status,
            ..after.clone()
        } == *before
}

fn new_question(decision: &Decision, locked: &BTreeSet<WorkItemId>) -> Result<(), EngineError> {
    if decision.status != DecisionStatus::Open || decision.outcome.is_some() {
        return Err(invalid(
            &decision.key,
            "new decisions must be open unless they replace a Decided choice",
        ));
    }
    if !decision.blocks.is_disjoint(locked) {
        return Err(invalid(
            &decision.key,
            "cannot add a gate to the basis of existing execution",
        ));
    }
    Ok(())
}

fn replacement(
    current: &Plan,
    decision: &Decision,
    target: dpm_model::DecisionId,
) -> Result<(), EngineError> {
    if current
        .decisions
        .get(&target)
        .is_none_or(|d| d.status != DecisionStatus::Decided)
    {
        return Err(invalid(
            &decision.key,
            "a replacement must supersede an existing Decided choice",
        ));
    }
    if decision.status != DecisionStatus::Decided
        || decision
            .rationale
            .as_ref()
            .is_none_or(|r| r.trim().is_empty())
    {
        return Err(invalid(
            &decision.key,
            "a replacement records a Decided outcome with its rationale",
        ));
    }
    if !decision.blocks.is_empty() {
        return Err(invalid(
            &decision.key,
            "a replacement is context; it cannot add execution gates",
        ));
    }
    Ok(())
}

/// Work whose explain context includes either side of each replacement in the proposal.
pub(super) fn affected_work(current: &Plan, proposed: &Plan) -> Vec<AffectedWork> {
    let mut affected = Vec::new();
    for new in proposed
        .decisions
        .values()
        .filter(|d| !current.decisions.contains_key(&d.id))
    {
        let Some(old) = new.supersedes.and_then(|id| proposed.decisions.get(&id)) else {
            continue;
        };
        for work in proposed.work_items.values() {
            let ids: BTreeSet<_> = lineage(proposed, work).iter().map(|w| w.id).collect();
            // Uses explain's own selection, so every item whose context changes is listed.
            let context = applicable_decisions(proposed, &ids);
            if context.contains(&old.id) || context.contains(&new.id) {
                affected.push(AffectedWork {
                    decision: old.key.clone(),
                    replacement: new.key.clone(),
                    work: work.id,
                    key: work.key.clone(),
                    kind: work.kind,
                    status: work.execution.status,
                });
            }
        }
    }
    affected.sort_by(|a, b| (&a.decision, &a.key).cmp(&(&b.decision, &b.key)));
    affected
}

#[cfg(test)]
mod tests;
