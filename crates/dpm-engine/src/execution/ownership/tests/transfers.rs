use super::*;

#[test]
fn a_plan_change_cannot_move_owners_or_rewrite_handoffs() {
    let (mut plan, a, _, _) = fs();
    ok(&mut plan, &first(), Command::Claim { work: a }, 0);
    ok(&mut plan, &lead(), handoff(a, first(), second()), 1);
    ok(&mut plan, &second(), release(a), 2);
    reviewed_edit_refused(&mut plan, a, |w| w.execution.handoffs.clear());
    reviewed_edit_refused(&mut plan, a, |w| w.execution.releases.clear());
    ok(&mut plan, &first(), Command::Claim { work: a }, 3);
    reviewed_edit_refused(&mut plan, a, |w| {
        w.execution.owner = Some(ActorId::agent("third"))
    });
    reviewed_edit_refused(&mut plan, a, |w| {
        let mut forged = w.execution.handoffs[0].clone();
        forged.from = ActorId::human("reviewer");
        w.execution.handoffs.push(forged);
    });
}

/// Ownership is changed only by claim, release and handoff, never by a reviewed plan change.
fn reviewed_edit_refused(plan: &mut Plan, work: WorkItemId, edit: fn(&mut dpm_model::WorkItem)) {
    let mut proposed = plan.clone();
    edit(proposed.work_items.get_mut(&work).expect("work"));
    let before = plan.clone();
    let error = crate::apply_plan_change(
        plan,
        lead(),
        &proposed,
        "reassign",
        t(4),
        dpm_model::OperationId::new(),
    )
    .expect_err("refused");
    assert_eq!(*plan, before, "a refused change alters nothing");
    assert!(
        matches!(error, EngineError::InvalidCommand { .. }),
        "{error:?}"
    );
}
