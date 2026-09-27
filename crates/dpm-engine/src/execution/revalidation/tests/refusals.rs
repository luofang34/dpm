use super::*;

#[test]
fn reviewed_changes_set_the_basis_policy_only_before_execution_and_never_author_history() {
    let mut p = provisional(0.0, DependencyPolicy::Hard);
    let mut verified = p.plan.clone();
    verified.dependencies[0].start_basis = StartBasis::Verified;
    let reason = "B waits for verified A";
    let mut probe = p.plan.clone();
    let change = crate::plan_change(&probe, &verified, reason).expect("delta");
    ok(&mut probe, &reviewer(), change, 0);
    started_on_first_attempt(&mut p);
    verified = p.plan.clone();
    verified.dependencies[0].start_basis = StartBasis::Verified;
    let mut forged = p.plan.clone();
    forged
        .work_items
        .get_mut(&p.b)
        .expect("b")
        .execution
        .basis
        .clear();
    for proposal in [verified, forged] {
        let before = p.plan.clone();
        crate::apply_plan_change(
            &mut p.plan,
            reviewer(),
            &proposal,
            reason,
            t(4),
            dpm_model::OperationId::new(),
        )
        .expect_err("refused");
        assert_eq!(p.plan, before, "a refused change alters nothing");
    }
}
