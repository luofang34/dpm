//! Calendars of one remaining projection, compiled once for every pass over the network.
//!
//! Each activity executes on the calendar its work resolves to, and a task still awaiting
//! verification finishes only when the verifier kind's calendar is open. Calendars are compiled
//! over a window of hours after the clock reading; a pass that leaves the window is repeated on a
//! wider one, so results never depend on the window chosen.

use crate::network::Network;
use crate::{RemainingOptions, ScheduleError};
use dpm_model::{
    BeyondCalendar, Calendars, MAX_WINDOW_HOURS, Plan, ResolvedCalendar, Timeline, WorkItemId,
    WorkingTime,
};
use std::collections::{BTreeMap, BTreeSet};

/// Wider windows tried before a projection gives up; each doubles the span, and no window
/// exceeds the calendar's maximum.
const WIDENINGS: usize = 24;

/// Compiled calendars and each activity's execution and review calendars.
#[derive(Debug)]
pub(crate) struct Placement {
    calendars: Calendars,
    origin: chrono::DateTime<chrono::Utc>,
    window: (f64, f64),
    names: Vec<String>,
    compiled: Vec<WorkingTime>,
    /// Execution calendar index by topological position.
    execution: Vec<usize>,
    /// Verifier calendar index by topological position, for tasks that still await verification.
    review: Vec<Option<usize>>,
    /// Resolved calendar of each executable task, for reports.
    pub(crate) resolved: BTreeMap<WorkItemId, ResolvedCalendar>,
    /// Elapsed hours a review waits after the work ends, before the verifier's calendar applies.
    review_delay: f64,
}

impl Placement {
    /// Placement of a remaining projection, or `None` for a plan without calendars that needs
    /// no review delay.
    ///
    /// `awaiting` lists the tasks whose verification the projection still waits for. A review
    /// delay in a plan without calendars places every activity on the `always` calendar, which
    /// reproduces the elapsed projection apart from the delay.
    pub(crate) fn new(
        plan: &Plan,
        network: &Network,
        timeline: &Timeline,
        awaiting: &BTreeSet<WorkItemId>,
        horizon: f64,
        options: RemainingOptions,
    ) -> Result<Option<Self>, ScheduleError> {
        let (calendars, reported) = match (&plan.calendars, options.delays_review()) {
            (Some(calendars), _) => (calendars.clone(), true),
            (None, true) => (elapsed_calendars(), false),
            (None, false) => return Ok(None),
        };
        let mut names: Vec<String> = Vec::new();
        let mut index = |name: &str| match names.iter().position(|n| n == name) {
            Some(found) => found,
            None => {
                names.push(name.to_owned());
                names.len() - 1
            }
        };
        let verifier = index(calendars.verifier_calendar());
        let mut execution = Vec::with_capacity(network.order().len());
        let mut review = Vec::with_capacity(network.order().len());
        let mut resolved = BTreeMap::new();
        for id in network.order() {
            let work = plan
                .work_items
                .get(id)
                .ok_or(ScheduleError::MissingWorkItem(*id))?;
            let calendar = calendars.resolve(work);
            execution.push(index(&calendar.calendar));
            review.push(awaiting.contains(id).then_some(verifier));
            if work.is_executable() && reported {
                resolved.insert(*id, calendar);
            }
        }
        let mut placement = Self {
            calendars,
            origin: timeline.now(),
            window: (
                -24.0 * 14.0,
                horizon.clamp(24.0 * 7.0 * 8.0, MAX_WINDOW_HOURS / 2.0),
            ),
            names,
            compiled: Vec::new(),
            execution,
            review,
            resolved,
            review_delay: options.review_delay_hours,
        };
        placement.compile()?;
        Ok(Some(placement))
    }

    fn compile(&mut self) -> Result<(), ScheduleError> {
        let zone = self
            .calendars
            .zone()
            .map_err(|_| ScheduleError::CalendarRange)?;
        self.compiled = self
            .names
            .iter()
            .map(|name| {
                let rules = self
                    .calendars
                    .rules(name)
                    .ok_or(ScheduleError::CalendarRange)?;
                WorkingTime::compile(rules, zone, self.origin, self.window.0, self.window.1)
                    .ok_or(ScheduleError::CalendarRange)
            })
            .collect::<Result<_, _>>()?;
        Ok(())
    }

    /// Widen the window, mostly forwards where projections grow, and recompile; a window at the
    /// maximum is out of range.
    fn widen(&mut self) -> Result<(), ScheduleError> {
        let span = self.window.1 - self.window.0;
        if span >= MAX_WINDOW_HOURS {
            return Err(ScheduleError::CalendarRange);
        }
        let grown = (span * 2.0).min(MAX_WINDOW_HOURS);
        let before = (grown - span) / 4.0;
        self.window = (
            self.window.0 - before,
            self.window.1 + (grown - span - before),
        );
        self.compile()
    }

    /// Run a pass, widening the window until the pass fits in it.
    pub(crate) fn fit<T>(
        &mut self,
        mut pass: impl FnMut(&Self) -> Result<T, Placed>,
    ) -> Result<T, ScheduleError> {
        for _ in 0..WIDENINGS {
            match pass(self) {
                Ok(value) => return Ok(value),
                Err(Placed::Beyond) => self.widen()?,
                Err(Placed::Schedule(error)) => return Err(error),
            }
        }
        Err(ScheduleError::CalendarRange)
    }

    fn calendar(&self, index: usize) -> Result<&WorkingTime, Placed> {
        self.compiled
            .get(index)
            .ok_or(Placed::Schedule(ScheduleError::CalendarRange))
    }

    /// Execution calendar of the activity at a position.
    pub(crate) fn execution(&self, position: usize) -> Result<&WorkingTime, Placed> {
        let index = self
            .execution
            .get(position)
            .ok_or(Placed::Schedule(ScheduleError::UnknownPosition(position)))?;
        self.calendar(*index)
    }

    /// Elapsed hours a review waits after the work ends.
    pub(crate) fn review_delay(&self) -> f64 {
        self.review_delay
    }

    /// Verifier calendar of a task at a position that still awaits verification.
    pub(crate) fn review(&self, position: usize) -> Result<Option<&WorkingTime>, Placed> {
        match self.review.get(position) {
            Some(Some(index)) => self.calendar(*index).map(Some),
            Some(None) => Ok(None),
            None => Err(Placed::Schedule(ScheduleError::UnknownPosition(position))),
        }
    }
}

/// Failure of one pass: the window was too narrow, or the projection itself is invalid.
#[derive(Debug)]
pub(crate) enum Placed {
    Beyond,
    Schedule(ScheduleError),
}

impl From<BeyondCalendar> for Placed {
    fn from(_: BeyondCalendar) -> Self {
        Self::Beyond
    }
}

impl From<ScheduleError> for Placed {
    fn from(error: ScheduleError) -> Self {
        Self::Schedule(error)
    }
}

/// Calendars in which every kind works every hour, so placement adds nothing but a review delay.
fn elapsed_calendars() -> Calendars {
    let always = || dpm_model::ALWAYS.to_owned();
    Calendars {
        kinds: dpm_model::KindCalendars {
            human: always(),
            agent: always(),
            service: always(),
        },
        ..Calendars::in_zone("UTC")
    }
}

/// Hours between two instants of a task's work, such as its start and the clock reading: working
/// hours of the task's calendar when the plan has calendars, elapsed hours otherwise; zero when
/// `until` precedes `from`.
///
/// Remaining forecasts measure started work with it, and calibration measures recorded work with
/// it, so both count the same hours.
pub fn worked_between(
    plan: &Plan,
    work: &dpm_model::WorkItem,
    started: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<f64, ScheduleError> {
    let hours = ((now - started).num_milliseconds() as f64 / 3_600_000.0).max(0.0);
    let Some(calendars) = plan.calendars.as_ref() else {
        return Ok(hours);
    };
    let rules = calendars
        .rules(&calendars.resolve(work).calendar)
        .ok_or(ScheduleError::CalendarRange)?;
    let zone = calendars.zone().map_err(|_| ScheduleError::CalendarRange)?;
    WorkingTime::compile(rules, zone, started, -24.0, hours + 24.0)
        .ok_or(ScheduleError::CalendarRange)?
        .between(0.0, hours)
        .map_err(|_| ScheduleError::CalendarRange)
}
