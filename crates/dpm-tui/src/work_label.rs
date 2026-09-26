use dpm_model::{WorkItem, WorkKind};

pub(crate) fn milestone_badge(work: &WorkItem, verified: bool) -> &'static str {
    match (work.kind, verified) {
        (WorkKind::Milestone, true) => "◆[M] ",
        (WorkKind::Milestone, false) => "◇[M] ",
        _ => "",
    }
}
