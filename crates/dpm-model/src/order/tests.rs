use super::*;

#[test]
fn insertions_do_not_change_neighbours_even_at_prefix_boundaries() {
    for (left, right) in [
        (None, None),
        (None, Some(SiblingOrder(vec![0, 1]))),
        (Some(SiblingOrder(vec![65535])), None),
        (Some(SiblingOrder(vec![1])), Some(SiblingOrder(vec![1, 1]))),
        (
            Some(SiblingOrder(vec![1, 65535])),
            Some(SiblingOrder(vec![2])),
        ),
    ] {
        let inserted = SiblingOrder::between(left.as_ref(), right.as_ref()).expect("position");
        assert!(inserted.is_valid());
        assert!(left.as_ref().is_none_or(|v| v < &inserted));
        assert!(right.as_ref().is_none_or(|v| &inserted < v));
    }
}

#[test]
fn repeated_insertions_keep_strict_order() {
    let left = SiblingOrder::default();
    let mut right = SiblingOrder(vec![32769]);
    for _ in 0..1000 {
        let next = SiblingOrder::between(Some(&left), Some(&right)).expect("position");
        assert!(left < next && next < right);
        right = next;
    }
}

#[test]
fn invalid_or_exhausted_bounds_are_explicit() {
    let one = SiblingOrder(vec![1]);
    assert_eq!(
        SiblingOrder::between(Some(&one), Some(&one)),
        Err(OrderError::InvalidBounds)
    );
    assert_eq!(
        SiblingOrder::between(None, Some(&SiblingOrder(vec![0]))),
        Err(OrderError::InvalidBounds)
    );
    let tiny = SiblingOrder([vec![0; 127], vec![1]].concat());
    assert_eq!(
        SiblingOrder::between(None, Some(&tiny)),
        Err(OrderError::Exhausted)
    );
}

#[test]
fn appending_large_outlines_does_not_exhaust_position_depth() {
    let mut last = SiblingOrder::default();
    for _ in 0..100_000 {
        let next = SiblingOrder::between(Some(&last), None).expect("append position");
        assert!(next > last && next.0.len() <= 3);
        last = next;
    }
}
