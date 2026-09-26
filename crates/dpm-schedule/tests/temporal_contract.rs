//! Temporal-constraint contract for the deterministic CPM projection, exercised via the public API.
#![allow(clippy::expect_used, clippy::panic)]

#[path = "temporal_contract/examples.rs"]
mod examples;
#[path = "temporal_contract/properties.rs"]
mod properties;

use dpm_model::{
    AcceptanceCriterion, Dependency, DependencyKind, Key, Plan, Priority, Project, ProjectId,
    ThreePointEstimate, WorkItem, WorkItemId, WorkKind, WorkStatus,
};
use std::collections::{BTreeMap, BTreeSet};

/// Tolerance the contract promises for derived hours.
pub(crate) const TOLERANCE: f64 = 1e-8;

pub(crate) const KINDS: [DependencyKind; 4] = [
    DependencyKind::FinishStart,
    DependencyKind::StartStart,
    DependencyKind::FinishFinish,
    DependencyKind::StartFinish,
];

/// Identities are derived from a counter so a failing case reproduces with the same graph order.
pub(crate) fn stable_id(n: u64) -> WorkItemId {
    serde_json::from_str(&format!("\"00000000-0000-4000-8000-{n:012x}\"")).expect("stable id")
}

/// Small plan builder whose identities are reproducible across runs.
pub(crate) struct Network {
    pub plan: Plan,
    project: ProjectId,
    durations: BTreeMap<WorkItemId, f64>,
}

impl Network {
    pub fn new() -> Self {
        let mut plan = Plan::empty("temporal-contract");
        let project = Project {
            id: ProjectId::new(),
            key: Key::new("P"),
            parent: None,
            title: "Temporal contract".into(),
            objective: "Exercise temporal bounds".into(),
        };
        let id = project.id;
        plan.projects.insert(id, project);
        Self {
            plan,
            project: id,
            durations: BTreeMap::new(),
        }
    }

    fn insert(&mut self, id: WorkItemId, kind: WorkKind, hours: f64) -> WorkItemId {
        let key = format!("W{}", self.plan.work_items.len());
        let estimate = (kind == WorkKind::Task).then_some(ThreePointEstimate {
            optimistic_hours: hours,
            likely_hours: hours,
            pessimistic_hours: hours,
        });
        let work = WorkItem {
            id,
            key: Key::new(key.clone()),
            project: self.project,
            parent: None,
            kind,
            title: key.clone(),
            objective: key,
            acceptance: vec![AcceptanceCriterion {
                text: "observable result exists".into(),
            }],
            instructions: None,
            status: WorkStatus::Planned,
            reported_progress_percent: 0,
            priority: Priority::P2,
            estimate,
            capabilities: BTreeSet::new(),
            requirement_ids: BTreeSet::new(),
            artifact_ids: BTreeSet::new(),
            owner: None,
            block_reason: None,
            events: Default::default(),
            last_rejection: None,
            resources: Vec::new(),
        };
        self.plan.work_items.insert(id, work);
        self.durations.insert(id, hours);
        id
    }

    /// Add a task with a point estimate, so its expected duration equals `hours`.
    pub fn task(&mut self, id: WorkItemId, hours: f64) -> WorkItemId {
        self.insert(id, WorkKind::Task, hours)
    }

    /// Add a zero-duration milestone.
    pub fn milestone(&mut self, id: WorkItemId) -> WorkItemId {
        self.insert(id, WorkKind::Milestone, 0.0)
    }

    pub fn link(&mut self, from: WorkItemId, to: WorkItemId, kind: DependencyKind, lag: f64) {
        self.plan
            .dependencies
            .push(Dependency::new(from, to, kind, lag));
    }

    pub fn durations(&self) -> &BTreeMap<WorkItemId, f64> {
        &self.durations
    }
}
