use std::error::Error;
use std::fmt::{Display, Formatter};

use crate::{CompensatedSum, ScientificImage};

/// Deterministic summary of usable samples in one scientific image or tile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageStatistics {
    total_samples: usize,
    usable_samples: usize,
    masked_samples: usize,
    non_finite_samples: usize,
    minimum: f64,
    maximum: f64,
    mean: f64,
    population_variance: f64,
    sample_variance: Option<f64>,
}

impl ImageStatistics {
    /// Total samples in the image, including rejected values.
    #[must_use]
    pub const fn total_samples(self) -> usize {
        self.total_samples
    }

    /// Unmasked finite samples included in every numerical result.
    #[must_use]
    pub const fn usable_samples(self) -> usize {
        self.usable_samples
    }

    /// Samples excluded because their quality flags were non-clear.
    #[must_use]
    pub const fn masked_samples(self) -> usize {
        self.masked_samples
    }

    /// Unmasked samples excluded because their value was NaN or infinite.
    #[must_use]
    pub const fn non_finite_samples(self) -> usize {
        self.non_finite_samples
    }

    /// Minimum usable value.
    #[must_use]
    pub const fn minimum(self) -> f64 {
        self.minimum
    }

    /// Maximum usable value.
    #[must_use]
    pub const fn maximum(self) -> f64 {
        self.maximum
    }

    /// Arithmetic mean of usable values.
    #[must_use]
    pub const fn mean(self) -> f64 {
        self.mean
    }

    /// Variance divided by the usable sample count.
    #[must_use]
    pub const fn population_variance(self) -> f64 {
        self.population_variance
    }

    /// Unbiased variance divided by `usable_samples - 1`, when defined.
    #[must_use]
    pub const fn sample_variance(self) -> Option<f64> {
        self.sample_variance
    }

    /// Population standard deviation.
    #[must_use]
    pub fn population_standard_deviation(self) -> f64 {
        self.population_variance.sqrt()
    }

    /// Sample standard deviation, when at least two samples are usable.
    #[must_use]
    pub fn sample_standard_deviation(self) -> Option<f64> {
        self.sample_variance.map(f64::sqrt)
    }
}

/// Failure to calculate a finite scientific summary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatisticsError {
    /// Every sample was masked or non-finite.
    NoUsableSamples {
        /// Total samples inspected.
        total: usize,
        /// Samples excluded by their mask.
        masked: usize,
        /// Unmasked samples excluded as non-finite.
        non_finite: usize,
    },
    /// The true variance is outside the finite `f64` result domain.
    VarianceOverflow,
}

impl Display for StatisticsError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoUsableSamples {
                total,
                masked,
                non_finite,
            } => write!(
                formatter,
                "no usable samples among {total}: {masked} masked and {non_finite} non-finite"
            ),
            Self::VarianceOverflow => {
                formatter.write_str("sample variance exceeds the finite f64 result domain")
            }
        }
    }
}

impl Error for StatisticsError {}

/// Computes strict reference statistics for all unmasked finite samples.
///
/// The implementation makes three deterministic planar-order passes. The first
/// establishes counts and a finite scale. The second accumulates values divided
/// by that scale and sample count with Neumaier compensation, avoiding overflow
/// in the mean. The third accumulates squared normalized deviations with the
/// same compensation before rescaling the variance.
///
/// A flagged value is counted as masked even when its payload is non-finite.
/// Unflagged NaN and infinity are counted separately and excluded. Positive and
/// negative zero results are canonicalized to positive zero.
///
/// # Errors
///
/// Returns [`StatisticsError::NoUsableSamples`] when no value can participate,
/// or [`StatisticsError::VarianceOverflow`] when the final variance cannot be
/// represented as finite `f64`.
pub fn image_statistics(image: &ScientificImage) -> Result<ImageStatistics, StatisticsError> {
    let mut usable_samples = 0_usize;
    let mut masked_samples = 0_usize;
    let mut non_finite_samples = 0_usize;
    let mut minimum = f64::INFINITY;
    let mut maximum = f64::NEG_INFINITY;
    let mut scale = 0.0_f64;

    for (value, flags) in image.pixels().iter().zip(image.mask().as_slice()) {
        if !flags.is_clear() {
            masked_samples += 1;
        } else if !value.is_finite() {
            non_finite_samples += 1;
        } else {
            usable_samples += 1;
            minimum = minimum.min(*value);
            maximum = maximum.max(*value);
            scale = scale.max(value.abs());
        }
    }

    if usable_samples == 0 {
        return Err(StatisticsError::NoUsableSamples {
            total: image.pixels().len(),
            masked: masked_samples,
            non_finite: non_finite_samples,
        });
    }

    let (mean, normalized_mean) = if scale == 0.0 {
        (0.0, 0.0)
    } else {
        let divisor = usable_samples as f64;
        let mut normalized_sum = CompensatedSum::new();
        for (value, flags) in image.pixels().iter().zip(image.mask().as_slice()) {
            if flags.is_clear() && value.is_finite() {
                normalized_sum.add((*value / scale) / divisor);
            }
        }
        // Roundoff cannot move a mathematical mean outside the observed range.
        // Clamping that final ulp protects multiplication by `f64::MAX` from a
        // spurious overflow when every sample is near the same extreme.
        let normalized_mean = normalized_sum
            .total()
            .max(minimum / scale)
            .min(maximum / scale);
        (canonical_zero(normalized_mean * scale), normalized_mean)
    };

    let (population_variance, sample_variance) = if scale == 0.0 {
        (0.0, (usable_samples > 1).then_some(0.0))
    } else {
        let mut squares = CompensatedSum::new();
        for (value, flags) in image.pixels().iter().zip(image.mask().as_slice()) {
            if flags.is_clear() && value.is_finite() {
                let deviation = (*value / scale) - normalized_mean;
                squares.add(deviation * deviation);
            }
        }
        // Every addend is non-negative. A negative zero or sub-ulp correction
        // has no physical meaning and is canonicalized before rescaling.
        let normalized_sum = squares.total().max(0.0);
        let population = rescale_variance(normalized_sum / usable_samples as f64, scale)?;
        let sample = if usable_samples > 1 {
            Some(rescale_variance(
                normalized_sum / (usable_samples - 1) as f64,
                scale,
            )?)
        } else {
            None
        };
        (population, sample)
    };

    Ok(ImageStatistics {
        total_samples: image.pixels().len(),
        usable_samples,
        masked_samples,
        non_finite_samples,
        minimum: canonical_zero(minimum),
        maximum: canonical_zero(maximum),
        mean,
        population_variance,
        sample_variance,
    })
}

fn rescale_variance(normalized: f64, scale: f64) -> Result<f64, StatisticsError> {
    let variance = scale * (scale * normalized);
    if variance.is_finite() {
        Ok(canonical_zero(variance))
    } else {
        Err(StatisticsError::VarianceOverflow)
    }
}

fn canonical_zero(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use crate::{Dimensions, PixelFlags};

    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn StdError>>;

    fn image(values: Vec<f64>) -> TestResult<ScientificImage> {
        let dimensions = Dimensions::new(values.len(), 1, 1)?;
        Ok(ScientificImage::from_pixels(dimensions, values)?)
    }

    #[test]
    fn computes_exact_reference_values_for_a_small_sequence() -> TestResult {
        let statistics = image_statistics(&image(vec![1.0, 2.0, 3.0, 4.0])?)?;

        assert_eq!(statistics.total_samples(), 4);
        assert_eq!(statistics.usable_samples(), 4);
        assert_eq!(statistics.minimum().to_bits(), 1.0_f64.to_bits());
        assert_eq!(statistics.maximum().to_bits(), 4.0_f64.to_bits());
        assert_eq!(statistics.mean().to_bits(), 2.5_f64.to_bits());
        assert_eq!(
            statistics.population_variance().to_bits(),
            1.25_f64.to_bits()
        );
        assert_eq!(statistics.sample_variance(), Some(5.0 / 3.0));
        assert_eq!(
            statistics.population_standard_deviation().to_bits(),
            1.25_f64.sqrt().to_bits()
        );
        assert_eq!(
            statistics.sample_standard_deviation(),
            Some((5.0_f64 / 3.0).sqrt())
        );
        Ok(())
    }

    #[test]
    fn scaled_mean_does_not_overflow_for_large_equal_values() -> TestResult {
        let statistics = image_statistics(&image(vec![f64::MAX, f64::MAX])?)?;

        assert_eq!(statistics.mean().to_bits(), f64::MAX.to_bits());
        assert_eq!(
            statistics.population_variance().to_bits(),
            0.0_f64.to_bits()
        );
        assert_eq!(statistics.sample_variance(), Some(0.0));
        Ok(())
    }

    #[test]
    fn separates_masked_and_unmasked_non_finite_samples() -> TestResult {
        let mut input = image(vec![1.0, f64::NAN, f64::INFINITY, 4.0])?;
        input.mask_mut().as_mut_slice()[2] = PixelFlags::INVALID;

        let statistics = image_statistics(&input)?;

        assert_eq!(statistics.total_samples(), 4);
        assert_eq!(statistics.usable_samples(), 2);
        assert_eq!(statistics.masked_samples(), 1);
        assert_eq!(statistics.non_finite_samples(), 1);
        assert_eq!(statistics.mean().to_bits(), 2.5_f64.to_bits());
        assert_eq!(
            statistics.population_variance().to_bits(),
            2.25_f64.to_bits()
        );
        Ok(())
    }

    #[test]
    fn rejects_images_without_usable_samples() -> TestResult {
        let mut input = image(vec![f64::NAN, 2.0])?;
        input.mask_mut().as_mut_slice()[1] = PixelFlags::MISSING;

        assert_eq!(
            image_statistics(&input),
            Err(StatisticsError::NoUsableSamples {
                total: 2,
                masked: 1,
                non_finite: 1,
            })
        );
        Ok(())
    }

    #[test]
    fn one_sample_has_population_but_not_sample_variance() -> TestResult {
        let statistics = image_statistics(&image(vec![42.0])?)?;

        assert_eq!(statistics.mean().to_bits(), 42.0_f64.to_bits());
        assert_eq!(
            statistics.population_variance().to_bits(),
            0.0_f64.to_bits()
        );
        assert_eq!(statistics.sample_variance(), None);
        assert_eq!(statistics.sample_standard_deviation(), None);
        Ok(())
    }

    #[test]
    fn reports_variance_outside_the_finite_result_domain() -> TestResult {
        let input = image(vec![f64::MAX, -f64::MAX])?;
        assert_eq!(
            image_statistics(&input),
            Err(StatisticsError::VarianceOverflow)
        );
        Ok(())
    }

    #[test]
    fn canonicalizes_signed_zero_results() -> TestResult {
        let statistics = image_statistics(&image(vec![-0.0, 0.0])?)?;

        assert_eq!(statistics.minimum().to_bits(), 0.0_f64.to_bits());
        assert_eq!(statistics.maximum().to_bits(), 0.0_f64.to_bits());
        assert_eq!(statistics.mean().to_bits(), 0.0_f64.to_bits());
        assert_eq!(
            statistics.population_variance().to_bits(),
            0.0_f64.to_bits()
        );
        Ok(())
    }
}
