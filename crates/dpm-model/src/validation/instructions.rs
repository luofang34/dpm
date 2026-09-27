use super::{ValidationError, entities::nonempty, invalid};
use crate::WorkItem;

pub(super) fn validate(work: &WorkItem) -> Result<(), ValidationError> {
    let Some(instructions) = &work.contract.instructions else {
        return Ok(());
    };
    if !work.is_executable() || instructions.steps.is_empty() {
        return Err(invalid(
            "work instructions",
            work.id,
            "instructions require an executable task and at least one step",
        ));
    }
    for (index, step) in instructions.steps.iter().enumerate() {
        if step.action.trim().is_empty() || step.expected_result.trim().is_empty() {
            return Err(invalid(
                "work instructions",
                work.id,
                format!("step at index {index} requires action and expected_result"),
            ));
        }
    }
    for (name, values) in [
        ("in_scope", &instructions.in_scope),
        ("out_of_scope", &instructions.out_of_scope),
        ("verification", &instructions.verification),
    ] {
        if values.is_empty() {
            return Err(invalid(
                "work instructions",
                work.id,
                format!("{name} must not be empty"),
            ));
        }
        for text in values {
            nonempty(name, work.id, text)?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests;
