//! Recorded execution beside the remaining schedule, so a Gantt draws what happened before the
//! clock reading as well as what is forecast after it.

use chrono::{DateTime, Utc};
use dpm_model::{Endpoint, EventTime, Plan, Timeline, WorkItemId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// When work actually started and finished, from recorded events only; nothing is inferred, so an
/// event that predates recorded times is absent rather than guessed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ActualSpan {
    /// A task's recorded start, or a package's earliest recorded descendant start.
    pub started_at: Option<DateTime<Utc>>,
    /// A task's verification, or a milestone's or package's completion; absent while outstanding.
    pub finished_at: Option<DateTime<Utc>>,
    /// `started_at` in elapsed hours from the clock reading; negative in the past.
    pub start_hours: Option<f64>,
    /// `finished_at` in elapsed hours from the clock reading; negative in the past.
    pub finish_hours: Option<f64>,
    /// On the as-built critical path: reached along driving recorded relations from the last
    /// recorded finish. Independent of the remaining schedule's criticality.
    #[serde(default)]
    pub critical: bool,
}

/// Recorded start and finish of every work item with at least one, measured from the timeline's
/// clock reading.
#[must_use]
pub fn actual_spans(plan: &Plan, timeline: &Timeline) -> BTreeMap<WorkItemId, ActualSpan> {
    let mut started: BTreeMap<WorkItemId, DateTime<Utc>> = BTreeMap::new();
    for item in plan.work_items.values().filter(|w| w.is_executable()) {
        let Some(EventTime::Recorded(at)) = timeline.event(plan, item.id, Endpoint::Start) else {
            continue;
        };
        let mut next = Some(item.id);
        while let Some(id) = next {
            started
                .entry(id)
                .and_modify(|earliest| *earliest = (*earliest).min(at))
                .or_insert(at);
            next = plan.work_items.get(&id).and_then(|w| w.parent);
        }
    }
    let hours = |at: DateTime<Utc>| (at - timeline.now()).num_milliseconds() as f64 / 3_600_000.0;
    plan.work_items
        .keys()
        .filter_map(|id| {
            let started_at = started.get(id).copied();
            let finished_at = match timeline.completed_at(*id) {
                Some(EventTime::Recorded(at)) => Some(at),
                _ => None,
            };
            (started_at.is_some() || finished_at.is_some()).then(|| {
                (
                    *id,
                    ActualSpan {
                        started_at,
                        finished_at,
                        start_hours: started_at.map(hours),
                        finish_hours: finished_at.map(hours),
                        critical: false,
                    },
                )
            })
        })
        .collect()
}
