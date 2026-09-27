use super::fixture;
use crate::view::View;

#[test]
fn outline_orders_numbered_keys_by_value() {
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
            parent: None,
            owner: None,
            status: dpm_model::WorkStatus::Planned,
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
    let expected: Vec<_> = (1..=12).map(|n| format!("N-{n}")).collect();
    assert_eq!(order, expected);
}
