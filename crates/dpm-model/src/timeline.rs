use crate::{
    Decision, DecisionStatus, Dependency, Endpoint, EventTime, Plan, Release, WorkItem, WorkItemId,
    WorkKind, WorkStatus,
};
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, BTreeSet};

/// Execution events and derived completion times of every work item at one clock reading.
///
/// This is the single evaluator of relation gates: task readiness and lifecycle commands, milestone
/// and package completion, progress and the remaining schedule all read edge releases from here, so
/// they cannot disagree. It is derived from authoritative facts and never persisted.
#[derive(Debug, Clone, PartialEq)]
pub struct Timeline {
    now: DateTime<Utc>,
    complete: BTreeMap<WorkItemId, EventTime>,
}

impl Timeline {
    /// Derive completion from a validated plan and an adapter-supplied clock reading.
    ///
    /// Tasks complete when verified. A milestone is reached when every unwaived incoming edge and
    /// every decision gating it or its containers is released; its time is the latest of those
    /// releases. A work package completes with all its children, at the latest child or gate time.
    #[must_use]
    pub fn at(plan: &Plan, now: DateTime<Utc>) -> Self {
        let mut timeline = Self {
            now,
            complete: plan
                .work_items
                .values()
                .filter(|w| w.is_executable())
                .filter_map(|w| task_event(w, Endpoint::Finish).map(|at| (w.id, at)))
                .collect(),
        };
        loop {
            let reached: Vec<_> = plan
                .work_items
                .values()
                .filter(|w| !timeline.complete.contains_key(&w.id))
                .filter_map(|w| timeline.aggregate_completion(plan, w).map(|at| (w.id, at)))
                .collect();
            if reached.is_empty() {
                return timeline;
            }
            timeline.complete.extend(reached);
        }
    }

    /// Clock reading this timeline was evaluated at.
    #[must_use]
    pub fn now(&self) -> DateTime<Utc> {
        self.now
    }

    /// Completion time of verified tasks, reached milestones and completed packages.
    #[must_use]
    pub fn completed_at(&self, work: WorkItemId) -> Option<EventTime> {
        self.complete.get(&work).copied()
    }

    /// Identities of all completed work.
    #[must_use]
    pub fn completed(&self) -> BTreeSet<WorkItemId> {
        self.complete.keys().copied().collect()
    }

    /// Time of a work item's start or finish event, if it has occurred.
    #[must_use]
    pub fn event(&self, plan: &Plan, work: WorkItemId, endpoint: Endpoint) -> Option<EventTime> {
        let item = plan.work_items.get(&work)?;
        if item.is_executable() {
            task_event(item, endpoint)
        } else {
            self.completed_at(work)
        }
    }

    /// Whether an edge's required predecessor event plus positive lag has elapsed.
    #[must_use]
    pub fn edge(&self, plan: &Plan, edge: &Dependency) -> Release {
        let event = self.event(plan, edge.predecessor, edge.kind.predecessor_endpoint());
        Release::evaluate(event, edge.lag_hours, self.now)
    }

    /// Decisions naming this work or a containing package as blocked, with their release state.
    #[must_use]
    pub fn decisions<'a>(&self, plan: &'a Plan, work: WorkItemId) -> Vec<(&'a Decision, Release)> {
        let Some(scope) = ancestors(plan, work) else {
            return Vec::new();
        };
        plan.decisions
            .values()
            .filter(|d| !d.blocks.is_disjoint(&scope))
            .map(|d| (d, Release::evaluate(decision_event(d), 0.0, self.now)))
            .collect()
    }

    fn aggregate_completion(&self, plan: &Plan, work: &WorkItem) -> Option<EventTime> {
        if work.status != WorkStatus::Planned {
            return None;
        }
        let prerequisites: Vec<Release> = match work.kind {
            WorkKind::Task => return None,
            WorkKind::Milestone => plan
                .enforced_dependencies()
                .filter(|d| d.successor == work.id)
                .map(|d| self.edge(plan, d))
                .collect(),
            WorkKind::WorkPackage => plan
                .work_items
                .values()
                .filter(|w| w.parent == Some(work.id))
                .map(|w| {
                    self.completed_at(w.id)
                        .map_or(Release::AwaitingEvent, |at| Release::Released { at })
                })
                .collect(),
        };
        if prerequisites.is_empty() {
            return None;
        }
        let gates = self.decisions(plan, work.id).into_iter().map(|(_, r)| r);
        prerequisites
            .into_iter()
            .chain(gates)
            .try_fold(None, |latest: Option<EventTime>, release| {
                let at = release.released_at()?;
                Some(Some(latest.map_or(at, |l| l.latest(at))))
            })
            .flatten()
    }
}

/// Derive completed task, milestone and package identities at a clock reading.
#[must_use]
pub fn completion(plan: &Plan, now: DateTime<Utc>) -> BTreeSet<WorkItemId> {
    Timeline::at(plan, now).completed()
}

/// A task's own event: its recorded time, or unknown when its lifecycle shows it occurred earlier.
fn task_event(work: &WorkItem, endpoint: Endpoint) -> Option<EventTime> {
    let (recorded, occurred) = match endpoint {
        Endpoint::Start => (
            work.events.started_at,
            matches!(
                work.status,
                WorkStatus::InProgress
                    | WorkStatus::Submitted
                    | WorkStatus::Verified
                    | WorkStatus::Done
            ),
        ),
        Endpoint::Finish => (work.events.verified_at, work.status.satisfies_dependency()),
    };
    match recorded {
        Some(at) => Some(EventTime::Recorded(at)),
        None => occurred.then_some(EventTime::Unrecorded),
    }
}

fn decision_event(decision: &Decision) -> Option<EventTime> {
    if decision.status == DecisionStatus::Open {
        return None;
    }
    Some(
        decision
            .resolved_at
            .map_or(EventTime::Unrecorded, EventTime::Recorded),
    )
}

fn ancestors(plan: &Plan, work: WorkItemId) -> Option<BTreeSet<WorkItemId>> {
    let mut ids = BTreeSet::new();
    let mut next = Some(work);
    while let Some(id) = next {
        if !ids.insert(id) {
            return None;
        }
        next = plan.work_items.get(&id)?.parent;
    }
    Some(ids)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
