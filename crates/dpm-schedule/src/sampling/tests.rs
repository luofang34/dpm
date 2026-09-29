use super::*;

fn estimate(
    optimistic_hours: f64,
    likely_hours: f64,
    pessimistic_hours: f64,
) -> ThreePointEstimate {
    ThreePointEstimate {
        optimistic_hours,
        likely_hours,
        pessimistic_hours,
    }
}

fn mean_of(distribution: &BetaPert, seed: u64, count: usize) -> f64 {
    let mut rng = XorShift64::new(seed);
    (0..count)
        .map(|_| distribution.sample(&mut rng))
        .sum::<f64>()
        / count as f64
}

#[test]
fn sampled_mean_matches_the_pert_expectation_used_by_cpm() {
    // 1/2/9 h: the PERT expectation is 3 h, a triangular distribution's mean would be 4 h, and the
    // beta-PERT standard deviation is about 1.3 h, so 100 000 samples resolve the mean to ~0.004 h.
    for (o, m, p) in [
        (1.0, 2.0, 9.0),
        (4.0, 8.0, 12.0),
        (0.0, 0.0, 10.0),
        (0.0, 10.0, 10.0),
        (12.0, 15.0, 24.0),
    ] {
        let estimate = estimate(o, m, p);
        let distribution = BetaPert::new(estimate).expect("non-degenerate");
        let mean = mean_of(&distribution, 0x05CE_0040, 100_000);
        let tolerance = 0.005 * (p - o);
        assert!(
            (mean - estimate.pert_expected_hours()).abs() < tolerance,
            "{o}/{m}/{p}: sampled mean {mean} vs PERT {}",
            estimate.pert_expected_hours()
        );
    }
}

#[test]
fn sampled_spread_matches_the_beta_pert_variance() {
    // Shapes 1.5 and 4.5 for 1/2/9 h: variance (P - O)^2 * ab / ((a + b)^2 (a + b + 1)) = 1.714 h^2.
    let distribution = BetaPert::new(estimate(1.0, 2.0, 9.0)).expect("non-degenerate");
    let mut rng = XorShift64::new(99);
    let samples: Vec<f64> = (0..100_000)
        .map(|_| distribution.sample(&mut rng))
        .collect();
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    let variance =
        samples.iter().map(|s| (s - mean) * (s - mean)).sum::<f64>() / (samples.len() - 1) as f64;
    assert!((variance - 64.0 * 6.75 / 252.0).abs() < 0.05, "{variance}");
}

#[test]
fn gamma_draws_have_the_shape_as_their_mean() {
    let mut rng = XorShift64::new(7);
    for shape in [1.0, 2.5, 5.0] {
        let mean = (0..50_000).map(|_| rng.gamma(shape)).sum::<f64>() / 50_000.0;
        assert!((mean - shape).abs() < 0.03 * shape, "{shape}: {mean}");
    }
}

#[test]
fn seeded_draws_are_reproducible() {
    let distribution = BetaPert::new(estimate(2.0, 3.0, 11.0)).expect("non-degenerate");
    let draw = |seed| {
        let mut rng = XorShift64::new(seed);
        (0..32)
            .map(|_| distribution.sample(&mut rng))
            .collect::<Vec<_>>()
    };
    assert_eq!(draw(5), draw(5));
    assert_ne!(draw(5), draw(6));
}

#[test]
fn degenerate_estimates_have_no_distribution() {
    assert!(BetaPert::new(estimate(4.0, 4.0, 4.0)).is_none());
    assert!(BetaPert::new(estimate(4.0, 4.0, 4.0 + EPSILON / 2.0)).is_none());
}

#[test]
fn extreme_finite_samples_stay_inside_estimate_bounds() {
    let mut rng = XorShift64::new(0);
    let distribution =
        BetaPert::new(estimate(0.0, f64::MAX / 2.0, f64::MAX)).expect("non-degenerate");
    for _ in 0..1000 {
        let sample = distribution.sample(&mut rng);
        assert!(sample.is_finite());
        assert!((0.0..=f64::MAX).contains(&sample));
    }
    let narrow = BetaPert::new(estimate(3.0, 3.0, 5.0)).expect("non-degenerate");
    for _ in 0..1000 {
        assert!((3.0..=5.0).contains(&narrow.sample(&mut rng)));
    }
}
