//! Bar glyphs. Each kind of bar has its own shape and each status its own symbol, so a bar reads
//! the same with or without colour; the colour only repeats what the glyph says.

use crate::theme::{Role, Theme};
use dpm_model::{WorkItem, WorkKind, WorkStatus};
use ratatui::{
    style::Style,
    text::{Line, Span},
};

/// Bar cells of work that already ran, before the clock reading.
pub(super) const RECORDED: char = '░';
/// Remaining work on the critical path.
pub(super) const CRITICAL: char = '█';
/// Remaining work with float.
pub(super) const TASK: char = '━';
/// A work package spanning its applicable descendants.
pub(super) const PACKAGE: char = '─';
/// Status symbols: they fill the whole bar and win over the critical shape, because what blocks
/// or awaits review is what the operator acts on; the inspector states criticality separately.
pub(super) const BLOCKED: char = '!';
pub(super) const REVIEW: char = '?';
pub(super) const VERIFIED: char = '.';
pub(super) const MILESTONE: char = '◇';
pub(super) const MILESTONE_REACHED: char = '◆';

/// The bar glyph and style of `work`: status first, then criticality, then its kind.
pub(super) fn activity(
    theme: Theme,
    work: &WorkItem,
    verified: bool,
    critical: bool,
) -> (char, Style) {
    let (symbol, role) = match work.kind {
        WorkKind::Milestone if verified => (MILESTONE_REACHED, Role::Verified),
        WorkKind::Milestone => (MILESTONE, Role::Accent),
        WorkKind::WorkPackage => (PACKAGE, Role::Dim),
        WorkKind::Task if verified => (VERIFIED, Role::Verified),
        WorkKind::Task if work.execution.status == WorkStatus::Blocked => (BLOCKED, Role::Warning),
        WorkKind::Task if work.execution.status == WorkStatus::Submitted => (REVIEW, Role::Review),
        WorkKind::Task if critical => (CRITICAL, Role::Critical),
        WorkKind::Task => (TASK, Role::Accent),
    };
    (symbol, theme.style(role))
}

/// Legend rows for the help: every glyph beside the word it stands for, in its own colour.
pub(super) fn legend(theme: Theme) -> Vec<Line<'static>> {
    let entry = |symbol: char, word: &str, role: Role| {
        Span::styled(format!("{symbol} {word}   "), theme.style(role))
    };
    vec![
        Line::from(vec![
            Span::raw("  bars     "),
            entry(CRITICAL, "critical", Role::Critical),
            entry(TASK, "task", Role::Accent),
            entry(PACKAGE, "package", Role::Dim),
            entry(RECORDED, "recorded", Role::Text),
            Span::raw("◇/◆ [M] milestone pending/reached"),
        ]),
        Line::from(vec![
            Span::raw("  status   "),
            entry(BLOCKED, "blocked", Role::Warning),
            entry(REVIEW, "review", Role::Review),
            entry(VERIFIED, "verified", Role::Verified),
            Span::raw("< > bar continues outside the window"),
        ]),
        Line::from(vec![
            Span::raw("  labels   "),
            Span::styled("<P predecessor   ", theme.style(Role::RelationPredecessor)),
            Span::styled(">S successor   ", theme.style(Role::RelationSuccessor)),
            Span::raw("❯ selected   ~ hover   · direct links to the inspected work"),
        ]),
    ]
}
