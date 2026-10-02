//! Run tools. Runs are observations kept beside the plan, so these tools are neither project
//! queries nor project mutations: the writers take no `base_revision` (a run never edits the plan),
//! their identities are the run id, `event_id` and activity `key`, and the optional `base_lineage`
//! refuses a store that continues another history. Their arguments are parsed on their own so the
//! shared argument struct stays within the member limit.

use super::{preconditions::Preconditions, required};
use dpm_app::{
    AppError, Application, Envelope, Query, RunActivityRequest, RunCommand, RunLinkRequest,
    RunReportRequest, RunStartRequest,
};
use dpm_model::{
    ActivityInput, ActivityKind, ActorId, Observation, OperationId, RunEventId, RunId, RunSession,
    RunSource, RunState,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};

/// Tool names this module owns, with the descriptions listed by `tools/list`.
pub(super) const NAMES: [(&str, &str); 8] = [
    (
        "start_run",
        "Record the start of a run on a claimed or started task this actor (or the named executor) owns, with the contract and plan revision it observed. The run id is the idempotency key; a run never changes the plan, its revision or the task's owner",
    ),
    (
        "report_run",
        "Record a run's lifecycle transition (working, waiting, failed, interrupted, completed); the last three are final. Ending a run submits, verifies and releases nothing",
    ),
    (
        "record_run_activity",
        "Record one bounded activity record (tool_started, tool_result, progress, input_requested, heartbeat) for a run. It changes no lifecycle state, plan or ownership; the key makes duplicate delivery harmless",
    ),
    (
        "link_run_operation",
        "Link a committed project operation to the run that performed it; refused unless it is the run's executor's operation on the run's task within the run's lifetime and lineage",
    ),
    (
        "list_runs",
        "List runs, newest first, with lifecycle, derived freshness (working, waiting, failed, interrupted, completed, stale or unknown) and attribution; optionally one task's",
    ),
    (
        "get_run",
        "Get one run with its observed contract, source, lifecycle, derived freshness, attribution and linked operations",
    ),
    (
        "run_lifecycle",
        "Read durable run lifecycle facts after a cursor; they are never pruned, so the feed is continuous",
    ),
    (
        "run_activity",
        "Read bounded run activity after a cursor independent of the lifecycle cursor; a cursor that retention outran reports a gap",
    ),
];

pub(super) fn handles(name: &str) -> bool {
    NAMES.iter().any(|(tool, _)| *tool == name)
}

fn is_read(name: &str) -> bool {
    matches!(
        name,
        "list_runs" | "get_run" | "run_lifecycle" | "run_activity"
    )
}

fn uuid(description: &str) -> Value {
    json!({"type":"string","format":"uuid","description":description})
}

fn timestamp(what: &str) -> Value {
    json!({"type":"string","format":"date-time","description":format!("When the executor says {what}; a claim, never used to judge freshness")})
}

fn sources_schema() -> Value {
    json!({"type":"array","maxItems":8,"description":"Exact source references: a full commit of a repository asset the task requires, or Git commit evidence attached to this task whose metadata and locator name one exact commit. A branch or abbreviation is not an exact source","items":{"oneOf":[
        {"type":"object","additionalProperties":false,"required":["kind","asset","commit"],"properties":{
            "kind":{"const":"git_commit"},"asset":uuid("Repository asset id"),
            "commit":{"type":"string","pattern":"^([0-9a-f]{40}|[0-9a-f]{64})$"}}},
        {"type":"object","additionalProperties":false,"required":["kind","artifact"],"properties":{
            "kind":{"const":"artifact"},"artifact":uuid("Artifact id of GitCommit evidence attached to this task, as attach_git_head records it")}}]}})
}

/// The properties and required names of one tool's input schema.
#[derive(Default)]
struct Schema {
    properties: Map<String, Value>,
    needed: Vec<&'static str>,
}

impl Schema {
    fn field(&mut self, name: &'static str, schema: Value, required: bool) {
        self.properties.insert(name.into(), schema);
        if required {
            self.needed.push(name);
        }
    }
}

fn start(schema: &mut Schema) {
    schema.field(
        "key",
        json!({"type":"string","description":"Key of the task being executed"}),
        true,
    );
    schema.field(
        "run_id",
        uuid("Version 7 run identity and idempotency key; minted when omitted"),
        false,
    );
    schema.field("executor", json!({"type":"string","pattern":"^(human|agent|service):.+$","description":"KIND:NAME doing the work; defaults to this tool's configured actor"}), false);
    schema.field("parent", uuid("Run that spawned this one"), false);
    schema.field("session", json!({"type":"object","additionalProperties":false,"required":["provider","session"],"properties":{
        "provider":{"type":"string","pattern":"^[a-z][a-z0-9_-]{0,31}$"},"session":{"type":"string","minLength":1},"turn":{"type":"string","minLength":1},
        "provenance":{"type":"object","additionalProperties":false,"description":"What the recorder attests about the runtime, fixed when the run starts","properties":{
            "requested_model":{"type":"string","minLength":1,"maxLength":128},"observed_model":{"type":"string","minLength":1,"maxLength":128},
            "runtime_version":{"type":"string","minLength":1,"maxLength":128},"configuration_digest":{"type":"string","pattern":"^[0-9a-f]{64}$"}}}}}), false);
    schema.field("observation", json!({"type":"string","enum":["managed","reported_only"],"default":"reported_only","description":"Only a service may record managed; silence from a reported-only run proves nothing"}), false);
    schema.field("sources", sources_schema(), false);
    schema.field("observed_at", timestamp("the run began"), false);
}

fn report(schema: &mut Schema) {
    schema.field("run", uuid("Run identity"), true);
    schema.field(
        "state",
        json!({"type":"string","enum":["working","waiting","failed","interrupted","completed"]}),
        true,
    );
    schema.field(
        "event_id",
        uuid("Version 7 transition identity and idempotency key; minted when omitted"),
        false,
    );
    schema.field(
        "detail",
        json!({"type":"string","minLength":1,"maxLength":2048}),
        false,
    );
    schema.field("observed_at", timestamp("it happened"), false);
}

fn record(schema: &mut Schema) {
    schema.field("run", uuid("Run identity"), true);
    schema.field("source_sequence", json!({"type":"integer","minimum":1,"maximum":18446744073709551615u64,"description":"The reporter's sequence number for this record within the run, at least 1 and increasing, gaps allowed. Resending it is harmless while the record is retained; one that retention already removed is refused as expired (activity_expired)"}), true);
    schema.field("kind", json!({"type":"string","enum":["tool_started","tool_result","progress","input_requested","heartbeat"]}), true);
    schema.field(
        "text",
        json!({"type":"string","description":"Public text only; cut at 4096 bytes and flagged"}),
        false,
    );
    schema.field("observed_at", timestamp("it happened"), false);
}

fn feed(schema: &mut Schema) {
    schema.field(
        "after_sequence",
        json!({"type":"integer","minimum":0,"default":0}),
        false,
    );
    schema.field(
        "limit",
        json!({"type":"integer","minimum":0,"maximum":1000,"default":100}),
        false,
    );
    schema.field("run", uuid("Only this run's records"), false);
}

/// The `tools/list` entry of one run tool.
pub(super) fn definition(name: &str, description: &str, cli: Option<&'static str>) -> Value {
    let mut schema = Schema::default();
    match name {
        "start_run" => start(&mut schema),
        "report_run" => report(&mut schema),
        "record_run_activity" => record(&mut schema),
        "link_run_operation" => {
            schema.field("run", uuid("Run identity"), true);
            schema.field(
                "operation",
                uuid("Operation id returned by the committed project command"),
                true,
            );
        }
        "list_runs" => {
            schema.field(
                "key",
                json!({"type":"string","description":"Only runs executing this task key"}),
                false,
            );
            schema.field(
                "limit",
                json!({"type":"integer","minimum":0,"maximum":1000,"default":100}),
                false,
            );
        }
        "get_run" => schema.field("run", uuid("Run identity"), true),
        _ => feed(&mut schema),
    }
    let read = is_read(name);
    if !read {
        schema.field("base_lineage", uuid("lineage_id observed with the project revision; a store continuing another lineage refuses the write with lineage_mismatch"), false);
    }
    json!({"name":name,"description":description,
        "inputSchema":{"type":"object","properties":schema.properties,"required":schema.needed,"additionalProperties":false},
        "annotations":{"readOnlyHint":read,"destructiveHint":false,"openWorldHint":false},
        "_meta":{"dpm/cli":cli}})
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunArguments {
    key: Option<String>,
    run_id: Option<RunId>,
    executor: Option<String>,
    parent: Option<RunId>,
    session: Option<RunSession>,
    observation: Option<Observation>,
    #[serde(default)]
    sources: Vec<RunSource>,
    observed_at: Option<chrono::DateTime<chrono::Utc>>,
    run: Option<RunId>,
    state: Option<RunState>,
    event_id: Option<RunEventId>,
    detail: Option<String>,
    kind: Option<ActivityKind>,
    text: Option<String>,
    operation: Option<OperationId>,
    #[serde(default)]
    after_sequence: u64,
    source_sequence: Option<u64>,
    limit: Option<u16>,
}

pub(super) fn call_blocking(
    app: &mut Application,
    actor: &ActorId,
    name: &str,
    value: Value,
    preconditions: Preconditions,
) -> Result<Value, AppError> {
    let args: RunArguments = serde_json::from_value(value)?;
    let base_lineage = preconditions.base_lineage;
    let limit = args.limit.unwrap_or(100);
    let response = match name {
        "start_run" => app.execute_run_blocking(RunCommand::Start(RunStartRequest {
            actor: actor.clone(),
            work_key: required(args.key, "key")?,
            executor: args.executor.as_deref().map(str::parse).transpose()?,
            run_id: args.run_id,
            parent: args.parent,
            session: args.session,
            observation: args.observation.unwrap_or(Observation::ReportedOnly),
            sources: args.sources,
            observed_at: args.observed_at,
            base_lineage,
        }))?,
        "report_run" => app.execute_run_blocking(RunCommand::Report(RunReportRequest {
            actor: actor.clone(),
            run: required(args.run, "run")?,
            state: required(args.state, "state")?,
            event_id: args.event_id,
            detail: args.detail,
            observed_at: args.observed_at,
            base_lineage,
        }))?,
        "record_run_activity" => {
            app.execute_run_blocking(RunCommand::Activity(RunActivityRequest {
                actor: actor.clone(),
                entries: vec![ActivityInput {
                    run: required(args.run, "run")?,
                    source_sequence: required(args.source_sequence, "source_sequence")?,
                    kind: required(args.kind, "kind")?,
                    text: args.text,
                    observed_at: args.observed_at,
                }],
                base_lineage,
            }))?
        }
        "link_run_operation" => app.execute_run_blocking(RunCommand::Link(RunLinkRequest {
            actor: actor.clone(),
            run: required(args.run, "run")?,
            operation: required(args.operation, "operation")?,
            base_lineage,
        }))?,
        "list_runs" => app.query_blocking(Query::Runs {
            key: args.key,
            limit,
        })?,
        "get_run" => app.query_blocking(Query::Run {
            id: required(args.run, "run")?,
        })?,
        "run_lifecycle" => app.query_blocking(Query::RunLifecycle {
            after_sequence: args.after_sequence,
            limit,
            run: args.run,
        })?,
        "run_activity" => app.query_blocking(Query::RunActivity {
            after_sequence: args.after_sequence,
            limit,
            run: args.run,
        })?,
        _ => return Err(AppError::InvalidRequest(format!("unknown tool {name}"))),
    };
    Ok(serde_json::to_value(Envelope::from(response))?)
}
