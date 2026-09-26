use dpm_model::{DependencyKind, Plan, WorkItemId};

pub(crate) const LEGEND: &str =
    "FS finish→start · SS start→start · FF finish→finish · SF start→finish · lag in hours";

pub(crate) fn abbreviation(kind: DependencyKind) -> &'static str {
    match kind {
        DependencyKind::FinishStart => "FS",
        DependencyKind::StartStart => "SS",
        DependencyKind::FinishFinish => "FF",
        DependencyKind::StartFinish => "SF",
    }
}

pub(crate) fn lines(plan: &Plan, selected: Option<WorkItemId>) -> Vec<String> {
    plan.dependencies
        .iter()
        .filter(|d| selected.is_none_or(|id| d.predecessor == id || d.successor == id))
        .map(|d| {
            let policy = match (d.policy, d.is_waived()) {
                (dpm_model::DependencyPolicy::Hard, _) => "",
                (dpm_model::DependencyPolicy::Soft, false) => " [soft]",
                (dpm_model::DependencyPolicy::Soft, true) => " [soft, waived]",
            };
            format!(
                "{} --{} ({:?}) {:+.1}h--> {}{policy}",
                plan.work_items[&d.predecessor].key,
                abbreviation(d.kind),
                d.kind,
                d.lag_hours,
                plan.work_items[&d.successor].key
            )
        })
        .collect()
}

#[derive(Clone, Copy)]
pub(super) enum Relation {
    None,
    Predecessor,
    Successor,
}
impl Relation {
    pub(super) fn marker(self) -> &'static str {
        match self {
            Self::None => "  ",
            Self::Predecessor => "<P ",
            Self::Successor => ">S ",
        }
    }
    pub(super) fn color(self) -> ratatui::style::Color {
        match self {
            Self::None => ratatui::style::Color::Reset,
            Self::Predecessor => ratatui::style::Color::Cyan,
            Self::Successor => ratatui::style::Color::Magenta,
        }
    }
}
pub(super) fn relation(plan: &Plan, focus: WorkItemId, other: WorkItemId) -> Relation {
    for edge in &plan.dependencies {
        if edge.successor == focus && edge.predecessor == other {
            return Relation::Predecessor;
        }
        if edge.predecessor == focus && edge.successor == other {
            return Relation::Successor;
        }
    }
    Relation::None
}
