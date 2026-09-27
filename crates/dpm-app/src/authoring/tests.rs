use super::*;
use crate::{Application, Query};
use chrono::{DateTime, Duration, TimeZone, Utc};
use dpm_engine::{Command, apply_command};
use dpm_model::{ActorId, DependencyPolicy, ExecutionEvents, StartBasis, WorkStatus};
use std::collections::{BTreeMap, BTreeSet};

mod model_record;
mod model_trace;
mod schema_walk;
mod validator;
use validator::Validator;

fn fixture(name: &str) -> Value {
    let text = match name {
        "execution" => include_str!("../../../../tests/support/execution-plan.json"),
        "conditional" => include_str!("../../../../tests/support/conditional-plan.json"),
        _ => include_str!("../../../../examples/self-host/dpm-alpha.json"),
    };
    serde_json::from_str(text).expect("fixture")
}

fn plan(value: &Value) -> Plan {
    serde_json::from_value(value.clone()).expect("the model accepts the document")
}

fn at(minutes: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0)
        .single()
        .expect("time")
        + Duration::minutes(minutes)
}

/// The execution fixture with a provisional and a soft edge, ready for the commands below.
fn prepared() -> (Plan, [String; 6]) {
    let mut plan = plan(&fixture("execution"));
    let id = |plan: &Plan, key: &str| plan.find_work_by_key(key).expect("work").id;
    let (b, c, e) = (
        id(&plan, "TEST-B"),
        id(&plan, "TEST-C"),
        id(&plan, "TEST-E"),
    );
    for edge in &mut plan.dependencies {
        if edge.predecessor == b && edge.successor == c {
            edge.start_basis = StartBasis::Provisional;
        }
        if edge.successor == e {
            edge.policy = DependencyPolicy::Soft;
        }
    }
    let soft = plan
        .dependencies
        .iter()
        .find(|d| d.successor == e)
        .expect("soft")
        .id;
    let gate = plan.find_decision_by_key("TEST-GATE").expect("gate").id;
    let ids = [
        id(&plan, "TEST-A").to_string(),
        b.to_string(),
        c.to_string(),
        e.to_string(),
        soft.to_string(),
        gate.to_string(),
    ];
    (plan, ids)
}

/// The execution fixture after the commands that write managed fields.
fn executed() -> Value {
    let (mut plan, [a, b, c, e, soft, gate]) = prepared();
    let worker = json!({"kind": "Agent", "name": "worker"});
    let reviewer = json!({"kind": "Human", "name": "reviewer"});
    let artifact = json!({
        "id": derived_id(1, 99), "kind": "TestResult", "uri": "file:report.txt", "label": "Report",
        "metadata": {"purpose": "evidence"}, "created_by": worker, "created_at": at(3)
    });
    let link = json!({
        "work": e, "reference": derived_id(2, 99), "label": "Issue 7",
        "identity": {"provider": "GitHub", "instance": "github.com", "namespace": "org/repo", "kind": "Issue", "external_id": "7"},
        "url": "https://github.com/org/repo/issues/7", "role": "Tracks", "observed": "Open"
    });
    let steps = [
        (
            &reviewer,
            json!({"Decide": {"decision": gate, "outcome": "accepted"}}),
        ),
        (&worker, json!({"Claim": {"work": a}})),
        (&worker, json!({"Start": {"work": a}})),
        (
            &worker,
            json!({"ReportProgress": {"work": a, "percent": 50, "note": null}}),
        ),
        (
            &worker,
            json!({"AttachArtifact": {"work": a, "artifact": artifact}}),
        ),
        (&worker, json!({"Block": {"work": a, "reason": "waiting"}})),
        (&worker, json!({"Unblock": {"work": a}})),
        (&worker, json!({"Submit": {"work": a, "note": null}})),
        (
            &reviewer,
            json!({"Reject": {"work": a, "reason": "incomplete"}}),
        ),
        (&worker, json!({"Submit": {"work": a, "note": null}})),
        (&reviewer, json!({"Verify": {"work": a, "note": null}})),
        (&worker, json!({"Claim": {"work": b}})),
        (&worker, json!({"Start": {"work": b}})),
        (&worker, json!({"Submit": {"work": b, "note": null}})),
        (&worker, json!({"Claim": {"work": c}})),
        (
            &worker,
            json!({"Release": {"work": c, "reason": "claimed early"}}),
        ),
        (&worker, json!({"Claim": {"work": c}})),
        (&worker, json!({"Start": {"work": c}})),
        (
            &reviewer,
            json!({"Handoff": {"work": c, "from": worker, "to": {"kind": "Agent", "name": "second"}, "reason": "reassigned"}}),
        ),
        (
            &reviewer,
            json!({"WaiveDependency": {"dependency": soft, "reason": "overlap accepted"}}),
        ),
        (&worker, json!({"LinkExternal": link})),
    ];
    for (minute, (actor, command)) in (0..).zip(steps) {
        let actor: ActorId = serde_json::from_value(actor.clone()).expect("actor");
        let command: Command = serde_json::from_value(command).expect("command shape");
        apply_command(
            &mut plan,
            actor,
            command,
            at(minute),
            dpm_model::OperationId::new(),
        )
        .expect("command");
    }
    serde_json::to_value(plan).expect("serialize")
}

/// Input-only and rarely produced shapes, each accepted by the model's deserializer.
fn authored_extremes() -> Value {
    let mut doc = executed();
    let first = |map: &mut Value| {
        map.as_object_mut()
            .and_then(|m| m.values_mut().next())
            .map(Value::take)
            .expect("entity")
    };
    let mut decision = first(&mut doc["decisions"]);
    decision["supersedes"] = Value::from(derived_id(3, 99));
    decision["rationale"] = "chosen for review".into();
    decision["related_work"] = decision["blocks"].clone();
    decision["artifact_ids"] = json!([]);
    decision["options"] = json!([{"key": "accepted", "label": "Accepted"}]);
    let id = decision["id"].clone();
    doc["decisions"] = json!({id.as_str().expect("id"): decision});
    let works = doc["work_items"].as_object_mut().expect("work");
    for (index, work) in works.values_mut().enumerate() {
        work["contract"]["join"] = if index % 2 == 0 {
            json!({"mode": "all_predecessors"})
        } else {
            json!({"mode": "active_branches", "allow_empty": true})
        };
        if let Some(basis) = work["execution"]
            .get_mut("basis")
            .and_then(Value::as_array_mut)
        {
            let mut revalidated = basis[0].clone();
            revalidated["source"] = json!({"kind": "revalidation", "actor": {"kind": "Human", "name": "lead"}, "reason": "re-read"});
            basis.push(revalidated);
        }
    }
    let ids: Vec<Value> = works
        .keys()
        .take(2)
        .map(|k| Value::from(k.as_str()))
        .collect();
    doc["links"] =
        json!([{"kind": "RelatesTo", "source": ids[0], "target": ids[1], "note": "context"}]);
    doc["dependencies"][0]["rationale"] = "ordering".into();
    for asset in doc["assets"]
        .as_object_mut()
        .into_iter()
        .flat_map(|m| m.values_mut())
    {
        asset["kind"] = json!({"Other": "Laboratory"});
    }
    for reference in doc["external_references"]
        .as_object_mut()
        .into_iter()
        .flat_map(|m| m.values_mut())
    {
        reference["identity"]["provider"] = json!({"Other": "Gerrit"});
        reference["identity"]["kind"] = json!({"Other": "Change"});
    }
    plan(&doc);
    doc
}

/// Work started before start times were recorded, then blocked: the only writer of
/// `events.start_unrecorded`.
fn legacy_blocked() -> Value {
    let mut plan = plan(&fixture("execution"));
    let worker = ActorId::agent("w");
    let work = plan.find_work_by_key_mut("TEST-A").expect("work");
    work.execution.status = WorkStatus::InProgress;
    work.execution.owner = Some(worker.clone());
    work.execution.events = ExecutionEvents::default();
    let id = work.id.to_string();
    let block = json!({"Block": {"work": id, "reason": "waiting on a vendor"}});
    let command: Command = serde_json::from_value(block).expect("command shape");
    apply_command(
        &mut plan,
        worker,
        command,
        at(0),
        dpm_model::OperationId::new(),
    )
    .expect("block");
    let document = serde_json::to_value(plan).expect("serialize");
    assert_eq!(
        document["work_items"][&id]["execution"]["events"],
        json!({"start_unrecorded": true})
    );
    document
}

fn documents() -> Vec<(&'static str, Value)> {
    let empty = Plan::empty("Authoring");
    vec![
        ("self-host seed", fixture("seed")),
        ("execution fixture", fixture("execution")),
        ("conditional fixture", fixture("conditional")),
        ("executed", executed()),
        ("authored extremes", authored_extremes()),
        ("legacy blocked", legacy_blocked()),
        ("empty", serde_json::to_value(&empty).expect("empty")),
        ("template", plan_template(&empty).expect("template")),
    ]
}

#[test]
fn the_schema_accepts_every_plan_the_model_writes_and_names_no_field_it_lacks() {
    let schema = plan_schema().expect("schema");
    let mut validator = Validator::new(&schema);
    for (name, document) in documents() {
        if let Err(error) = validator.validate(&document) {
            panic!("{name}: {error}");
        }
    }
    let unseen: BTreeSet<_> = validator
        .declared()
        .difference(validator.seen())
        .cloned()
        .collect();
    assert!(
        unseen.is_empty(),
        "schema properties no document exercises: {unseen:?}"
    );
}

/// Disagreements between `schema` and the model's own serde field and variant names.
fn model_gaps(schema: &Value) -> Vec<String> {
    let types = model_trace::trace_plan().expect("trace the model");
    let tags: BTreeMap<String, String> = types
        .iter()
        .filter_map(|(name, container)| match container {
            model_trace::Container::Tagged { tag, .. } => Some((name.clone(), tag.clone())),
            _ => None,
        })
        .collect();
    let mut tagged = model_record::TaggedFields::new();
    for (name, document) in documents() {
        if let Err(error) = model_record::record(&plan(&document), &tags, &mut tagged) {
            panic!("{name}: {error}");
        }
    }
    // A work item omits its default join, so no plan serializes that variant.
    model_record::record(&dpm_model::JoinPolicy::default(), &tags, &mut tagged).expect("join");
    schema_walk::compare(schema, &types, &tagged)
}

#[test]
fn the_schema_declares_exactly_the_fields_and_variants_the_model_serializes() {
    let gaps = model_gaps(&plan_schema().expect("schema"));
    assert!(
        gaps.is_empty(),
        "schema and model disagree:\n{}",
        gaps.join("\n")
    );
}

#[test]
fn the_model_comparison_reports_each_kind_of_drift() {
    let schema = plan_schema().expect("schema");
    let join = "/$defs/WorkContract/properties/join/oneOf/1/properties";
    let drifts = [
        (
            "writes `join`",
            "/$defs/WorkContract/properties",
            "join",
            None,
        ),
        ("writes `allow_empty`", join, "allow_empty", None),
        (
            "declares `bogus`",
            join,
            "bogus",
            Some(json!({"type": "string"})),
        ),
        (
            "declares `bogus`",
            "/$defs/Events/properties",
            "bogus",
            Some(json!({"type": "boolean"})),
        ),
        (
            "writes `Soft`",
            "/$defs/Dependency/properties/policy",
            "enum",
            Some(json!(["Hard"])),
        ),
        (
            "declares Some(\"number\")",
            "/$defs/Attempt/properties/number",
            "type",
            Some("number".into()),
        ),
        (
            "allows null",
            "/$defs/WorkItem/properties",
            "title",
            Some(json!({"anyOf": [{"type": "string"}, {"type": "null"}]})),
        ),
        (
            "reaches this definition",
            "/$defs",
            "Stale",
            Some(json!({"type": "string"})),
        ),
    ];
    for (expected, at, key, value) in drifts {
        let mut drifted = schema.clone();
        let object = drifted
            .pointer_mut(at)
            .and_then(Value::as_object_mut)
            .expect("schema object");
        match value {
            Some(value) => drop(object.insert(key.into(), value)),
            None => drop(object.remove(key).expect("declared")),
        }
        let gaps = model_gaps(&drifted);
        assert!(
            gaps.iter().any(|gap| gap.contains(expected)),
            "{expected} not reported: {gaps:?}"
        );
    }
}

#[test]
fn the_schema_rejects_misnamed_and_mistyped_authoring_fields() {
    let schema = plan_schema().expect("schema");
    let mut validator = Validator::new(&schema);
    let mut document = fixture("execution");
    let work = document["work_items"]
        .as_object_mut()
        .and_then(|m| m.values_mut().next())
        .expect("work");
    work["description"] = "misnamed objective".into();
    assert!(validator.validate(&document).is_err());
    let mut document = fixture("execution");
    document["dependencies"][0]["kind"] = "FS".into();
    assert!(validator.validate(&document).is_err());
    assert!(
        serde_json::from_value::<Plan>(document).is_err(),
        "the model agrees"
    );
}

#[test]
fn the_template_applies_to_an_empty_workspace_and_is_refused_elsewhere() {
    let empty = Plan::empty("Authoring");
    let first = plan_template(&empty).expect("template");
    assert_eq!(
        first,
        plan_template(&empty).expect("again"),
        "deterministic"
    );
    let template = plan(&first);
    assert_eq!(template.workspace, empty.workspace);
    let tasks: Vec<_> = template
        .work_items
        .values()
        .filter(|w| w.is_executable())
        .collect();
    assert!(
        tasks
            .iter()
            .all(|t| t.execution.status == dpm_model::WorkStatus::Proposed
                && !t.contract.acceptance.is_empty())
    );
    let mut app = Application::in_memory_blocking(&empty).expect("app");
    let data = app.query_blocking(Query::PlanTemplate).expect("query").data;
    assert_eq!(data, first);
    let preview = app
        .query_blocking(Query::ProposeChange {
            plan: Box::new(template.clone()),
        })
        .expect("diff");
    assert_eq!(preview.revision, 0);
    app.apply_plan_change_blocking(crate::PlanChangeRequest {
        actor: ActorId::human("reviewer"),
        base_revision: 0,
        plan: Box::new(template),
        reason: "start from the template".into(),
    })
    .expect("apply");
    let applied = app.plan_blocking().expect("plan");
    assert_eq!(applied.work_items.len(), 4);
    assert_eq!(
        app.query_blocking(Query::PlanTemplate)
            .expect_err("refused")
            .code(),
        "invalid_request"
    );
    let fixture = plan(&fixture("execution"));
    assert!(plan_template(&fixture).is_err());
}
