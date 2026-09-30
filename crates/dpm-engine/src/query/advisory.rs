//! Explained advice attached to `next` that never hides, removes or reorders candidates.

use dpm_model::{ActorId, Key, Plan, WorkStatus};
use serde::{Deserialize, Serialize};

/// Advice about the caller's situation, shown beside the ranking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Advisory {
    /// The actor already holds claimed, started or blocked work.
    HoldingClaims {
        /// Actor the advice is for.
        actor: ActorId,
        /// Keys of the work it holds, in key order.
        holding: Vec<Key>,
        /// Why this is worth knowing before claiming more.
        reason: String,
    },
}

/// Advice for an actor about to choose work: whether it already holds claims.
#[must_use]
pub fn advisories(plan: &Plan, actor: &ActorId) -> Vec<Advisory> {
    const HELD: [WorkStatus; 3] = [
        WorkStatus::Claimed,
        WorkStatus::InProgress,
        WorkStatus::Blocked,
    ];
    let mut holding: Vec<Key> = plan
        .work_items
        .values()
        .filter(|w| HELD.contains(&w.execution.status))
        .filter(|w| w.execution.owner.as_ref() == Some(actor))
        .map(|w| w.key.clone())
        .collect();
    if holding.is_empty() {
        return Vec::new();
    }
    holding.sort_by(Key::natural_cmp);
    let reason = format!(
        "{actor} already holds {} claimed, started or blocked task(s); finishing, releasing or \
         handing off held work before claiming more keeps work in progress and its aging low. \
         Advice only: the candidates are neither filtered nor reordered",
        holding.len()
    );
    vec![Advisory::HoldingClaims {
        actor: actor.clone(),
        holding,
        reason,
    }]
}

#[cfg(test)]
mod tests;
