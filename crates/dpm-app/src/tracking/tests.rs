use super::*;
use crate::CommandRequest;
use dpm_model::{ActorId, ExternalObjectKind, ExternalProvider};
use tempfile::TempDir;

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

fn github(namespace: &str) -> ExternalIdentity {
    ExternalIdentity {
        provider: ExternalProvider::GitHub,
        instance: "GitHub.com".into(),
        namespace: Some(namespace.into()),
        kind: ExternalObjectKind::Issue,
        external_id: "#42".into(),
    }
}

fn input(identity: ExternalIdentity, role: ExternalLinkRole) -> ExternalLinkInput {
    ExternalLinkInput {
        identity,
        label: None,
        url: None,
        role,
        observed: None,
    }
}

fn execute(app: &mut Application, base_revision: u64, command: Command) -> Result<u64, AppError> {
    app.execute_blocking(CommandRequest {
        actor: ActorId::agent("linker"),
        base_revision,
        base_lineage: None,
        operation_id: None,
        command,
    })
    .map(|operation| operation.operation.resulting_revision)
}

#[test]
fn equivalent_spellings_resolve_to_one_reference_and_persist_across_restart() {
    let temp = TempDir::new().expect("temp");
    let path = temp.path().join("state.sqlite");
    let mut app = Application::initialize_blocking(&path, &fixture()).expect("app");
    let link = app
        .external_link_command_blocking(
            "TEST-A",
            input(github("Ops/DPM"), ExternalLinkRole::Tracks),
        )
        .expect("command");
    execute(&mut app, 0, link).expect("link");
    let again = app
        .external_link_command_blocking(
            "TEST-B",
            input(github("ops/dpm"), ExternalLinkRole::Tracks),
        )
        .expect("command");
    let error = execute(&mut app, 1, again).expect_err("second owner");
    assert_eq!(error.code(), "tracking_conflict");
    assert!(error.to_string().contains("TEST-A"), "{error}");
    drop(app);
    let reopened = Application::open_blocking(&path).expect("reopen");
    let plan = reopened.plan_blocking().expect("plan");
    let [reference] = plan.external_references.values().collect::<Vec<_>>()[..] else {
        panic!("one reference");
    };
    assert_eq!(
        reference.identity.to_string(),
        "GitHub:github.com/ops/dpm:issue:42"
    );
    assert_eq!(reference.label, reference.identity.to_string());
}

#[test]
fn stale_and_missing_requests_are_rejected_without_persisting_anything() {
    let mut app = Application::in_memory_blocking(&fixture()).expect("app");
    let link = app
        .external_link_command_blocking(
            "TEST-A",
            input(github("ops/dpm"), ExternalLinkRole::Tracks),
        )
        .expect("command");
    execute(&mut app, 0, link.clone()).expect("link");
    let snapshot = serde_json::to_string(&app.plan_blocking().expect("plan")).expect("json");
    let unchanged = |app: &Application| {
        assert_eq!(
            serde_json::to_string(&app.plan_blocking().expect("plan")).expect("json"),
            snapshot
        );
    };
    assert_eq!(
        execute(&mut app, 0, link).expect_err("stale").code(),
        "revision_conflict"
    );
    unchanged(&app);
    let unlink = app
        .external_unlink_command_blocking("TEST-A", &github("ops/dpm"))
        .expect("unlink");
    assert_eq!(
        execute(&mut app, 0, unlink).expect_err("stale").code(),
        "revision_conflict"
    );
    unchanged(&app);
    let missing = app
        .external_unlink_command_blocking("TEST-A", &github("ops/other"))
        .expect_err("unknown identity");
    assert_eq!(missing.code(), "not_found");
    let unrelated = app
        .external_unlink_command_blocking("TEST-B", &github("ops/dpm"))
        .expect("command");
    assert_eq!(
        execute(&mut app, 1, unrelated).expect_err("no link").code(),
        "not_found"
    );
    let error = app
        .external_link_command_blocking("NOPE", input(github("ops/dpm"), ExternalLinkRole::Relates))
        .expect_err("unknown work");
    assert_eq!(error.code(), "not_found");
    let mut secret = input(github("ops/secret"), ExternalLinkRole::Relates);
    secret.url = Some("https://token@github.com/ops/secret/issues/42".into());
    let command = app
        .external_link_command_blocking("TEST-B", secret)
        .expect("command");
    assert_eq!(
        execute(&mut app, 1, command)
            .expect_err("credential")
            .code(),
        "invalid_command"
    );
    unchanged(&app);
}

#[test]
fn previews_refuse_link_preparation() {
    let app = Application::preview(fixture()).expect("preview");
    let error = app
        .external_link_command_blocking(
            "TEST-A",
            input(github("ops/dpm"), ExternalLinkRole::Tracks),
        )
        .expect_err("read only");
    assert_eq!(error.code(), "read_only_project");
}
