use super::*;
use ratatui::{
    Terminal,
    backend::TestBackend,
    layout::{Constraint, Layout},
};

const ROLES: [Role; 10] = [
    Role::Text,
    Role::Dim,
    Role::Accent,
    Role::Critical,
    Role::Warning,
    Role::Review,
    Role::Verified,
    Role::RelationPredecessor,
    Role::RelationSuccessor,
    Role::Gate,
];

#[test]
fn no_color_resets_every_role_and_colour_uses_named_ansi_colours() {
    let mono = Theme { enabled: false };
    assert!(ROLES.iter().all(|role| mono.color(*role) == Color::Reset));
    let colour = Theme { enabled: true };
    assert!(
        ROLES
            .iter()
            .all(|role| !matches!(colour.color(*role), Color::Rgb(..) | Color::Indexed(_)))
    );
    // Status roles colour bars and relation roles colour labels; each keeps its own name.
    assert_eq!(colour.color(Role::Critical), Color::Red);
    assert_eq!(colour.color(Role::Warning), Color::Yellow);
    assert_eq!(colour.color(Role::Verified), Color::Green);
    assert_eq!(colour.color(Role::RelationPredecessor), Color::Cyan);
}

#[test]
fn panes_are_rounded_and_focus_stays_visible_without_colour() {
    for enabled in [true, false] {
        let theme = Theme { enabled };
        let mut terminal = Terminal::new(TestBackend::new(8, 3)).expect("terminal");
        terminal
            .draw(|frame| {
                let [left, right] =
                    Layout::horizontal([Constraint::Length(4), Constraint::Length(4)])
                        .areas(frame.area());
                frame.render_widget(theme.pane(true), left);
                frame.render_widget(theme.pane(false), right);
            })
            .expect("draw");
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, 0)].symbol(), "╭");
        assert_eq!(buffer[(7, 2)].symbol(), "╯");
        assert!(buffer[(0, 0)].modifier.contains(Modifier::BOLD));
        assert!(!buffer[(4, 0)].modifier.contains(Modifier::BOLD));
        let accent = if enabled {
            Color::LightBlue
        } else {
            Color::Reset
        };
        assert_eq!(buffer[(0, 0)].fg, accent);
        let dim = if enabled {
            Color::DarkGray
        } else {
            Color::Reset
        };
        assert_eq!(buffer[(4, 0)].fg, dim);
    }
}
