use crate::applicability::{self, Derived};
use crate::{
    Applicability, Decision, Dependency, Endpoint, EventTime, Plan, Release, WorkItem, WorkItemId,
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
    index: crate::graph_index::GraphIndex,
    now: DateTime<Utc>,
    complete: BTreeMap<WorkItemId, EventTime>,
    applicability: BTreeMap<WorkItemId, Applicability>,
    choice_at: BTreeMap<WorkItemId, EventTime>,
}

const APPLICABLE: Applicability = Applicability::Applicable;

impl Timeline {
    /// Derive completion from a validated plan and an adapter-supplied clock reading.
    ///
    /// Tasks complete when verified and their conditions are selected. A milestone is reached when
    /// it is applicable and every unwaived incoming edge and every decision gating it or its
    /// containers is released; its time is the latest of those releases and of the choices that
    /// selected it or skipped its branches. A work package completes when every child that a
    /// choice did not exclude is complete, and at least one is, at the latest child, choice or
    /// gate time; a package whose children were all excluded or cannot proceed is not applicable,
    /// so it is never complete.
    #[must_use]
    pub fn at(plan: &Plan, now: DateTime<Utc>) -> Self {
        let index = crate::graph_index::GraphIndex::new(plan);
        let Derived { states, choice_at } = applicability::derive_indexed(plan, &index);
        let mut timeline = Self {
            index,
            now,
            complete: plan
                .work_items
                .values()
                .filter(|w| w.is_executable())
                .filter(|w| {
                    states
                        .get(&w.id)
                        .is_none_or(Applicability::condition_selected)
                })
                .filter_map(|w| task_event(w, Endpoint::Finish).map(|at| (w.id, at)))
                .collect(),
            applicability: states,
            choice_at,
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

    /// Enforced incoming edges in source order, for the same immutable plan used by this timeline.
    pub fn incoming<'a>(
        &'a self,
        plan: &'a Plan,
        work: WorkItemId,
    ) -> impl Iterator<Item = &'a Dependency> {
        self.index
            .incoming
            .get(&work)
            .into_iter()
            .flatten()
            .filter_map(|i| plan.dependencies.get(*i))
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

    /// Whether work belongs to the active graph, and if not, why.
    #[must_use]
    pub fn applicability(&self, work: WorkItemId) -> &Applicability {
        self.applicability.get(&work).unwrap_or(&APPLICABLE)
    }

    /// Whether an edge's required predecessor event plus positive lag has elapsed.
    ///
    /// A constraint from not-selected work is a skipped branch, released when the choice was made,
    /// only into an active-branch join; into any other successor it never releases.
    #[must_use]
    pub fn edge(&self, plan: &Plan, edge: &Dependency) -> Release {
        if self.applicability(edge.predecessor).is_not_selected() {
            let join = plan
                .work_items
                .get(&edge.successor)
                .map(|w| w.contract.join);
            return match (join, self.choice_at.get(&edge.predecessor)) {
                (Some(join), Some(at)) if join.skips_unselected() => {
                    Release::SkippedBranch { at: *at }
                }
                _ => Release::NotSelected,
            };
        }
        let event = self.event(plan, edge.predecessor, edge.kind.predecessor_endpoint());
        Release::evaluate(event, edge.lag_hours, self.now)
    }

    /// Decisions naming this work or a containing package as blocked, with their release state.
    #[must_use]
    pub fn decisions<'a>(&self, plan: &'a Plan, work: WorkItemId) -> Vec<(&'a Decision, Release)> {
        let Some(scope) = ancestors(plan, work) else {
            return Vec::new();
        };
        let ids: BTreeSet<_> = scope
            .iter()
            .flat_map(|id| self.index.decisions.get(id).into_iter().flatten())
            .collect();
        ids.into_iter()
            .filter_map(|id| plan.decisions.get(id))
            .map(|d| (d, Release::evaluate(gate_event(d), 0.0, self.now)))
            .collect()
    }

    fn aggregate_completion(&self, plan: &Plan, work: &WorkItem) -> Option<EventTime> {
        if work.execution.status != WorkStatus::Planned
            || !self.applicability(work.id).is_applicable()
        {
            return None;
        }
        let prerequisites: Vec<Release> = match work.kind {
            WorkKind::Task => return None,
            WorkKind::Milestone => self
                .incoming(plan, work.id)
                .map(|d| self.edge(plan, d))
                .collect(),
            WorkKind::WorkPackage => self
                .index
                .children
                .get(&work.id)
                .into_iter()
                .flatten()
                .filter_map(|id| plan.work_items.get(id))
                .map(|w| match self.choice_at.get(&w.id) {
                    Some(at) if self.applicability(w.id).is_not_selected() => {
                        Release::SkippedBranch { at: *at }
                    }
                    _ => self
                        .completed_at(w.id)
                        .map_or(Release::AwaitingEvent, |at| Release::Released { at }),
                })
                .collect(),
        };
        // Without one branch that happened, completion would rest only on excluded work.
        let permits_empty = matches!(
            work.contract.join,
            crate::JoinPolicy::ActiveBranches { allow_empty: true }
        );
        let happened = prerequisites
            .iter()
            .any(|r| !matches!(r, Release::SkippedBranch { .. }));
        if prerequisites.is_empty() || (!happened && !permits_empty) {
            return None;
        }
        let gates = self.decisions(plan, work.id).into_iter().map(|(_, r)| r);
        let chosen = self
            .choice_at
            .get(&work.id)
            .map(|at| Release::Released { at: *at });
        prerequisites
            .into_iter()
            .chain(gates)
            .chain(chosen)
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
    match endpoint {
        Endpoint::Start => work.start_event(),
        Endpoint::Finish => match work.execution.events.verified_at {
            Some(at) => Some(EventTime::Recorded(at)),
            None => work
                .execution
                .status
                .satisfies_dependency()
                .then_some(EventTime::Unrecorded),
        },
    }
}

/// A gate is released once it is no longer open, whether decided or superseded.
fn gate_event(decision: &Decision) -> Option<EventTime> {
    (decision.status != crate::DecisionStatus::Open).then(|| {
        decision
            .resolved_at
            .map_or(EventTime::Unrecorded, EventTime::Recorded)
    })
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
