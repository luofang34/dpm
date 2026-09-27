use super::fixture;
use crate::view::View;

#[test]
fn outline_uses_explicit_order_even_when_keys_disagree() {
    let mut plan = fixture();
    let template = plan
        .work_items
        .values()
        .find(|w| w.is_executable())
        .cloned()
        .expect("task");
    for n in 1..=12 {
        let work = dpm_model::WorkItem {
            id: dpm_model::WorkItemId::new(),
            key: dpm_model::Key::new(format!("N-{n}")),
            order: dpm_model::SiblingOrder(vec![100 + (12 - n) * 100]),
            parent: None,
            execution: dpm_model::ExecutionRecord {
                owner: None,
                status: dpm_model::WorkStatus::Planned,
                ..(template.clone()).execution
            },
            ..template.clone()
        };
        plan.work_items.insert(work.id, work);
    }
    let view = View::new(&plan, chrono::Utc::now()).expect("view");
    let order: Vec<_> = view
        .work
        .iter()
        .map(|w| w.key.0.as_str())
        .filter(|k| k.starts_with("N-"))
        .collect();
    let expected: Vec<_> = (1..=12).rev().map(|n| format!("N-{n}")).collect();
    assert_eq!(order, expected);
}
