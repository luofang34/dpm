use super::*;
use crate::{ExternalLinkRequest, apply_command, change::differences};
use dpm_model::{
    DependencyId, ExternalIdentity, ExternalLinkRole, ExternalObjectKind, ExternalProvider,
    ExternalReferenceId, Key, WorkItemId, WorkLink, WorkLinkKind, WorkStatus,
};

type Edit = fn(&mut Plan);

fn fixtures() -> Vec<(&'static str, Plan)> {
    let parse = |json: &str| serde_json::from_str::<Plan>(json).expect("fixture");
    vec![
        (
            "execution",
            parse(include_str!(
                "../../../../../tests/support/execution-plan.json"
            )),
        ),
        (
            "conditional",
            parse(include_str!(
                "../../../../../tests/support/conditional-plan.json"
            )),
        ),
        (
            "self-host",
            parse(include_str!(
                "../../../../../examples/self-host/dpm-alpha.json"
            )),
        ),
    ]
}

/// Plans are equal up to the order of edges and links, which the difference treats as sets.
fn canonical(plan: &Plan) -> Plan {
    let mut plan = plan.clone();
    plan.dependencies.sort_by_key(|edge| edge.id);
    plan.links.sort_by_key(|l| (l.kind, l.source, l.target));
    plan
}

fn first_key<K: Copy, V>(map: &std::collections::BTreeMap<K, V>) -> Option<K> {
    map.keys().next().copied()
}

fn last_key<K: Copy, V>(map: &std::collections::BTreeMap<K, V>) -> Option<K> {
    map.keys().next_back().copied()
}

fn clone_first<K: Copy + Ord, V: Clone>(
    map: &mut std::collections::BTreeMap<K, V>,
    fresh: K,
    rekey: impl Fn(&mut V, K),
) {
    if let Some(mut copy) = map.values().next().cloned() {
        rekey(&mut copy, fresh);
        map.insert(fresh, copy);
    }
}

fn edits() -> Vec<(&'static str, Edit)> {
    [work_edits(), context_edits(), relation_edits()].concat()
}

fn work_edits() -> Vec<(&'static str, Edit)> {
    vec![
        ("workspace", |p| p.workspace.name.push_str(" (renamed)")),
        ("work title", |p| {
            if let Some(w) = p.work_items.values_mut().next() {
                w.title.push_str(" clarified");
            }
        }),
        ("work added", |p| {
            clone_first(&mut p.work_items, WorkItemId::new(), |w, id| {
                w.id = id;
                w.key = Key::new("ADDED-WORK");
                w.execution.status = WorkStatus::Proposed;
            });
        }),
        ("work removed", |p| {
            let id = last_key(&p.work_items);
            id.map(|id| p.work_items.remove(&id));
        }),
    ]
}

fn context_edits() -> Vec<(&'static str, Edit)> {
    vec![
        ("project edited", |p| {
            if let Some(project) = p.projects.values_mut().next() {
                project.title.push_str(" (renamed)");
            }
        }),
        ("project added", |p| {
            clone_first(&mut p.projects, dpm_model::ProjectId::new(), |x, id| {
                x.id = id;
                x.key = Key::new("ADDED-PROJECT");
            });
        }),
        ("requirement edited and added", |p| {
            if let Some(r) = p.requirements.values_mut().next() {
                r.title.push_str(" (tightened)");
            }
            clone_first(
                &mut p.requirements,
                dpm_model::RequirementId::new(),
                |r, id| {
                    r.id = id;
                    r.key = Key::new("ADDED-REQ");
                },
            );
        }),
        ("requirement removed", |p| {
            let id = first_key(&p.requirements);
            id.map(|id| p.requirements.remove(&id));
        }),
        ("decision edited, added and removed", |p| {
            if let Some(d) = p.decisions.values_mut().next() {
                d.question.push_str(" (restated)");
            }
            let removed = last_key(&p.decisions);
            clone_first(&mut p.decisions, dpm_model::DecisionId::new(), |d, id| {
                d.id = id;
                d.key = Key::new("ADDED-DEC");
            });
            removed.map(|id| p.decisions.remove(&id));
        }),
        ("risk edited, added and removed", |p| {
            if let Some(r) = p.risks.values_mut().next() {
                r.description.push_str(" (reassessed)");
            }
            let removed = last_key(&p.risks);
            clone_first(&mut p.risks, dpm_model::RiskId::new(), |r, id| {
                r.id = id;
                r.key = Key::new("ADDED-RISK");
            });
            removed.map(|id| p.risks.remove(&id));
        }),
        ("asset edited, added and removed", |p| {
            if let Some(r) = p.assets.values_mut().next() {
                r.label.push_str(" (relabelled)");
            }
            let removed = last_key(&p.assets);
            clone_first(&mut p.assets, dpm_model::AssetId::new(), |r, id| {
                r.id = id;
                r.key = Key::new("ADDED-RESOURCE");
            });
            removed.map(|id| p.assets.remove(&id));
        }),
    ]
}

fn relation_edits() -> Vec<(&'static str, Edit)> {
    vec![
        ("dependency edited, removed and appended", |p| {
            if let Some(edge) = p.dependencies.first_mut() {
                edge.lag_hours += 2.0;
            }
            let copy = p.dependencies.pop();
            if let Some(mut copy) = copy {
                copy.id = DependencyId::new();
                p.dependencies.push(copy);
            }
        }),
        ("links added", |p| {
            let ids: Vec<_> = p.work_items.keys().copied().take(3).collect();
            for pair in ids.windows(2) {
                p.links.push(WorkLink {
                    kind: WorkLinkKind::RelatesTo,
                    source: pair[1],
                    target: pair[0],
                    note: None,
                });
            }
        }),
        ("tracking reference relabelled", |p| {
            if let Some(r) = p.external_references.values_mut().next() {
                r.label.push_str(" (moved)");
            }
        }),
        ("tracking reference removed", |p| {
            let id = first_key(&p.external_references);
            id.map(|id| p.external_references.remove(&id));
        }),
    ]
}

/// A fixture with one tracking reference and one link, so every collection has an entity.
fn populated(mut plan: Plan) -> Plan {
    let ids: Vec<_> = plan.work_items.keys().copied().take(2).collect();
    if let [a, b] = ids[..] {
        plan.links.push(WorkLink {
            kind: WorkLinkKind::DerivedFrom,
            source: b,
            target: a,
            note: Some("shared context".into()),
        });
    }
    let work = plan
        .work_items
        .values()
        .find(|w| w.is_executable())
        .map(|w| w.id)
        .expect("task");
    let link = Command::LinkExternal(ExternalLinkRequest {
        work,
        reference: ExternalReferenceId::new(),
        identity: ExternalIdentity {
            provider: ExternalProvider::Forgejo,
            instance: "code.example.org".into(),
            namespace: Some("ops/dpm".into()),
            kind: ExternalObjectKind::Issue,
            external_id: "42".into(),
        },
        label: "Tracked issue".into(),
        url: None,
        role: ExternalLinkRole::Tracks,
        observed: None,
    });
    apply_command(
        &mut plan,
        ActorId::human("linker"),
        link,
        Utc::now(),
        OperationId::new(),
    )
    .expect("link");
    plan
}

#[test]
fn patching_with_the_difference_reproduces_every_scripted_edit() {
    for (name, fixture) in fixtures() {
        let current = populated(fixture);
        let mut all = current.clone();
        for (label, edit) in edits() {
            let mut proposed = current.clone();
            edit(&mut proposed);
            edit(&mut all);
            let changes = differences(&current, &proposed).expect("diff");
            let patched = patch(&current, &changes).expect("patch");
            assert_eq!(patched, proposed, "{name}: {label}");
        }
        let changes = differences(&current, &all).expect("diff");
        assert!(changes.len() > 10, "{name}: every edit is a change");
        let patched = patch(&current, &changes).expect("patch");
        assert_eq!(canonical(&patched), canonical(&all), "{name}: all edits");
    }
}

#[test]
fn patch_keeps_retained_edges_in_place_and_appends_additions() {
    let (_, current) = fixtures().remove(0);
    let mut proposed = current.clone();
    let removed = proposed.dependencies.remove(1);
    let mut first = removed.clone();
    first.id = DependencyId::new();
    let mut second = removed;
    second.id = DependencyId::new();
    // Inserting at the front is a reorder the difference does not record.
    proposed.dependencies.insert(0, first.clone());
    proposed.dependencies.push(second.clone());
    let patched = patch(&current, &differences(&current, &proposed).expect("diff")).expect("patch");
    assert_eq!(canonical(&patched), canonical(&proposed));
    let retained: Vec<_> = current
        .dependencies
        .iter()
        .map(|e| e.id)
        .filter(|id| proposed.find_dependency(*id).is_some())
        .collect();
    let order: Vec<_> = patched.dependencies.iter().map(|e| e.id).collect();
    assert_eq!(order[..retained.len()], retained[..]);
    assert!(order[retained.len()..].contains(&first.id));
    assert!(order[retained.len()..].contains(&second.id));
}

#[test]
fn a_change_computed_against_another_state_is_stale() {
    let (_, current) = fixtures().remove(0);
    let mut proposed = current.clone();
    proposed.find_work_by_key_mut("TEST-A").expect("task").title = "Reviewed title".into();
    let command = plan_change(&current, &proposed, "clarify").expect("delta");
    let mut moved = current.clone();
    moved
        .find_work_by_key_mut("TEST-A")
        .expect("task")
        .title
        .push_str(" meanwhile");
    let Command::ApplyChange { changes, .. } = &command else {
        panic!("plan change");
    };
    let error = patch(&moved, changes).expect_err("stale");
    assert!(
        matches!(&error, EngineError::StaleChange { collection, id: Some(_) } if collection == "work_items"),
        "{error:?}"
    );
    assert!(
        error.to_string().contains("diff the proposal again"),
        "{error}"
    );
    let before = moved.clone();
    let refused = apply_command(
        &mut moved,
        ActorId::human("lead"),
        command,
        Utc::now(),
        OperationId::new(),
    );
    assert!(matches!(refused, Err(EngineError::StaleChange { .. })));
    assert_eq!(moved, before, "a stale change alters nothing");
}

#[test]
fn an_addition_of_an_existing_entity_is_stale() {
    let (_, current) = fixtures().remove(0);
    let mut proposed = current.clone();
    proposed.workspace.name = "Renamed".into();
    let mut changes = differences(&current, &proposed).expect("diff");
    changes[0].before = Value::Null;
    assert!(matches!(
        patch(&current, &changes),
        Err(EngineError::StaleChange { .. })
    ));
}

#[test]
fn duplicate_padded_or_reordered_changes_are_refused() {
    let (_, current) = fixtures().remove(0);
    let mut proposed = current.clone();
    proposed.workspace.name = "Renamed".into();
    proposed.find_work_by_key_mut("TEST-A").expect("task").title = "Reviewed title".into();
    let changes = differences(&current, &proposed).expect("diff");
    let duplicated = [changes.clone(), changes[..1].to_vec()].concat();
    assert!(matches!(
        patch(&current, &duplicated),
        Err(EngineError::InvalidCommand { .. })
    ));
    let mut reordered = changes.clone();
    reordered.reverse();
    let mut plan = current.clone();
    let refused = apply_command(
        &mut plan,
        ActorId::human("lead"),
        Command::ApplyChange {
            changes: reordered,
            reason: "reordered".into(),
        },
        Utc::now(),
        OperationId::new(),
    );
    assert!(matches!(refused, Err(EngineError::InvalidCommand { .. })));
    assert_eq!(plan, current);
    let mut mislabelled = changes;
    mislabelled[0].fields = vec!["id".into()];
    let refused = apply_command(
        &mut plan,
        ActorId::human("lead"),
        Command::ApplyChange {
            changes: mislabelled,
            reason: "mislabelled".into(),
        },
        Utc::now(),
        OperationId::new(),
    );
    assert!(matches!(refused, Err(EngineError::InvalidCommand { .. })));
}

#[test]
fn unknown_targets_are_refused() {
    let (_, current) = fixtures().remove(0);
    let change = EntityChange {
        collection: "artifacts".into(),
        id: Some("x".into()),
        fields: Vec::new(),
        before: Value::Null,
        after: serde_json::json!({}),
    };
    assert!(matches!(
        patch(&current, &[change]),
        Err(EngineError::InvalidCommand { .. })
    ));
}

#[test]
fn a_one_field_edit_on_a_large_plan_logs_a_small_operation() {
    let (_, mut plan) = fixtures().remove(2);
    let template = plan
        .work_items
        .values()
        .find(|w| w.execution.status == WorkStatus::Planned)
        .cloned()
        .expect("planned work");
    for index in 0..400 {
        let mut copy = template.clone();
        copy.id = WorkItemId::new();
        copy.key = Key::new(format!("BULK-{index}"));
        plan.work_items.insert(copy.id, copy);
    }
    plan.validate().expect("large plan");
    let mut proposed = plan.clone();
    proposed
        .work_items
        .values_mut()
        .find(|w| w.execution.status == WorkStatus::Planned)
        .expect("planned")
        .title = "One clarified title".into();
    let plan_size = serde_json::to_string(&plan).expect("json").len();
    let operation = apply_plan_change(
        &mut plan,
        ActorId::human("lead"),
        &proposed,
        "clarify one title",
        Utc::now(),
        OperationId::new(),
    )
    .expect("apply");
    let logged = serde_json::to_string(&operation).expect("json").len();
    assert!(
        logged * 10 < plan_size,
        "operation {logged} bytes, plan {plan_size} bytes"
    );
    assert_eq!(canonical(&plan).work_items, canonical(&proposed).work_items);
}

#[test]
fn logged_commands_and_operations_reject_unknown_fields() {
    let legacy = serde_json::json!({"ApplyChange": {"plan": {}, "reason": "whole plan"}});
    assert!(serde_json::from_value::<Command>(legacy).is_err());
    let padded = serde_json::json!({"Claim": {"work": WorkItemId::new(), "extra": 1}});
    assert!(serde_json::from_value::<Command>(padded).is_err());
    let (_, mut plan) = fixtures().remove(0);
    let work = plan.find_work_by_key("TEST-A").expect("task").id;
    let operation = apply_command(
        &mut plan,
        ActorId::agent("owner"),
        Command::Claim { work },
        Utc::now(),
        OperationId::new(),
    )
    .expect("claim");
    let mut json = serde_json::to_value(&operation).expect("json");
    assert!(serde_json::from_value::<Operation>(json.clone()).is_ok());
    json["note"] = "unexpected".into();
    assert!(serde_json::from_value::<Operation>(json).is_err());
    let mut change = serde_json::to_value(EntityChange {
        collection: "workspace".into(),
        id: None,
        fields: Vec::new(),
        before: Value::Null,
        after: Value::Null,
    })
    .expect("json");
    change["extra"] = true.into();
    assert!(serde_json::from_value::<EntityChange>(change).is_err());
}
