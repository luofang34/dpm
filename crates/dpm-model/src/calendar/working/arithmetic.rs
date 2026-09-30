//! Queries on compiled working spans, exact in integer milliseconds.
//!
//! Every forward operation has a backward partner that never undershoots it: `sub(add(t, h), h)`
//! and `add_late(sub(t, h), h)` are at or after `t`. Schedule passes rely on this, so a latest
//! time computed backwards is never earlier than the earliest time computed forwards.

use super::{BeyondCalendar, WorkingTime, hours, millis};

impl WorkingTime {
    fn at(&self, t: f64) -> Result<i64, BeyondCalendar> {
        let t = millis(t).ok_or(BeyondCalendar)?;
        if t < self.window.0 || t > self.window.1 {
            return Err(BeyondCalendar);
        }
        Ok(t)
    }

    fn span(&self, index: usize) -> Result<((i64, i64), i64), BeyondCalendar> {
        match (self.spans.get(index), self.before.get(index)) {
            (Some(span), Some(before)) => Ok((*span, *before)),
            _ => Err(BeyondCalendar),
        }
    }

    /// Working milliseconds between the window start and `t`, and the last span starting at or
    /// before `t`.
    fn locate(&self, t: i64) -> Result<(i64, Option<(i64, i64)>), BeyondCalendar> {
        let Some(index) = self.starts.through(t).checked_sub(1) else {
            return Ok((0, None));
        };
        let ((start, end), before) = self.span(index)?;
        Ok((before + t.min(end) - start, Some((start, end))))
    }

    /// Working milliseconds between the window start and `t`.
    fn worked(&self, t: i64) -> Result<i64, BeyondCalendar> {
        Ok(self.locate(t)?.0)
    }

    /// The instant inside span `index` at which `target` working milliseconds have passed.
    fn instant(&self, index: usize, target: i64) -> Result<i64, BeyondCalendar> {
        let ((start, _), before) = self.span(index)?;
        let at = start + (target - before);
        if at > self.window.1 {
            return Err(BeyondCalendar);
        }
        Ok(at)
    }

    /// Earliest time by which `hours` of working time after `t` have passed.
    pub fn add(&self, t: f64, hours_of_work: f64) -> Result<f64, BeyondCalendar> {
        if self.always {
            return Ok(t + hours_of_work.max(0.0));
        }
        let from = self.at(t)?;
        let amount = millis(hours_of_work).ok_or(BeyondCalendar)?;
        if amount <= 0 {
            return Ok(t);
        }
        let (worked, span) = self.locate(from)?;
        // Work that ends inside the span it starts in needs no second lookup.
        if let Some((start, end)) = span
            && from >= start
            && from < end
            && from + amount <= end
        {
            return Ok(hours(from + amount));
        }
        let target = worked + amount;
        let end = self.instant(self.through.below(target), target)?;
        Ok(hours(end.max(from)))
    }

    /// Latest time at which `hours` of working time after `t` have passed: past a span's end it
    /// is the next span's start, the same working instant.
    pub fn add_late(&self, t: f64, hours_of_work: f64) -> Result<f64, BeyondCalendar> {
        if self.always {
            return Ok(t + hours_of_work.max(0.0));
        }
        let from = self.at(t)?;
        let amount = millis(hours_of_work).ok_or(BeyondCalendar)?.max(0);
        let target = self.worked(from)? + amount;
        let end = self.instant(self.through.through(target), target)?;
        Ok(hours(end.max(from)))
    }

    /// Latest time from which `hours` of working time reach `t`.
    pub fn sub(&self, t: f64, hours_of_work: f64) -> Result<f64, BeyondCalendar> {
        if self.always {
            return Ok(t - hours_of_work.max(0.0));
        }
        let to = self.at(t)?;
        let amount = millis(hours_of_work).ok_or(BeyondCalendar)?;
        if amount <= 0 {
            return Ok(t);
        }
        let (worked, span) = self.locate(to)?;
        // Work that starts inside the span it ends in needs no second lookup.
        if let Some((start, end)) = span
            && to > start
            && to <= end
            && to - amount > start
        {
            return Ok(hours(to - amount));
        }
        let target = worked - amount;
        if target < 0 {
            return Err(BeyondCalendar);
        }
        let start = self.instant(self.through.through(target), target)?;
        Ok(hours(start.min(to)))
    }

    /// A signed lag forwards: `add` for a non-negative amount and `sub` for a negative one.
    pub fn shift(&self, t: f64, hours_of_work: f64) -> Result<f64, BeyondCalendar> {
        if hours_of_work >= 0.0 {
            self.add(t, hours_of_work)
        } else {
            self.sub(t, -hours_of_work)
        }
    }

    /// The latest time whose forward `shift` by `hours` is at most `t`.
    pub fn unshift(&self, t: f64, hours_of_work: f64) -> Result<f64, BeyondCalendar> {
        if hours_of_work >= 0.0 {
            self.sub(t, hours_of_work)
        } else {
            self.add_late(t, -hours_of_work)
        }
    }

    /// Earliest working moment at or after `t`; span ends count as working.
    pub fn align(&self, t: f64) -> Result<f64, BeyondCalendar> {
        if self.always {
            return Ok(t);
        }
        let at = self.at(t)?;
        let ((start, _), _) = self.span(self.ends.below(at))?;
        Ok(if start > at { hours(start) } else { t })
    }

    /// Earliest moment at or after `t` that working time follows; a span's end moves to the next
    /// span, so work starting there is placed where it actually begins.
    pub fn next_working(&self, t: f64) -> Result<f64, BeyondCalendar> {
        if self.always {
            return Ok(t);
        }
        let at = self.at(t)?;
        let ((start, _), _) = self.span(self.ends.through(at))?;
        Ok(if start > at { hours(start) } else { t })
    }

    /// Where work of `hours` starting no earlier than `t` runs: its start, the first moment that
    /// working time follows, and its end, the earliest time the hours have passed. Equal to
    /// `next_working` then `add`, with one lookup when the work fits in one span.
    pub fn place(&self, t: f64, hours_of_work: f64) -> Result<(f64, f64), BeyondCalendar> {
        if self.always {
            return Ok((t, t + hours_of_work.max(0.0)));
        }
        let amount = millis(hours_of_work).ok_or(BeyondCalendar)?;
        if amount <= 0 {
            return Ok((t, t));
        }
        let at = self.at(t)?;
        let index = self.ends.through(at);
        let ((start, end), before) = self.span(index)?;
        let begin = start.max(at);
        let starts = if begin > at { hours(begin) } else { t };
        if begin + amount <= end {
            return Ok((starts, hours(begin + amount)));
        }
        let target = before + (begin - start) + amount;
        let finish = self.instant(self.through.below(target), target)?;
        Ok((starts, hours(finish.max(begin))))
    }

    /// The latest start from which `hours` of work end by `align_back(t)`: equal to `align_back`
    /// then `sub`, with one lookup when the work fits in one span.
    pub fn retreat(&self, t: f64, hours_of_work: f64) -> Result<f64, BeyondCalendar> {
        if self.always {
            return Ok(t - hours_of_work.max(0.0));
        }
        let at = self.at(t)?;
        let index = self
            .starts
            .through(at)
            .checked_sub(1)
            .ok_or(BeyondCalendar)?;
        let ((start, end), before) = self.span(index)?;
        let to = at.min(end);
        let amount = millis(hours_of_work).ok_or(BeyondCalendar)?;
        if amount <= 0 {
            return Ok(if end < at { hours(end) } else { t });
        }
        if to - amount > start {
            return Ok(hours(to - amount));
        }
        let target = before + (to - start) - amount;
        if target < 0 {
            return Err(BeyondCalendar);
        }
        let begin = self.instant(self.through.through(target), target)?;
        Ok(hours(begin.min(to)))
    }

    /// Latest moment at or before `t` whose `align` is not after `t`.
    pub fn align_back(&self, t: f64) -> Result<f64, BeyondCalendar> {
        if self.always {
            return Ok(t);
        }
        let at = self.at(t)?;
        let index = self
            .starts
            .through(at)
            .checked_sub(1)
            .ok_or(BeyondCalendar)?;
        let ((_, end), _) = self.span(index)?;
        Ok(if end < at { hours(end) } else { t })
    }

    /// Working hours between `a` and a later `b`.
    pub fn between(&self, a: f64, b: f64) -> Result<f64, BeyondCalendar> {
        if self.always {
            return Ok((b - a).max(0.0));
        }
        let worked = self.worked(self.at(b)?)? - self.worked(self.at(a)?)?;
        Ok(hours(worked.max(0)))
    }
}
