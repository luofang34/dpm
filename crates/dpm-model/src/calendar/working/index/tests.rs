use super::*;

#[test]
fn ranks_match_a_linear_count() {
    let values = vec![-3.5, -3.5, 0.0, 0.25, 0.75, 1.0, 7.0, 7.0, 7.5, 30.0];
    let ranked = Ranked::new(values.clone(), -4.0, 31.0);
    let mut probe = -6.0;
    while probe < 33.0 {
        let below = values.iter().filter(|v| **v < probe).count();
        let through = values.iter().filter(|v| **v <= probe).count();
        assert_eq!(
            (ranked.below(probe), ranked.through(probe)),
            (below, through),
            "{probe}"
        );
        probe += 0.125;
    }
}
