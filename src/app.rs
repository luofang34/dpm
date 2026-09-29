use crate::{
    args::{Cli, Commands, PlanCommand, Preconditions, StoreCommand, WorkspaceCommand},
    bootstrap::{initialize_blocking, read_candidate_blocking, read_plan_blocking},
    error::{CliError, io_error},
    output,
};
use dpm_app::{Application, CommandRequest, PlanChangeRequest, Query, open_workspace_blocking};
use dpm_engine::{Command, NextWorkResult};
use dpm_model::{ActorId, ActorKind, Plan};
use std::{fs, path::Path};

pub(crate) fn run_blocking(cli: Cli) -> Result<(), CliError> {
    let Cli {
        database,
        project,
        json,
        base_revision,
        base_lineage,
        operation_id,
        clock,
        command,
    } = cli;
    let preconditions = Preconditions {
        base_revision,
        base_lineage,
        operation_id,
    };
    let cwd = std::env::current_dir().map_err(io_error("read current directory", "."))?;
    let command = command.unwrap_or(if json {
        Commands::Status {
            no_simulation: false,
        }
    } else {
        Commands::Tui
    });
    let query_clock = crate::clock::for_command(&command, clock)?;
    let root = project.as_deref().unwrap_or(&cwd);
    match command {
        Commands::Workspace { command } => {
            workspace_command_blocking(command, database.as_deref(), json)
        }
        Commands::Init { name } => {
            initialize_blocking(root, database.as_deref(), Plan::empty(name), json)
        }
        Commands::Demo => initialize_blocking(
            root,
            database.as_deref(),
            serde_json::from_str(include_str!("../examples/self-host/dpm-alpha.json"))?,
            json,
        ),
        Commands::Import { file } => {
            initialize_blocking(root, database.as_deref(), read_plan_blocking(&file)?, json)
        }
        // The format is readable before any workspace exists.
        Commands::Plan {
            command: PlanCommand::Schema,
        } => {
            let schema = dpm_app::plan_schema()?;
            if json {
                output::success_blocking(None, &schema)
            } else {
                output::json_blocking(&schema)
            }
        }
        Commands::Validate { file } => crate::bootstrap::validate_blocking(&file, json),
        Commands::Store(StoreCommand::Restore { from, to }) => {
            crate::recovery::restore_blocking(&from, &to, json)
        }
        Commands::Store(StoreCommand::VerifyStore { path }) => {
            crate::recovery::verify_blocking(&cwd, path, project, database, json)
        }
        command => {
            let mut app = open_workspace_blocking(&cwd, project.as_deref(), database.as_deref())?
                .with_query_clock(query_clock);
            run_open_blocking(&mut app, command, json, preconditions)
        }
    }
}

fn run_open_blocking(
    app: &mut Application,
    command: Commands,
    json: bool,
    preconditions: Preconditions,
) -> Result<(), CliError> {
    match command {
        Commands::Plan { command } => plan_command_blocking(app, command, json, preconditions),
        Commands::Store(StoreCommand::Revision) => query_blocking(app, Query::Revision, json),
        Commands::Store(StoreCommand::History {
            after_sequence,
            limit,
        }) => query_blocking(
            app,
            Query::History {
                after_sequence,
                limit,
            },
            json,
        ),
        Commands::Status { no_simulation } => {
            let response = app.query_blocking(Query::Status {
                probabilistic: !no_simulation,
            })?;
            if json {
                return output::response_blocking(response, true);
            }
            let unestimated: Vec<dpm_model::Key> = serde_json::from_value(
                response
                    .data
                    .get("unestimated")
                    .cloned()
                    .unwrap_or_default(),
            )
            .unwrap_or_default();
            if let Some(line) = output::unestimated_line(&unestimated) {
                output::text_blocking(&line)?;
            }
            output::text_blocking(&serde_json::to_string_pretty(&response.data)?)
        }
        Commands::Next {
            capabilities,
            project_keys,
            asset_keys,
            limit,
            deterministic_only,
        } => {
            let response = app.query_blocking(Query::Next {
                capabilities: capabilities.into_iter().collect(),
                limit,
                probabilistic: !deterministic_only,
                project_keys: project_keys.into_iter().collect(),
                asset_keys: asset_keys.into_iter().collect(),
            })?;
            if json {
                return output::response_blocking(response, true);
            }
            let result: NextWorkResult = serde_json::from_value(response.data)?;
            output::next_text_blocking(&result)
        }
        Commands::Show { key } => query_blocking(app, Query::Show { key }, json),
        Commands::Explain { key } => query_blocking(app, Query::Explain { key }, json),
        // Without --json the bare plan is printed, ready to redirect into a file for import.
        Commands::Export => {
            let response = app.query_blocking(Query::Export)?;
            if json {
                output::response_blocking(response, true)
            } else {
                output::json_blocking(&response.data)
            }
        }
        Commands::Store(StoreCommand::Backup { to }) => {
            crate::recovery::backup_blocking(app, &to, json)
        }
        Commands::Tui => crate::console::run_blocking(app),
        mutation => mutate_blocking(app, mutation, json, preconditions),
    }
}

pub(crate) fn actor(value: &str) -> Result<ActorId, CliError> {
    let (kind, name) = value.split_once(':').ok_or_else(|| {
        CliError::Input("actor must be human:NAME, agent:NAME, or service:NAME".into())
    })?;
    if name.trim().is_empty() {
        return Err(CliError::Input("actor name must not be empty".into()));
    }
    let kind = match kind {
        "human" => ActorKind::Human,
        "agent" => ActorKind::Agent,
        "service" => ActorKind::Service,
        _ => return Err(CliError::Input(format!("unknown actor kind {kind}"))),
    };
    Ok(ActorId {
        kind,
        name: name.into(),
    })
}

fn query_blocking(app: &Application, query: Query, json: bool) -> Result<(), CliError> {
    output::response_blocking(app.query_blocking(query)?, json)
}

fn mutate_blocking(
    app: &mut Application,
    command: Commands,
    json: bool,
    preconditions: Preconditions,
) -> Result<(), CliError> {
    app.ensure_writable()?;
    let plan = app.plan_blocking()?;
    let (actor, command) = app.build_command_blocking(preconditions.operation_id, |app| {
        mutation_blocking(app, command)
    })?;
    let operation = app.execute_blocking(CommandRequest {
        actor,
        base_revision: preconditions.base_revision.unwrap_or(plan.revision),
        base_lineage: preconditions.base_lineage,
        operation_id: preconditions.operation_id,
        command,
    })?;
    output::operation_blocking(operation, json)
}

fn mutation_blocking(app: &Application, command: Commands) -> Result<(ActorId, Command), CliError> {
    let pair = match command {
        lifecycle
        @ (Commands::Ratify { .. } | Commands::Reject { .. } | Commands::Start { .. }) => {
            lifecycle_mutation_blocking(app, lifecycle)?
        }
        Commands::Ownership(command) => crate::ownership::mutation_blocking(app, command)?,
        Commands::Block {
            key,
            reason,
            actor: who,
        } => (
            actor(&who)?,
            Command::Block {
                work: app.work_id_blocking(&key)?,
                reason,
            },
        ),
        Commands::Unblock { key, actor: who } => (
            actor(&who)?,
            Command::Unblock {
                work: app.work_id_blocking(&key)?,
            },
        ),
        Commands::Progress {
            key,
            percent,
            note,
            actor: who,
        } => (
            actor(&who)?,
            Command::ReportProgress {
                work: app.work_id_blocking(&key)?,
                percent,
                note,
            },
        ),
        Commands::Submit {
            key,
            note,
            actor: who,
        } => (
            actor(&who)?,
            Command::Submit {
                work: app.work_id_blocking(&key)?,
                note,
            },
        ),
        Commands::Verify {
            key,
            note,
            actor: who,
        } => (
            actor(&who)?,
            Command::Verify {
                work: app.work_id_blocking(&key)?,
                note,
            },
        ),
        Commands::Decide {
            key,
            outcome,
            actor: who,
        } => {
            let decision = app.decision_id_blocking(&key)?;
            (actor(&who)?, Command::Decide { decision, outcome })
        }
        artifact @ (Commands::Artifact { .. } | Commands::AttachGitHead { .. }) => {
            artifact_mutation_blocking(app, artifact)?
        }
        external @ (Commands::LinkExternal { .. } | Commands::UnlinkExternal { .. }) => {
            crate::tracking::mutation_blocking(app, external)?
        }
        waiver @ (Commands::WaiveDependency { .. }
        | Commands::RestoreDependency { .. }
        | Commands::RevalidateBasis { .. }) => waiver_mutation_blocking(app, waiver)?,
        _ => return Err(CliError::Input("expected a state-changing command".into())),
    };
    Ok(pair)
}

fn artifact_mutation_blocking(
    app: &Application,
    command: Commands,
) -> Result<(ActorId, Command), CliError> {
    match command {
        Commands::Artifact {
            key,
            file,
            actor: who,
        } => {
            let artifact = serde_json::from_str(
                &fs::read_to_string(&file).map_err(io_error("read artifact", &file))?,
            )?;
            Ok((
                actor(&who)?,
                Command::AttachArtifact {
                    work: app.work_id_blocking(&key)?,
                    artifact,
                },
            ))
        }
        Commands::AttachGitHead {
            key,
            actor: who,
            asset,
        } => {
            let principal = actor(&who)?;
            let artifact = app.git_head_artifact_blocking(
                principal.clone(),
                app.work_id_blocking(&key)?,
                asset.as_deref(),
            )?;
            Ok((
                principal,
                Command::AttachArtifact {
                    work: app.work_id_blocking(&key)?,
                    artifact,
                },
            ))
        }
        _ => Err(CliError::Input("expected artifact command".into())),
    }
}

fn waiver_mutation_blocking(
    app: &Application,
    command: Commands,
) -> Result<(ActorId, Command), CliError> {
    Ok(match command {
        Commands::WaiveDependency {
            dependency,
            reason,
            actor: who,
        } => (
            actor(&who)?,
            Command::WaiveDependency {
                dependency: app.dependency_id_blocking(&dependency)?,
                reason,
            },
        ),
        Commands::RestoreDependency {
            dependency,
            reason,
            actor: who,
        } => (
            actor(&who)?,
            Command::RestoreDependency {
                dependency: app.dependency_id_blocking(&dependency)?,
                reason,
            },
        ),
        Commands::RevalidateBasis {
            key,
            dependency,
            attempt,
            reason,
            actor: who,
        } => (
            actor(&who)?,
            Command::RevalidateBasis {
                work: app.work_id_blocking(&key)?,
                dependency: app.dependency_id_blocking(&dependency)?,
                attempt,
                reason,
            },
        ),
        _ => return Err(CliError::Input("expected dependency command".into())),
    })
}

fn lifecycle_mutation_blocking(
    app: &Application,
    command: Commands,
) -> Result<(ActorId, Command), CliError> {
    Ok(match command {
        Commands::Ratify { key, actor: who } => (
            actor(&who)?,
            Command::RatifyContract {
                work: app.work_id_blocking(&key)?,
            },
        ),
        Commands::Reject {
            key,
            reason,
            actor: who,
        } => (
            actor(&who)?,
            Command::Reject {
                work: app.work_id_blocking(&key)?,
                reason,
            },
        ),
        Commands::Start { key, actor: who } => (
            actor(&who)?,
            Command::Start {
                work: app.work_id_blocking(&key)?,
            },
        ),
        _ => {
            return Err(CliError::Input("expected contract or start command".into()));
        }
    })
}

fn workspace_command_blocking(
    command: WorkspaceCommand,
    database: Option<&Path>,
    json: bool,
) -> Result<(), CliError> {
    let registry =
        dpm_app::WorkspaceRegistry::from_environment().map_err(dpm_app::AppError::from)?;
    match command {
        WorkspaceCommand::Register { replace } => {
            let database = database.ok_or_else(|| {
                CliError::Input("workspace register requires --database PATH".into())
            })?;
            output::value_blocking(&registry.register_blocking(database, replace)?, None, json)
        }
        WorkspaceCommand::List => output::value_blocking(
            &registry
                .inspect_blocking()
                .map_err(dpm_app::AppError::from)?,
            None,
            json,
        ),
    }
}

fn plan_command_blocking(
    app: &mut Application,
    command: PlanCommand,
    json: bool,
    preconditions: Preconditions,
) -> Result<(), CliError> {
    match command {
        PlanCommand::Template => query_blocking(app, Query::PlanTemplate, json),
        PlanCommand::Schema => query_blocking(app, Query::PlanSchema, json),
        PlanCommand::Diff { file } => query_blocking(
            app,
            Query::ProposeChange {
                plan: Box::new(read_candidate_blocking(&file)?),
            },
            json,
        ),
        PlanCommand::Apply {
            file,
            reason,
            actor: who,
        } => {
            app.ensure_writable()?;
            let plan = read_candidate_blocking(&file)?;
            let operation = app.apply_plan_change_blocking(PlanChangeRequest {
                actor: actor(&who)?,
                base_revision: preconditions.base_revision.unwrap_or(plan.revision),
                base_lineage: preconditions.base_lineage,
                operation_id: preconditions.operation_id,
                plan: Box::new(plan),
                reason,
            })?;
            output::operation_blocking(operation, json)
        }
        interchange @ (PlanCommand::ImportMspdi { .. } | PlanCommand::ExportMspdi { .. }) => {
            crate::interchange::plan_command_blocking(app, interchange, json)
        }
    }
}
