use crate::{
    Decision, DecisionId, DecisionStatus, DependencyId, EventTime, Key, Plan, WorkItem, WorkItemId,
    WorkKind,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

mod packages;

/// Whether work belongs to the active execution graph, derived from decisions and constraints.
///
/// Only `Applicable` work can take lifecycle transitions or enter the remaining schedule. The
/// state is recomputed from authoritative decisions, conditions, join policies and unwaived
/// constraints; it is never persisted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Applicability {
    /// Every condition names the selected option and every prerequisite can proceed.
    Applicable,
    /// A condition on the work or a containing package awaits an open decision.
    Undecided {
        /// Decision that has not selected an option.
        decision: Key,
        /// Option the work requires.
        option: String,
    },
    /// A decision selected a different option than a condition on the work or a container.
    NotSelected {
        /// Decision that excluded the work.
        decision: Key,
        /// Option the work requires.
        option: String,
        /// Option the decision selected.
        selected: String,
    },
    /// A prerequisite is undecided or itself waits for a choice, so the work's place in the plan
    /// is not yet committed. For a work package with no applicable child, the prerequisite is a
    /// child that still waits for a choice.
    AwaitingChoice {
        /// Prerequisite, or package child, whose applicability is still unknown.
        predecessor: Key,
    },
    /// An ordinary constraint from not-selected work, or from work that can never proceed, never
    /// releases; only a reviewed plan change can let this work proceed.
    Stranded {
        /// Constraint that can never be released.
        dependency: DependencyId,
        /// Predecessor of that constraint.
        predecessor: Key,
    },
    /// Every branch into an active-branch join was excluded and the join does not permit that.
    EmptyJoin,
    /// Every child of this work package was excluded by a choice, so the package is excluded as
    /// well: it holds no work that could complete it.
    AllChildrenExcluded {
        /// Decisions whose selected options excluded the children, in key order.
        decisions: Vec<Key>,
    },
    /// No child of this work package is applicable and at least one that a choice did not exclude
    /// is stranded or an empty join, so the package can never complete without a plan change.
    ChildrenStranded {
        /// First such child, in key order.
        child: Key,
    },
}

impl Applicability {
    /// Whether the work belongs to the active graph.
    #[must_use]
    pub fn is_applicable(&self) -> bool {
        matches!(self, Self::Applicable)
    }

    /// Whether a choice excluded the work from the plan's scope, directly or through every child
    /// of a work package.
    #[must_use]
    pub fn is_not_selected(&self) -> bool {
        matches!(
            self,
            Self::NotSelected { .. } | Self::AllChildrenExcluded { .. }
        )
    }

    /// Whether the work's own conditions are selected, so its lifecycle may count as completion.
    #[must_use]
    pub fn condition_selected(&self) -> bool {
        !matches!(self, Self::NotSelected { .. } | Self::Undecided { .. })
    }
}

/// Applicability of every work item, with the time each choice that affects it was made.
pub(crate) struct Derived {
    pub(crate) states: BTreeMap<WorkItemId, Applicability>,
    /// For excluded work, when it was excluded; for selected conditional work, when the latest of
    /// its conditions was decided.
    pub(crate) choice_at: BTreeMap<WorkItemId, EventTime>,
}

/// Derive applicability: conditions through containment first, then constraints in dependency
/// order, then work packages from their children.
pub(crate) fn derive(plan: &Plan) -> Derived {
    let mut derived = Derived {
        states: BTreeMap::new(),
        choice_at: BTreeMap::new(),
    };
    for work in plan.work_items.values() {
        let (state, at) = conditions(plan, work);
        if let Some(at) = at {
            derived.choice_at.insert(work.id, at);
        }
        derived.states.insert(work.id, state);
    }
    for id in dependency_order(plan) {
        let Some(work) = plan.work_items.get(&id) else {
            continue;
        };
        // A selected task already verified has taken every transition; its prerequisites no longer
        // decide whether it can proceed, so a later choice change cannot strand it.
        let finished = work.is_executable() && work.status.satisfies_dependency();
        if work.kind == WorkKind::WorkPackage || finished || !derived.states[&id].is_applicable() {
            continue;
        }
        let state = through_constraints(plan, work, &derived.states);
        derived.states.insert(id, state);
    }
    packages::derive(plan, &mut derived);
    derived
}

impl Plan {
    /// Applicability of every work item; time-independent, so reviews can compare two plans.
    #[must_use]
    pub fn applicability(&self) -> BTreeMap<WorkItemId, Applicability> {
        derive(self).states
    }

    /// The decision whose outcome currently stands for `id`, following replacements.
    #[must_use]
    pub fn effective_decision(&self, id: DecisionId) -> Option<&Decision> {
        effective_decision(self, id)
    }
}

/// The decision whose outcome currently stands for `id`, following replacements.
pub(crate) fn effective_decision(plan: &Plan, id: DecisionId) -> Option<&Decision> {
    let mut current = plan.decisions.get(&id)?;
    let mut steps = 0_usize;
    while let Some(next) = plan
        .decisions
        .values()
        .find(|d| d.supersedes == Some(current.id))
    {
        current = next;
        steps = steps.wrapping_add(1);
        if steps > plan.decisions.len() {
            break;
        }
    }
    Some(current)
}

/// Since when the standing outcome of a resolved decision has held; `None` while open or withdrawn
/// without replacement.
///
/// A replacement that reaffirms the outcome it supersedes does not re-make the choice: work the
/// outcome selected or excluded was already released at the earlier resolution, and moving that
/// time would re-close gates that execution has relied on. The time is therefore the earliest
/// resolution in the unbroken run of equal outcomes ending at `decision`; a changed outcome starts
/// a new run at its own resolution.
pub(crate) fn standing_since(plan: &Plan, decision: &Decision) -> Option<EventTime> {
    if decision.status != DecisionStatus::Decided {
        return None;
    }
    let mut since = own_time(decision);
    let mut current = decision;
    let mut steps = 0_usize;
    while let Some(previous) = current.supersedes.and_then(|id| plan.decisions.get(&id)) {
        steps = steps.wrapping_add(1);
        if previous.outcome != decision.outcome || steps > plan.decisions.len() {
            break;
        }
        since = earliest(since, own_time(previous));
        current = previous;
    }
    Some(since)
}

fn own_time(decision: &Decision) -> EventTime {
    decision
        .resolved_at
        .map_or(EventTime::Unrecorded, EventTime::Recorded)
}

/// An unrecorded time predates recording, so it is the earlier of the two.
fn earliest(a: EventTime, b: EventTime) -> EventTime {
    match (a, b) {
        (EventTime::Recorded(a), EventTime::Recorded(b)) => EventTime::Recorded(a.min(b)),
        _ => EventTime::Unrecorded,
    }
}

fn conditions(plan: &Plan, work: &WorkItem) -> (Applicability, Option<EventTime>) {
    let mut undecided = None;
    let mut decided: Option<EventTime> = None;
    let mut next = Some(work);
    let mut depth = 0_usize;
    while let Some(item) = next {
        if let Some(condition) = &item.condition
            && let Some(decision) = effective_decision(plan, condition.decision)
        {
            match (standing_since(plan, decision), decision.outcome.as_deref()) {
                (Some(at), Some(selected)) if selected != condition.option => {
                    let state = Applicability::NotSelected {
                        decision: decision.key.clone(),
                        option: condition.option.clone(),
                        selected: selected.into(),
                    };
                    return (state, Some(at));
                }
                (Some(at), Some(_)) => decided = Some(decided.map_or(at, |d| d.latest(at))),
                _ => {
                    undecided.get_or_insert_with(|| Applicability::Undecided {
                        decision: decision.key.clone(),
                        option: condition.option.clone(),
                    });
                }
            }
        }
        depth = depth.wrapping_add(1);
        next = item
            .parent
            .and_then(|id| plan.work_items.get(&id))
            .filter(|_| depth <= plan.work_items.len());
    }
    match undecided {
        Some(state) => (state, None),
        None => (Applicability::Applicable, decided),
    }
}

fn through_constraints(
    plan: &Plan,
    work: &WorkItem,
    states: &BTreeMap<WorkItemId, Applicability>,
) -> Applicability {
    let mut awaiting = None;
    let (mut incoming, mut active) = (0_usize, 0_usize);
    for edge in plan
        .enforced_dependencies()
        .filter(|e| e.successor == work.id)
    {
        incoming = incoming.wrapping_add(1);
        let Some(predecessor) = plan.work_items.get(&edge.predecessor) else {
            continue;
        };
        match states.get(&edge.predecessor) {
            Some(state) if state.is_not_selected() && work.join.skips_unselected() => {}
            Some(
                Applicability::NotSelected { .. }
                | Applicability::AllChildrenExcluded { .. }
                | Applicability::Stranded { .. }
                | Applicability::ChildrenStranded { .. }
                | Applicability::EmptyJoin,
            ) => {
                return Applicability::Stranded {
                    dependency: edge.id,
                    predecessor: predecessor.key.clone(),
                };
            }
            Some(Applicability::Undecided { .. } | Applicability::AwaitingChoice { .. }) => {
                awaiting.get_or_insert_with(|| predecessor.key.clone());
                active = active.wrapping_add(1);
            }
            Some(Applicability::Applicable) | None => active = active.wrapping_add(1),
        }
    }
    if let Some(predecessor) = awaiting {
        return Applicability::AwaitingChoice { predecessor };
    }
    let permits_empty = matches!(
        work.join,
        crate::JoinPolicy::ActiveBranches { allow_empty: true }
    );
    if work.join.skips_unselected() && incoming > 0 && active == 0 && !permits_empty {
        return Applicability::EmptyJoin;
    }
    Applicability::Applicable
}

/// Work in dependency order; items on a cycle (rejected by validation) are omitted.
fn dependency_order(plan: &Plan) -> Vec<WorkItemId> {
    let mut indegree: BTreeMap<_, usize> = plan.work_items.keys().map(|id| (*id, 0)).collect();
    let mut outgoing: BTreeMap<_, Vec<_>> = BTreeMap::new();
    for edge in &plan.dependencies {
        if let Some(count) = indegree.get_mut(&edge.successor) {
            *count = count.wrapping_add(1);
            outgoing
                .entry(edge.predecessor)
                .or_default()
                .push(edge.successor);
        }
    }
    let mut queue: VecDeque<_> = indegree
        .iter()
        .filter_map(|(id, n)| (*n == 0).then_some(*id))
        .collect();
    let mut order = Vec::with_capacity(plan.work_items.len());
    while let Some(id) = queue.pop_front() {
        order.push(id);
        for child in outgoing.get(&id).into_iter().flatten() {
            if let Some(count) = indegree.get_mut(child) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    queue.push_back(*child);
                }
            }
        }
    }
    order
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
