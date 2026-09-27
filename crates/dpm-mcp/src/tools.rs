use dpm_app::ExternalLinkInput;
use dpm_app::{AppError, Application, CommandRequest, PlanChangeRequest, Query};
use dpm_engine::Command;
use dpm_model::{ActorId, Artifact, ExternalIdentity, ExternalLinkRole, ExternalState};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;

mod interchange;
mod ownership;

const NAMES: &[(&str, &str)] = &[
    (
        "export_plan",
        "Get the full authoritative plan for a reviewed proposal",
    ),
    (
        "import_mspdi",
        "Map a Microsoft Project XML document onto a reviewed candidate with a per-item report; changes nothing, and new tasks stay Proposed",
    ),
    (
        "export_mspdi",
        "Write one project's work as the supported Microsoft Project XML subset with a report of omitted data",
    ),
    (
        "plan_schema",
        "Get the JSON Schema of the plan that export_plan returns and propose_change/apply_change accept",
    ),
    (
        "plan_template",
        "Get a minimal valid proposal for this workspace while it has no projects or work; edit it, then propose_change and apply_change",
    ),
    (
        "propose_change",
        "Validate an edited export and preview semantic differences without changing state; new tasks must be Proposed",
    ),
    (
        "apply_change",
        "Apply a reviewed proposal as a human or service with a reason; execution and evidence remain protected",
    ),
    (
        "history",
        "Read append-only semantic operations in chronological pages",
    ),
    (
        "workspace_list",
        "List device-local workspace bindings with each store's status and any identity sharing its path",
    ),
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
        "Rank executable leaf tasks on the full graph, then apply capability eligibility, optional project/resource scope and limit; eligible work outside scope stays listed",
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
    (
        "claim_work",
        "Reserve a ready task for this configured actor; a claim is not a start",
    ),
    (
        "start_work",
        "Start a claimed task owned by this actor, recording the start event SS/SF successors wait for",
    ),
    ("report_blocker", "Suspend work with a concrete blocker"),
    ("unblock_work", "Resume blocked work preserving ownership"),
    (
        "submit_work",
        "Submit started work for independent verification once FF/SF prerequisites are released",
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
    (
        "decide_gate",
        "Human or service resolves an open decision (agents are refused); a decision with options takes one option key, which selects the work conditioned on it",
    ),
    (
        "link_external",
        "Link work to a provider-scoped external issue or pull request; context only, never evidence, dependency satisfaction or verification",
    ),
    (
        "unlink_external",
        "Remove a work item's external tracking link without changing the work graph",
    ),
    (
        "waive_dependency",
        "Stop enforcing a soft dependency as a human or service with a reason; hard dependencies need plan review",
    ),
    (
        "restore_dependency",
        "Enforce a waived soft dependency again as a human or service with a reason",
    ),
    (
        "revalidate_basis",
        "Re-base started work on the predecessor's current attempt after the attempt it relied on was rejected; an independent human or service only",
    ),
];

pub(crate) fn definitions() -> Vec<Value> {
    NAMES.iter().chain(ownership::NAMES.iter()).map(|(name,description)| {
        let read = matches!(*name, "export_plan" | "plan_schema" | "plan_template" | "import_mspdi" | "export_mspdi" | "propose_change" | "history" | "workspace_list" | "project_status" | "next_work" | "get_work" | "explain_work");
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();
        if matches!(*name,"ratify_contract"|"reject_work"|"get_work"|"explain_work"|"claim_work"|"start_work"|"report_blocker"|"unblock_work"|"submit_work"|"verify_work"|"report_progress"|"add_artifact"|"attach_git_head"|"link_external"|"unlink_external"|"revalidate_basis") {
            properties.insert("key".into(),json!({"type":"string"})); required.push("key");
        }
        if !read && *name != "workspace_register" {
            properties.insert("base_revision".into(),json!({"type":"integer","minimum":0}));
            required.push("base_revision");
        }
        match *name {
            "propose_change"|"apply_change" => {
                properties.insert("plan".into(), json!({"type":"object","description":"Full export_plan data (shape: plan_schema; start an empty workspace from plan_template) with the same workspace identity and observed revision; preserve execution/evidence fields. Omitted entities are reviewed deletions."})); required.push("plan");
                if *name == "apply_change" { properties.insert("reason".into(),json!({"type":"string","minLength":1})); required.push("reason"); }
            },
            "import_mspdi" | "export_mspdi" => interchange::schema(name, &mut properties, &mut required),
            "release_work" | "handoff_work" => ownership::schema(name, &mut properties, &mut required),
            "history" => { properties.insert("after_sequence".into(),json!({"type":"integer","minimum":0,"default":0})); properties.insert("limit".into(),json!({"type":"integer","minimum":0,"maximum":1000,"default":100})); },
            "workspace_register" => { properties.insert("database".into(),json!({"type":"string"})); properties.insert("replace".into(),json!({"type":"boolean","default":false})); required.push("database"); },
            "project_status" => { properties.insert("probabilistic".into(),json!({"type":"boolean"})); },
            "next_work" => { properties.insert("capabilities".into(),json!({"type":"array","items":{"type":"string"}})); properties.insert("limit".into(),json!({"type":"integer","minimum":0,"default":5})); properties.insert("probabilistic".into(),json!({"type":"boolean","default":true}));
                properties.insert("project_keys".into(),json!({"type":"array","items":{"type":"string"},"description":"Project keys whose subtrees form a query-only scope; not a directory"}));
                properties.insert("resource_keys".into(),json!({"type":"array","items":{"type":"string"},"description":"Resource keys returned work must fit: at least one named, every write listed"})); },
            "reject_work" => { properties.insert("reason".into(),json!({"type":"string","minLength":1})); required.push("reason"); },
            "report_blocker" => { properties.insert("blocker".into(),json!({"type":"string","minLength":1})); required.push("blocker"); },
            "report_progress" => {
                properties.insert("percent".into(),json!({"type":"integer","minimum":0,"maximum":100}));
                properties.insert("note".into(),json!({"type":"string"})); required.push("percent");
            },
            "submit_work"|"verify_work" => { properties.insert("note".into(),json!({"type":"string"})); },
            "attach_git_head" => { properties.insert("resource".into(),json!({"type":"string"})); },
            "add_artifact" => { properties.insert("artifact".into(),artifact_schema()); required.push("artifact"); },
            "link_external"|"unlink_external" => {
                properties.insert("identity".into(),identity_schema()); required.push("identity");
                if *name == "link_external" {
                    properties.insert("label".into(),json!({"type":"string","minLength":1}));
                    properties.insert("url".into(),json!({"type":"string","description":"http(s) URL on the identity instance (www.github.com is github.com): no userinfo, a plain path, any query parameter the credential detector does not flag, and a plain fragment"}));
                    properties.insert("role".into(),json!({"type":"string","enum":["Tracks","Relates"],"default":"Tracks"}));
                    properties.insert("observed".into(),json!({"type":"string","enum":["Open","Closed","Merged"],"description":"Reported external state; an observation only"}));
                }
            }
            "waive_dependency"|"restore_dependency" => {
                properties.insert("dependency".into(),json!({"type":"string","format":"uuid","description":"Stable dependency id from explain_work context.dependencies or export_plan"}));
                properties.insert("reason".into(),json!({"type":"string","minLength":1})); required.extend(["dependency","reason"]);
            },
            "revalidate_basis" => {
                properties.insert("dependency".into(),json!({"type":"string","format":"uuid","description":"Provisional edge id from explain_work basis.relies_on or gates"}));
                properties.insert("attempt".into(),json!({"type":"integer","minimum":1,"description":"Predecessor attempt the reviewer checked; must be its current pending or verified attempt"}));
                properties.insert("reason".into(),json!({"type":"string","minLength":1})); required.extend(["dependency","attempt","reason"]);
            },
            "decide_gate" => {
                properties.insert("decision".into(),json!({"type":"string"})); properties.insert("outcome".into(),json!({"type":"string","minLength":1,"description":"Outcome text, or exactly one option key when the decision lists options"})); required.extend(["decision","outcome"]);
            },
            _ => {},
        }
        json!({"name":name,"description":description,"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},"annotations":{"readOnlyHint":read,"destructiveHint":!read,"openWorldHint":false}})
    }).collect()
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Arguments {
    plan: Option<Box<dpm_model::Plan>>,
    #[serde(default)]
    after_sequence: u64,
    key: Option<String>,
    database: Option<String>,
    #[serde(default)]
    replace: bool,
    base_revision: Option<u64>,
    #[serde(default)]
    capabilities: BTreeSet<String>,
    #[serde(default)]
    project_keys: BTreeSet<String>,
    #[serde(default)]
    resource_keys: BTreeSet<String>,
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
    identity: Option<ExternalIdentity>,
    label: Option<String>,
    url: Option<String>,
    role: Option<ExternalLinkRole>,
    observed: Option<ExternalState>,
    dependency: Option<String>,
    attempt: Option<u32>,
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
    if ownership::handles(name) {
        return ownership::call_blocking(app, actor, name, value);
    }
    if interchange::handles(name) {
        return Ok(serde_json::to_value(
            app.query_blocking(interchange::query(name, value)?)?,
        )?);
    }
    let history_limit = value.get("limit").cloned().unwrap_or(json!(100));
    let args: Arguments = serde_json::from_value(value)?;
    if matches!(name, "workspace_list" | "workspace_register") {
        return registry_tool_blocking(name, args);
    }
    let query = match name {
        "export_plan" => Some(Query::Export),
        "plan_schema" => Some(Query::PlanSchema),
        "plan_template" => Some(Query::PlanTemplate),
        "propose_change" => Some(Query::ProposeChange {
            plan: required(args.plan.clone(), "plan")?,
        }),
        "history" => Some(Query::History {
            after_sequence: args.after_sequence,
            limit: serde_json::from_value(history_limit)?,
        }),
        "project_status" => Some(Query::Status {
            probabilistic: args.probabilistic,
        }),
        "next_work" => Some(Query::Next {
            capabilities: args.capabilities.clone(),
            probabilistic: args.probabilistic,
            limit: args.limit,
            project_keys: args.project_keys.clone(),
            resource_keys: args.resource_keys.clone(),
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
    let operation = if name == "apply_change" {
        app.ensure_writable()?;
        app.apply_plan_change_blocking(PlanChangeRequest {
            actor: actor.clone(),
            base_revision,
            plan: required(args.plan, "plan")?,
            reason: required(args.reason, "reason")?,
        })?
    } else {
        let command = mutation_blocking(app, actor, name, args)?;
        app.execute_blocking(CommandRequest {
            actor: actor.clone(),
            base_revision,
            command,
        })?
    };
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
    if matches!(
        name,
        "waive_dependency" | "restore_dependency" | "revalidate_basis"
    ) {
        return dependency_command(app, name, args);
    }
    let key = required(args.key, "key")?;
    if name == "link_external" {
        let input = ExternalLinkInput {
            identity: required(args.identity, "identity")?,
            label: args.label,
            url: args.url,
            role: args.role.unwrap_or(ExternalLinkRole::Tracks),
            observed: args.observed,
        };
        return app.external_link_command_blocking(&key, input);
    }
    if name == "unlink_external" {
        return app.external_unlink_command_blocking(&key, &required(args.identity, "identity")?);
    }
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
        "start_work" => Ok(Command::Start { work }),
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

/// Commands naming a dependency edge by its stable identity.
fn dependency_command(app: &Application, name: &str, args: Arguments) -> Result<Command, AppError> {
    // The CLI resolves the work key before the edge; the same order yields the same error.
    let work = match name {
        "revalidate_basis" => Some(app.work_id_blocking(&required(args.key, "key")?)?),
        _ => None,
    };
    let dependency = app.dependency_id_blocking(&required(args.dependency, "dependency")?)?;
    let reason = required(args.reason, "reason")?;
    Ok(match (name, work) {
        ("waive_dependency", _) => Command::WaiveDependency { dependency, reason },
        ("restore_dependency", _) => Command::RestoreDependency { dependency, reason },
        (_, work) => Command::RevalidateBasis {
            work: required(work, "key")?,
            dependency,
            attempt: required(args.attempt, "attempt")?,
            reason,
        },
    })
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

fn identity_schema() -> Value {
    let other = json!({"type":"object","required":["Other"],"additionalProperties":false,"properties":{"Other":{"type":"string","minLength":1}}});
    json!({"type":"object","required":["provider","instance","kind","external_id"],"additionalProperties":false,
    "description":"Provider-scoped identity, separate from label and URL; equal IDs on other instances or namespaces are different objects",
    "properties":{
        "provider":{"oneOf":[{"type":"string","enum":["GitHub","GitLab","Forgejo","Gitea","Jira","Linear"]},other]},
        "instance":{"type":"string","minLength":1,"description":"host[:port] of the hosted or self-hosted instance"},
        "namespace":{"type":"string","description":"Tenant, owner/repository or project namespace; required for forges and Linear"},
        "kind":{"oneOf":[{"type":"string","enum":["Issue","PullRequest"]},other],"description":"Kind from the provider table; kinds sharing a number space (GitHub issue/pull request/discussion, GitLab issue/incident/task, any Jira or Linear issue type) name one object"},
        "external_id":{"type":"string","minLength":1,"description":"A number on GitHub, GitLab, Forgejo and Gitea; a PROJECT-N key on Jira and Linear"}
    }})
}

fn registry_tool_blocking(name: &str, args: Arguments) -> Result<Value, AppError> {
    let registry = dpm_app::WorkspaceRegistry::from_environment()?;
    let data = if name == "workspace_list" {
        serde_json::to_value(registry.inspect_blocking()?)?
    } else {
        let database = required(args.database, "database")?;
        serde_json::to_value(
            registry.register_blocking(std::path::Path::new(&database), args.replace)?,
        )?
    };
    Ok(json!({"api_version":dpm_app::API_VERSION,"local_config":true,"data":data}))
}
