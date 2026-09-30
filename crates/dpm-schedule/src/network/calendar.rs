//! Forward and backward passes whose durations and working-time lags follow calendars.
//!
//! An activity starts at its first working moment after its start constraints, works its
//! duration in working hours, and a task still awaiting verification finishes at the verifier
//! calendar's next working moment after any requested review delay. Finish constraints (FF, SF) move the start so that the work
//! ends no earlier than they require. Elapsed lags stay elapsed hours; working lags count the
//! successor's calendar in both passes, as the execution gates do.

use super::{Edge, Network, Times, finite, time_at};
use crate::ScheduleError;
use crate::placement::{Placed, Placement};
use dpm_model::{Endpoint, WorkingTime};

/// Earliest starts, verified finishes and review waits by topological position.
type Forward = (Vec<f64>, Vec<f64>, Vec<f64>);

/// A projected time at a topological position, or the far end of an edge.
fn read(values: &[f64], position: usize) -> Result<f64, Placed> {
    Ok(time_at(values, position)?)
}

/// The bound an edge puts on its successor, measured from the predecessor's event.
fn forward(edge: &Edge, event: f64, successor: &WorkingTime) -> Result<f64, Placed> {
    if edge.working {
        Ok(successor.shift(event, edge.lag)?)
    } else {
        Ok(event + edge.lag)
    }
}

/// The latest predecessor event an edge allows, measured back from the successor's event; it
/// never precedes an event whose forward bound is the successor's event.
fn backward(edge: &Edge, event: f64, successor: &WorkingTime) -> Result<f64, Placed> {
    if edge.working {
        Ok(successor.unshift(event, edge.lag)?)
    } else {
        Ok(event - edge.lag)
    }
}

impl Network {
    /// Forward and backward pass with calendar-dependent durations, repeated on wider calendar
    /// windows until every time fits.
    pub(crate) fn times_placed(
        &self,
        durations: &[f64],
        placement: &mut Placement,
    ) -> Result<Times, ScheduleError> {
        placement.fit(|placement| self.placed_pass(durations, placement))
    }

    fn placed_pass(&self, durations: &[f64], placement: &Placement) -> Result<Times, Placed> {
        let (earliest, earliest_finish, review_wait) = self.forward_placed(durations, placement)?;
        let finish = earliest_finish.iter().copied().fold(0.0_f64, f64::max);
        let (latest, latest_finish) = self.backward_placed(durations, placement, finish)?;
        Ok(Times {
            earliest,
            latest,
            earliest_finish,
            latest_finish,
            review_wait,
            finish,
        })
    }

    /// Earliest starts, verified finishes and review waits by topological position.
    fn forward_placed(&self, durations: &[f64], placement: &Placement) -> Result<Forward, Placed> {
        let count = self.order.len();
        let (mut earliest, mut earliest_finish) =
            (Vec::with_capacity(count), Vec::with_capacity(count));
        let mut review_wait = Vec::with_capacity(count);
        for (position, (id, edges)) in self.order.iter().zip(&self.incoming).enumerate() {
            let duration = super::at(durations, position, *id)?;
            let calendar = placement.execution(position)?;
            let (mut start, mut finish_bound) = (0.0_f64, f64::NEG_INFINITY);
            for edge in edges {
                let event = match edge.kind.predecessor_endpoint() {
                    Endpoint::Start => read(&earliest, edge.other)?,
                    Endpoint::Finish => read(&earliest_finish, edge.other)?,
                };
                let bound = finite(*id, forward(edge, event, calendar)?)?;
                match edge.kind.successor_endpoint() {
                    Endpoint::Start => start = start.max(bound),
                    Endpoint::Finish => finish_bound = finish_bound.max(bound),
                }
            }
            if finish_bound > start {
                start = start.max(calendar.sub(finish_bound, duration)?);
            }
            let (start, mut done) = calendar.place(start, duration)?;
            // Work placed on the calendar ends at a working moment, already aligned for a review
            // on the same calendar without delay.
            let mut aligned = duration > 0.0 && placement.reviewed_on_own_calendar(position);
            if done < finish_bound {
                aligned = false;
                // The work cannot end in a gap of its calendar: it is held to the next working
                // moment at or after the finish constraint.
                done = if duration > 0.0 {
                    calendar.align(finish_bound)?
                } else {
                    finish_bound
                };
            }
            let verified = match placement.review(position)? {
                Some(_) if aligned => done,
                Some(verifier) => verifier.align(done + placement.review_delay())?,
                None => done,
            };
            earliest.push(finite(*id, start)?);
            earliest_finish.push(finite(*id, verified)?);
            review_wait.push(verified - done);
        }
        Ok((earliest, earliest_finish, review_wait))
    }

    /// Latest starts and finishes by topological position for a project finish.
    fn backward_placed(
        &self,
        durations: &[f64],
        placement: &Placement,
        finish: f64,
    ) -> Result<(Vec<f64>, Vec<f64>), Placed> {
        let count = self.order.len();
        let mut latest = vec![0.0; count];
        let mut latest_finish = vec![0.0; count];
        for (position, (id, edges)) in self.order.iter().zip(&self.outgoing).enumerate().rev() {
            let duration = super::at(durations, position, *id)?;
            let (mut start_bound, mut finish_bound) = (f64::INFINITY, finish);
            for edge in edges {
                let successor = placement.execution(edge.other)?;
                let event = match edge.kind.successor_endpoint() {
                    Endpoint::Start => read(&latest, edge.other)?,
                    Endpoint::Finish => read(&latest_finish, edge.other)?,
                };
                let bound = finite(*id, backward(edge, event, successor)?)?;
                match edge.kind.predecessor_endpoint() {
                    Endpoint::Start => start_bound = start_bound.min(bound),
                    Endpoint::Finish => finish_bound = finish_bound.min(bound),
                }
            }
            let calendar = placement.execution(position)?;
            let review = placement.review(position)?;
            // Inverts the forward review: aligning back never undershoots a verified finish the
            // forward pass reached, so the work may end the whole delay before it.
            let latest_start = match review {
                Some(_) if placement.reviewed_on_own_calendar(position) => {
                    calendar.retreat(finish_bound, duration)?
                }
                Some(verifier) => calendar.sub(
                    verifier.align_back(finish_bound)? - placement.review_delay(),
                    duration,
                )?,
                None => calendar.sub(finish_bound, duration)?,
            };
            let start = start_bound.min(latest_start);
            // The latest finish is the bound itself: a finish held to a constraint in a calendar
            // gap may end later than its latest start plus the work.
            if let (Some(slot), Some(end)) =
                (latest.get_mut(position), latest_finish.get_mut(position))
            {
                *slot = finite(*id, start)?;
                *end = finite(*id, finish_bound)?;
            }
        }
        Ok((latest, latest_finish))
    }

    /// Slack before a successor bound or the project finish would move, on calendars.
    pub(super) fn free_float_placed(
        &self,
        times: &Times,
        position: usize,
        placement: &Placement,
    ) -> Result<f64, ScheduleError> {
        let (Some(id), Some(edges)) = (self.order.get(position), self.outgoing.get(position))
        else {
            return Err(ScheduleError::UnknownPosition(position));
        };
        let start = time_at(&times.earliest, position)?;
        let finish = time_at(&times.earliest_finish, position)?;
        let mut available = finite(*id, times.finish - finish)?;
        for edge in edges {
            let own = match edge.kind.predecessor_endpoint() {
                Endpoint::Start => start,
                Endpoint::Finish => finish,
            };
            let successor = match edge.kind.successor_endpoint() {
                Endpoint::Start => time_at(&times.earliest, edge.other)?,
                Endpoint::Finish => time_at(&times.earliest_finish, edge.other)?,
            };
            let calendar = placement
                .execution(edge.other)
                .map_err(|_| ScheduleError::CalendarRange)?;
            // A bound past the compiled window leaves more slack than any in-window successor.
            let Ok(bound) = forward(edge, own, calendar) else {
                continue;
            };
            available = available.min(finite(*id, successor - bound)?);
        }
        Ok(available.max(0.0))
    }
}
