use super::*;

#[test]
fn gantt_footers_list_navigation_and_end_with_the_help_key() {
    assert_eq!(
        footer(Context::Chart),
        "↑↓ select · ←→ pan · +/- zoom · f fit · tab inspect · ? for shortcuts"
    );
    assert_eq!(
        footer(Context::Inspector),
        "↑↓ scroll · tab chart · pgup/pgdn page · ? for shortcuts"
    );
    assert_eq!(
        footer(Context::Help),
        "↑↓ scroll · pgup/pgdn page · esc close"
    );
}

#[test]
fn pages_without_help_do_not_offer_the_help_key() {
    for context in [Context::Work, Context::Text] {
        let hints = footer(context);
        assert!(!hints.contains('?'), "{hints}");
        assert!(
            hints.contains("1–5 pages") && hints.ends_with("q quit"),
            "{hints}"
        );
    }
    assert!(footer(Context::Text).contains("pgup/pgdn page"));
    assert!(!footer(Context::Work).contains("pgup/pgdn"));
}

#[test]
fn every_footer_hint_is_lowercase_and_described_in_help() {
    let help: Vec<String> = help().collect();
    for shortcut in SHORTCUTS {
        assert_eq!(shortcut.action, shortcut.action.to_lowercase());
        assert_eq!(shortcut.keys, shortcut.keys.to_lowercase());
        assert!(
            help.iter()
                .any(|row| row.trim_start().starts_with(shortcut.keys)),
            "{} is not in help",
            shortcut.keys
        );
    }
}
