//! Seeded, dependency-free random draws for the Monte Carlo projection.
//!
//! Every draw comes from one `XorShift64` stream, so a seed reproduces the same samples for the
//! same plan, and no hidden state (such as a cached second normal) is needed to replay a simulation
//! step by step. Transcendental functions come from `libm` rather than the platform math library,
//! so the samples are bit-identical on every device; clippy's disallowed methods keep it that way.

use crate::network::EPSILON;
use dpm_model::ThreePointEstimate;

/// Upper bound on Marsaglia–Tsang proposals per gamma draw.
///
/// Each proposal is accepted with probability above 0.95 for the shapes used here, so the bound is
/// never reached in practice; it only guarantees termination for any generator state.
const GAMMA_ATTEMPTS: usize = 64;

/// Shape weight of the most-likely value in the standard beta-PERT distribution.
///
/// With this weight the distribution's mean is exactly the PERT expectation `(O + 4M + P) / 6`
/// that the deterministic projection uses.
const PERT_SHAPE_WEIGHT: f64 = 4.0;

#[derive(Debug, Clone, Copy)]
pub(crate) struct XorShift64(u64);

impl XorShift64 {
    pub(crate) fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// 53 random bits, uniform in [0, 1).
    pub(crate) fn unit_f64(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64) * (1.0 / ((1_u64 << 53) as f64))
    }

    /// Uniform in (0, 1], so its logarithm is always finite.
    fn open_unit(&mut self) -> f64 {
        1.0 - self.unit_f64()
    }

    /// Box–Muller standard normal; the second value of the pair is discarded so the generator
    /// state alone determines every later draw.
    fn standard_normal(&mut self) -> f64 {
        let radius = (-2.0 * libm::log(self.open_unit())).sqrt();
        radius * libm::cos(std::f64::consts::TAU * self.unit_f64())
    }

    /// Marsaglia–Tsang gamma draw with unit scale for `shape >= 1`.
    ///
    /// Should every proposal be rejected, the draw is the distribution's mean, `shape`.
    fn gamma(&mut self, shape: f64) -> f64 {
        let d = shape - 1.0 / 3.0;
        let c = 1.0 / (9.0 * d).sqrt();
        for _ in 0..GAMMA_ATTEMPTS {
            let x = self.standard_normal();
            let base = 1.0 + c * x;
            let v = base * base * base;
            if v <= 0.0 {
                continue;
            }
            let u = self.open_unit();
            let x2 = x * x;
            if u < 1.0 - 0.0331 * x2 * x2 || libm::log(u) < 0.5 * x2 + d * (1.0 - v + libm::log(v))
            {
                return d * v;
            }
        }
        shape
    }
}

/// Beta-PERT distribution of one three-point estimate with a non-degenerate range.
///
/// Its shapes are `alpha = 1 + 4(M - O)/(P - O)` and `beta = 1 + 4(P - M)/(P - O)`, both in
/// [1, 5], so the density on [O, P] is bounded and its mean is the PERT expectation.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BetaPert {
    optimistic: f64,
    pessimistic: f64,
    range: f64,
    alpha: f64,
    beta: f64,
}

impl BetaPert {
    /// The distribution of a validated estimate, or `None` when its range is within tolerance of
    /// zero and the duration is effectively exact.
    pub(crate) fn new(estimate: ThreePointEstimate) -> Option<Self> {
        let range = estimate.pessimistic_hours - estimate.optimistic_hours;
        if range <= EPSILON {
            return None;
        }
        let low = (estimate.likely_hours - estimate.optimistic_hours) / range;
        let high = (estimate.pessimistic_hours - estimate.likely_hours) / range;
        Some(Self {
            optimistic: estimate.optimistic_hours,
            pessimistic: estimate.pessimistic_hours,
            range,
            alpha: 1.0 + PERT_SHAPE_WEIGHT * low.clamp(0.0, 1.0),
            beta: 1.0 + PERT_SHAPE_WEIGHT * high.clamp(0.0, 1.0),
        })
    }

    /// One sampled duration in elapsed hours, inside [O, P].
    pub(crate) fn sample(&self, rng: &mut XorShift64) -> f64 {
        let a = rng.gamma(self.alpha);
        let b = rng.gamma(self.beta);
        self.hours_at(a / (a + b))
    }

    /// Duration at `fraction` of the range from the optimistic bound, kept inside [O, P].
    pub(crate) fn hours_at(&self, fraction: f64) -> f64 {
        (self.optimistic + self.range * fraction).clamp(self.optimistic, self.pessimistic)
    }

    /// Fraction of the range that `hours` lies above the optimistic bound, unclamped.
    pub(crate) fn fraction_of(&self, hours: f64) -> f64 {
        (hours - self.optimistic) / self.range
    }

    /// Density at `fraction` of the range, up to a constant factor.
    pub(crate) fn relative_density(&self, fraction: f64) -> f64 {
        libm::pow(fraction, self.alpha - 1.0) * libm::pow(1.0 - fraction, self.beta - 1.0)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
