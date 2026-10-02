//! The `run` command group, built with clap's builder API and read back by hand.
//!
//! The group is declared here rather than derived so that no generated lint overrides enter the
//! build; it plugs into the derived command tree through the `Subcommand` and `FromArgMatches`
//! traits.

use chrono::{DateTime, Utc};
use clap::{
    Arg, ArgAction, ArgMatches, Command, FromArgMatches, Subcommand, builder::ValueParser,
    error::ErrorKind, value_parser,
};
use dpm_model::{ActivityKind, ArtifactId, Observation, OperationId, RunEventId, RunId, RunState};

/// Agent runs and their activity. A run never changes the plan, its revision or the task's owner
/// and lifecycle: it only records what was observed about an executor.
#[derive(Debug)]
pub(crate) enum RunCommand {
    /// Start a run on a claimed or started task its executor owns.
    Start(Box<StartArgs>),
    /// Record a run's lifecycle transition.
    Report {
        run: RunId,
        state: RunState,
        actor: String,
        event_id: Option<RunEventId>,
        detail: Option<String>,
        observed_at: Option<DateTime<Utc>>,
    },
    /// Record one activity record.
    Record {
        run: RunId,
        kind: ActivityKind,
        source_sequence: u64,
        text: Option<String>,
        observed_at: Option<DateTime<Utc>>,
        actor: String,
    },
    /// Link a committed project operation to the run that performed it.
    Link {
        run: RunId,
        operation: OperationId,
        actor: String,
    },
    /// List runs, newest first.
    List { key: Option<String>, limit: u16 },
    /// Show one run.
    Show { run: RunId },
    /// Read lifecycle facts after a cursor.
    Lifecycle {
        after_sequence: u64,
        limit: u16,
        run: Option<RunId>,
    },
    /// Read activity after a cursor.
    Activity {
        after_sequence: u64,
        limit: u16,
        run: Option<RunId>,
    },
}

/// Arguments of `run start`.
#[derive(Debug)]
pub(crate) struct StartArgs {
    pub(crate) key: String,
    pub(crate) actor: String,
    pub(crate) executor: Option<String>,
    pub(crate) run_id: Option<RunId>,
    pub(crate) parent: Option<RunId>,
    pub(crate) provider: Option<String>,
    pub(crate) session: Option<String>,
    pub(crate) turn: Option<String>,
    pub(crate) requested_model: Option<String>,
    pub(crate) observed_model: Option<String>,
    pub(crate) runtime_version: Option<String>,
    pub(crate) configuration_digest: Option<String>,
    pub(crate) observation: Observation,
    pub(crate) source_commits: Vec<String>,
    pub(crate) source_artifacts: Vec<ArtifactId>,
    pub(crate) observed_at: Option<DateTime<Utc>>,
}

fn error(kind: ErrorKind, message: impl std::fmt::Display) -> clap::Error {
    clap::Error::raw(kind, message)
}

/// One typed value of an argument, or `None` when it was not given.
fn one<T: Clone + Send + Sync + 'static>(
    matches: &ArgMatches,
    id: &str,
) -> Result<Option<T>, clap::Error> {
    matches
        .try_get_one::<T>(id)
        .map(|value| value.cloned())
        .map_err(|source| error(ErrorKind::InvalidValue, source))
}

fn required<T: Clone + Send + Sync + 'static>(
    matches: &ArgMatches,
    id: &str,
) -> Result<T, clap::Error> {
    one(matches, id)?.ok_or_else(|| {
        error(
            ErrorKind::MissingRequiredArgument,
            format!("the argument {id} is required"),
        )
    })
}

fn many<T: Clone + Send + Sync + 'static>(
    matches: &ArgMatches,
    id: &str,
) -> Result<Vec<T>, clap::Error> {
    matches
        .try_get_many::<T>(id)
        .map(|values| {
            values
                .map(|values| values.cloned().collect())
                .unwrap_or_default()
        })
        .map_err(|source| error(ErrorKind::InvalidValue, source))
}

fn parsed<T>() -> ValueParser
where
    T: std::str::FromStr + Clone + Send + Sync + 'static,
    T::Err: Into<Box<dyn std::error::Error + Send + Sync + 'static>>,
{
    value_parser!(T).into()
}

fn positional(id: &'static str, help: &'static str, parser: ValueParser) -> Arg {
    Arg::new(id).required(true).value_parser(parser).help(help)
}

fn option(id: &'static str, flag: &'static str, help: &'static str, parser: ValueParser) -> Arg {
    Arg::new(id).long(flag).value_parser(parser).help(help)
}

fn actor() -> Arg {
    Arg::new("actor")
        .long("actor")
        .required(true)
        .value_parser(value_parser!(String))
        .help("Acting principal as KIND:NAME")
}

fn observed_at(what: &'static str) -> Arg {
    option(
        "observed_at",
        "observed-at",
        what,
        value_parser!(DateTime<Utc>).into(),
    )
    .value_name("RFC3339")
}

fn run_id() -> ValueParser {
    value_parser!(RunId).into()
}

fn page(command: Command) -> Command {
    command
        .arg(
            Arg::new("after_sequence")
                .long("after-sequence")
                .value_parser(value_parser!(u64))
                .default_value("0")
                .help("Exclusive feed cursor; zero starts from the beginning"),
        )
        .arg(
            Arg::new("limit")
                .long("limit")
                .value_parser(value_parser!(u16))
                .default_value("100")
                .help("Most entries to return; at most 1000"),
        )
        .arg(option("run", "run", "Only this run's entries", run_id()))
}

fn start() -> Command {
    Command::new("start")
        .about(
            "Start a run on a claimed or started task its executor owns, recording the contract and \
             plan revision it observed; send the same --run-id to retry safely",
        )
        .arg(positional("key", "Key of the task being executed", value_parser!(String)))
        .arg(actor().help("Principal recording the run: the executor, or a human or service acting for it"))
        .arg(option("executor", "executor", "The principal doing the work as KIND:NAME; defaults to --actor", value_parser!(String)))
        .arg(option("run_id", "run-id", "Version 7 run identity and idempotency key; minted when omitted", run_id()))
        .arg(option("parent", "parent", "Run that spawned this one", run_id()))
        .arg(option("provider", "provider", "Provider word of the runtime session, such as codex", value_parser!(String)).requires("session"))
        .arg(option("session", "session", "The provider's session identifier", value_parser!(String)).requires("provider"))
        .arg(option("turn", "turn", "The provider's turn identifier within the session", value_parser!(String)).requires("session"))
        .arg(option("requested_model", "requested-model", "The model the recorder asked the runtime to use", value_parser!(String)).requires("session"))
        .arg(option("observed_model", "observed-model", "The model the runtime announced", value_parser!(String)).requires("session"))
        .arg(option("runtime_version", "runtime-version", "The runtime's version as it announced it", value_parser!(String)).requires("session"))
        .arg(
            option(
                "configuration_digest",
                "configuration-digest",
                "SHA-256 in lowercase hex of the canonical public configuration the runtime started with",
                value_parser!(String),
            )
            .requires("session"),
        )
        .arg(
            option(
                "observation",
                "observation",
                "managed (only a service may say so) or reported_only: silence from a reported-only run proves nothing, so it turns stale and is never taken for finished",
                parsed::<Observation>(),
            )
            .default_value("reported_only"),
        )
        .arg(
            option(
                "source_commits",
                "source-commit",
                "Exact commit as ASSET_ID=FULL_HEX_COMMIT of a repository asset the task requires; repeatable. A branch or abbreviation is not an exact source",
                value_parser!(String),
            )
            .value_name("ASSET_ID=COMMIT")
            .action(ArgAction::Append),
        )
        .arg(
            option(
                "source_artifacts",
                "source-artifact",
                "Git commit evidence attached to this task, as an artifact id; its metadata and locator must name one exact commit; repeatable",
                parsed::<ArtifactId>(),
            )
            .value_name("ARTIFACT_ID")
            .action(ArgAction::Append),
        )
        .arg(observed_at("When the executor says the run began; a claim, never used to judge freshness"))
}

fn report() -> Command {
    Command::new("report")
        .about(
            "Record a run's lifecycle transition: working, waiting, failed, interrupted or completed; \
             ending a run submits and verifies nothing and releases nothing",
        )
        .arg(positional("run", "The run", run_id()))
        .arg(positional("state", "working, waiting, failed, interrupted or completed; the last three are final", parsed::<RunState>()))
        .arg(actor())
        .arg(option("event_id", "event-id", "Version 7 transition identity and idempotency key; minted when omitted", value_parser!(RunEventId).into()))
        .arg(option("detail", "detail", "Why, such as what made the run fail", value_parser!(String)))
        .arg(observed_at("When the executor says it happened"))
}

fn record() -> Command {
    Command::new("record")
        .about(
            "Record one activity record: tool_started, tool_result, progress, input_requested or \
             heartbeat; bounded telemetry that changes nothing else",
        )
        .arg(positional("run", "The run", run_id()))
        .arg(positional("kind", "tool_started, tool_result, progress, input_requested or heartbeat", parsed::<ActivityKind>()))
        .arg(
            option(
                "sequence",
                "sequence",
                "The reporter's sequence number for this record within the run, at least 1 and increasing; resending it is harmless while the record is retained, and one that retention removed is refused as expired",
                value_parser!(u64).into(),
            )
            .required(true),
        )
        .arg(option("text", "text", "Public text such as a tool name or progress note; cut at 4096 bytes and flagged, with the whole text still fingerprinted", value_parser!(String)))
        .arg(observed_at("When the source says it happened"))
        .arg(actor())
}

fn link() -> Command {
    Command::new("link")
        .about(
            "Link a committed project operation to the run that performed it; refused unless it is \
             the run's executor's operation on the run's task, within the run's lifetime and lineage",
        )
        .arg(positional("run", "The run", run_id()))
        .arg(positional("operation", "Operation id as returned by the project command", value_parser!(OperationId).into()))
        .arg(actor())
}

fn list() -> Command {
    Command::new("list")
        .about("List runs, newest first, with lifecycle and derived freshness")
        .arg(option(
            "key",
            "key",
            "Only runs executing this task key",
            value_parser!(String),
        ))
        .arg(
            Arg::new("limit")
                .long("limit")
                .value_parser(value_parser!(u16))
                .default_value("100")
                .help("Most runs to return; at most 1000"),
        )
}

fn show() -> Command {
    Command::new("show")
        .about("Show one run with its lifecycle, freshness, attribution and linked operations")
        .arg(positional("run", "The run", run_id()))
}

impl Subcommand for RunCommand {
    fn augment_subcommands(command: Command) -> Command {
        command
            .subcommand(start())
            .subcommand(report())
            .subcommand(record())
            .subcommand(link())
            .subcommand(list())
            .subcommand(show())
            .subcommand(page(
                Command::new("lifecycle")
                    .about("Read durable lifecycle facts after a cursor; they are never pruned"),
            ))
            .subcommand(page(Command::new("activity").about(
                "Read bounded activity after a cursor; a cursor that retention outran reports a gap",
            )))
    }

    fn augment_subcommands_for_update(command: Command) -> Command {
        Self::augment_subcommands(command)
    }

    fn has_subcommand(name: &str) -> bool {
        [
            "start",
            "report",
            "record",
            "link",
            "list",
            "show",
            "lifecycle",
            "activity",
        ]
        .contains(&name)
    }
}

impl FromArgMatches for RunCommand {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        match matches.subcommand() {
            Some(("start", m)) => Ok(Self::Start(Box::new(StartArgs {
                key: required(m, "key")?,
                actor: required(m, "actor")?,
                executor: one(m, "executor")?,
                run_id: one(m, "run_id")?,
                parent: one(m, "parent")?,
                provider: one(m, "provider")?,
                session: one(m, "session")?,
                turn: one(m, "turn")?,
                requested_model: one(m, "requested_model")?,
                observed_model: one(m, "observed_model")?,
                runtime_version: one(m, "runtime_version")?,
                configuration_digest: one(m, "configuration_digest")?,
                observation: required(m, "observation")?,
                source_commits: many(m, "source_commits")?,
                source_artifacts: many(m, "source_artifacts")?,
                observed_at: one(m, "observed_at")?,
            }))),
            Some(("report", m)) => Ok(Self::Report {
                run: required(m, "run")?,
                state: required(m, "state")?,
                actor: required(m, "actor")?,
                event_id: one(m, "event_id")?,
                detail: one(m, "detail")?,
                observed_at: one(m, "observed_at")?,
            }),
            Some(("record", m)) => Ok(Self::Record {
                run: required(m, "run")?,
                kind: required(m, "kind")?,
                source_sequence: required(m, "sequence")?,
                text: one(m, "text")?,
                observed_at: one(m, "observed_at")?,
                actor: required(m, "actor")?,
            }),
            Some(("link", m)) => Ok(Self::Link {
                run: required(m, "run")?,
                operation: required(m, "operation")?,
                actor: required(m, "actor")?,
            }),
            Some(("list", m)) => Ok(Self::List {
                key: one(m, "key")?,
                limit: required(m, "limit")?,
            }),
            Some(("show", m)) => Ok(Self::Show {
                run: required(m, "run")?,
            }),
            Some((name @ ("lifecycle" | "activity"), m)) => {
                let (after_sequence, limit, run) = (
                    required(m, "after_sequence")?,
                    required(m, "limit")?,
                    one(m, "run")?,
                );
                Ok(if name == "lifecycle" {
                    Self::Lifecycle {
                        after_sequence,
                        limit,
                        run,
                    }
                } else {
                    Self::Activity {
                        after_sequence,
                        limit,
                        run,
                    }
                })
            }
            Some((other, _)) => Err(error(
                ErrorKind::InvalidSubcommand,
                format!("unknown run command {other}"),
            )),
            None => Err(error(
                ErrorKind::MissingSubcommand,
                "a run command is required",
            )),
        }
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), clap::Error> {
        *self = Self::from_arg_matches(matches)?;
        Ok(())
    }
}
