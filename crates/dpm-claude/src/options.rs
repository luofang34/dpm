//! The command line: which workspace and task, which provider configuration, and the bounds.
//!
//! The adapter always starts the provider confined to its working directory, in safe mode, in a
//! permission mode that never approves anything on its own. It does not accept the modes that grant
//! permission automatically.

use dpm_model::{ActorId, ArtifactId, AssetId, RunId, RunSource};
use std::{ffi::OsString, path::PathBuf, str::FromStr, time::Duration};
use thiserror::Error;
use uuid::Uuid;

/// The longest whole-run time bound, in seconds: a day.
pub const MAX_SECONDS: u64 = 86_400;

/// The shortest and longest line the adapter will keep from the provider, in bytes.
pub const MIN_LINE_BYTES: usize = 1024;
/// See [`MIN_LINE_BYTES`].
pub const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

/// The most output messages that may wait, so the memory a run holds is bounded.
pub const MAX_QUEUE: usize = 4096;

/// What the adapter prints for `--help`.
pub const USAGE: &str = "\
Usage: dpm-claude run    [SOURCE] --work KEY --executor KIND:NAME --directory DIR PROMPT [OPTIONS]
       dpm-claude resume [SOURCE] --run RUN_ID --directory DIR PROMPT [OPTIONS]

Runs one bounded Claude Code turn as a managed run of a task and records its public activity
through DPM. The provider keeps its own authentication; DPM never reads or stores credentials.
The task must be claimed or started by --executor. Nothing is submitted or verified.

SOURCE   --project DIR | --database PATH     (default: the nearest project locator)
PROMPT   --prompt TEXT | --prompt-file PATH  (never recorded)
OPTIONS  --recorder service:NAME   service recording the run (default service:dpm-claude)
         --model ID                the provider model (default: the runtime's)
         --tools LIST              tools the provider may use, comma separated (default Read)
         --allow-tool RULE         a tool rule allowed without a request; repeatable
         --permission-mode MODE    manual (default), plan or dontAsk; never an approving mode
         --claude PATH             the provider executable (default claude)
         --max-seconds N           whole-run time bound, 1..=86400 (default 300)
         --max-line-bytes N        longest provider line kept, 1024..=8388608 (default 1048576)
         --queue N                 output messages that may wait, 1..=4096 (default 256)
         --source-commit ASSET=COMMIT   attest an exact Git commit of a repository the task requires;
                                   repeatable. Without one the run records no exact source: the
                                   adapter never borrows one from the plan's history.
         --source-artifact ID      attest the commit of an attached Git evidence artifact; repeatable
         --persist-session         let the provider save its session, so the run can be resumed
         --verbose                 diagnostics on standard error

Permission requests are always refused. Exit status: 0 the provider reported a finished turn,
1 the run ended otherwise and was recorded, 2 usage or start failure, 3 the end could not be recorded.
";

/// A command line the adapter cannot run.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum OptionsError {
    /// A flag it does not have, or one that lacks its value.
    #[error("{0}")]
    Usage(String),
    /// A permission mode that would grant permission by itself.
    #[error(
        "--permission-mode {0} grants permission automatically; this adapter never approves anything"
    )]
    ApprovingMode(String),
}

/// What to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Start a new run on a task.
    Run,
    /// Continue a run whose adapter stopped, in the provider session it recorded.
    Resume(RunId),
}

/// How the prompt is given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prompt {
    /// On the command line.
    Text(String),
    /// In a file, read when the run starts.
    File(PathBuf),
}

/// Everything the command line says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Start or resume.
    pub command: Command,
    /// Project directory containing `.dpm/project.toml`.
    pub project: Option<PathBuf>,
    /// Explicit store path.
    pub database: Option<PathBuf>,
    /// The task's key, for a new run.
    pub work: Option<String>,
    /// The principal doing the work.
    pub executor: Option<ActorId>,
    /// The service recording the run.
    pub recorder: ActorId,
    /// The provider's working directory.
    pub directory: PathBuf,
    /// The prompt.
    pub prompt: Prompt,
    /// The provider model, if chosen.
    pub model: Option<String>,
    /// Tools the provider may use.
    pub tools: String,
    /// Tool rules allowed without a request.
    pub allowed: Vec<String>,
    /// The permission mode.
    pub permission_mode: String,
    /// The provider executable.
    pub claude: PathBuf,
    /// Whole-run time bound.
    pub max_seconds: u64,
    /// Longest provider line kept.
    pub max_line_bytes: usize,
    /// Output messages that may wait.
    pub queue: usize,
    /// Whether the provider may save its session.
    pub persist_session: bool,
    /// Exact sources the operator attests; the application validates them against the plan.
    pub sources: Vec<RunSource>,
    /// Whether to log request-level detail.
    pub verbose: bool,
}

/// What the command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// Run with these options.
    Run(Box<Options>),
    /// Print the usage.
    Help,
}

fn number<T: FromStr>(flag: &str, value: &str) -> Result<T, OptionsError> {
    value
        .parse()
        .map_err(|_| OptionsError::Usage(format!("{flag} needs a whole number, not {value:?}")))
}

/// What parsing has seen that is not a field of the options themselves.
#[derive(Default)]
struct Seen {
    resume: Option<RunId>,
    prompt: bool,
}

impl Options {
    fn defaults() -> Self {
        Self {
            command: Command::Run,
            project: None,
            database: None,
            work: None,
            executor: None,
            recorder: ActorId::service("dpm-claude"),
            directory: PathBuf::new(),
            prompt: Prompt::Text(String::new()),
            model: None,
            tools: "Read".into(),
            allowed: Vec::new(),
            permission_mode: "manual".into(),
            claude: PathBuf::from("claude"),
            max_seconds: 300,
            max_line_bytes: 1 << 20,
            queue: 256,
            persist_session: false,
            sources: Vec::new(),
            verbose: false,
        }
    }

    /// Parse the arguments after the program name.
    pub fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Parsed, OptionsError> {
        let mut arguments = arguments.into_iter();
        let word = arguments
            .next()
            .ok_or_else(|| OptionsError::Usage("a command is required: run or resume".into()))?;
        if matches!(word.as_str(), "--help" | "-h") {
            return Ok(Parsed::Help);
        }
        let mut options = Self::defaults();
        let mut seen = Seen::default();
        while let Some(flag) = arguments.next() {
            let mut value = |name: &str| {
                arguments
                    .next()
                    .ok_or_else(|| OptionsError::Usage(format!("{name} needs a value")))
            };
            if matches!(flag.as_str(), "--help" | "-h") {
                return Ok(Parsed::Help);
            }
            options.apply(&flag, &mut value, &mut seen)?;
        }
        options.command = match word.as_str() {
            "run" => Command::Run,
            "resume" => Command::Resume(
                seen.resume
                    .ok_or_else(|| OptionsError::Usage("resume needs --run RUN_ID".into()))?,
            ),
            other => {
                return Err(OptionsError::Usage(format!(
                    "unknown command {other}; expected run or resume"
                )));
            }
        };
        options.validate(seen.prompt)?;
        Ok(Parsed::Run(Box::new(options)))
    }

    /// Apply one flag and, if it takes one, its value.
    fn apply(
        &mut self,
        flag: &str,
        value: &mut dyn FnMut(&str) -> Result<String, OptionsError>,
        seen: &mut Seen,
    ) -> Result<(), OptionsError> {
        match flag {
            "--verbose" => self.verbose = true,
            "--persist-session" => self.persist_session = true,
            "--project" => self.project = Some(value(flag)?.into()),
            "--database" => self.database = Some(value(flag)?.into()),
            "--work" => self.work = Some(value(flag)?),
            "--executor" => self.executor = Some(actor(flag, &value(flag)?)?),
            "--recorder" => self.recorder = actor(flag, &value(flag)?)?,
            "--run" => {
                let text = value(flag)?;
                seen.resume = Some(RunId(Uuid::parse_str(&text).map_err(|_| {
                    OptionsError::Usage(format!("--run {text:?} is not a run identity"))
                })?));
            }
            "--directory" => self.directory = value(flag)?.into(),
            "--prompt" => {
                self.prompt = Prompt::Text(value(flag)?);
                seen.prompt = true;
            }
            "--prompt-file" => {
                self.prompt = Prompt::File(value(flag)?.into());
                seen.prompt = true;
            }
            "--model" => self.model = Some(value(flag)?),
            "--tools" => self.tools = value(flag)?,
            "--allow-tool" => self.allowed.push(value(flag)?),
            "--source-commit" => self.sources.push(source_commit(&value(flag)?)?),
            "--source-artifact" => {
                let text = value(flag)?;
                let artifact: ArtifactId = text.parse().map_err(|_| {
                    OptionsError::Usage(format!("--source-artifact {text:?} is not an artifact id"))
                })?;
                self.sources.push(RunSource::Artifact { artifact });
            }
            "--permission-mode" => self.permission_mode = value(flag)?,
            "--claude" => self.claude = value(flag)?.into(),
            "--max-seconds" => self.max_seconds = number(flag, &value(flag)?)?,
            "--max-line-bytes" => self.max_line_bytes = number(flag, &value(flag)?)?,
            "--queue" => self.queue = number(flag, &value(flag)?)?,
            other => return Err(OptionsError::Usage(format!("unknown argument {other}"))),
        }
        Ok(())
    }

    fn validate(&self, have_prompt: bool) -> Result<(), OptionsError> {
        if self.project.is_some() && self.database.is_some() {
            return Err(OptionsError::Usage(
                "--project and --database are mutually exclusive".into(),
            ));
        }
        if self.command == Command::Run && (self.work.is_none() || self.executor.is_none()) {
            return Err(OptionsError::Usage(
                "run needs --work KEY and --executor KIND:NAME".into(),
            ));
        }
        if self.directory.as_os_str().is_empty() {
            return Err(OptionsError::Usage(
                "--directory DIR is required: the provider may reach only that directory".into(),
            ));
        }
        if !have_prompt {
            return Err(OptionsError::Usage(
                "a prompt is required: --prompt TEXT or --prompt-file PATH".into(),
            ));
        }
        if self.sources.len() > dpm_model::MAX_SOURCES {
            return Err(OptionsError::Usage(format!(
                "at most {} source references",
                dpm_model::MAX_SOURCES
            )));
        }
        if !matches!(self.permission_mode.as_str(), "manual" | "plan" | "dontAsk") {
            return Err(OptionsError::ApprovingMode(self.permission_mode.clone()));
        }
        let seconds = 1..=MAX_SECONDS;
        let line = MIN_LINE_BYTES..=MAX_LINE_BYTES;
        let queue = 1..=MAX_QUEUE;
        if !seconds.contains(&self.max_seconds)
            || !line.contains(&self.max_line_bytes)
            || !queue.contains(&self.queue)
        {
            return Err(OptionsError::Usage(format!(
                "--max-seconds must be 1..={MAX_SECONDS}, --max-line-bytes {MIN_LINE_BYTES}..={MAX_LINE_BYTES} and --queue 1..={MAX_QUEUE}"
            )));
        }
        Ok(())
    }

    /// The whole-run time bound.
    #[must_use]
    pub fn deadline(&self) -> Duration {
        Duration::from_secs(self.max_seconds)
    }

    /// The provider's arguments for `session`: a new session, or a resumed one.
    #[must_use]
    pub fn provider_arguments(&self, session: &str, resume: bool) -> Vec<OsString> {
        let mut args: Vec<String> = [
            "--output-format",
            "stream-json",
            "--verbose",
            "--input-format",
            "stream-json",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        args.extend([
            "--safe-mode".into(),
            "--restricted".into(),
            "--permission-mode".into(),
            self.permission_mode.clone(),
        ]);
        args.extend([
            "--permission-prompt-tool".into(),
            "stdio".into(),
            "--tools".into(),
            self.tools.clone(),
        ]);
        if !self.allowed.is_empty() {
            args.extend(["--allowedTools".into(), self.allowed.join(",")]);
        }
        if let Some(model) = &self.model {
            args.extend(["--model".into(), model.clone()]);
        }
        args.push(if resume {
            format!("--resume={session}")
        } else {
            format!("--session-id={session}")
        });
        if !self.persist_session {
            args.push("--no-session-persistence".into());
        }
        args.into_iter().map(OsString::from).collect()
    }
}

fn source_commit(text: &str) -> Result<RunSource, OptionsError> {
    let usage = || OptionsError::Usage(format!("--source-commit {text:?} must be ASSET_ID=COMMIT"));
    let (asset, commit) = text.split_once('=').ok_or_else(usage)?;
    let asset: AssetId = asset.parse().map_err(|_| usage())?;
    Ok(RunSource::GitCommit {
        asset,
        commit: commit.to_string(),
    })
}

fn actor(flag: &str, text: &str) -> Result<ActorId, OptionsError> {
    text.parse()
        .map_err(|error| OptionsError::Usage(format!("{flag}: {error}")))
}

#[cfg(test)]
mod tests;
