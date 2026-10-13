//! Pass rates and their bootstrap confidence interval. Deterministic: the
//! resampler is a seeded SplitMix64, so the same outcomes and seed give the
//! same interval on every machine and every run.

pub const DEFAULT_SEED: u64 = 0x5EED_A11E_7E57;
pub const DEFAULT_RESAMPLES: usize = 2000;
pub const MAX_RESAMPLES: usize = 20_000;

/// SplitMix64 (Steele, Lea, Flood 2014): tiny, seedable, good enough to
/// pick resample indices.
pub struct SplitMix64(u64);

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in `0..n` (`n > 0`), without modulo bias.
    pub fn below(&mut self, n: usize) -> usize {
        let n = n as u64;
        let zone = u64::MAX - (u64::MAX % n);
        loop {
            let x = self.next_u64();
            if x < zone {
                return (x % n) as usize;
            }
        }
    }
}

pub fn rate(outcomes: &[bool]) -> Option<f64> {
    (!outcomes.is_empty()).then(|| outcomes.iter().filter(|x| **x).count() as f64 / outcomes.len() as f64)
}

/// Percentile bootstrap 95% interval of the pass rate: resample the cases
/// with replacement `resamples` times. `None` for no cases.
pub fn bootstrap_ci95(outcomes: &[bool], seed: u64, resamples: usize) -> Option<(f64, f64)> {
    let n = outcomes.len();
    if n == 0 {
        return None;
    }
    let b = resamples.clamp(100, MAX_RESAMPLES);
    let mut rng = SplitMix64::new(seed);
    let mut rates: Vec<f64> = (0..b)
        .map(|_| (0..n).filter(|_| outcomes[rng.below(n)]).count() as f64 / n as f64)
        .collect();
    rates.sort_by(|a, b| a.total_cmp(b));
    let lo = rates[((0.025 * b as f64).floor() as usize).min(b - 1)];
    let hi = rates[((0.975 * b as f64).ceil() as usize).saturating_sub(1).min(b - 1)];
    Some((round4(lo), round4(hi)))
}

pub fn round4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}
