use super::*;
use crate::{ExecutionStep, Plan, WorkInstructions, WorkKind};

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../../tests/support/execution-plan.json"
    ))
    .expect("synthetic graph")
}

fn instructions() -> WorkInstructions {
    WorkInstructions {
        steps: vec![ExecutionStep {
            action: "Inspect the supplied input".into(),
            expected_result: "Recorded input validation result".into(),
        }],
        in_scope: vec!["Input validation report".into()],
        out_of_scope: vec!["Changing the input".into()],
        verification: vec!["Compare the report with the input contract".into()],
    }
}

#[test]
fn absent_instructions_remain_compatible_and_present_instructions_roundtrip() {
    let mut plan = fixture();
    assert!(
        plan.work_items
            .values()
            .all(|w| w.contract.instructions.is_none())
    );
    plan.validate().expect("legacy plan");
    plan.find_work_by_key_mut("TEST-A")
        .expect("task")
        .contract
        .instructions = Some(instructions());
    plan.validate().expect("complete instructions");
    let json = serde_json::to_string(&plan).expect("serialize");
    assert_eq!(
        serde_json::from_str::<Plan>(&json).expect("roundtrip"),
        plan
    );
}

#[test]
fn incomplete_steps_scope_and_checks_are_rejected_with_work_context() {
    for edit in [
        |i: &mut WorkInstructions| i.steps.clear(),
        |i: &mut WorkInstructions| i.steps[0].action = " ".into(),
        |i: &mut WorkInstructions| i.steps[0].expected_result.clear(),
        |i: &mut WorkInstructions| i.in_scope.clear(),
        |i: &mut WorkInstructions| i.out_of_scope.clear(),
        |i: &mut WorkInstructions| i.verification.clear(),
        |i: &mut WorkInstructions| i.in_scope[0].clear(),
        |i: &mut WorkInstructions| i.out_of_scope[0] = " ".into(),
        |i: &mut WorkInstructions| i.verification[0] = "\n".into(),
    ] {
        let mut plan = fixture();
        let work = plan.find_work_by_key_mut("TEST-A").expect("task");
        let id = work.id;
        let mut value = instructions();
        edit(&mut value);
        work.contract.instructions = Some(value);
        let error = plan
            .validate()
            .expect_err("incomplete contract")
            .to_string();
        assert!(error.contains(&id.to_string()), "{error}");
    }
}

#[test]
fn aggregate_work_cannot_carry_execution_instructions() {
    for kind in [WorkKind::Milestone, WorkKind::WorkPackage] {
        let mut plan = fixture();
        let work = plan.find_work_by_key_mut("TEST-M1").expect("milestone");
        work.kind = kind;
        work.contract.instructions = Some(instructions());
        assert!(validate(work).is_err());
    }
}
