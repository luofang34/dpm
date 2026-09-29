use super::invalid;
use crate::EngineError;
use dpm_model::Plan;

/// Waivers are accountable acts of a human or service, so only waive/restore commands record them.
pub(super) fn protect_waivers(current: &Plan, proposed: &Plan) -> Result<(), EngineError> {
    // A waiver records who accepted skipping one specific constraint; editing or deleting the
    // edge under it would attribute a different constraint to that decision, or erase it.
    for waived in current.dependencies.iter().filter(|e| e.waiver.is_some()) {
        if proposed.find_dependency(waived.id) != Some(waived) {
            return Err(invalid(
                waived.id,
                "restore a waived dependency before a reviewed change edits or removes it",
            ));
        }
    }
    for edge in &proposed.dependencies {
        let before = current
            .find_dependency(edge.id)
            .and_then(|e| e.waiver.as_ref());
        if edge.waiver.as_ref() != before {
            return Err(invalid(
                edge.id,
                "waivers change only through the waive/restore dependency commands",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
