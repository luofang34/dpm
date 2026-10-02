//! `dpm run`: agent runs and their activity through the shared application boundary.
//!
//! Run commands are not project mutations, so they take no revision or operation precondition:
//! their identities are the run, event and activity keys. The lineage precondition still applies.

use crate::{
    app::actor,
    args::{Preconditions, RunCommand, StartArgs},
    error::CliError,
    output,
};
use dpm_app::{
    Application, Query, RunActivityRequest, RunCommand as Command, RunLinkRequest,
    RunReportRequest, RunStartRequest,
};
use dpm_model::{ActivityInput, AssetId, LineageId, RunProvenance, RunSession, RunSource};

/// What a `run` subcommand asks the application to do.
enum Action {
    Write(Command),
    Read(Query),
}

pub(crate) fn command_blocking(
    app: &mut Application,
    command: RunCommand,
    json: bool,
    preconditions: Preconditions,
) -> Result<(), CliError> {
    match action(command, preconditions.base_lineage)? {
        Action::Read(query) => output::response_blocking(app.query_blocking(query)?, json),
        Action::Write(command) => {
            if preconditions.base_revision.is_some() || preconditions.operation_id.is_some() {
                return Err(CliError::Input(
                    "run commands take no --base-revision or --operation-id: a run observes the \
                     plan and never edits it; their identities are the run id, --event-id and --key"
                        .into(),
                ));
            }
            output::response_blocking(app.execute_run_blocking(command)?, json)
        }
    }
}

fn action(command: RunCommand, base_lineage: Option<LineageId>) -> Result<Action, CliError> {
    Ok(match command {
        RunCommand::Start(args) => {
            Action::Write(Command::Start(start_request(*args, base_lineage)?))
        }
        RunCommand::Report {
            run,
            state,
            actor: who,
            event_id,
            detail,
            observed_at,
        } => Action::Write(Command::Report(RunReportRequest {
            actor: actor(&who)?,
            run,
            state,
            event_id,
            detail,
            observed_at,
            base_lineage,
        })),
        RunCommand::Record {
            run,
            kind,
            source_sequence,
            text,
            observed_at,
            actor: who,
        } => Action::Write(Command::Activity(RunActivityRequest {
            actor: actor(&who)?,
            entries: vec![ActivityInput {
                run,
                source_sequence,
                kind,
                text,
                observed_at,
            }],
            base_lineage,
        })),
        RunCommand::Link {
            run,
            operation,
            actor: who,
        } => Action::Write(Command::Link(RunLinkRequest {
            actor: actor(&who)?,
            run,
            operation,
            base_lineage,
        })),
        RunCommand::List { key, limit } => Action::Read(Query::Runs { key, limit }),
        RunCommand::Show { run } => Action::Read(Query::Run { id: run }),
        RunCommand::Lifecycle {
            after_sequence,
            limit,
            run,
        } => Action::Read(Query::RunLifecycle {
            after_sequence,
            limit,
            run,
        }),
        RunCommand::Activity {
            after_sequence,
            limit,
            run,
        } => Action::Read(Query::RunActivity {
            after_sequence,
            limit,
            run,
        }),
    })
}

fn start_request(
    args: StartArgs,
    base_lineage: Option<LineageId>,
) -> Result<RunStartRequest, CliError> {
    let provenance = [
        &args.requested_model,
        &args.observed_model,
        &args.runtime_version,
        &args.configuration_digest,
    ]
    .iter()
    .any(|value| value.is_some())
    .then(|| {
        Box::new(RunProvenance {
            requested_model: args.requested_model.clone(),
            observed_model: args.observed_model.clone(),
            runtime_version: args.runtime_version.clone(),
            configuration_digest: args.configuration_digest.clone(),
        })
    });
    let session = match (args.provider, args.session) {
        (Some(provider), Some(session)) => Some(RunSession {
            provider,
            session,
            turn: args.turn,
            provenance,
        }),
        _ => None,
    };
    let mut sources = args
        .source_commits
        .iter()
        .map(|text| source_commit(text))
        .collect::<Result<Vec<_>, _>>()?;
    sources.extend(
        args.source_artifacts
            .into_iter()
            .map(|artifact| RunSource::Artifact { artifact }),
    );
    Ok(RunStartRequest {
        actor: actor(&args.actor)?,
        work_key: args.key,
        executor: args.executor.as_deref().map(actor).transpose()?,
        run_id: args.run_id,
        parent: args.parent,
        session,
        observation: args.observation,
        sources,
        observed_at: args.observed_at,
        base_lineage,
    })
}

fn source_commit(text: &str) -> Result<RunSource, CliError> {
    let (asset, commit) = text.split_once('=').ok_or_else(|| {
        CliError::Input(format!("--source-commit {text:?} must be ASSET_ID=COMMIT"))
    })?;
    let asset: AssetId = asset
        .parse()
        .map_err(|_| CliError::Input(format!("{asset:?} is not an asset id (a UUID)")))?;
    Ok(RunSource::GitCommit {
        asset,
        commit: commit.into(),
    })
}
