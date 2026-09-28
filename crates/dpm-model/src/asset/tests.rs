use super::*;
use crate::Plan;

#[test]
fn asset_identity_survives_renames_and_contract_references_are_validated() {
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let work = plan.find_work_by_key("TEST-A").expect("work").id;
    let id = AssetId::new();
    let requirement = AssetRequirement {
        asset: id,
        access: AssetAccess::Write,
    };
    plan.work_items
        .get_mut(&work)
        .expect("work")
        .contract
        .assets
        .push(requirement.clone());
    assert!(plan.validate().is_err());
    plan.assets.insert(
        id,
        WorkspaceAsset {
            id,
            key: Key::new("REPO"),
            label: "Repository".into(),
            kind: AssetKind::GitRepository { remotes: vec![] },
        },
    );
    plan.validate().expect("valid");
    plan.assets.get_mut(&id).expect("asset").key = Key::new("RENAMED");
    plan.validate().expect("rename");
    assert!(
        plan.work_items[&work]
            .contract
            .assets
            .iter()
            .any(|r| r.asset == id)
    );
    plan.work_items
        .get_mut(&work)
        .expect("work")
        .contract
        .assets
        .push(requirement);
    assert!(plan.validate().is_err());
    plan.work_items
        .get_mut(&work)
        .expect("work")
        .contract
        .assets
        .clear();
    plan.assets.clear();
    for work in plan.work_items.values_mut() {
        work.contract.assets.clear();
    }
    plan.validate().expect("non-code task with no assets");
    plan.format_version = 1;
    assert!(plan.validate().is_err());
}

#[test]
fn credential_free_remotes_are_portable_without_losing_ssh_login_names() {
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let id = *plan.assets.keys().next().expect("repository");
    for (remote, accepted) in [
        ("https://example.invalid/org/repo.git", true),
        ("git@example.invalid:org/repo.git", true),
        ("ssh://git@example.invalid/org/repo.git", true),
        ("ssh://alice@example.invalid:22/org/repo.git", true),
        ("alice@example.invalid:org/repo.git", true),
        ("https://example.invalid/org/repo.git?branch=main", true),
        ("https://EXAMPLE_TOKEN@example.invalid/org/repo.git", false),
        (
            "https://example.invalid/org/repo.git?access_token=EXAMPLE",
            false,
        ),
        ("https://user%3AEXAMPLE@example.invalid/org/repo.git", false),
        ("https://example.invalid/org/repo.git#token=EXAMPLE", false),
        ("alice@example.invalid:org/repo.git?%74oken=EXAMPLE", false),
        ("https://user:token@example.invalid/org/repo.git", false),
        ("ssh://user:secret@example.invalid:22/repo.git", false),
    ] {
        plan.assets.get_mut(&id).expect("asset").kind = AssetKind::GitRepository {
            remotes: vec![remote.into()],
        };
        assert_eq!(plan.validate().is_ok(), accepted, "{remote}");
    }
}

#[test]
fn asset_variant_fields_are_never_silently_dropped() {
    let unknown = serde_json::json!({"GitRepository": {
        "remotes": [], "unrecognized_policy": "must-not-disappear"
    }});
    assert!(serde_json::from_value::<AssetKind>(unknown).is_err());
}
