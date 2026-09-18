/// Neumaier accumulator for double-precision sums.
///
/// Compensation recovers some bits lost when a small signal is added to a much
/// larger value. It does not make floating-point addition associative, so the
/// input order must remain deterministic.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CompensatedSum {
    sum: f64,
    correction: f64,
}

impl CompensatedSum {
    /// Creates a zero-valued accumulator.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            sum: 0.0,
            correction: 0.0,
        }
    }

    /// Adds one value to the accumulator.
    ///
    /// NaNs and infinities are not filtered: they propagate according to IEEE
    /// 754. Callers must apply the validity mask before invoking this method.
    pub fn add(&mut self, value: f64) {
        // The compensation formula contains subtractions. With an infinity,
        // `inf - inf` would produce NaN even when ordinary IEEE 754 addition
        // should remain infinite. Direct addition preserves the expected
        // semantics for non-finite values here.
        if !self.sum.is_finite() || !self.correction.is_finite() || !value.is_finite() {
            self.sum = (self.sum + self.correction) + value;
            self.correction = 0.0;
            return;
        }

        let next = self.sum + value;
        if self.sum.abs() >= value.abs() {
            self.correction += (self.sum - next) + value;
        } else {
            self.correction += (value - next) + self.sum;
        }
        self.sum = next;
    }

    /// Corrected sum value.
    #[must_use]
    pub fn total(self) -> f64 {
        self.sum + self.correction
    }
}

impl Extend<f64> for CompensatedSum {
    fn extend<I: IntoIterator<Item = f64>>(&mut self, values: I) {
        for value in values {
            self.add(value);
        }
    }
}

impl FromIterator<f64> for CompensatedSum {
    fn from_iter<I: IntoIterator<Item = f64>>(values: I) -> Self {
        let mut accumulator = Self::new();
        accumulator.extend(values);
        accumulator
    }
}

/// Sums a sequence using Neumaier compensation.
#[must_use]
pub fn compensated_sum(values: impl IntoIterator<Item = f64>) -> f64 {
    values.into_iter().collect::<CompensatedSum>().total()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_small_signal_during_catastrophic_cancellation() {
        let naive: f64 = [1.0e16, 1.0, -1.0e16].into_iter().sum();
        let compensated = compensated_sum([1.0e16, 1.0, -1.0e16]);

        assert_eq!(naive.to_bits(), 0.0_f64.to_bits());
        assert_eq!(compensated.to_bits(), 1.0_f64.to_bits());
    }

    #[test]
    fn propagates_non_finite_values() {
        assert!(compensated_sum([1.0, f64::NAN]).is_nan());
        let positive_infinity = compensated_sum([1.0, f64::INFINITY]);
        assert!(positive_infinity.is_infinite() && positive_infinity.is_sign_positive());
        assert!(compensated_sum([f64::INFINITY, f64::NEG_INFINITY]).is_nan());
    }

    #[test]
    fn empty_sum_is_zero() {
        assert_eq!(compensated_sum([]).to_bits(), 0.0_f64.to_bits());
    }
}
