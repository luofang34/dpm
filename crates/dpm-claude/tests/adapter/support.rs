//! Fixtures shared by the adapter's end-to-end tests: a disposable store with a task its executor
//! owns, scripted provider processes that speak the stream-json control protocol, and read-back
//! through the application's public queries. Each test crate uses its own part of them.
#![allow(dead_code)]

use dpm_app::{Application, CommandRequest};
use dpm_claude::{Options, ParsedOptions};
use dpm_engine::Command;
use dpm_model::{ActivityEntry, ActorId, LifecycleEntry, Plan, RunId, WorkItemId};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// The principal that owns the task and executes the run.
pub fn pilot() -> ActorId {
    ActorId::agent("pilot")
}

/// A disposable workspace whose first task `pilot` has claimed and started.
pub struct Fixture {
    pub directory: tempfile::TempDir,
    pub database: PathBuf,
    pub work_dir: PathBuf,
    pub work: WorkItemId,
}

fn plan() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture plan")
}

fn command(app: &mut Application, actor: &ActorId, command: Command) {
    let base_revision = app.revision_blocking().expect("revision").revision;
    app.execute_blocking(CommandRequest {
        actor: actor.clone(),
        base_revision,
        base_lineage: None,
        operation_id: None,
        command,
    })
    .expect("project command");
}

impl Fixture {
    pub fn new() -> Self {
        let directory = tempfile::tempdir().expect("directory");
        let database = directory.path().join("state.sqlite");
        let work_dir = directory.path().join("work");
        fs::create_dir_all(&work_dir).expect("work directory");
        let plan = plan();
        let work = plan.find_work_by_key("TEST-A").expect("TEST-A").id;
        let mut app = Application::initialize_blocking(&database, &plan).expect("store");
        command(&mut app, &pilot(), Command::Claim { work });
        command(
            &mut app,
            &pilot(),
            Command::Start {
                work,
                occurred_at: None,
            },
        );
        Self {
            directory,
            database,
            work_dir,
            work,
        }
    }

    /// Apply one project command as `actor` through a fresh handle on the store.
    pub fn project_command(&self, actor: &ActorId, project: Command) {
        command(&mut self.app(), actor, project);
    }

    pub fn app(&self) -> Application {
        Application::open_blocking(&self.database).expect("open")
    }

    /// The project's revision and operation count: what a run must never change.
    pub fn project_state(&self) -> (u64, usize) {
        let app = self.app();
        (
            app.revision_blocking().expect("revision").revision,
            app.history_blocking(0, 1000)
                .expect("history")
                .entries
                .len(),
        )
    }

    /// An executable script in the work directory, run by `sh`.
    ///
    /// A short-lived `sh` writes it, never this process: a child another test thread forks inherits
    /// every descriptor open here until it execs, and on Linux executing a file some process holds
    /// open for writing fails with ETXTBSY. With no write descriptor here, no child can hold one.
    pub fn script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.directory.path().join(name);
        let mut writer = std::process::Command::new("/bin/sh")
            .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
            .arg(&path)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .expect("script writer");
        if let Some(mut input) = writer.stdin.take() {
            std::io::Write::write_all(&mut input, format!("#!/bin/sh\n{PRELUDE}{body}").as_bytes())
                .expect("script");
        }
        assert!(writer.wait().expect("script writer").success(), "script");
        path
    }

    /// The command line of a new run of TEST-A by `pilot` with this provider.
    pub fn options(&self, provider: &Path, extra: &[&str]) -> Options {
        let work = self.work_dir.to_string_lossy().into_owned();
        let database = self.database.to_string_lossy().into_owned();
        let claude = provider.to_string_lossy().into_owned();
        let mut words = vec![
            "run",
            "--work",
            "TEST-A",
            "--executor",
            "agent:pilot",
            "--directory",
            &work,
            "--prompt",
            "run the maintenance check",
            "--database",
            &database,
            "--claude",
            &claude,
            "--max-seconds",
            "30",
        ];
        words.extend_from_slice(extra);
        parse(&words)
    }

    /// The command line of a recovery of `run`.
    pub fn resume_options(&self, provider: &Path, run: RunId, extra: &[&str]) -> Options {
        let work = self.work_dir.to_string_lossy().into_owned();
        let database = self.database.to_string_lossy().into_owned();
        let claude = provider.to_string_lossy().into_owned();
        let id = run.to_string();
        let mut words = vec![
            "resume",
            "--run",
            &id,
            "--directory",
            &work,
            "--prompt",
            "continue the maintenance check",
            "--database",
            &database,
            "--claude",
            &claude,
            "--max-seconds",
            "30",
        ];
        words.extend_from_slice(extra);
        parse(&words)
    }

    pub fn activity(&self, run: RunId) -> Vec<ActivityEntry> {
        self.app()
            .run_activity_blocking(0, 1000, Some(run))
            .expect("activity")
            .data
            .entries
    }

    pub fn texts(&self, run: RunId) -> Vec<String> {
        self.activity(run)
            .into_iter()
            .filter_map(|entry| entry.record.text)
            .collect()
    }

    pub fn lifecycle(&self, run: RunId) -> Vec<LifecycleEntry> {
        self.app()
            .run_lifecycle_blocking(0, 100, Some(run))
            .expect("lifecycle")
            .data
            .entries
    }

    /// A marker file a provider script leaves in its working directory.
    pub fn marker(&self, name: &str) -> PathBuf {
        self.work_dir.join(name)
    }
}

pub fn parse(words: &[&str]) -> Options {
    match Options::parse(words.iter().map(|word| (*word).to_string())).expect("valid options") {
        ParsedOptions::Run(options) => *options,
        ParsedOptions::Help => panic!("help is not a run"),
    }
}

/// What every scripted provider starts with: the provider session it was told to use.
const PRELUDE: &str = "SESSION=\"\"\nfor a in \"$@\"; do case \"$a\" in --session-id=*) SESSION=\"${a#--session-id=}\";; --resume=*) SESSION=\"${a#--resume=}\";; esac; done\nprintf '%s\\n' \"$@\" > args.txt\n";

/// A line of provider output, with `@SESSION@` replaced by the session the script was started with.
pub fn emit(value: &Value) -> String {
    let text = value
        .to_string()
        .replace('\'', "'\\''")
        .replace("@SESSION@", "'\"$SESSION\"'");
    format!("printf '%s\\n' '{text}'\n")
}

/// Answer `initialize`, then wait for the prompt, which only arrives once the run is recorded.
pub const HANDSHAKE: &str = "read initialize_request\nprintf '%s\\n' '{\"type\":\"control_response\",\"response\":{\"subtype\":\"success\",\"request_id\":\"dpm-init-1\",\"response\":{}}}'\nread user_prompt && printf '%s\\n' \"$user_prompt\" > prompt.txt && touch prompt-seen\n";

pub fn init_event() -> Value {
    json!({"type": "system", "subtype": "init", "session_id": "@SESSION@", "model": "claude-sonnet-5-5",
        "permissionMode": "default", "claude_code_version": "2.1.287", "apiKeySource": "none", "tools": ["Read", "Write", "AskUserQuestion"],
        "account": {"email": "someone@example.com"}})
}

pub fn tool_use(uuid: &str, id: &str, name: &str, input: &Value) -> Value {
    json!({"type": "assistant", "uuid": uuid, "session_id": "@SESSION@", "message": {"id": "msg_shared", "content": [
        {"type": "tool_use", "id": id, "name": name, "input": input}]}})
}

pub fn tool_result(id: &str, error: bool, text: &str) -> Value {
    json!({"type": "user", "uuid": format!("r-{id}"), "session_id": "@SESSION@", "message": {"content": [
        {"type": "tool_result", "tool_use_id": id, "is_error": error, "content": text}]}})
}

pub fn text(uuid: &str, words: &str) -> Value {
    json!({"type": "assistant", "uuid": uuid, "session_id": "@SESSION@", "message": {"id": "msg_text", "content": [{"type": "text", "text": words}]}})
}

pub fn finished(denials: &Value) -> Value {
    json!({"type": "result", "subtype": "success", "is_error": false, "session_id": "@SESSION@", "num_turns": 3,
        "terminal_reason": "completed", "permission_denials": denials, "total_cost_usd": 0.01})
}
