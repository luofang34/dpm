//! Running one provider turn against a workspace: the entry the binary and the tests share.
//!
//! A new run is recorded only after the task is checked, and again by the application when the run
//! starts, to be claimed or started by the named executor. A recovery never continues another
//! run: it closes the prior run honestly, with its gap and an unknown fate, and starts a new run
//! with a new turn identity that names the prior as its parent, in the provider's recorded session.

use crate::{
    event::Session,
    host::{HostError, Hosted, ProviderRecord, directory},
    options::{Command, Options, Prompt},
    process::Stopped,
    process::{Launch, ProcessError, Provider},
    session::{BeginError, Begun, Report, Retry, Turn, drive_blocking},
    sink::{
        AppSink, NewRun, Prior, SinkError, close_prior_blocking, prior_blocking, start_blocking,
    },
};
use dpm_app::{AppError, Application, open_workspace_blocking, store_path_blocking};
use dpm_model::{ActorId, RunId, RunProvenance, WorkStatus};
use std::{
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::Duration,
};
use thiserror::Error;
use uuid::Uuid;

/// The longest prompt accepted, so what is sent to the provider is bounded.
pub const MAX_PROMPT_BYTES: usize = 4096;

/// Why a run could not be carried out.
#[derive(Debug, Error)]
pub enum CommandError {
    /// The workspace could not be opened or read.
    #[error("workspace: {0}")]
    Workspace(#[source] AppError),
    /// The prompt is blank or too long.
    #[error("prompt: {0}")]
    Prompt(String),
    /// The prompt file could not be read.
    #[error("prompt file {}", .path.display())]
    PromptFile {
        /// The file.
        path: PathBuf,
        /// Why it could not be read.
        #[source]
        source: io::Error,
    },
    /// The working directory could not be determined.
    #[error("working directory")]
    Directory(#[source] io::Error),
    /// The task is not in a state this executor may run on.
    #[error("task {key}: {reason}")]
    Task {
        /// The task's key.
        key: String,
        /// What is wrong with it.
        reason: String,
    },
    /// A run to recover cannot be recovered.
    #[error("run {run}: {reason}")]
    Recover {
        /// The run.
        run: RunId,
        /// Why not.
        reason: String,
        /// The failure underneath, when the recovery was refused by the session's host record.
        #[source]
        source: Option<HostError>,
    },
    /// The provider session cannot be hosted: another host has it, or its last provider may still
    /// run. Nothing was changed.
    #[error("session cannot be hosted: {0}")]
    Host(#[from] HostError),
    /// The provider could not be started.
    #[error(transparent)]
    Provider(#[from] ProcessError),
    /// The run could not be recorded; the provider was stopped.
    #[error("recording the run failed: {source}")]
    Begin {
        /// Why, with the failure underneath as its source.
        #[source]
        source: BeginError,
        /// How stopping the provider went.
        stopped: Stopped,
    },
}

/// What a run did.
#[derive(Debug)]
pub struct Outcome {
    /// The run this invocation recorded.
    pub run: RunId,
    /// The run it recovered and closed, if it was a recovery.
    pub recovered: Option<RunId>,
    /// The session's report.
    pub report: Report,
}

fn prompt_text(prompt: &Prompt) -> Result<String, CommandError> {
    let text = match prompt {
        Prompt::Text(text) => text.clone(),
        Prompt::File(path) => {
            let mut text = String::new();
            File::open(path)
                .and_then(|file| {
                    file.take(MAX_PROMPT_BYTES as u64 + 1)
                        .read_to_string(&mut text)
                })
                .map_err(|source| CommandError::PromptFile {
                    path: path.clone(),
                    source,
                })?;
            text
        }
    };
    if text.trim().is_empty() || text.len() > MAX_PROMPT_BYTES {
        return Err(CommandError::Prompt(format!(
            "must be 1..={MAX_PROMPT_BYTES} bytes and not blank"
        )));
    }
    Ok(text)
}

/// SHA-256 of the canonical public configuration: what identifies how the provider was started,
/// without storing the configuration or anything private.
fn configuration_digest(options: &Options) -> String {
    use sha2::{Digest, Sha256};
    let canonical = format!(
        "permission_mode={};tools={};allowed={};safe_mode=1;restricted=1;persist_session={};protocol=stream-json-control",
        options.permission_mode,
        options.tools,
        options.allowed.join(","),
        u8::from(options.persist_session),
    );
    Sha256::digest(canonical.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn provenance(options: &Options, announced: Option<&Session>) -> Box<RunProvenance> {
    Box::new(RunProvenance {
        requested_model: options.model.clone(),
        observed_model: announced.and_then(|session| session.model.clone()),
        runtime_version: announced.and_then(|session| session.version.clone()),
        configuration_digest: Some(configuration_digest(options)),
    })
}

/// Check that `executor` owns `key` and that it is being executed, before anything runs.
fn preflight_blocking(
    app: &Application,
    key: &str,
    executor: &ActorId,
) -> Result<(), CommandError> {
    let work = app
        .show_blocking(key)
        .map_err(CommandError::Workspace)?
        .data
        .work;
    let refuse = |reason: String| CommandError::Task {
        key: key.to_string(),
        reason,
    };
    if !matches!(
        work.execution.status,
        WorkStatus::Claimed | WorkStatus::InProgress
    ) {
        return Err(refuse(format!(
            "is {:?}; a run needs claimed or started work",
            work.execution.status
        )));
    }
    if work.execution.owner.as_ref() != Some(executor) {
        return Err(refuse(format!("is not owned by {executor}")));
    }
    Ok(())
}

/// The next ordinal after a recorded turn: the provider has no turn identity here, so the adapter
/// numbers its turns within a session.
fn next_turn(prior: &Prior) -> String {
    let previous = prior
        .view
        .run
        .session
        .as_ref()
        .and_then(|session| session.turn.as_deref())
        .and_then(|turn| turn.parse::<u64>().ok())
        .unwrap_or(1);
    previous.wrapping_add(1).to_string()
}

/// What a command resolved to before the provider starts.
struct Resolved {
    new: NewRun,
    prior: Option<Prior>,
    resume: bool,
}

fn resolve(app: &Application, options: &Options, session: &str) -> Result<Resolved, CommandError> {
    match &options.command {
        Command::Run => {
            let (Some(work), Some(executor)) = (options.work.clone(), options.executor.clone())
            else {
                return Err(CommandError::Task {
                    key: String::new(),
                    reason: "a run needs --work and --executor".into(),
                });
            };
            preflight_blocking(app, &work, &executor)?;
            Ok(Resolved {
                new: NewRun {
                    recorder: options.recorder.clone(),
                    executor,
                    work_key: work,
                    parent: None,
                    session: session.to_string(),
                    turn: "1".into(),
                    provenance: None,
                    sources: options.sources.clone(),
                },
                prior: None,
                resume: false,
            })
        }
        Command::Resume(run) => {
            let prior = prior_blocking(app, *run).map_err(CommandError::Workspace)?;
            let recover = |reason: &str| CommandError::Recover {
                run: *run,
                reason: reason.to_string(),
                source: None,
            };
            if prior.view.state.is_terminal() {
                return Err(recover("it already ended; start a new run instead"));
            }
            let Some(recorded) = prior
                .view
                .run
                .session
                .clone()
                .filter(|recorded| recorded.provider == "claude")
            else {
                return Err(recover("it has no recorded claude session to resume"));
            };
            let key = prior.view.run.contract.work_key.to_string();
            let executor = prior.view.run.executor.clone();
            preflight_blocking(app, &key, &executor)?;
            Ok(Resolved {
                new: NewRun {
                    recorder: options.recorder.clone(),
                    executor,
                    work_key: key,
                    parent: Some(*run),
                    session: recorded.session,
                    turn: next_turn(&prior),
                    provenance: None,
                    sources: options.sources.clone(),
                },
                prior: Some(prior),
                resume: true,
            })
        }
    }
}

/// Take the claim on the provider session before anything is changed or started; it is held until
/// the returned value is dropped. Where it lives follows the store's canonical identity, so no
/// other working directory reaches a different one. A new session must be new; a recovery must
/// find the record of the host it replaces and be shown that no provider can still be running.
fn host_session(
    here: &Path,
    options: &Options,
    prior: Option<&Prior>,
    session: &str,
) -> Result<Hosted, CommandError> {
    let store = store_path_blocking(
        here,
        options.project.as_deref(),
        options.database.as_deref(),
    )
    .map_err(CommandError::Workspace)?;
    let locks = directory(&store).map_err(|source| {
        CommandError::Host(HostError::Io {
            path: store.clone(),
            source,
        })
    })?;
    match prior {
        None => Hosted::begin_fresh_blocking(&locks, session).map_err(CommandError::Host),
        Some(prior) => {
            Hosted::recover_blocking(&locks, session).map_err(|source| CommandError::Recover {
                run: prior.view.run.id,
                reason: "the session's last host cannot be shown to be gone".into(),
                source: Some(source),
            })
        }
    }
}

/// Start the provider, with the host record saying so before it exists and which process after, so
/// a host that dies before saying which process it started is not taken for one that started none.
fn start_provider(
    options: &Options,
    session: &str,
    resume: bool,
    hosted: &mut Hosted,
) -> Result<Provider, CommandError> {
    let launch = Launch {
        program: options.claude.clone(),
        args: options.provider_arguments(session, resume),
        directory: options.directory.clone(),
        remove_env: vec!["CLAUDECODE".into()],
    };
    hosted.record(ProviderRecord::Starting)?;
    let provider = match Provider::spawn_blocking(&launch, options.queue, options.max_line_bytes) {
        Ok(provider) => provider,
        Err(error @ ProcessError::Spawn { .. }) => {
            // The program never started, so no provider exists. Any other failure may have left
            // one, and the record keeps saying one was about to start.
            if let Err(record) = hosted.record(ProviderRecord::Nothing) {
                tracing::warn!(%record, "the host record could not be cleared");
            }
            return Err(error.into());
        }
        Err(error) => return Err(error.into()),
    };
    hosted.record(ProviderRecord::Running(provider.process_id()))?;
    Ok(provider)
}

fn turn_for(options: &Options, session: &str, prompt: String) -> Turn {
    Turn {
        session: session.to_string(),
        prompt,
        deadline: options.deadline(),
        // This CLI announces its session only after a prompt, so the run is recorded as soon as
        // `initialize` is acknowledged, before anything is asked of the provider.
        announce_wait: Duration::ZERO,
        batch: 50,
        grace: Duration::from_secs(5),
        retry: Retry {
            attempts: 5,
            backoff: Duration::from_millis(100),
            budget: Duration::from_secs(5),
        },
    }
}

/// Carry out one command: check the task, start the provider, record the run once the provider
/// has acknowledged its handshake, and record how it ended.
pub fn execute_blocking(options: &Options, cancel: &AtomicBool) -> Result<Outcome, CommandError> {
    let prompt = prompt_text(&options.prompt)?;
    let here = std::env::current_dir().map_err(CommandError::Directory)?;
    let mut app = open_workspace_blocking(
        &here,
        options.project.as_deref(),
        options.database.as_deref(),
    )
    .and_then(|app| app.plan_blocking().map(|_| app))
    .map_err(CommandError::Workspace)?;
    let fresh = Uuid::new_v4().to_string();
    let Resolved {
        mut new,
        prior,
        resume,
    } = resolve(&app, options, &fresh)?;
    let mut hosted = host_session(&here, options, prior.as_ref(), &new.session)?;
    let provider = start_provider(options, &new.session, resume, &mut hosted)?;
    let turn = turn_for(options, &new.session, prompt);
    let recovered = prior.as_ref().map(|prior| prior.view.run.id);
    let recorder = options.recorder.clone();
    let begin = move |announced: Option<&Session>| -> Result<Begun<AppSink>, BeginError> {
        if let Some(prior) = &prior {
            close_prior_blocking(&mut app, &recorder, prior)
                .map_err(|error: SinkError| BeginError::new("closing the recovered run", error))?;
        }
        new.provenance = Some(provenance(options, announced));
        let started = start_blocking(&mut app, &new)
            .map_err(|error| BeginError::new("recording the run", error))?;
        Ok(Begun {
            sink: AppSink::new(app, recorder, started.run, started.lineage),
            run: started.run,
        })
    };
    let report = drive_blocking(provider, &turn, begin, cancel);
    // The record says no provider is running only when the provider was shown to be reaped, on
    // every path: a stop that did not complete leaves the record saying one runs.
    let reaped = match &report {
        Ok(report) => report.stopped.reaped,
        Err(failure) => failure.stopped.reaped,
    };
    let report = report.map_err(|failure| CommandError::Begin {
        source: failure.error,
        stopped: failure.stopped,
    });
    if reaped && let Err(error) = hosted.record(ProviderRecord::Nothing) {
        tracing::warn!(%error, "the host record could not be cleared");
    }
    let report = report?;
    Ok(Outcome {
        run: report.run,
        recovered,
        report,
    })
}
