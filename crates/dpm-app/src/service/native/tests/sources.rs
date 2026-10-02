//! A connection that stays open while its locator is repointed must refuse to read or write the
//! source it opened, through the typed boundary and the JSON one alike.

use super::*;
use crate::{ProjectLocation, open_workspace_blocking};
use dpm_model::{LineageId, WorkspaceId};

fn locate_blocking(root: &Path, workspace: WorkspaceId, source: &str) {
    std::fs::create_dir_all(root.join(".dpm")).expect("locator directory");
    std::fs::write(
        root.join(".dpm/project.toml"),
        format!("version = 3\nworkspace = \"{workspace}\"\n{source}\n"),
    )
    .expect("locator");
}

fn open_located_blocking(root: &Path) -> Application {
    open_workspace_blocking(root, Some(root), None).expect("open the located source")
}

fn claim_request_blocking(app: &Application, base_revision: u64) -> CommandRequest {
    CommandRequest {
        actor: worker(),
        base_revision,
        base_lineage: None,
        operation_id: None,
        command: Command::Claim {
            work: task_blocking(app),
        },
    }
}

/// The reason a call was refused as a changed source, from a response in either form.
fn changed(response: &NativeResponse) -> SourceChange {
    let error = response.error.as_ref().expect("a refusal");
    assert_eq!(error.code, "source_changed", "{error:?}");
    serde_json::from_value(error.details.as_ref().expect("details")["reason"].clone())
        .expect("a reason")
}

/// Every public call that reads or writes, as a typed request.
fn calls_blocking(app: &Application, attachment: Attachment) -> Vec<(&'static str, NativeCall)> {
    vec![
        (
            "attach",
            NativeCall::Attach {
                expect_workspace: None,
            },
        ),
        (
            "query",
            NativeCall::Query {
                query: Query::Revision,
                attached: Some(attachment),
            },
        ),
        (
            "changes",
            NativeCall::Changes {
                since: Cursors::default(),
                limit: None,
                attached: Some(attachment),
            },
        ),
        (
            "command",
            NativeCall::Command {
                request: claim_request_blocking(app, 1),
                attached: Some(attachment),
            },
        ),
    ]
}

fn typed(call: NativeCall) -> NativeRequest {
    NativeRequest {
        protocol: NATIVE_PROTOCOL_VERSION,
        id: "typed".into(),
        call,
    }
}

fn history_len_blocking(path: &Path) -> usize {
    Application::open_blocking(path)
        .expect("open")
        .history_blocking(0, 1000)
        .expect("history")
        .entries
        .len()
}

#[test]
fn a_locator_switched_to_a_restored_store_is_refused_for_every_call_and_writes_neither_store() {
    let directory = tempfile::tempdir().expect("directory");
    let root = directory.path();
    let plan = fixture_plan();
    let workspace = plan.workspace.id;
    let a = root.join(".dpm/a.sqlite");
    let b = root.join(".dpm/b.sqlite");
    let mut app = Application::initialize_blocking(&a, &plan).expect("store A");
    let task = task_blocking(&app);
    commit_blocking(&mut app, &worker(), Command::Claim { work: task });
    let archive = root.join("archive.sqlite");
    app.backup_blocking(&archive).expect("backup");
    crate::restore_store_blocking(&archive, &b).expect("restore as store B");
    drop(app);

    locate_blocking(root, workspace, r#"database = "a.sqlite""#);
    let mut app = open_located_blocking(root);
    let attached = attach_blocking(&mut app);
    assert_eq!(attached.source, SourceKind::Live);
    // Through the located source, a native command commits to A.
    let committed = app.native_blocking(typed(NativeCall::Command {
        request: CommandRequest {
            command: Command::Start {
                work: task,
                occurred_at: None,
            },
            ..claim_request_blocking(&app, 1)
        },
        attached: Some(attached.attachment),
    }));
    assert!(committed.ok, "{:?}", committed.error);
    assert_eq!((history_len_blocking(&a), history_len_blocking(&b)), (2, 1));

    // The locator is repointed at B while this connection stays open on A.
    locate_blocking(root, workspace, r#"database = "b.sqlite""#);
    for (name, call) in calls_blocking(&app, attached.attachment) {
        let response = app.native_blocking(typed(call));
        assert!(!response.ok, "{name}");
        assert_eq!(changed(&response), SourceChange::Locator, "{name}");
    }
    // The same through the JSON boundary a helper or foreign host uses.
    let request = claim_request_blocking(&app, 2);
    let response = exchange_blocking(&mut app, json!({"type": "command", "request": request}));
    assert_eq!(refusal(&response)["code"], "source_changed");
    assert_eq!(refusal(&response)["details"]["reason"], "locator");
    assert_eq!(refusal(&response)["details"]["attached"]["kind"], "live");
    // Neither store took a write: the refused command committed nowhere.
    assert_eq!((history_len_blocking(&a), history_len_blocking(&b)), (2, 1));

    // Pointing the locator back makes the same connection valid again.
    locate_blocking(root, workspace, r#"database = "a.sqlite""#);
    assert_eq!(attach_blocking(&mut app).attachment, attached.attachment);
    let location = ProjectLocation::at_blocking(root).expect("locator");
    assert_eq!(location.workspace, workspace);
}

fn preview_blocking(root: &Path, name: &str, plan: &Plan) {
    std::fs::create_dir_all(root.join(".dpm")).expect("directory");
    std::fs::write(
        root.join(".dpm").join(name),
        serde_json::to_string(plan).expect("plan"),
    )
    .expect("preview plan");
}

#[test]
fn one_preview_swapped_for_another_is_not_mistaken_for_it_because_both_lack_a_lineage() {
    let directory = tempfile::tempdir().expect("directory");
    let root = directory.path();
    let first = fixture_plan();
    let mut second = fixture_plan();
    second.workspace.id = WorkspaceId::new();
    preview_blocking(root, "a.json", &first);
    preview_blocking(root, "b.json", &second);
    locate_blocking(root, first.workspace.id, r#"preview = "a.json""#);
    let mut app = open_located_blocking(root);
    let attached = attach_blocking(&mut app);
    assert_eq!(attached.source, SourceKind::Preview);
    assert_eq!(attached.attachment.lineage_id, None);
    assert_eq!(attached.attachment.workspace_id, first.workspace.id);

    // Another preview: neither has a lineage, so only the file and workspace tell them apart.
    locate_blocking(root, second.workspace.id, r#"preview = "b.json""#);
    for (name, call) in calls_blocking(&app, attached.attachment) {
        let response = app.native_blocking(typed(call));
        assert_eq!(changed(&response), SourceChange::Locator, "{name}");
    }

    // The same file rewritten in place with another workspace is also another source.
    locate_blocking(root, first.workspace.id, r#"preview = "a.json""#);
    assert_eq!(attach_blocking(&mut app).attachment, attached.attachment);
    preview_blocking(root, "a.json", &second);
    let response = app.native_blocking(typed(NativeCall::Attach {
        expect_workspace: None,
    }));
    assert_eq!(changed(&response), SourceChange::Workspace);
    let body = response.error.expect("refusal").details.expect("details");
    assert_eq!(body["attached"]["workspace_id"], json!(first.workspace.id));
    assert_eq!(body["current"]["workspace_id"], json!(second.workspace.id));

    // The same file edited without changing workspace is a refresh, not another source.
    let mut edited = fixture_plan();
    edited.revision = 5;
    preview_blocking(root, "a.json", &edited);
    assert!(
        app.native_blocking(typed(NativeCall::Attach {
            expect_workspace: None
        }))
        .ok
    );
}

#[test]
fn every_difference_between_what_is_held_and_what_is_selected_has_its_own_reason() {
    let workspace = WorkspaceId::new();
    let lineage = LineageId::new();
    let held = SourceIdentity {
        kind: SourceKind::Live,
        workspace_id: Some(workspace),
        lineage_id: Some(lineage),
    };
    // A store is identified by lineage and leaves the workspace unread.
    let same = SourceIdentity {
        workspace_id: None,
        ..held
    };
    assert_eq!(source::difference(true, &held, &same), None);
    assert_eq!(
        source::difference(false, &held, &same),
        Some(SourceChange::Locator)
    );
    let archived = SourceIdentity {
        kind: SourceKind::Archive,
        ..same
    };
    assert_eq!(
        source::difference(true, &held, &archived),
        Some(SourceChange::Kind)
    );
    let restored = SourceIdentity {
        lineage_id: Some(LineageId::new()),
        ..same
    };
    assert_eq!(
        source::difference(true, &held, &restored),
        Some(SourceChange::Lineage)
    );
    let other = SourceIdentity {
        workspace_id: Some(WorkspaceId::new()),
        ..held
    };
    assert_eq!(
        source::difference(true, &held, &other),
        Some(SourceChange::Workspace)
    );
    let preview = SourceIdentity {
        kind: SourceKind::Preview,
        workspace_id: Some(workspace),
        lineage_id: None,
    };
    assert_eq!(
        source::difference(true, &held, &preview),
        Some(SourceChange::Kind)
    );
}

#[test]
fn an_unsupported_protocol_on_the_typed_boundary_refuses_every_call_and_changes_nothing() {
    let directory = tempfile::tempdir().expect("directory");
    let mut app = live_blocking(directory.path());
    let task = task_blocking(&app);
    commit_blocking(&mut app, &worker(), Command::Claim { work: task });
    let before = serde_json::to_value(app.history_blocking(0, 1000).expect("history"))
        .expect("history json");
    let revision = app.revision_blocking().expect("revision");
    let attachment = attach_blocking(&mut app).attachment;

    for protocol in [0, NATIVE_PROTOCOL_VERSION + 1, u32::MAX] {
        for (name, call) in calls_blocking(&app, attachment) {
            let response = app.native_blocking(NativeRequest {
                protocol,
                id: format!("p{protocol}-{name}"),
                call,
            });
            let error = response.error.as_ref().expect("a refusal");
            assert!(!response.ok && response.result.is_none(), "{name}");
            assert_eq!(error.code, "unsupported_protocol", "{name} at {protocol}");
            assert_eq!(
                error.details.as_ref().expect("details")["offered"],
                json!([protocol])
            );
            assert_eq!(
                error.details.as_ref().expect("details")["supported"],
                json!([1])
            );
            assert_eq!(response.id, format!("p{protocol}-{name}"));
        }
    }
    // The mutation was refused before the application saw it: history and revision are exactly
    // as they were.
    assert_eq!(
        serde_json::to_value(app.history_blocking(0, 1000).expect("history")).expect("json"),
        before
    );
    assert_eq!(app.revision_blocking().expect("revision"), revision);

    // `hello` alone is how a version is negotiated, so a newer client may send it.
    let hello = app.native_blocking(NativeRequest {
        protocol: NATIVE_PROTOCOL_VERSION + 1,
        id: "hello".into(),
        call: NativeCall::Hello {
            protocols: vec![NATIVE_PROTOCOL_VERSION + 1, NATIVE_PROTOCOL_VERSION],
        },
    });
    assert!(hello.ok, "{:?}", hello.error);
    assert!(matches!(hello.result, Some(NativeResult::Hello(ref found)) if found.protocol == 1));
}

/// A store at `.dpm/a.sqlite` the locator selects, and a connection opened on it.
fn located_store_blocking(
    root: &Path,
    extra: &str,
) -> (Application, WorkspaceId, std::path::PathBuf) {
    let plan = fixture_plan();
    let workspace = plan.workspace.id;
    let store = root.join(".dpm/a.sqlite");
    drop(Application::initialize_blocking(&store, &plan).expect("store"));
    locate_blocking(root, workspace, &format!("database = \"a.sqlite\"{extra}"));
    (open_located_blocking(root), workspace, store)
}

/// Every public call, refused exactly as a fresh open of the same locator is refused.
fn assert_refused_like_a_fresh_open_blocking(
    app: &mut Application,
    root: &Path,
    attachment: Attachment,
) {
    let fresh = open_workspace_blocking(root, Some(root), None)
        .err()
        .expect("a fresh open refuses this locator")
        .response();
    for (name, call) in calls_blocking(app, attachment) {
        let response = app.native_blocking(typed(call));
        let error = response.error.as_ref().expect("a refusal");
        assert_eq!(error.code, fresh.code, "{name}");
        assert_eq!(error.message, fresh.message, "{name}");
    }
    let request = claim_request_blocking(app, 1);
    let json = exchange_blocking(app, json!({"type": "command", "request": request}));
    assert_eq!(refusal(&json)["code"], json!(fresh.code));
}

#[test]
fn a_locator_expecting_another_workspace_cannot_write_the_open_database() {
    let directory = tempfile::tempdir().expect("directory");
    let root = directory.path();
    let (mut app, workspace, store) = located_store_blocking(root, "");
    let attached = attach_blocking(&mut app);

    // Only the declared workspace changes; the database path is the same file.
    locate_blocking(root, WorkspaceId::new(), r#"database = "a.sqlite""#);
    assert_refused_like_a_fresh_open_blocking(&mut app, root, attached.attachment);
    assert_eq!(
        history_len_blocking(&store),
        0,
        "nothing was written to the old database"
    );

    // Restoring the declaration makes the same connection valid again, history untouched.
    locate_blocking(root, workspace, r#"database = "a.sqlite""#);
    assert_eq!(attach_blocking(&mut app).attachment, attached.attachment);
    assert_eq!(attach_blocking(&mut app).watermark, attached.watermark);
}

#[test]
fn a_locator_asset_is_judged_as_a_fresh_open_judges_it() {
    let directory = tempfile::tempdir().expect("directory");
    let root = directory.path();
    let (mut app, workspace, store) = located_store_blocking(root, "\nasset = \"TEST-REPO\"");
    let attached = attach_blocking(&mut app);

    // An asset the plan does not have is the same refusal a fresh open gives.
    locate_blocking(
        root,
        workspace,
        "database = \"a.sqlite\"\nasset = \"NO-SUCH-ASSET\"",
    );
    assert_refused_like_a_fresh_open_blocking(&mut app, root, attached.attachment);

    // No asset at all is a valid locator, but another context than this connection was opened with:
    // queries would be scoped differently, so the connection must be reopened, never reused.
    locate_blocking(root, workspace, r#"database = "a.sqlite""#);
    open_located_blocking(root);
    for (name, call) in calls_blocking(&app, attached.attachment) {
        let response = app.native_blocking(typed(call));
        assert_eq!(changed(&response), SourceChange::Asset, "{name}");
    }
    assert_eq!(
        history_len_blocking(&store),
        0,
        "nothing was written to the database"
    );

    // The declaration it was opened with is valid again.
    locate_blocking(
        root,
        workspace,
        "database = \"a.sqlite\"\nasset = \"TEST-REPO\"",
    );
    assert_eq!(attach_blocking(&mut app).attachment, attached.attachment);
}
