//! Flow of work through execution: cycle and lead time, throughput, aging work in progress and
//! how reliably a claim ends in verified work.

use super::calibration::{HistoryIndex, ascending, elapsed_hours, exclusions};
use crate::{Command, Exclusion, ExclusionReason, Operation};
use chrono::{DateTime, Datelike, TimeDelta, Utc};
use dpm_model::{ActorId, ActorKind, Key, Plan, WorkItem, WorkItemId, WorkStatus};
use dpm_schedule::percentile;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// ISO weeks the throughput history covers, the current one included.
pub const THROUGHPUT_WEEKS: usize = 8;

/// Flow metrics at one clock reading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowReport {
    /// Elapsed hours from start to verification of verified tasks not recorded in bulk.
    pub cycle_time: Durations,
    /// Elapsed hours from the first claim to verification of verified tasks not recorded in bulk.
    pub lead_time: Durations,
    /// Tasks verified in recent periods.
    pub throughput: Throughput,
    /// Every claimed, started, blocked or submitted task, oldest claim first.
    pub aging: Vec<AgingWork>,
    /// How claims ended, overall and per holder kind.
    pub reliability: Reliability,
}

/// Distribution of elapsed durations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Durations {
    /// Number of tasks measured.
    pub count: usize,
    /// Median elapsed hours; absent without tasks.
    pub median_hours: Option<f64>,
    /// 85th percentile elapsed hours; absent without tasks.
    pub p85_hours: Option<f64>,
    /// Verified tasks not measured, grouped by reason.
    pub excluded: Vec<Exclusion>,
}

/// Tasks verified in recent periods, counted by their verification time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Throughput {
    /// Verified in the seven days up to the clock reading.
    pub last_7_days: usize,
    /// Verified in the 28 days up to the clock reading.
    pub last_28_days: usize,
    /// Verified per ISO week (UTC), the current week last.
    pub weeks: Vec<WeekCount>,
}

/// Tasks verified in one ISO week.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WeekCount {
    /// ISO week such as `2026-W40`.
    pub week: String,
    /// Tasks verified in it.
    pub verified: usize,
}

/// One task in progress and how long it has been held.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgingWork {
    /// Task key.
    pub key: Key,
    /// Lifecycle state.
    pub status: WorkStatus,
    /// Current holder.
    pub owner: Option<ActorId>,
    /// Elapsed hours since the current holder claimed it or received it by handoff, when known.
    pub hours_since_claim: Option<f64>,
    /// Elapsed hours since its recorded start.
    pub hours_since_start: Option<f64>,
}

/// How claims ended: each claim, or handoff to a new holder, opens one episode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reliability {
    /// Every episode in the log.
    pub total: ClaimOutcomes,
    /// Episodes per holder kind.
    pub by_holder: Vec<HolderOutcomes>,
}

/// Episodes of one holder kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HolderOutcomes {
    /// Kind of the holder.
    pub holder: ActorKind,
    /// How its episodes ended.
    #[serde(flatten)]
    pub outcomes: ClaimOutcomes,
}

/// Counts of claim episodes by how they ended.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ClaimOutcomes {
    /// Episodes in total.
    pub episodes: usize,
    /// Ended in verification.
    pub verified: usize,
    /// Ended in verification after at least one rejection.
    pub verified_after_rejection: usize,
    /// The holder gave the claim back.
    pub released: usize,
    /// A human or service moved the work to another holder; a transfer, not a failure.
    pub handed_off: usize,
    /// Still held.
    pub open: usize,
    /// `verified / (verified + released)`: handed-off and open episodes count on neither side;
    /// absent while no episode was verified or released.
    pub verified_fraction: Option<f64>,
}

/// Flow metrics of a plan and the log it results from.
///
/// Cycle and lead time skip the work calibration found recorded in bulk (`bulk`): its start and
/// submission are when someone typed them in, not when the work happened.
pub(crate) fn flow(
    plan: &Plan,
    index: &HistoryIndex<'_>,
    bulk: &BTreeSet<WorkItemId>,
    now: DateTime<Utc>,
) -> FlowReport {
    let done: Vec<&WorkItem> = plan
        .work_items
        .values()
        .filter(|w| w.is_executable() && w.execution.status.satisfies_dependency())
        .collect();
    let mut cycle = Measured::default();
    let mut lead = Measured::default();
    let mut verified = Vec::new();
    for work in done {
        let Some(at) = work.execution.events.verified_at else {
            cycle.skip(ExclusionReason::VerificationUnrecorded, work);
            lead.skip(ExclusionReason::VerificationUnrecorded, work);
            continue;
        };
        verified.push(at);
        if bulk.contains(&work.id) {
            cycle.skip(ExclusionReason::BulkRecorded, work);
            lead.skip(ExclusionReason::BulkRecorded, work);
            continue;
        }
        match work.execution.events.started_at {
            Some(started) => cycle.hours.push(elapsed_hours(started, at)),
            None => cycle.skip(ExclusionReason::StartUnrecorded, work),
        }
        let claim = index
            .of(work.id)
            .iter()
            .find(|op| matches!(op.command, Command::Claim { .. }));
        match claim {
            Some(claim) => lead.hours.push(elapsed_hours(claim.timestamp, at)),
            None => lead.skip(ExclusionReason::ClaimedBeforeHistory, work),
        }
    }
    FlowReport {
        cycle_time: cycle.durations(),
        lead_time: lead.durations(),
        throughput: throughput(verified.iter().copied(), now),
        aging: aging(plan, index, now),
        reliability: reliability(index.work_operations()),
    }
}

/// Durations measured so far and the tasks skipped, with why.
#[derive(Default)]
struct Measured {
    hours: Vec<f64>,
    skipped: Vec<(ExclusionReason, Key)>,
}

impl Measured {
    fn skip(&mut self, reason: ExclusionReason, work: &WorkItem) {
        self.skipped.push((reason, work.key.clone()));
    }

    fn durations(self) -> Durations {
        let sorted = ascending(self.hours);
        let known = !sorted.is_empty();
        Durations {
            count: sorted.len(),
            median_hours: known.then(|| percentile(&sorted, 0.5)),
            p85_hours: known.then(|| percentile(&sorted, 0.85)),
            excluded: exclusions(self.skipped),
        }
    }
}

fn throughput(
    verified: impl Iterator<Item = DateTime<Utc>> + Clone,
    now: DateTime<Utc>,
) -> Throughput {
    let within = |days: i64| {
        verified
            .clone()
            .filter(|at| *at <= now && now - *at < TimeDelta::days(days))
            .count()
    };
    let week_of = |at: DateTime<Utc>| {
        let week = at.iso_week();
        (week.year(), week.week())
    };
    let weeks = (0..THROUGHPUT_WEEKS)
        .rev()
        .filter_map(|back| now.checked_sub_signed(TimeDelta::weeks(back as i64)))
        .map(|at| {
            let (year, week) = week_of(at);
            WeekCount {
                week: format!("{year}-W{week:02}"),
                verified: verified
                    .clone()
                    .filter(|v| week_of(*v) == (year, week))
                    .count(),
            }
        })
        .collect();
    Throughput {
        last_7_days: within(7),
        last_28_days: within(28),
        weeks,
    }
}

fn aging(plan: &Plan, index: &HistoryIndex<'_>, now: DateTime<Utc>) -> Vec<AgingWork> {
    const HELD: [WorkStatus; 4] = [
        WorkStatus::Claimed,
        WorkStatus::InProgress,
        WorkStatus::Blocked,
        WorkStatus::Submitted,
    ];
    let mut aging: Vec<AgingWork> = plan
        .work_items
        .values()
        .filter(|w| w.is_executable() && HELD.contains(&w.execution.status))
        .map(|w| {
            let held_since = w.execution.events.claimed_at.or_else(|| {
                index
                    .last(w.id, |c| {
                        matches!(c, Command::Claim { .. } | Command::Handoff { .. })
                    })
                    .map(|op| op.timestamp)
            });
            AgingWork {
                key: w.key.clone(),
                status: w.execution.status,
                owner: w.execution.owner.clone(),
                hours_since_claim: held_since.map(|at| elapsed_hours(at, now)),
                hours_since_start: w
                    .execution
                    .events
                    .started_at
                    .map(|at| elapsed_hours(at, now)),
            }
        })
        .collect();
    aging.sort_by(|a, b| {
        let age = |w: &AgingWork| w.hours_since_claim.unwrap_or(f64::NEG_INFINITY);
        age(b)
            .total_cmp(&age(a))
            .then_with(|| a.key.natural_cmp(&b.key))
    });
    aging
}

/// How an episode ended.
#[derive(Clone, Copy)]
enum Ending {
    Verified { rejected: bool },
    Released,
    HandedOff,
    Open,
}

fn reliability(operations: &[&Operation]) -> Reliability {
    let mut open: BTreeMap<WorkItemId, (ActorKind, bool)> = BTreeMap::new();
    let mut ended: Vec<(ActorKind, Ending)> = Vec::new();
    for operation in operations {
        match &operation.command {
            Command::Claim { work } => {
                open.insert(*work, (operation.actor.kind, false));
            }
            Command::Handoff { work, to, .. } => {
                if let Some((holder, _)) = open.insert(*work, (to.kind, false)) {
                    ended.push((holder, Ending::HandedOff));
                }
            }
            Command::Release { work, .. } => {
                if let Some((holder, _)) = open.remove(work) {
                    ended.push((holder, Ending::Released));
                }
            }
            Command::Reject { work, .. } => {
                if let Some(episode) = open.get_mut(work) {
                    episode.1 = true;
                }
            }
            Command::Verify { work, .. } => {
                if let Some((holder, rejected)) = open.remove(work) {
                    ended.push((holder, Ending::Verified { rejected }));
                }
            }
            _ => {}
        }
    }
    ended.extend(open.into_values().map(|(holder, _)| (holder, Ending::Open)));
    let mut by_holder: BTreeMap<ActorKind, ClaimOutcomes> = BTreeMap::new();
    let mut total = ClaimOutcomes::default();
    for (holder, ending) in ended {
        count(&mut total, ending);
        count(by_holder.entry(holder).or_default(), ending);
    }
    Reliability {
        total: finish(total),
        by_holder: by_holder
            .into_iter()
            .map(|(holder, outcomes)| HolderOutcomes {
                holder,
                outcomes: finish(outcomes),
            })
            .collect(),
    }
}

fn count(outcomes: &mut ClaimOutcomes, ending: Ending) {
    outcomes.episodes = outcomes.episodes.wrapping_add(1);
    let slot = match ending {
        Ending::Verified { rejected } => {
            if rejected {
                outcomes.verified_after_rejection =
                    outcomes.verified_after_rejection.wrapping_add(1);
            }
            &mut outcomes.verified
        }
        Ending::Released => &mut outcomes.released,
        Ending::HandedOff => &mut outcomes.handed_off,
        Ending::Open => &mut outcomes.open,
    };
    *slot = slot.wrapping_add(1);
}

/// A handoff transfers the work rather than failing it, so its episode counts on neither side of
/// `verified_fraction`.
fn finish(mut outcomes: ClaimOutcomes) -> ClaimOutcomes {
    let closed = outcomes.verified + outcomes.released;
    outcomes.verified_fraction = (closed > 0).then(|| outcomes.verified as f64 / closed as f64);
    outcomes
}

#[cfg(test)]
mod tests;
