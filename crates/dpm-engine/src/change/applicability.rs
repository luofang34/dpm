use dpm_model::{Applicability, Key, Plan, WorkItemId, WorkStatus};
use serde::{Deserialize, Serialize};

/// Work whose membership in the active graph a proposal changes.
///
/// A reviewed change may switch a choice under started work; the work keeps its lifecycle and
/// evidence rather than being cancelled, and this entry makes the consequence visible to review.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApplicabilityChange {
    /// Stable identity of the affected work.
    pub work: WorkItemId,
    /// Human-readable key of the affected work.
    pub key: Key,
    /// Current lifecycle.
    pub status: WorkStatus,
    /// Whether the work is claimed, started, submitted, blocked or accepted.
    pub in_flight: bool,
    /// Applicability in the current plan; absent for new work.
    pub before: Option<Applicability>,
    /// Applicability in the proposed plan; absent for removed work.
    pub after: Option<Applicability>,
}

pub(super) fn changes(current: &Plan, proposed: &Plan) -> Vec<ApplicabilityChange> {
    let before = current.applicability();
    let after = proposed.applicability();
    let mut ids: Vec<_> = before.keys().chain(after.keys()).copied().collect();
    ids.sort();
    ids.dedup();
    let mut changes: Vec<_> = ids
        .into_iter()
        .filter(|id| before.get(id) != after.get(id))
        .filter(|id| {
            before.contains_key(id) && after.contains_key(id)
                || !is_plain(before.get(id).or(after.get(id)))
        })
        .filter_map(|id| {
            let work = proposed
                .work_items
                .get(&id)
                .or_else(|| current.work_items.get(&id))?;
            Some(ApplicabilityChange {
                work: id,
                key: work.key.clone(),
                status: work.status,
                in_flight: !matches!(work.status, WorkStatus::Proposed | WorkStatus::Planned),
                before: before.get(&id).cloned(),
                after: after.get(&id).cloned(),
            })
        })
        .collect();
    changes.sort_by(|a, b| a.key.cmp(&b.key));
    changes
}

/// Added or removed unconditional applicable work is already visible as an entity change.
fn is_plain(state: Option<&Applicability>) -> bool {
    state.is_none_or(Applicability::is_applicable)
}
