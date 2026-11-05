//! A tiny deterministic random number generator.

/// SplitMix64 (Steele, Lea, Flood 2014): a 64-bit generator with one word of state.
///
/// The sequence for a given seed is fixed by the algorithm, so shuffles and splits are identical on
/// every platform. Not suitable for cryptography.
#[derive(Debug, Clone)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// Creates a generator from `seed`.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Returns the next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Returns a uniform value in `[0, 1)` with 53 random bits.
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0)
    }

    /// Returns a value in `0..bound` (multiply-shift reduction, bias below `bound / 2^64`).
    /// Returns 0 when `bound` is 0.
    pub fn next_below(&mut self, bound: usize) -> usize {
        let wide = u128::from(self.next_u64()) * (bound as u128);
        (wide >> 64) as usize
    }

    /// Fisher-Yates shuffle of `items`, walking from the back.
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.next_below(i + 1);
            items.swap(i, j);
        }
    }
}
