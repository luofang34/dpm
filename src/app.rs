use crate::{
    args::{Cli, Commands},
    error::{CliError, io_error},
    output,
};
use dpm_app::{Application, CommandRequest, Query, git_head_artifact_blocking};
use dpm_engine::{Command, NextWorkCandidate};
use dpm_model::{ActorId, ActorKind, Plan};
use dpm_store::SqliteStore;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(crate) fn run_blocking(cli: Cli) -> Result<(), CliError> {
    let database = cli
        .database
        .unwrap_or_else(|| PathBuf::from(".dpm/dpm.sqlite"));
    let command = cli.command.unwrap_or(Commands::Tui);
    match command {
        Commands::Init { name } => initialize_blocking(&database, Plan::empty(name), cli.json),
        Commands::Demo => initialize_blocking(
            &database,
            serde_json::from_str(include_str!("../examples/self-host/dpm-alpha.json"))?,
            cli.json,
        ),
        Commands::Import { file } => {
            initialize_blocking(&database, read_plan_blocking(&file)?, cli.json)
        }
        Commands::Validate { file } => {
            read_plan_blocking(&file)?;
            output::value_blocking(&serde_json::json!({"valid": true, "file": file}), cli.json)
        }
        Commands::Status { no_simulation } => query_blocking(
            &database,
            Query::Status {
                probabilistic: !no_simulation,
            },
            cli.json,
        ),
        Commands::Next {
            capabilities,
            limit,
            deterministic_only,
        } => {
            let response = Application::open_blocking(&database)?.query_blocking(Query::Next {
                capabilities: capabilities.into_iter().collect(),
                limit,
                probabilistic: !deterministic_only,
            })?;
            let candidates: Vec<NextWorkCandidate> = serde_json::from_value(response.data)?;
            output::candidates_blocking(&candidates, cli.json)
        }
        Commands::Show { key } => query_blocking(&database, Query::Show { key }, cli.json),
        Commands::Explain { key } => query_blocking(&database, Query::Explain { key }, cli.json),
        Commands::Export => output::json_blocking(&load_plan_blocking(&database)?),
        Commands::Tui => {
            dpm_tui::run_blocking(&load_plan_blocking(&database)?)?;
            Ok(())
        }
        mutation => mutate_blocking(&database, mutation, cli.json, cli.base_revision),
    }
}

fn initialize_blocking(database: &Path, plan: Plan, json: bool) -> Result<(), CliError> {
    plan.validate()?;
    if let Some(parent) = database.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(io_error("create database directory", parent))?;
    }
    let mut store = SqliteStore::open_blocking(database)?;
    store.initialize_blocking(&plan)?;
    output::value_blocking(
        &serde_json::json!({"database": database, "revision": plan.revision, "workspace": plan.workspace.name}),
        json,
    )
}

fn read_plan_blocking(path: &Path) -> Result<Plan, CliError> {
    let text = fs::read_to_string(path).map_err(io_error("read plan", path))?;
    let plan: Plan = serde_json::from_str(&text)?;
    plan.validate()?;
    Ok(plan)
}

fn load_plan_blocking(database: &Path) -> Result<Plan, CliError> {
    SqliteStore::open_existing_blocking(database)?
        .load_blocking()?
        .ok_or_else(|| {
            CliError::Input(format!(
                "workspace is not initialized at {}; run dpm init or demo",
                database.display()
            ))
        })
}

fn actor(value: &str) -> Result<ActorId, CliError> {
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

fn query_blocking(database: &Path, query: Query, json: bool) -> Result<(), CliError> {
    let response = Application::open_blocking(database)?.query_blocking(query)?;
    if json {
        output::json_blocking(&response.data)
    } else {
        output::text_blocking(&serde_json::to_string_pretty(&response.data)?)
    }
}

fn mutate_blocking(
    database: &Path,
    command: Commands,
    json: bool,
    base_revision: Option<u64>,
) -> Result<(), CliError> {
    let mut app = Application::open_blocking(database)?;
    let plan = app.plan_blocking()?;
    let (actor, command) = mutation_blocking(&app, command)?;
    let operation = app.execute_blocking(CommandRequest {
        actor,
        base_revision: base_revision.unwrap_or(plan.revision),
        command,
    })?;
    output::value_blocking(&operation, json)
}

fn mutation_blocking(app: &Application, command: Commands) -> Result<(ActorId, Command), CliError> {
    let pair = match command {
        Commands::Claim { key, actor: who } => (
            actor(&who)?,
            Command::Claim {
                work: app.work_id_blocking(&key)?,
            },
        ),
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
        Commands::AttachGitHead { key, actor: who } => {
            let principal = actor(&who)?;
            let artifact = git_head_artifact_blocking(principal.clone())?;
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
