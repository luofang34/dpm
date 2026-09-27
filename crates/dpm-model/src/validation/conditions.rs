use super::{ValidationError, invalid};
use crate::{Decision, DecisionStatus, Plan, WorkKind};
use std::collections::BTreeSet;

/// Decision options, work conditions and join policies must reference a well-formed choice.
pub(super) fn validate(plan: &Plan) -> Result<(), ValidationError> {
    for decision in plan.decisions.values() {
        options(decision)?;
        if let Some(target) = decision.supersedes.and_then(|id| plan.decisions.get(&id))
            && option_keys(target) != option_keys(decision)
        {
            // Conditions follow replacements, so a replacement must offer exactly the same keys.
            return Err(invalid(
                "decision",
                decision.id,
                format!("a replacement must keep the option keys of {}", target.key),
            ));
        }
    }
    for work in plan.work_items.values() {
        if let Some(condition) = &work.contract.condition {
            let decision = plan.decisions.get(&condition.decision).ok_or_else(|| {
                invalid(
                    "work condition",
                    work.id,
                    format!("missing decision {}", condition.decision),
                )
            })?;
            if !decision.has_option(&condition.option) {
                return Err(invalid(
                    "work condition",
                    work.id,
                    format!(
                        "decision {} has no option {:?}; conditions name a structured option",
                        decision.key, condition.option
                    ),
                ));
            }
        }
        if !work.contract.join.is_default() && work.kind == WorkKind::WorkPackage {
            return Err(invalid(
                "work join",
                work.id,
                "join policies apply to task and milestone merge points, not work packages",
            ));
        }
    }
    Ok(())
}

fn options(decision: &Decision) -> Result<(), ValidationError> {
    if decision.options.is_empty() {
        return Ok(());
    }
    if decision.options.len() < 2 {
        return Err(invalid(
            "decision options",
            decision.id,
            "a structured choice needs at least two options",
        ));
    }
    let mut keys = BTreeSet::new();
    for option in &decision.options {
        if option.key.trim().is_empty()
            || option.key.trim() != option.key
            || option.label.trim().is_empty()
            || !keys.insert(option.key.as_str())
        {
            return Err(invalid(
                "decision options",
                decision.id,
                format!(
                    "option {:?} needs a unique trimmed key and a label",
                    option.key
                ),
            ));
        }
    }
    let selected = decision.outcome.as_deref();
    if decision.status != DecisionStatus::Open && !selected.is_some_and(|o| keys.contains(o)) {
        return Err(invalid(
            "decision",
            decision.id,
            format!(
                "outcome {:?} must be one of the option keys {keys:?}",
                selected.unwrap_or_default()
            ),
        ));
    }
    Ok(())
}

fn option_keys(decision: &Decision) -> BTreeSet<&str> {
    decision.options.iter().map(|o| o.key.as_str()).collect()
}
