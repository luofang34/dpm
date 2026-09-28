use crate::*;
use serde_json::json;

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

fn key(plan: &Plan, key: &str) -> WorkItemId {
    plan.find_work_by_key(key).expect("work").id
}

fn waiver(actor: ActorId) -> DependencyWaiver {
    DependencyWaiver {
        actor,
        at: chrono::Utc::now(),
        reason: "prototype proved the interface".into(),
    }
}

fn rejects(plan: &Plan, fragment: &str) {
    let error = plan.validate().expect_err("invalid").to_string();
    assert!(error.contains(fragment), "{error}");
}

#[test]
fn persisted_edges_require_identity_and_keep_it_when_the_relation_changes() {
    let plan = fixture();
    let source = serde_json::to_value(&plan).expect("serialize");
    for null in [false, true] {
        let mut missing = source.clone();
        let edge = missing["dependencies"][0].as_object_mut().expect("edge");
        if null {
            edge.insert("id".into(), serde_json::Value::Null);
        } else {
            edge.remove("id");
        }
        assert!(serde_json::from_value::<Plan>(missing).is_err());
    }
    let mut edited = source;
    edited["dependencies"][0]["kind"] = json!("StartStart");
    let loaded: Plan = serde_json::from_value(edited).expect("explicit identity");
    assert_eq!(loaded.dependencies[0].id, plan.dependencies[0].id);
    assert_eq!(loaded.dependencies[0].kind, DependencyKind::StartStart);
    loaded.validate().expect("edited relation");
}

#[test]
fn derived_identity_distinguishes_relation_and_direction_and_explicit_ids_persist() {
    let (a, b) = (WorkItemId::new(), WorkItemId::new());
    let ss = Dependency::derived_id(a, b, DependencyKind::StartStart);
    assert_eq!(ss, Dependency::derived_id(a, b, DependencyKind::StartStart));
    assert_ne!(
        ss,
        Dependency::derived_id(a, b, DependencyKind::FinishFinish)
    );
    assert_ne!(ss, Dependency::derived_id(b, a, DependencyKind::StartStart));
    assert_eq!(ss.0.get_version_num(), 8);
    let explicit = DependencyId::new();
    let edge: Dependency = serde_json::from_value(json!({
        "id": explicit, "predecessor": a, "successor": b, "kind": "FinishFinish",
        "lag_hours": 1.0, "policy": "Soft", "rationale": "overlap is acceptable"
    }))
    .expect("edge");
    assert_eq!(edge.id, explicit);
    assert_eq!(edge.policy, DependencyPolicy::Soft);
    let unknown = serde_json::from_value::<Dependency>(json!({
        "predecessor": a, "successor": b, "kind": "FinishStart", "lag_hours": 0.0, "hard": true
    }));
    assert!(unknown.is_err());
}

#[test]
fn start_start_and_finish_finish_coexist_but_identities_and_relations_are_unique() {
    let mut plan = fixture();
    let (a, b) = (key(&plan, "TEST-A"), key(&plan, "TEST-B"));
    plan.dependencies
        .push(Dependency::new(a, b, DependencyKind::StartStart, 1.0));
    plan.dependencies
        .push(Dependency::new(a, b, DependencyKind::FinishFinish, 2.0));
    plan.validate().expect("SS and FF on one ordered pair");
    let mut duplicate_id = plan.clone();
    let mut copy = Dependency::new(a, b, DependencyKind::StartFinish, 0.0);
    copy.id = plan.dependencies[0].id;
    duplicate_id.dependencies.push(copy);
    rejects(&duplicate_id, "duplicate dependency identity");
    let mut duplicate_relation = plan.clone();
    let mut again = Dependency::new(a, b, DependencyKind::StartStart, 3.0);
    again.id = DependencyId::new();
    duplicate_relation.dependencies.push(again);
    rejects(&duplicate_relation, "duplicate SS relation");
}

type EdgeEdit = fn(&mut Dependency);

#[test]
fn waivers_require_soft_policy_a_non_agent_actor_and_reasons() {
    let base = fixture();
    let cases: Vec<(EdgeEdit, &str)> = vec![
        (
            |d| d.waiver = Some(waiver(ActorId::human("lead"))),
            "hard constraints cannot be waived",
        ),
        (
            |d| {
                d.policy = DependencyPolicy::Soft;
                d.waiver = Some(waiver(ActorId::agent("coder")));
            },
            "human or service actor",
        ),
        (
            |d| {
                d.policy = DependencyPolicy::Soft;
                let mut record = waiver(ActorId::human("lead"));
                record.reason = " ".into();
                d.waiver = Some(record);
            },
            "human or service actor and a reason",
        ),
        (
            |d| d.rationale = Some("\n".into()),
            "rationale must not be empty",
        ),
    ];
    for (edit, fragment) in cases {
        let mut plan = base.clone();
        edit(&mut plan.dependencies[0]);
        rejects(&plan, fragment);
    }
    let mut plan = base;
    plan.dependencies[0].policy = DependencyPolicy::Soft;
    plan.dependencies[0].waiver = Some(waiver(ActorId::service("release-bot")));
    plan.validate().expect("soft waiver by a service");
}

#[test]
fn work_links_validate_references_self_links_duplicates_and_notes() {
    let base = fixture();
    let (a, b) = (key(&base, "TEST-A"), key(&base, "TEST-B"));
    let link = |kind, source, target| WorkLink {
        kind,
        source,
        target,
        note: None,
    };
    let mut plan = base.clone();
    plan.links = vec![
        link(WorkLinkKind::RelatesTo, a, b),
        link(WorkLinkKind::Duplicates, a, b),
        link(WorkLinkKind::DerivedFrom, b, a),
        link(WorkLinkKind::Supersedes, b, a),
    ];
    plan.validate().expect("one link of each kind");
    let cases = [
        (
            link(WorkLinkKind::RelatesTo, a, WorkItemId::new()),
            "missing work",
        ),
        (
            link(WorkLinkKind::Supersedes, a, a),
            "cannot link to itself",
        ),
        (link(WorkLinkKind::RelatesTo, b, a), "duplicate link"),
        (link(WorkLinkKind::Supersedes, a, b), "duplicate link"),
        (
            WorkLink {
                note: Some(" ".into()),
                ..link(WorkLinkKind::RelatesTo, a, key(&base, "TEST-C"))
            },
            "note must not be empty",
        ),
    ];
    for (bad, fragment) in cases {
        let mut invalid = plan.clone();
        invalid.links.push(bad);
        rejects(&invalid, fragment);
    }
    let serialized = serde_json::to_value(&base).expect("serialize");
    assert!(serialized.get("links").is_none());
}

/// Stores that have not been rewritten since edge identity was introduced rely on re-deriving the
/// same identities on every load, so the derivation is pinned to a known vector.
#[test]
fn derived_identity_is_pinned_and_spreads_similar_inputs() {
    let id = |text: &str| WorkItemId(uuid::Uuid::parse_str(text).expect("uuid"));
    let pinned = Dependency::derived_id(
        id("b75ac42f-0b89-504f-933a-20175beaf664"),
        id("ee7071a5-4557-5d2f-8737-d958a34187b8"),
        DependencyKind::FinishStart,
    );
    assert_eq!(pinned.to_string(), "92f552d8-2ee8-8c06-b145-bdcbabd20cbc");
    let (a, b) = (
        id("00000007-0000-4000-8000-000000000001"),
        id("00000007-0000-4000-8000-000000000002"),
    );
    let ss = Dependency::derived_id(a, b, DependencyKind::StartStart);
    let ff = Dependency::derived_id(a, b, DependencyKind::FinishFinish);
    assert_ne!(ss.0.as_bytes()[..4], ff.0.as_bytes()[..4]);
}
