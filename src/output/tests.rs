use super::*;
use dpm_model::Key;

fn keys(names: &[&str]) -> Vec<Key> {
    names.iter().map(|n| Key((*n).into())).collect()
}

#[test]
fn estimated_plans_print_no_unestimated_line() {
    assert_eq!(unestimated_line(&[]), None);
}

#[test]
fn unestimated_line_names_the_count_and_truncates_long_lists() {
    assert_eq!(
        unestimated_line(&keys(&["A", "B"])).as_deref(),
        Some("Unestimated: 2 task(s) count as 0 h, so the forecast is optimistic: A, B")
    );
    let many = keys(&["A", "B", "C", "D", "E", "F", "G"]);
    assert_eq!(
        unestimated_line(&many).as_deref(),
        Some(
            "Unestimated: 7 task(s) count as 0 h, so the forecast is optimistic: A, B, C, D, E (+2 more)"
        )
    );
}
