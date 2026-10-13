//! Semantic styles shared by every pane. Roles name what text means; the theme maps each role to
//! an ANSI 16 named colour so the console follows the operator's terminal palette. Every role that
//! carries meaning is paired with words or a symbol at its use, so turning colour off (NO_COLOR)
//! loses emphasis but no information.

use ratatui::{
    style::{Color, Modifier, Style},
    widgets::{Block, BorderType},
};

/// What a piece of text means, independent of the colour that shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    /// Ordinary text in the terminal's own foreground.
    Text,
    /// Secondary text: labels, metadata, hints and unfocused borders.
    Dim,
    /// The focused pane, the product name and ordinary scheduled work.
    Accent,
    /// Work on the critical path.
    Critical,
    /// Blocked work and other cautions, such as a read-only source.
    Warning,
    /// Work submitted for review.
    Review,
    /// Verified work and reached milestones.
    Verified,
    /// Labels of direct predecessors of the inspected work.
    RelationPredecessor,
    /// Labels of direct successors of the inspected work.
    RelationSuccessor,
    /// Decisions that gate the inspected work.
    Gate,
}

/// Colour switch for every role; `enabled == false` is NO_COLOR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Theme {
    pub(crate) enabled: bool,
}

impl Theme {
    /// The colour of `role`, or the terminal default when colour is off.
    pub(crate) fn color(self, role: Role) -> Color {
        if !self.enabled {
            return Color::Reset;
        }
        match role {
            Role::Text => Color::Reset,
            Role::Dim => Color::DarkGray,
            Role::Accent => Color::LightBlue,
            Role::Critical => Color::Red,
            Role::Warning => Color::Yellow,
            Role::Review | Role::RelationSuccessor => Color::Magenta,
            Role::Verified => Color::Green,
            Role::RelationPredecessor => Color::Cyan,
            Role::Gate => Color::LightYellow,
        }
    }

    pub(crate) fn style(self, role: Role) -> Style {
        Style::default().fg(self.color(role))
    }

    /// A pane with rounded corners. The focused pane's border is the accent and bold, so focus
    /// stays visible without colour; other borders are dim.
    pub(crate) fn pane(self, focused: bool) -> Block<'static> {
        let border = if focused {
            self.style(Role::Accent).add_modifier(Modifier::BOLD)
        } else {
            self.style(Role::Dim)
        };
        Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(border)
    }
}

#[cfg(test)]
mod tests;
