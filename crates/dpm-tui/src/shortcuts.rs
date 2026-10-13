//! The console's key table. The footer hint line and the Gantt help both read it, so a hint never
//! names a key the help does not describe. The handlers that act on these keys live in the view and
//! the Gantt; a key added there is added here too.

/// Where keyboard focus is, which decides the hints the footer offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Context {
    /// The Gantt chart rows.
    Chart,
    /// The Gantt inspector, focused with Tab.
    Inspector,
    /// The Gantt help overlay.
    Help,
    /// The Work list.
    Work,
    /// Now, Network and Detail: one scrollable text pane.
    Text,
}

/// One key binding as the operator reads it.
pub(crate) struct Shortcut {
    /// Keys as printed, lowercase.
    pub(crate) keys: &'static str,
    /// Lowercase action the footer prints after the keys.
    pub(crate) action: &'static str,
    /// What help says the keys do; empty for an entry that only rephrases another for the footer.
    pub(crate) help: &'static str,
    /// Contexts whose footer offers this hint.
    pub(crate) footer: &'static [Context],
}

use Context::{Chart, Help, Inspector, Text, Work};

/// Every binding, in the order hints and help list them.
pub(crate) const SHORTCUTS: &[Shortcut] = &[
    Shortcut {
        keys: "↑↓",
        action: "select",
        help: "select work (also j/k)",
        footer: &[Chart, Work, Text],
    },
    Shortcut {
        keys: "↑↓",
        action: "scroll",
        help: "scroll the focused inspector or this help (also j/k)",
        footer: &[Inspector, Help],
    },
    Shortcut {
        keys: "←→",
        action: "pan",
        help: "pan the timeline",
        footer: &[Chart],
    },
    Shortcut {
        keys: "+/-",
        action: "zoom",
        help: "zoom the timeline in or out",
        footer: &[Chart],
    },
    Shortcut {
        keys: "f",
        action: "fit",
        help: "fit the whole timeline",
        footer: &[Chart],
    },
    Shortcut {
        keys: "home/end",
        action: "start/end",
        help: "timeline start or end; in the inspector or help, top or end of the text",
        footer: &[],
    },
    Shortcut {
        keys: "tab",
        action: "inspect",
        help: "move focus between the chart and the inspector",
        footer: &[Chart],
    },
    Shortcut {
        keys: "tab",
        action: "chart",
        help: "",
        footer: &[Inspector],
    },
    Shortcut {
        keys: "pgup/pgdn",
        action: "page",
        help: "page the inspector from either focus; page Now, Network and Detail text",
        footer: &[Inspector, Help, Text],
    },
    Shortcut {
        keys: "enter",
        action: "detail",
        help: "full task Detail of the selected work (also 4)",
        footer: &[Work, Text],
    },
    Shortcut {
        keys: "1–5",
        action: "pages",
        help: "Now, Work, Network, Detail, Gantt (g also opens Gantt)",
        footer: &[Work, Text],
    },
    Shortcut {
        keys: "esc",
        action: "close",
        help: "close help, or return inspector focus to the chart; otherwise quit",
        footer: &[Help],
    },
    Shortcut {
        keys: "?",
        action: "for shortcuts",
        help: "toggle this help",
        footer: &[Chart, Inspector],
    },
    Shortcut {
        keys: "r",
        action: "reload",
        help: "reload a fresh snapshot; navigation never changes the plan",
        footer: &[Work, Text],
    },
    Shortcut {
        keys: "q",
        action: "quit",
        help: "quit (ctrl+c also quits)",
        footer: &[Work, Text],
    },
];

/// The footer hint line for `context`: lowercase `keys action` pairs joined with ` · `.
pub(crate) fn footer(context: Context) -> String {
    SHORTCUTS
        .iter()
        .filter(|shortcut| shortcut.footer.contains(&context))
        .map(|shortcut| format!("{} {}", shortcut.keys, shortcut.action))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Help rows: each binding's keys in a column, then what they do.
pub(crate) fn help() -> impl Iterator<Item = String> {
    SHORTCUTS
        .iter()
        .filter(|shortcut| !shortcut.help.is_empty())
        .map(|shortcut| format!("  {:<10} {}", shortcut.keys, shortcut.help))
}

#[cfg(test)]
mod tests;
