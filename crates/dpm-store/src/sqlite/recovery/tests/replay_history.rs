use super::*;
use chrono::{DateTime, TimeZone};
use dpm_engine::Operation;
use dpm_model::{
    Artifact, ArtifactId, ArtifactKind, DependencyId, DependencyPolicy, ExternalIdentity,
    ExternalLinkRole, ExternalObjectKind, ExternalProvider, ExternalReferenceId, Key, StartBasis,
    WorkItemId, WorkStatus,
};

fn t(hours: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0)
        .single()
        .expect("fixed time")
        + chrono::TimeDelta::hours(hours)
}

/// A live store whose every write is followed by a full verification, replay included.
struct Recorder {
    path: PathBuf,
    store: SqliteStore,
    hour: i64,
}

impl Recorder {
    fn new(dir: &Path) -> Self {
        Self::with_plan(dir, &fixture())
    }

    fn with_plan(dir: &Path, plan: &Plan) -> Self {
        let path = dir.join("replayed.sqlite");
        let mut store = SqliteStore::open_blocking(&path).expect("store");
        store.initialize_blocking(plan).expect("initialize");
        Self {
            path,
            store,
            hour: 0,
        }
    }

    fn plan(&self) -> Plan {
        self.store.load_blocking().expect("load").expect("plan")
    }

    fn id(&self, key: &str) -> WorkItemId {
        self.plan().find_work_by_key(key).expect(key).id
    }

    fn run(&mut self, actor: ActorId, command: Command) {
        let label = format!("{command:?}");
        let mut plan = self.plan();
        self.hour += 1;
        let operation = apply_command(
            &mut plan,
            actor,
            command,
            t(self.hour),
            dpm_model::OperationId::new(),
        )
        .unwrap_or_else(|error| panic!("{label}: {error}"));
        self.persist(&plan, &operation, &label);
    }

    fn change(&mut self, edit: impl FnOnce(&mut Plan)) {
        let mut plan = self.plan();
        let mut proposed = plan.clone();
        edit(&mut proposed);
        self.hour += 1;
        let operation = dpm_engine::apply_plan_change(
            &mut plan,
            lead(),
            &proposed,
            "reviewed scope",
            t(self.hour),
            dpm_model::OperationId::new(),
        )
        .expect("plan change");
        self.persist(&plan, &operation, "ApplyChange");
    }

    fn persist(&mut self, plan: &Plan, operation: &Operation, label: &str) {
        self.store
            .persist_blocking(plan, operation)
            .unwrap_or_else(|error| panic!("{label}: {error}"));
        let report = verify_store_blocking(&self.path)
            .unwrap_or_else(|error| panic!("replay after {label}: {error}"));
        assert_eq!(report.revision, plan.revision, "{label}");
        assert_eq!(report.genesis_revision, 0);
    }
}

fn lead() -> ActorId {
    ActorId::human("lead")
}
fn reviewer() -> ActorId {
    ActorId::human("reviewer")
}
fn author() -> ActorId {
    ActorId::agent("author")
}
fn successor() -> ActorId {
    ActorId::agent("builder")
}

fn identity() -> ExternalIdentity {
    ExternalIdentity {
        provider: ExternalProvider::Forgejo,
        instance: "code.example.org".into(),
        namespace: Some("ops/dpm".into()),
        kind: ExternalObjectKind::Issue,
        external_id: "7".into(),
    }
}

/// Make A -> B provisional and A -> D soft, and propose one new task, as one reviewed change.
fn prepare(recorder: &mut Recorder) -> (DependencyId, DependencyId, WorkItemId) {
    let (a, b, d) = (
        recorder.id("TEST-A"),
        recorder.id("TEST-B"),
        recorder.id("TEST-D"),
    );
    let added = WorkItemId::new();
    recorder.change(|plan| {
        for edge in &mut plan.dependencies {
            if (edge.predecessor, edge.successor) == (a, b) {
                edge.start_basis = StartBasis::Provisional;
            }
            if (edge.predecessor, edge.successor) == (a, d) {
                edge.policy = DependencyPolicy::Soft;
            }
        }
        let mut new = plan.work_items[&a].clone();
        new.id = added;
        new.key = Key::new("TEST-NEW");
        new.execution.status = WorkStatus::Proposed;
        plan.work_items.insert(added, new);
    });
    let plan = recorder.plan();
    let edge = |to| {
        plan.dependencies
            .iter()
            .find(|e| (e.predecessor, e.successor) == (a, to))
            .expect("edge")
            .id
    };
    (edge(b), edge(d), added)
}

fn evidence() -> Artifact {
    Artifact {
        id: ArtifactId::new(),
        kind: ArtifactKind::TestResult,
        uri: "file:///results/a.txt".into(),
        label: "A acceptance run".into(),
        metadata: std::collections::BTreeMap::new(),
        created_by: ActorId::agent("finisher"),
        created_at: t(0),
    }
}

type Step = (ActorId, Command);

fn finisher() -> ActorId {
    ActorId::agent("finisher")
}

fn text(value: &str) -> String {
    value.to_string()
}

/// Scope, gate and tracking commands, then a released claim and a handoff.
fn context_steps(a: WorkItemId, added: WorkItemId, gate: dpm_model::DecisionId) -> Vec<Step> {
    let reference = ExternalReferenceId::new();
    let link = dpm_engine::ExternalLinkRequest {
        work: a,
        reference,
        identity: identity(),
        label: text("Tracking issue"),
        url: None,
        role: ExternalLinkRole::Tracks,
        observed: None,
    };
    let (work, outcome) = (a, text("proceed"));
    vec![
        (lead(), Command::RatifyContract { work: added }),
        (
            lead(),
            Command::Decide {
                decision: gate,
                outcome,
            },
        ),
        (lead(), Command::LinkExternal(link)),
        (lead(), Command::UnlinkExternal { work, reference }),
        (author(), Command::Claim { work }),
        (
            author(),
            Command::Release {
                work,
                reason: text("claimed by mistake"),
            },
        ),
        (author(), Command::Claim { work }),
        (
            lead(),
            Command::Handoff {
                work,
                from: author(),
                to: finisher(),
                reason: text("reassigned"),
            },
        ),
    ]
}

/// Execution of A, a provisional start of B on A's first attempt, and A's rejection.
fn execution_steps(a: WorkItemId, b: WorkItemId) -> Vec<Step> {
    let work = a;
    vec![
        (finisher(), Command::Start { work }),
        (
            finisher(),
            Command::ReportProgress {
                work,
                percent: 50,
                note: Some(text("half")),
            },
        ),
        (
            finisher(),
            Command::Block {
                work,
                reason: text("waiting for a fixture"),
            },
        ),
        (finisher(), Command::Unblock { work }),
        (
            finisher(),
            Command::AttachArtifact {
                work,
                artifact: evidence(),
            },
        ),
        (finisher(), Command::Submit { work, note: None }),
        (successor(), Command::Claim { work: b }),
        (successor(), Command::Start { work: b }),
        (
            reviewer(),
            Command::Reject {
                work,
                reason: text("acceptance check fails"),
            },
        ),
    ]
}

/// Resubmission, revalidation of B's basis, a waiver and its restoration, and verification.
fn review_steps(
    a: WorkItemId,
    b: WorkItemId,
    provisional: DependencyId,
    soft: DependencyId,
) -> Vec<Step> {
    let revalidate = Command::RevalidateBasis {
        work: b,
        dependency: provisional,
        attempt: 2,
        reason: text("B still matches A"),
    };
    let dependency = soft;
    vec![
        (
            finisher(),
            Command::Submit {
                work: a,
                note: None,
            },
        ),
        (reviewer(), revalidate),
        (
            reviewer(),
            Command::WaiveDependency {
                dependency,
                reason: text("prototype unblocks D"),
            },
        ),
        (
            reviewer(),
            Command::RestoreDependency {
                dependency,
                reason: text("prototype withdrawn"),
            },
        ),
        (
            reviewer(),
            Command::Verify {
                work: a,
                note: None,
            },
        ),
    ]
}

/// Exhaustive without a wildcard, so a new command kind cannot be added without being replayed here.
fn kind(command: &Command) -> &'static str {
    match command {
        Command::ApplyChange { .. } => "ApplyChange",
        Command::RatifyContract { .. } => "RatifyContract",
        Command::Reject { .. } => "Reject",
        Command::Claim { .. } => "Claim",
        Command::Release { .. } => "Release",
        Command::Handoff { .. } => "Handoff",
        Command::Start { .. } => "Start",
        Command::Block { .. } => "Block",
        Command::Unblock { .. } => "Unblock",
        Command::ReportProgress { .. } => "ReportProgress",
        Command::Submit { .. } => "Submit",
        Command::Verify { .. } => "Verify",
        Command::AttachArtifact { .. } => "AttachArtifact",
        Command::WaiveDependency { .. } => "WaiveDependency",
        Command::RestoreDependency { .. } => "RestoreDependency",
        Command::RevalidateBasis { .. } => "RevalidateBasis",
        Command::Decide { .. } => "Decide",
        Command::LinkExternal(_) => "LinkExternal",
        Command::UnlinkExternal { .. } => "UnlinkExternal",
    }
}

const COMMAND_KINDS: usize = 19;

#[test]
fn replay_reproduces_the_snapshot_after_every_command_kind() {
    let dir = tempfile::tempdir().expect("directory");
    let mut r = Recorder::new(dir.path());
    let (provisional, soft, added) = prepare(&mut r);
    let (a, b) = (r.id("TEST-A"), r.id("TEST-B"));
    let gate = r.plan().find_decision_by_key("TEST-GATE").expect("gate").id;
    let steps = [
        context_steps(a, added, gate),
        execution_steps(a, b),
        review_steps(a, b, provisional, soft),
    ]
    .concat();
    let count = steps.len();
    let mut kinds: std::collections::BTreeSet<_> = steps.iter().map(|(_, c)| kind(c)).collect();
    kinds.insert("ApplyChange");
    assert_eq!(kinds.len(), COMMAND_KINDS, "{kinds:?}");
    for (actor, command) in steps {
        r.run(actor, command);
    }
    let recorded = r.store.operation_count_blocking().expect("count");
    assert_eq!(recorded, u64::try_from(count + 1).expect("count"));
}

#[test]
fn a_rewritten_snapshot_operation_or_genesis_is_a_replay_finding() {
    let dir = tempfile::tempdir().expect("directory");
    let source = dir.path().join("source.sqlite");
    // Claim, then block: the snapshot keeps the recorded block reason.
    store_with_history(&source, 2)
        .backup_blocking(&dir.path().join("snapshot.sqlite"))
        .expect("backup");
    let copy = |name: &str| {
        let path = dir.path().join(name);
        std::fs::copy(dir.path().join("snapshot.sqlite"), &path).expect("copy");
        path
    };
    let (snapshot, operation, genesis) = (
        dir.path().join("snapshot.sqlite"),
        copy("operation.sqlite"),
        copy("genesis.sqlite"),
    );
    let edit = |path: &Path, sql: &str| {
        Connection::open(path)
            .expect("raw connection")
            .execute_batch(sql)
            .expect("tamper");
    };
    let retitle = "json_set({}, '$.workspace.name', 'Rewritten')";
    edit(
        &snapshot,
        &format!(
            "UPDATE plan_state SET snapshot_json = {}",
            retitle.replace("{}", "snapshot_json")
        ),
    );
    edit(
        &operation,
        "UPDATE operations SET command_json = json_set(command_json, '$.Block.reason', 'forged') \
         WHERE sequence = 2",
    );
    edit(
        &genesis,
        &format!(
            "UPDATE genesis SET plan_json = {}",
            retitle.replace("{}", "plan_json")
        ),
    );
    for path in [&snapshot, &operation, &genesis] {
        let before = std::fs::read(path).expect("bytes");
        let error = verify_store_blocking(path).expect_err("diverged");
        assert!(
            matches!(&error, StoreError::ReplayDiverged { differing, .. } if !differing.is_empty()),
            "{path:?}: {error:?}"
        );
        assert!(error.is_corruption());
        assert!(
            std::fs::read(path).expect("bytes") == before,
            "never repaired"
        );
        let target = dir.path().join("restored.sqlite");
        assert!(restore_store_blocking(path, &target).is_err());
        assert!(!target.exists());
    }
}

#[test]
fn an_operation_the_engine_refuses_on_replay_is_named_by_sequence() {
    let dir = tempfile::tempdir().expect("directory");
    let backup = dir.path().join("refused.sqlite");
    store_with_history(&dir.path().join("live.sqlite"), 3)
        .backup_blocking(&backup)
        .expect("backup");
    Connection::open(&backup)
        .expect("raw connection")
        .execute_batch(
            "UPDATE operations SET actor_json = '{\"kind\":\"Agent\",\"name\":\"intruder\"}' \
             WHERE sequence = 2",
        )
        .expect("tamper");
    let error = verify_store_blocking(&backup).expect_err("refused");
    assert!(
        matches!(error, StoreError::ReplayRefused { sequence: 2, .. }),
        "{error:?}"
    );
    assert!(error.is_corruption());
}

#[test]
fn a_missing_genesis_fails_verification() {
    let dir = tempfile::tempdir().expect("directory");
    let backup = dir.path().join("genesis.sqlite");
    store_with_history(&dir.path().join("live.sqlite"), 1)
        .backup_blocking(&backup)
        .expect("backup");
    Connection::open(&backup)
        .expect("raw connection")
        .execute_batch("DELETE FROM genesis")
        .expect("tamper");
    assert!(matches!(
        verify_store_blocking(&backup),
        Err(StoreError::MissingGenesis { .. })
    ));
}

mod precision;
