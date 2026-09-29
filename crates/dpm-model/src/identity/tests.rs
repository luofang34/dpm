use super::Key;
use std::cmp::Ordering;

fn sorted(keys: &[&str]) -> Vec<String> {
    let mut keys: Vec<Key> = keys.iter().map(|k| Key::new(*k)).collect();
    keys.sort_by(Key::natural_cmp);
    keys.into_iter().map(|k| k.0).collect()
}

#[test]
fn digit_runs_compare_by_value() {
    assert_eq!(
        sorted(&["OP-10", "OP-2", "OP-1", "OP-100", "OP-20"]),
        ["OP-1", "OP-2", "OP-10", "OP-20", "OP-100"]
    );
    assert_eq!(
        sorted(&["WP-10-X", "WP-2-CORE", "WP-2-AUTH", "M10", "M9"]),
        ["M9", "M10", "WP-2-AUTH", "WP-2-CORE", "WP-10-X"]
    );
}

#[test]
fn order_is_total_and_consistent_with_equality() {
    let pairs = [
        ("T-7", "T-07"),
        ("A", "A1"),
        ("A1", "A-1"),
        ("", "0"),
        ("x", "x"),
    ];
    for (a, b) in pairs {
        let (a, b) = (Key::new(a), Key::new(b));
        assert_eq!(a.natural_cmp(&b), b.natural_cmp(&a).reverse(), "{a} {b}");
        assert_eq!(a.natural_cmp(&b) == Ordering::Equal, a == b, "{a} {b}");
    }
    assert_eq!(sorted(&["T-07", "T-7", "T-8"]), ["T-07", "T-7", "T-8"]);
}

#[test]
fn operation_and_lineage_identities_are_minted_time_ordered() {
    let first = super::OperationId::new();
    let second = super::OperationId::new();
    assert!(first.is_time_ordered() && second.is_time_ordered());
    assert!(first <= second, "v7 identities sort by creation");
    assert_eq!(super::LineageId::new().0.get_version_num(), 7);
    assert!(!super::OperationId(uuid::Uuid::new_v4()).is_time_ordered());
}
