use dpm_app::{AppError, Application, CommandRequest, Query};
use dpm_engine::Command;
use dpm_model::{ActorId, Artifact};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;

const NAMES: &[(&str, &str)] = &[
    ("workspace_list", "List device-local workspace bindings"),
    (
        "workspace_register",
        "Bind an existing local database without changing its project state",
    ),
    (
        "project_status",
        "Project execution counts and schedule forecast",
    ),
    (
        "next_work",
        "Rank executable leaf tasks using explicit capabilities",
    ),
    (
        "get_work",
        "Get task objective, ordered instructions, scope and acceptance criteria",
    ),
    (
        "explain_work",
        "Get the task contract plus resolved requirements, gates, risks, dependencies and evidence",
    ),
    (
        "ratify_contract",
        "Approve a proposed execution contract as a human or service",
    ),
    (
        "reject_work",
        "Return a submission for rework with an independent review",
    ),
    ("claim_work", "Claim a ready task for this configured actor"),
    ("report_blocker", "Suspend work with a concrete blocker"),
    ("unblock_work", "Resume blocked work preserving ownership"),
    (
        "submit_work",
        "Submit owned work for independent verification",
    ),
    (
        "report_progress",
        "Report owned task execution percent; 100% does not verify the result",
    ),
    ("verify_work", "Verify another actor's submitted result"),
    ("add_artifact", "Attach evidence attributed to this actor"),
    (
        "attach_git_head",
        "Attach the current repository HEAD as immutable evidence",
    ),
    ("decide_gate", "Resolve an open decision gate"),
];

pub(crate) fn definitions() -> Vec<Value> {
    NAMES.iter().map(|(name,description)| {
        let read = matches!(*name, "workspace_list" | "project_status" | "next_work" | "get_work" | "explain_work");
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();
        if matches!(*name,"ratify_contract"|"reject_work"|"get_work"|"explain_work"|"claim_work"|"report_blocker"|"unblock_work"|"submit_work"|"verify_work"|"report_progress"|"add_artifact"|"attach_git_head") {
            properties.insert("key".into(),json!({"type":"string"})); required.push("key");
        }
        if !read && *name != "workspace_register" {
            properties.insert("base_revision".into(),json!({"type":"integer","minimum":0}));
            required.push("base_revision");
        }
        match *name {
            "workspace_register" => { properties.insert("database".into(),json!({"type":"string"})); properties.insert("replace".into(),json!({"type":"boolean","default":false})); required.push("database"); },
            "project_status" => { properties.insert("probabilistic".into(),json!({"type":"boolean"})); },
            "next_work" => { properties.insert("capabilities".into(),json!({"type":"array","items":{"type":"string"}})); properties.insert("limit".into(),json!({"type":"integer","minimum":0,"default":5})); properties.insert("probabilistic".into(),json!({"type":"boolean","default":true})); },
            "reject_work" => { properties.insert("reason".into(),json!({"type":"string","minLength":1})); required.push("reason"); },
            "report_blocker" => { properties.insert("blocker".into(),json!({"type":"string","minLength":1})); required.push("blocker"); },
            "report_progress" => {
                properties.insert("percent".into(),json!({"type":"integer","minimum":0,"maximum":100}));
                properties.insert("note".into(),json!({"type":"string"})); required.push("percent");
            },
            "submit_work"|"verify_work" => { properties.insert("note".into(),json!({"type":"string"})); },
            "attach_git_head" => { properties.insert("resource".into(),json!({"type":"string"})); },
            "add_artifact" => { properties.insert("artifact".into(),artifact_schema()); required.push("artifact"); },
            "decide_gate" => {
                properties.insert("decision".into(),json!({"type":"string"})); properties.insert("outcome".into(),json!({"type":"string","minLength":1})); required.extend(["decision","outcome"]);
            },
            _ => {},
        }
        json!({"name":name,"description":description,"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},"annotations":{"readOnlyHint":read,"destructiveHint":!read,"openWorldHint":false}})
    }).collect()
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Arguments {
    key: Option<String>,
    database: Option<String>,
    #[serde(default)]
    replace: bool,
    base_revision: Option<u64>,
    #[serde(default)]
    capabilities: BTreeSet<String>,
    #[serde(default = "default_true")]
    probabilistic: bool,
    #[serde(default = "default_limit")]
    limit: usize,
    blocker: Option<String>,
    reason: Option<String>,
    note: Option<String>,
    percent: Option<u8>,
    artifact: Option<Artifact>,
    resource: Option<String>,
    decision: Option<String>,
    outcome: Option<String>,
}

fn default_true() -> bool {
    true
}
fn default_limit() -> usize {
    5
}

fn required<T>(value: Option<T>, name: &str) -> Result<T, AppError> {
    value.ok_or_else(|| AppError::InvalidRequest(format!("{name} is required")))
}

pub(crate) fn call_tool_blocking(
    app: &mut Application,
    actor: &ActorId,
    name: &str,
    value: Value,
) -> Result<Value, AppError> {
    validate_argument_names(name, &value)?;
    let args: Arguments = serde_json::from_value(value)?;
    if matches!(name, "workspace_list" | "workspace_register") {
        return registry_tool_blocking(name, args);
    }
    let query = match name {
        "project_status" => Some(Query::Status {
            probabilistic: args.probabilistic,
        }),
        "next_work" => Some(Query::Next {
            capabilities: args.capabilities.clone(),
            probabilistic: args.probabilistic,
            limit: args.limit,
        }),
        "get_work" => Some(Query::Show {
            key: required(args.key.clone(), "key")?,
        }),
        "explain_work" => Some(Query::Explain {
            key: required(args.key.clone(), "key")?,
        }),
        _ => None,
    };
    if let Some(query) = query {
        return Ok(serde_json::to_value(app.query_blocking(query)?)?);
    }
    let base_revision = required(args.base_revision, "base_revision")?;
    let command = mutation_blocking(app, actor, name, args)?;
    let operation = app.execute_blocking(CommandRequest {
        actor: actor.clone(),
        base_revision,
        command,
    })?;
    Ok(
        json!({"api_version":dpm_app::API_VERSION,"revision":operation.resulting_revision,"data":operation}),
    )
}

fn mutation_blocking(
    app: &Application,
    actor: &ActorId,
    name: &str,
    args: Arguments,
) -> Result<Command, AppError> {
    app.ensure_writable()?;
    if name == "decide_gate" {
        let key = required(args.decision, "decision")?;
        let decision = app.decision_id_blocking(&key)?;
        return Ok(Command::Decide {
            decision,
            outcome: required(args.outcome, "outcome")?,
        });
    }
    let key = required(args.key, "key")?;
    let work = app.work_id_blocking(&key)?;
    match name {
        "attach_git_head" => Ok(Command::AttachArtifact {
            work,
            artifact: app.git_head_artifact_blocking(
                actor.clone(),
                work,
                args.resource.as_deref(),
            )?,
        }),
        "ratify_contract" => Ok(Command::RatifyContract { work }),
        "reject_work" => Ok(Command::Reject {
            work,
            reason: required(args.reason, "reason")?,
        }),
        "claim_work" => Ok(Command::Claim { work }),
        "report_blocker" => Ok(Command::Block {
            work,
            reason: required(args.blocker, "blocker")?,
        }),
        "unblock_work" => Ok(Command::Unblock { work }),
        "report_progress" => Ok(Command::ReportProgress {
            work,
            percent: required(args.percent, "percent")?,
            note: args.note,
        }),
        "submit_work" => Ok(Command::Submit {
            work,
            note: args.note,
        }),
        "verify_work" => Ok(Command::Verify {
            work,
            note: args.note,
        }),
        "add_artifact" => Ok(Command::AttachArtifact {
            work,
            artifact: required(args.artifact, "artifact")?,
        }),
        _ => Err(AppError::InvalidRequest(format!("unknown tool {name}"))),
    }
}

fn validate_argument_names(name: &str, value: &Value) -> Result<(), AppError> {
    let definition = definitions()
        .into_iter()
        .find(|d| d["name"] == name)
        .ok_or_else(|| AppError::InvalidRequest(format!("unknown tool {name}")))?;
    let fields = value
        .as_object()
        .ok_or_else(|| AppError::InvalidRequest("arguments must be an object".into()))?;
    for key in fields.keys() {
        if definition["inputSchema"]["properties"].get(key).is_none() {
            return Err(AppError::InvalidRequest(format!(
                "unexpected argument {key} for {name}"
            )));
        }
    }
    Ok(())
}
fn artifact_schema() -> Value {
    json!({"type":"object","required":["id","kind","uri","label","metadata","created_by","created_at"],"additionalProperties":false,
    "properties":{
        "id":{"type":"string","format":"uuid"},
        "kind":{"type":"string","enum":["GitCommit","PullRequest","File","Build","TestResult","Datasheet","Quote","PurchaseOrder","Cad","Photo","Other"]},
        "uri":{"type":"string","minLength":1},"label":{"type":"string","minLength":1},
        "metadata":{"type":"object","additionalProperties":{"type":"string"}},
        "created_by":{"type":"object","required":["kind","name"],"additionalProperties":false,
            "properties":{"kind":{"type":"string","enum":["Human","Agent","Service"]},"name":{"type":"string","minLength":1}}},
        "created_at":{"type":"string","format":"date-time"}
    }})
}

fn registry_tool_blocking(name: &str, args: Arguments) -> Result<Value, AppError> {
    let registry = dpm_app::WorkspaceRegistry::from_environment()?;
    let data = if name == "workspace_list" {
        serde_json::to_value(registry.list_blocking()?)?
    } else {
        let database = required(args.database, "database")?;
        serde_json::to_value(
            registry.register_blocking(std::path::Path::new(&database), args.replace)?,
        )?
    };
    Ok(json!({"api_version":dpm_app::API_VERSION,"local_config":true,"data":data}))
}
