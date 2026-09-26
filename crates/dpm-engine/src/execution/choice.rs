//! Resolving a decision, including a structured choice that decides which conditional work applies.

use super::nonempty;
use crate::EngineError;
use chrono::{DateTime, Utc};
use dpm_model::{DecisionId, DecisionStatus, Plan, WorkStatus};

/// Resolve an open decision; a structured choice must name one of its option keys.
///
/// A choice that would take started or reserved work out of the active graph is refused: that
/// change needs a reviewed plan change, and in-flight work is never cancelled implicitly.
pub(super) fn decide(
    plan: &mut Plan,
    decision: DecisionId,
    outcome: &str,
    at: DateTime<Utc>,
) -> Result<(), EngineError> {
    nonempty(&decision.to_string(), "outcome", outcome)?;
    let before = plan.applicability();
    let gate = plan
        .decisions
        .get_mut(&decision)
        .ok_or(EngineError::MissingDecision(decision))?;
    if gate.status != DecisionStatus::Open {
        return Err(EngineError::DecisionNotOpen(decision));
    }
    if !gate.options.is_empty() && !gate.has_option(outcome) {
        let options: Vec<_> = gate.options.iter().map(|o| o.key.as_str()).collect();
        return Err(EngineError::InvalidCommand {
            entity: gate.key.to_string(),
            reason: format!(
                "outcome {outcome:?} must be one of the option keys {}",
                options.join(", ")
            ),
        });
    }
    gate.status = DecisionStatus::Decided;
    gate.outcome = Some(outcome.into());
    gate.resolved_at = Some(at);
    let key = gate.key.clone();
    let after = plan.applicability();
    let affected: Vec<_> = plan
        .work_items
        .values()
        .filter(|w| !matches!(w.status, WorkStatus::Proposed | WorkStatus::Planned))
        .filter(|w| {
            let now = after.get(&w.id);
            now != before.get(&w.id) && now.is_some_and(|a| !a.is_applicable())
        })
        .map(|w| w.key.to_string())
        .collect();
    if !affected.is_empty() {
        return Err(EngineError::InvalidCommand {
            entity: key.to_string(),
            reason: format!(
                "this choice would exclude in-flight work {}; change it through a reviewed plan change",
                affected.join(", ")
            ),
        });
    }
    Ok(())
}
