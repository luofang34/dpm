use crate::{app::actor, args::OwnershipCommand, error::CliError};
use dpm_app::Application;
use dpm_engine::Command;
use dpm_model::ActorId;

/// Map claim, release and handoff flags to the shared application requests the agent adapter uses.
pub(crate) fn mutation_blocking(
    app: &Application,
    command: OwnershipCommand,
) -> Result<(ActorId, Command), CliError> {
    Ok(match command {
        OwnershipCommand::Claim { key, actor: who } => (
            actor(&who)?,
            Command::Claim {
                work: app.work_id_blocking(&key)?,
            },
        ),
        OwnershipCommand::Release {
            key,
            reason,
            actor: who,
        } => (actor(&who)?, app.release_command_blocking(&key, reason)?),
        OwnershipCommand::Handoff {
            key,
            to,
            reason,
            actor: who,
        } => (
            actor(&who)?,
            app.handoff_command_blocking(&key, &to, reason)?,
        ),
    })
}
