//! Authoritative domain types and structural validation for execution plans.

mod actor;
mod completion;
pub use completion::{completion, decisions_resolved};
mod context;
mod identity;
mod instructions;
mod plan;
mod project;
mod resource;
pub use resource::*;
mod validation;
mod work;

pub use actor::*;
pub use context::*;
pub use identity::*;
pub use instructions::*;
pub use plan::*;
pub use project::*;
pub use validation::{EstimateError, ValidationError};
pub use work::*;
