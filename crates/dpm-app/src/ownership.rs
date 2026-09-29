use crate::{AppError, Application};
use dpm_engine::{Command, EngineError};
use dpm_model::ActorId;

impl Application {
    /// Build a release command for a work key; the engine decides whether the caller may release.
    pub fn release_command_blocking(&self, key: &str, reason: String) -> Result<Command, AppError> {
        self.ensure_writable()?;
        Ok(Command::Release {
            work: self.work_id_blocking(key)?,
            reason,
        })
    }

    /// Build a handoff command naming the owner observed in the current snapshot as `from`, so the
    /// operation records both sides and a concurrent change of owner is refused.
    ///
    /// `to` uses the `KIND:NAME` spelling shared by every adapter.
    pub fn handoff_command_blocking(
        &self,
        key: &str,
        to: &str,
        reason: String,
    ) -> Result<Command, AppError> {
        self.ensure_writable()?;
        let to: ActorId = to.parse()?;
        let plan = self.plan_blocking()?;
        let item = plan
            .find_work_by_key(key)
            .ok_or_else(|| AppError::UnknownWork(key.into()))?;
        let from = item
            .execution
            .owner
            .clone()
            .ok_or(EngineError::InvalidTransition {
                work: item.id,
                status: item.execution.status,
            })?;
        Ok(Command::Handoff {
            work: item.id,
            from,
            to,
            reason,
        })
    }
}

#[cfg(all(test, feature = "sqlite"))]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
