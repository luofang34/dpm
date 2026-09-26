use super::*;
use crate::Plan;

#[test]
fn resource_identity_survives_renames_and_contract_references_are_validated() {
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let work = plan.find_work_by_key("TEST-A").expect("work").id;
    let id = ResourceId::new();
    let requirement = ResourceRequirement {
        resource: id,
        access: ResourceAccess::Write,
    };
    plan.work_items
        .get_mut(&work)
        .expect("work")
        .resources
        .push(requirement.clone());
    assert!(plan.validate().is_err());
    plan.resources.insert(
        id,
        Resource {
            id,
            key: Key::new("REPO"),
            label: "Repository".into(),
            kind: ResourceKind::GitRepository { remotes: vec![] },
        },
    );
    plan.validate().expect("valid");
    plan.resources.get_mut(&id).expect("resource").key = Key::new("RENAMED");
    plan.validate().expect("rename");
    assert!(
        plan.work_items[&work]
            .resources
            .iter()
            .any(|r| r.resource == id)
    );
    plan.work_items
        .get_mut(&work)
        .expect("work")
        .resources
        .push(requirement);
    assert!(plan.validate().is_err());
    plan.work_items
        .get_mut(&work)
        .expect("work")
        .resources
        .clear();
    plan.resources.clear();
    for work in plan.work_items.values_mut() {
        work.resources.clear();
    }
    plan.validate().expect("non-code task with no resources");
    plan.format_version = 1;
    assert!(plan.validate().is_err());
}
