use super::invalid;
use crate::EngineError;
use dpm_model::Plan;

/// Waivers are accountable acts of a human or service, so only waive/restore commands record them.
pub(super) fn protect_waivers(current: &Plan, proposed: &Plan) -> Result<(), EngineError> {
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
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
