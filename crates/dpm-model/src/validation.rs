use crate::Plan;
use thiserror::Error;

mod dependency;
mod entities;
mod events;
mod graph;
mod instructions;
mod provisional;

/// Invalid duration uncertainty supplied by a plan author.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum EstimateError {
    /// At least one bound is NaN or infinite.
    #[error("estimate values must be finite")]
    NonFinite,
    /// The optimistic bound is negative.
    #[error("optimistic duration must be non-negative")]
    Negative,
    /// The bounds are not ordered optimistic <= likely <= pessimistic.
    #[error("estimate must satisfy optimistic <= likely <= pessimistic")]
    Unordered,
}

/// A graph invariant violated by external or edited domain data.
#[derive(Debug, Error)]
pub enum ValidationError {
    /// Malformed entity, reference, or lifecycle invariant.
    #[error("invalid {entity} {id}: {reason}")]
    Invalid {
        /// Entity category containing the invalid value.
        entity: &'static str,
        /// Identifier or human-readable context of the invalid entity.
        id: String,
        /// Invariant that was violated.
        reason: String,
    },
    /// A task contains invalid duration uncertainty.
    #[error("invalid estimate for work {work}: {source}")]
    Estimate {
        /// Work with the malformed estimate.
        work: crate::WorkItemId,
        /// Original estimate validation failure.
        #[source]
        source: EstimateError,
    },
}

pub(super) fn invalid(
    entity: &'static str,
    id: impl std::fmt::Display,
    reason: impl Into<String>,
) -> ValidationError {
    ValidationError::Invalid {
        entity,
        id: id.to_string(),
        reason: reason.into(),
    }
}

impl Plan {
    /// Validate references, keys, containment, execution contracts, and numerical inputs.
    ///
    /// Call this at import, command, and persistence boundaries. It does not mutate the plan.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.format_version != 2 {
            return Err(invalid(
                "plan format",
                self.workspace.id,
                format!("unsupported version {}; expected 2", self.format_version),
            ));
        }
        crate::resource::validate(self)?;
        entities::validate(self)?;
        crate::tracking::validate(self)?;
        graph::validate(self)?;
        dependency::validate(self)?;
        provisional::validate(self)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
