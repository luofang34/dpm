use crate::{
    app::actor,
    args::{Commands, ExternalIdentityArgs},
    error::CliError,
};
use dpm_app::{Application, ExternalLinkInput};
use dpm_engine::Command;
use dpm_model::{
    ActorId, ExternalIdentity, ExternalLinkRole, ExternalObjectKind, ExternalProvider,
    ExternalState,
};

/// Map link/unlink flags to the shared application request used by the agent adapter too.
pub(crate) fn mutation_blocking(
    app: &Application,
    command: Commands,
) -> Result<(ActorId, Command), CliError> {
    match command {
        Commands::LinkExternal {
            key,
            identity,
            label,
            url,
            role,
            observed,
            actor: who,
        } => {
            let input = ExternalLinkInput {
                identity: identity_from(identity),
                label,
                url,
                role: role_from(&role)?,
                observed: observed.as_deref().map(state_from).transpose()?,
            };
            Ok((
                actor(&who)?,
                app.external_link_command_blocking(&key, input)?,
            ))
        }
        Commands::UnlinkExternal {
            key,
            identity,
            actor: who,
        } => Ok((
            actor(&who)?,
            app.external_unlink_command_blocking(&key, &identity_from(identity))?,
        )),
        _ => Err(CliError::Input("expected an external link command".into())),
    }
}

fn identity_from(args: ExternalIdentityArgs) -> ExternalIdentity {
    ExternalIdentity {
        provider: ExternalProvider::parse(&args.provider),
        instance: args.instance,
        namespace: args.namespace,
        kind: ExternalObjectKind::parse(&args.kind),
        external_id: args.external_id,
    }
}

fn role_from(value: &str) -> Result<ExternalLinkRole, CliError> {
    match value.to_lowercase().as_str() {
        "tracks" => Ok(ExternalLinkRole::Tracks),
        "relates" => Ok(ExternalLinkRole::Relates),
        _ => Err(CliError::Input(format!(
            "unknown link role {value}; use tracks or relates"
        ))),
    }
}

fn state_from(value: &str) -> Result<ExternalState, CliError> {
    match value.to_lowercase().as_str() {
        "open" => Ok(ExternalState::Open),
        "closed" => Ok(ExternalState::Closed),
        "merged" => Ok(ExternalState::Merged),
        _ => Err(CliError::Input(format!(
            "unknown external state {value}; use open, closed or merged"
        ))),
    }
}
