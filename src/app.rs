use crate::{
    args::{Cli, Commands, PlanCommand, StoreCommand, WorkspaceCommand},
    error::{CliError, io_error},
    output,
};
use dpm_app::{
    Application, CommandRequest, Query, initialize_project_blocking, open_workspace_blocking,
};
use dpm_engine::{Command, NextWorkResult};
use dpm_model::{ActorId, ActorKind, Plan};
use std::{fs, path::Path};

pub(crate) fn run_blocking(cli: Cli) -> Result<(), CliError> {
    let Cli {
        database,
        project,
        json,
        base_revision,
        command,
    } = cli;
    let cwd = std::env::current_dir().map_err(io_error("read current directory", "."))?;
    let command = command.unwrap_or(if json {
        Commands::Status {
            no_simulation: false,
        }
    } else {
        Commands::Tui
    });
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
        Commands::Validate { file } => {
            read_plan_blocking(&file)?;
            output::value_blocking(&serde_json::json!({"valid": true, "file": file}), json)
        }
        Commands::Store(StoreCommand::Restore { from, to }) => {
            crate::recovery::restore_blocking(&from, &to, json)
        }
        Commands::Store(StoreCommand::VerifyStore { path }) => {
            crate::recovery::verify_blocking(&cwd, path, project, database, json)
        }
        command => {
            let mut app = open_workspace_blocking(&cwd, project.as_deref(), database.as_deref())?;
            run_open_blocking(&mut app, command, json, base_revision)
        }
    }
}

fn run_open_blocking(
    app: &mut Application,
    command: Commands,
    json: bool,
    base_revision: Option<u64>,
) -> Result<(), CliError> {
    match command {
        Commands::Plan { command } => plan_command_blocking(app, command, json, base_revision),
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
        Commands::Status { no_simulation } => query_blocking(
            app,
            Query::Status {
                probabilistic: !no_simulation,
            },
            json,
        ),
        Commands::Next {
            capabilities,
            project_keys,
            resource_keys,
            limit,
            deterministic_only,
        } => {
            let response = app.query_blocking(Query::Next {
                capabilities: capabilities.into_iter().collect(),
                limit,
                probabilistic: !deterministic_only,
                project_keys: project_keys.into_iter().collect(),
                resource_keys: resource_keys.into_iter().collect(),
            })?;
            if json {
                return output::json_blocking(&response.data);
            }
            let result: NextWorkResult = serde_json::from_value(response.data)?;
            output::next_text_blocking(&result)
        }
        Commands::Show { key } => query_blocking(app, Query::Show { key }, json),
        Commands::Explain { key } => query_blocking(app, Query::Explain { key }, json),
        Commands::Export => query_blocking(app, Query::Export, true),
        Commands::Store(StoreCommand::Backup { to }) => {
            crate::recovery::backup_blocking(app, &to, json)
        }
        Commands::Tui => {
            let plan = app.plan_blocking()?;
            dpm_tui::run_reloading_blocking(&plan, app.is_read_only(), || {
                app.refreshed_plan_blocking()
            })?;
            Ok(())
        }
        mutation => mutate_blocking(app, mutation, json, base_revision),
    }
}

fn initialize_blocking(
    root: &Path,
    database: Option<&Path>,
    plan: Plan,
    json: bool,
) -> Result<(), CliError> {
    let database = if let Some(path) = database {
        Application::initialize_blocking(path, &plan)?;
        path.to_path_buf()
    } else {
        initialize_project_blocking(root, &plan)?
    };
    output::value_blocking(
        &serde_json::json!({"database": database, "revision": plan.revision, "workspace": plan.workspace.name}),
        json,
    )
}

fn read_plan_blocking(path: &Path) -> Result<Plan, CliError> {
    let plan = read_candidate_blocking(path)?;
    plan.validate()?;
    Ok(plan)
}

fn read_candidate_blocking(path: &Path) -> Result<Plan, CliError> {
    let text = fs::read_to_string(path).map_err(io_error("read plan", path))?;
    Ok(serde_json::from_str(&text)?)
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
    let response = app.query_blocking(query)?;
    if json {
        output::json_blocking(&response.data)
    } else {
        output::text_blocking(&serde_json::to_string_pretty(&response.data)?)
    }
}

fn mutate_blocking(
    app: &mut Application,
    command: Commands,
    json: bool,
    base_revision: Option<u64>,
) -> Result<(), CliError> {
    app.ensure_writable()?;
    let plan = app.plan_blocking()?;
    let (actor, command) = mutation_blocking(app, command)?;
    let operation = app.execute_blocking(CommandRequest {
        actor,
        base_revision: base_revision.unwrap_or(plan.revision),
        command,
    })?;
    output::value_blocking(&operation, json)
}

fn mutation_blocking(app: &Application, command: Commands) -> Result<(ActorId, Command), CliError> {
    let pair = match command {
        lifecycle @ (Commands::Ratify { .. }
        | Commands::Reject { .. }
        | Commands::Claim { .. }
        | Commands::Start { .. }) => lifecycle_mutation_blocking(app, lifecycle)?,
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
            resource,
        } => {
            let principal = actor(&who)?;
            let artifact = app.git_head_artifact_blocking(
                principal.clone(),
                app.work_id_blocking(&key)?,
                resource.as_deref(),
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
        Commands::Claim { key, actor: who } => (
            actor(&who)?,
            Command::Claim {
                work: app.work_id_blocking(&key)?,
            },
        ),
        Commands::Start { key, actor: who } => (
            actor(&who)?,
            Command::Start {
                work: app.work_id_blocking(&key)?,
            },
        ),
        _ => {
            return Err(CliError::Input(
                "expected contract, claim or start command".into(),
            ));
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
            output::value_blocking(&registry.register_blocking(database, replace)?, json)
        }
        WorkspaceCommand::List => output::value_blocking(
            &registry.list_blocking().map_err(dpm_app::AppError::from)?,
            json,
        ),
    }
}

fn plan_command_blocking(
    app: &mut Application,
    command: PlanCommand,
    json: bool,
    base_revision: Option<u64>,
) -> Result<(), CliError> {
    match command {
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
            let operation = app.execute_blocking(CommandRequest {
                actor: actor(&who)?,
                base_revision: base_revision.unwrap_or(plan.revision),
                command: Command::ApplyChange {
                    plan: Box::new(plan),
                    reason,
                },
            })?;
            output::value_blocking(&operation, json)
        }
        interchange @ (PlanCommand::ImportMspdi { .. } | PlanCommand::ExportMspdi { .. }) => {
            crate::interchange::plan_command_blocking(app, interchange, json)
        }
    }
}
