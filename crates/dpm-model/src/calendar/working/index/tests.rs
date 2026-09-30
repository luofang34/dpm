use super::*;

#[test]
fn ranks_match_a_linear_count() {
    let day = BUCKET;
    let values = vec![
        -3 * day,
        -3 * day,
        -1,
        0,
        day / 4,
        day - 1,
        day,
        7 * day,
        7 * day,
        7 * day + 1,
        30 * day,
    ];
    let ranked = Ranked::new(values.clone(), -4 * day - 17, 31 * day);
    let mut probe = -6 * day;
    while probe < 33 * day {
        for t in [probe - 1, probe, probe + 1] {
            let below = values.iter().filter(|v| **v < t).count();
            let through = values.iter().filter(|v| **v <= t).count();
            assert_eq!(
                (ranked.below(t), ranked.through(t)),
                (below, through),
                "{t}"
            );
        }
        probe += day / 8;
    }
}
