//! The calendar imported work follows, so it schedules on the hours the source tool uses.
//!
//! The source schedules a task on the 24-hour `always` calendar when its duration is elapsed time,
//! else on its own calendar, else on the project calendar. The candidate names a calendar only
//! where the workspace's rules would not already resolve that one, so a re-imported export keeps
//! existing work unchanged.

use crate::mspdi::calendars::Imported;
use crate::mspdi::encoding::{TimeBasis, time_basis};
use crate::mspdi::report::{Finding, ItemReport};
use crate::mspdi::source::SourceTask;
use dpm_model::{ALWAYS, WorkItem, WorkKind};

pub(super) fn map(
    task: &SourceTask,
    existing: Option<&WorkItem>,
    work: &mut WorkItem,
    report: &mut ItemReport,
    imported: &Imported,
) {
    if work.kind == WorkKind::WorkPackage {
        return;
    }
    let elapsed = task.duration_format.and_then(time_basis) == Some(TimeBasis::Elapsed);
    let target = match task.calendar_uid {
        _ if elapsed => ALWAYS.to_owned(),
        None => imported.project.clone(),
        Some(uid) => match imported.name(uid) {
            Some(name) => name.to_owned(),
            None => {
                report.approximated.push(Finding::new(
                    "calendar",
                    format!(
                        "task calendar UID {uid} is not imported; the task follows the project calendar {:?}",
                        imported.project
                    ),
                ));
                imported.project.clone()
            }
        },
    };
    let resolves = |work: &WorkItem| imported.calendars.resolve(work).calendar == target;
    if existing.is_some() && resolves(work) {
        report.preserved.push("calendar".into());
        return;
    }
    work.schedule.calendar = None;
    if !resolves(work) {
        work.schedule.calendar = Some(target);
    }
    report.preserved.push("calendar".into());
}
