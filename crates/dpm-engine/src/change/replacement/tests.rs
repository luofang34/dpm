use crate::{Command, EngineError, apply_command, explain_work, gate_report, propose_change};
use chrono::Utc;
use dpm_model::{
    ActorId, Artifact, ArtifactId, ArtifactKind, Decision, DecisionId, DecisionStatus, Key, Plan,
    WorkItemId, WorkStatus,
};
use std::collections::{BTreeMap, BTreeSet};

fn work(plan: &Plan, key: &str) -> WorkItemId {
    plan.find_work_by_key(key).expect("work").id
}

/// Fixture with a claimed task and a Decided, non-gating choice backed by a source artifact.
fn fixture() -> (Plan, DecisionId) {
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let source = Artifact {
        id: ArtifactId::new(),
        kind: ArtifactKind::File,
        uri: "repo:docs/choice.md".into(),
        label: "Design discussion".into(),
        metadata: BTreeMap::new(),
        created_by: ActorId::human("planner"),
        created_at: Utc::now(),
    };
    let choice = Decision {
        id: DecisionId::new(),
        key: Key::new("TEST-CHOICE"),
        project: plan.find_work_by_key("TEST-A").expect("task").project,
        question: "Which input format?".into(),
        status: DecisionStatus::Decided,
        outcome: Some("JSON".into()),
        resolved_at: None,
        rationale: Some("Existing tooling reads JSON".into()),
        related_work: BTreeSet::from([work(&plan, "TEST-A"), work(&plan, "TEST-C")]),
        artifact_ids: BTreeSet::from([source.id]),
        blocks: BTreeSet::new(),
        supersedes: None,
    };
    plan.artifacts.insert(source.id, source);
    let id = choice.id;
    plan.decisions.insert(id, choice);
    plan.validate().expect("valid fixture");
    let task = work(&plan, "TEST-A");
    apply_command(
        &mut plan,
        ActorId::agent("worker"),
        Command::Claim { work: task },
        Utc::now(),
    )
    .expect("claim");
    (plan, id)
}

fn replace(plan: &Plan, old: DecisionId) -> (Plan, Decision) {
    let mut proposal = plan.clone();
    let mut new = proposal.decisions[&old].clone();
    proposal.decisions.get_mut(&old).expect("old").status = DecisionStatus::Superseded;
    new.id = DecisionId::new();
    new.key = Key::new("TEST-CHOICE-2");
    new.status = DecisionStatus::Decided;
    new.outcome = Some("TOML".into());
    new.rationale = Some("Reviewers edit TOML by hand".into());
    new.artifact_ids.clear();
    new.related_work = BTreeSet::from([work(plan, "TEST-A"), work(plan, "TEST-F")]);
    new.supersedes = Some(old);
    proposal.decisions.insert(new.id, new.clone());
    (proposal, new)
}

fn apply(plan: &mut Plan, proposed: Plan) -> Result<crate::Operation, EngineError> {
    apply_command(
        plan,
        ActorId::human("planner"),
        Command::ApplyChange {
            plan: Box::new(proposed),
            reason: "revise the input format".into(),
        },
        Utc::now(),
    )
}

fn gates(plan: &Plan) -> Vec<(WorkItemId, bool)> {
    plan.work_items
        .keys()
        .map(|id| {
            (
                *id,
                gate_report(plan, *id, crate::Transition::Claim, chrono::Utc::now())
                    .expect("gates")
                    .ready,
            )
        })
        .collect()
}

#[test]
fn replacement_preserves_prior_reasoning_lists_affected_work_and_adds_no_gate() {
    let (mut plan, old) = fixture();
    let (proposal, new) = replace(&plan, old);
    let preview = propose_change(&plan, &proposal).expect("reviewable replacement");
    let superseded = preview
        .changes
        .iter()
        .find(|c| c.id.as_deref() == Some(&old.to_string()))
        .expect("old decision change");
    assert_eq!(superseded.fields, ["status"]);
    let affected: Vec<_> = preview
        .affected_work
        .iter()
        .map(|a| (a.decision.0.as_str(), a.key.0.as_str(), a.status))
        .collect();
    assert_eq!(
        affected,
        [
            ("TEST-CHOICE", "TEST-A", WorkStatus::Claimed),
            ("TEST-CHOICE", "TEST-C", WorkStatus::Planned),
            ("TEST-CHOICE", "TEST-F", WorkStatus::Planned),
        ]
    );
    let readiness = gates(&plan);
    let prior = plan.decisions[&old].clone();
    apply(&mut plan, proposal).expect("apply replacement");
    assert_eq!(gates(&plan), readiness, "context must not become a gate");
    let kept = &plan.decisions[&old];
    assert_eq!(kept.status, DecisionStatus::Superseded);
    assert_eq!(
        (&kept.rationale, &kept.artifact_ids, &kept.outcome),
        (&prior.rationale, &prior.artifact_ids, &prior.outcome)
    );
    let context = explain_work(&plan, work(&plan, "TEST-A"), chrono::Utc::now())
        .expect("explain")
        .context;
    let keys: Vec<_> = context.decisions.iter().map(|d| d.key.0.as_str()).collect();
    assert!(keys.contains(&"TEST-CHOICE") && keys.contains(&"TEST-CHOICE-2"));
    assert!(
        context
            .decisions
            .iter()
            .any(|d| d.id == new.id && d.supersedes == Some(old))
    );
    assert!(
        context
            .artifacts
            .iter()
            .any(|a| prior.artifact_ids.contains(&a.id))
    );
}

#[test]
fn gate_bypass_rewritten_reasoning_and_unlinked_replacements_fail_atomically() {
    let (mut plan, old) = fixture();
    let before = plan.clone();
    let gate = plan
        .decisions
        .values()
        .find(|d| d.status == DecisionStatus::Open)
        .expect("open gate")
        .id;
    for case in 0..7 {
        let (mut proposal, new) = replace(&plan, old);
        match case {
            0 => {
                let (mut bypass, replacement) = replace(&plan, gate);
                let replacement = bypass.decisions.get_mut(&replacement.id).expect("new");
                replacement.blocks.clear();
                proposal = bypass;
            }
            1 => {
                proposal.decisions.get_mut(&old).expect("old").rationale =
                    Some("Rewritten history".into());
            }
            2 => {
                proposal.decisions.remove(&new.id);
            }
            3 => {
                let new = proposal.decisions.get_mut(&new.id).expect("new");
                new.blocks.insert(work(&plan, "TEST-F"));
            }
            4 => {
                proposal.decisions.get_mut(&new.id).expect("new").rationale = None;
            }
            5 => {
                proposal.decisions.get_mut(&new.id).expect("new").supersedes =
                    Some(DecisionId::new());
            }
            _ => {
                proposal.decisions.remove(&new.id);
                proposal.decisions.remove(&old);
            }
        }
        let error = propose_change(&plan, &proposal)
            .expect_err("rejected")
            .to_string();
        let expected = [
            "use decide for an open outcome",
            "use decide for an open outcome",
            "needs a linked replacement",
            "cannot add execution gates",
            "Decided outcome with its rationale",
            "missing replaced decision",
            "use decide for an open outcome",
        ][case];
        assert!(error.contains(expected), "case {case}: {error}");
        assert!(apply(&mut plan, proposal).is_err(), "case {case}");
        assert_eq!(plan, before, "case {case}");
    }
    let (mut stale, _) = replace(&plan, old);
    stale.revision = stale.revision.wrapping_add(1);
    assert!(matches!(
        apply(&mut plan, stale),
        Err(EngineError::RevisionConflict { .. })
    ));
    assert_eq!(plan, before);
}

#[test]
fn work_linked_only_to_a_superseded_choice_still_sees_its_replacement() {
    let (mut plan, old) = fixture();
    let (proposal, new) = replace(&plan, old);
    assert!(!new.related_work.contains(&work(&plan, "TEST-C")));
    apply(&mut plan, proposal).expect("reviewed replacement");
    let context = explain_work(&plan, work(&plan, "TEST-C"), chrono::Utc::now())
        .expect("explain")
        .context;
    let keys: BTreeSet<_> = context.decisions.iter().map(|d| d.key.0.as_str()).collect();
    assert_eq!(keys, BTreeSet::from(["TEST-CHOICE", "TEST-CHOICE-2"]));
    assert!(
        context
            .decisions
            .iter()
            .any(|d| d.id == new.id && d.supersedes == Some(old))
    );
    let unrelated = explain_work(&plan, work(&plan, "TEST-B"), chrono::Utc::now())
        .expect("explain")
        .context;
    assert!(unrelated.decisions.iter().all(|d| d.id != new.id));
}

#[test]
fn chained_replacement_lists_work_that_sees_it_only_through_the_chain() {
    let (mut plan, first) = fixture();
    let (proposal, second) = replace(&plan, first);
    apply(&mut plan, proposal).expect("first replacement");
    let mut proposal = plan.clone();
    proposal
        .decisions
        .get_mut(&second.id)
        .expect("second")
        .status = DecisionStatus::Superseded;
    let mut third = second.clone();
    third.id = DecisionId::new();
    third.key = Key::new("TEST-CHOICE-3");
    third.outcome = Some("YAML".into());
    third.rationale = Some("Configuration files already use YAML".into());
    third.related_work.clear();
    third.supersedes = Some(second.id);
    proposal.decisions.insert(third.id, third.clone());
    let preview = propose_change(&plan, &proposal).expect("reviewable chain");
    let listed: BTreeSet<_> = preview
        .affected_work
        .iter()
        .map(|a| a.key.0.as_str())
        .collect();
    assert_eq!(listed, BTreeSet::from(["TEST-A", "TEST-C", "TEST-F"]));
    apply(&mut plan, proposal).expect("second replacement");
    for key in listed {
        let context = explain_work(&plan, work(&plan, key), chrono::Utc::now())
            .expect("explain")
            .context;
        assert!(context.decisions.iter().any(|d| d.id == third.id), "{key}");
    }
}
