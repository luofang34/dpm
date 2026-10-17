//! The as-built critical path: which recorded relations actually drove the work that happened.
//!
//! The remaining-work schedule says nothing about finished work: once everything is verified, every
//! remaining duration is zero. What happened is answered from recorded events instead. A relation
//! whose two events were both recorded has a gap, the time its successor's event lay past what the
//! relation alone required; among a successor's recorded relations the smallest gap drove it. The
//! as-built critical path follows driving relations back from the last recorded finish. Derived
//! only, from recorded events; nothing here is stored.

use super::ActualSpan;
use dpm_model::{DependencyId, DependencyKind, Plan, Timeline, WorkItemId, WorkKind};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Gaps this close are equal: recorded instants are exact, but the hours derived from them are not.
const TOLERANCE_HOURS: f64 = 1e-6;

/// One relation whose events were both recorded, and how it bound the work that happened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordedRelation {
    /// Stable dependency identity.
    pub id: DependencyId,
    /// Work providing the constrained event.
    pub predecessor: WorkItemId,
    /// Work whose event was bounded.
    pub successor: WorkItemId,
    /// FS, SS, FF or SF.
    pub kind: DependencyKind,
    /// How far the successor's recorded event lay past the predecessor's plus the lag, in elapsed
    /// hours; negative when the recorded order broke the relation.
    pub gap_hours: f64,
    /// The smallest gap among the successor's recorded relations: this one held it back.
    pub driving: bool,
    /// Driving, and on the chain that ends at the last recorded finish.
    pub critical: bool,
}

/// The recorded relations, in plan order, and the work on the as-built critical path.
pub(super) struct AsBuilt {
    pub relations: Vec<RecordedRelation>,
    pub critical: BTreeSet<WorkItemId>,
}

/// A recorded event of one work item, in elapsed hours; a milestone's start is its completion.
fn event(actuals: &BTreeMap<WorkItemId, ActualSpan>, id: WorkItemId, start: bool) -> Option<f64> {
    let span = actuals.get(&id)?;
    if start {
        span.start_hours.or(span.finish_hours)
    } else {
        span.finish_hours
    }
}

pub(super) fn as_built(
    plan: &Plan,
    timeline: &Timeline,
    actuals: &BTreeMap<WorkItemId, ActualSpan>,
) -> AsBuilt {
    let applicable = |id: WorkItemId| timeline.applicability(id).is_applicable();
    let mut relations: Vec<RecordedRelation> = plan
        .dependencies
        .iter()
        .filter(|d| d.waiver.is_none() && applicable(d.predecessor) && applicable(d.successor))
        .filter_map(|d| {
            let (from_start, to_start) = match d.kind {
                DependencyKind::FinishStart => (false, true),
                DependencyKind::StartStart => (true, true),
                DependencyKind::FinishFinish => (false, false),
                DependencyKind::StartFinish => (true, false),
            };
            let required = event(actuals, d.predecessor, from_start)? + d.lag_hours;
            let gap_hours = event(actuals, d.successor, to_start)? - required;
            Some(RecordedRelation {
                id: d.id,
                predecessor: d.predecessor,
                successor: d.successor,
                kind: d.kind,
                gap_hours,
                driving: false,
                critical: false,
            })
        })
        .collect();
    let mut least: BTreeMap<WorkItemId, f64> = BTreeMap::new();
    for r in &relations {
        least
            .entry(r.successor)
            .and_modify(|gap| *gap = gap.min(r.gap_hours))
            .or_insert(r.gap_hours);
    }
    for r in &mut relations {
        r.driving = least
            .get(&r.successor)
            .is_some_and(|gap| r.gap_hours <= gap + TOLERANCE_HOURS);
    }
    let critical = walk_back(plan, timeline, actuals, &mut relations);
    AsBuilt {
        relations,
        critical,
    }
}

/// Mark the work and driving relations reached backwards from the last recorded finish.
fn walk_back(
    plan: &Plan,
    timeline: &Timeline,
    actuals: &BTreeMap<WorkItemId, ActualSpan>,
    relations: &mut [RecordedRelation],
) -> BTreeSet<WorkItemId> {
    // Packages finish when their descendants do; the chain runs through the work itself.
    let finishes: Vec<(WorkItemId, f64)> = plan
        .work_items
        .values()
        .filter(|w| w.kind != WorkKind::WorkPackage && timeline.applicability(w.id).is_applicable())
        .filter_map(|w| Some((w.id, actuals.get(&w.id)?.finish_hours?)))
        .collect();
    let Some(last) = finishes.iter().map(|(_, h)| *h).reduce(f64::max) else {
        return BTreeSet::new();
    };
    let mut critical: BTreeSet<WorkItemId> = finishes
        .iter()
        .filter(|(_, h)| *h >= last - TOLERANCE_HOURS)
        .map(|(id, _)| *id)
        .collect();
    let mut frontier: Vec<WorkItemId> = critical.iter().copied().collect();
    while let Some(successor) = frontier.pop() {
        for r in relations
            .iter_mut()
            .filter(|r| r.driving && r.successor == successor)
        {
            r.critical = true;
            if critical.insert(r.predecessor) {
                frontier.push(r.predecessor);
            }
        }
    }
    critical
}
