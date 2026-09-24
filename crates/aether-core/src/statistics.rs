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
    /// A repeated streaming pass did not contain the same usable samples.
    PassSampleMismatch {
        /// Usable finite samples established by the first pass.
        expected: usize,
        /// Usable finite samples observed by the repeated pass.
        actual: usize,
    },
    /// Accumulated sample accounting overflowed the platform size domain.
    SampleCountOverflow,
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
            Self::PassSampleMismatch { expected, actual } => write!(
                formatter,
                "statistics pass observed {actual} usable samples; expected {expected}"
            ),
            Self::SampleCountOverflow => formatter.write_str("statistics sample count overflows"),
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
    let mut first = StatisticsFirstPass::new();
    first.observe_image(image)?;
    let mut mean = first.finish()?;
    mean.observe_image(image)?;
    let mut variance = mean.finish()?;
    variance.observe_image(image)?;
    variance.finish()
}

/// Order-independent first pass for bounded or tiled image statistics.
///
/// This pass establishes validity counts, extrema, and a finite scale. Those
/// quantities are independent of chunk boundaries and traversal order. The
/// following mean and variance passes remain order-sensitive and must observe
/// finite values in the canonical image order.
#[derive(Clone, Copy, Debug)]
pub struct StatisticsFirstPass {
    total_samples: usize,
    usable_samples: usize,
    masked_samples: usize,
    non_finite_samples: usize,
    minimum: f64,
    maximum: f64,
    scale: f64,
}

impl Default for StatisticsFirstPass {
    fn default() -> Self {
        Self::new()
    }
}

impl StatisticsFirstPass {
    /// Creates an empty first-pass accumulator.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            total_samples: 0,
            usable_samples: 0,
            masked_samples: 0,
            non_finite_samples: 0,
            minimum: f64::INFINITY,
            maximum: f64::NEG_INFINITY,
            scale: 0.0,
        }
    }

    /// Adds one internally consistent scientific image or bounded image chunk.
    ///
    /// # Errors
    ///
    /// Returns [`StatisticsError::SampleCountOverflow`] if aggregate accounting
    /// cannot be represented by `usize`.
    pub fn observe_image(&mut self, image: &ScientificImage) -> Result<(), StatisticsError> {
        self.total_samples = self
            .total_samples
            .checked_add(image.pixels().len())
            .ok_or(StatisticsError::SampleCountOverflow)?;

        for (value, flags) in image.pixels().iter().zip(image.mask().as_slice()) {
            if !flags.is_clear() {
                self.masked_samples = self
                    .masked_samples
                    .checked_add(1)
                    .ok_or(StatisticsError::SampleCountOverflow)?;
            } else if !value.is_finite() {
                self.non_finite_samples = self
                    .non_finite_samples
                    .checked_add(1)
                    .ok_or(StatisticsError::SampleCountOverflow)?;
            } else {
                self.usable_samples = self
                    .usable_samples
                    .checked_add(1)
                    .ok_or(StatisticsError::SampleCountOverflow)?;
                self.minimum = self.minimum.min(*value);
                self.maximum = self.maximum.max(*value);
                self.scale = self.scale.max(value.abs());
            }
        }
        Ok(())
    }

    /// Adds consecutive physical values without a separate quality mask.
    ///
    /// This is the bounded-stream counterpart used by FITS readers. Every
    /// finite value is usable and every NaN or infinity is counted as an
    /// unmasked non-finite sample. Callers that need distinct mask accounting
    /// must use [`Self::observe_image`] for the first pass.
    ///
    /// # Errors
    ///
    /// Returns [`StatisticsError::SampleCountOverflow`] if aggregate accounting
    /// cannot be represented by `usize`.
    pub fn observe_values(&mut self, values: &[f64]) -> Result<(), StatisticsError> {
        self.total_samples = self
            .total_samples
            .checked_add(values.len())
            .ok_or(StatisticsError::SampleCountOverflow)?;
        let mut usable_samples = 0_usize;
        let mut non_finite_samples = 0_usize;
        for value in values {
            if value.is_finite() {
                usable_samples += 1;
                self.minimum = self.minimum.min(*value);
                self.maximum = self.maximum.max(*value);
                self.scale = self.scale.max(value.abs());
            } else {
                non_finite_samples += 1;
            }
        }
        self.usable_samples = self
            .usable_samples
            .checked_add(usable_samples)
            .ok_or(StatisticsError::SampleCountOverflow)?;
        self.non_finite_samples = self
            .non_finite_samples
            .checked_add(non_finite_samples)
            .ok_or(StatisticsError::SampleCountOverflow)?;
        Ok(())
    }

    /// Freezes first-pass accounting and starts the canonical-order mean pass.
    ///
    /// # Errors
    ///
    /// Returns [`StatisticsError::NoUsableSamples`] when every observed sample
    /// was masked or non-finite.
    pub fn finish(self) -> Result<StatisticsMeanPass, StatisticsError> {
        if self.usable_samples == 0 {
            return Err(StatisticsError::NoUsableSamples {
                total: self.total_samples,
                masked: self.masked_samples,
                non_finite: self.non_finite_samples,
            });
        }
        Ok(StatisticsMeanPass {
            first: self,
            normalized_sum: CompensatedSum::new(),
            observed_usable: 0,
        })
    }
}

/// Second statistics pass accumulating the scaled mean in canonical order.
#[derive(Clone, Copy, Debug)]
pub struct StatisticsMeanPass {
    first: StatisticsFirstPass,
    normalized_sum: CompensatedSum,
    observed_usable: usize,
}

impl StatisticsMeanPass {
    /// Adds one image in canonical order while honoring its quality mask.
    ///
    /// # Errors
    ///
    /// Returns [`StatisticsError::SampleCountOverflow`] on count overflow.
    pub fn observe_image(&mut self, image: &ScientificImage) -> Result<(), StatisticsError> {
        for (value, flags) in image.pixels().iter().zip(image.mask().as_slice()) {
            if flags.is_clear() && value.is_finite() {
                self.observe_finite(*value)?;
            }
        }
        Ok(())
    }

    /// Adds the next consecutive values from the canonical image stream.
    ///
    /// Masked output values may be represented by non-finite sentinels; every
    /// non-finite value is excluded. Chunk boundaries do not affect the exact
    /// accumulation order of the remaining finite samples.
    ///
    /// # Errors
    ///
    /// Returns [`StatisticsError::SampleCountOverflow`] on count overflow.
    pub fn observe_values(&mut self, values: &[f64]) -> Result<(), StatisticsError> {
        for value in values.iter().copied().filter(|value| value.is_finite()) {
            self.observe_finite(value)?;
        }
        Ok(())
    }

    fn observe_finite(&mut self, value: f64) -> Result<(), StatisticsError> {
        self.observed_usable = self
            .observed_usable
            .checked_add(1)
            .ok_or(StatisticsError::SampleCountOverflow)?;
        if self.first.scale != 0.0 {
            let divisor = self.first.usable_samples as f64;
            self.normalized_sum
                .add((value / self.first.scale) / divisor);
        }
        Ok(())
    }

    /// Validates the repeated sample count and starts the variance pass.
    ///
    /// # Errors
    ///
    /// Returns [`StatisticsError::PassSampleMismatch`] if the repeated stream
    /// differs in finite-sample count from the first pass.
    pub fn finish(self) -> Result<StatisticsVariancePass, StatisticsError> {
        if self.observed_usable != self.first.usable_samples {
            return Err(StatisticsError::PassSampleMismatch {
                expected: self.first.usable_samples,
                actual: self.observed_usable,
            });
        }
        let normalized_mean = if self.first.scale == 0.0 {
            0.0
        } else {
            self.normalized_sum
                .total()
                .max(self.first.minimum / self.first.scale)
                .min(self.first.maximum / self.first.scale)
        };
        Ok(StatisticsVariancePass {
            first: self.first,
            normalized_mean,
            squares: CompensatedSum::new(),
            observed_usable: 0,
        })
    }
}

/// Third statistics pass accumulating scaled squared deviations.
#[derive(Clone, Copy, Debug)]
pub struct StatisticsVariancePass {
    first: StatisticsFirstPass,
    normalized_mean: f64,
    squares: CompensatedSum,
    observed_usable: usize,
}

impl StatisticsVariancePass {
    /// Adds one image in canonical order while honoring its quality mask.
    ///
    /// # Errors
    ///
    /// Returns [`StatisticsError::SampleCountOverflow`] on count overflow.
    pub fn observe_image(&mut self, image: &ScientificImage) -> Result<(), StatisticsError> {
        for (value, flags) in image.pixels().iter().zip(image.mask().as_slice()) {
            if flags.is_clear() && value.is_finite() {
                self.observe_finite(*value)?;
            }
        }
        Ok(())
    }

    /// Adds the next consecutive values from the same canonical image stream.
    ///
    /// # Errors
    ///
    /// Returns [`StatisticsError::SampleCountOverflow`] on count overflow.
    pub fn observe_values(&mut self, values: &[f64]) -> Result<(), StatisticsError> {
        for value in values.iter().copied().filter(|value| value.is_finite()) {
            self.observe_finite(value)?;
        }
        Ok(())
    }

    fn observe_finite(&mut self, value: f64) -> Result<(), StatisticsError> {
        self.observed_usable = self
            .observed_usable
            .checked_add(1)
            .ok_or(StatisticsError::SampleCountOverflow)?;
        if self.first.scale != 0.0 {
            let deviation = (value / self.first.scale) - self.normalized_mean;
            self.squares.add(deviation * deviation);
        }
        Ok(())
    }

    /// Completes the three-pass finite statistics calculation.
    ///
    /// # Errors
    ///
    /// Returns a pass mismatch if the stream changed, or a variance overflow
    /// when the finite result lies outside the `f64` domain.
    pub fn finish(self) -> Result<ImageStatistics, StatisticsError> {
        if self.observed_usable != self.first.usable_samples {
            return Err(StatisticsError::PassSampleMismatch {
                expected: self.first.usable_samples,
                actual: self.observed_usable,
            });
        }

        let mean = canonical_zero(self.normalized_mean * self.first.scale);
        let (population_variance, sample_variance) = if self.first.scale == 0.0 {
            (0.0, (self.first.usable_samples > 1).then_some(0.0))
        } else {
            let normalized_sum = self.squares.total().max(0.0);
            let population = rescale_variance(
                normalized_sum / self.first.usable_samples as f64,
                self.first.scale,
            )?;
            let sample = if self.first.usable_samples > 1 {
                Some(rescale_variance(
                    normalized_sum / (self.first.usable_samples - 1) as f64,
                    self.first.scale,
                )?)
            } else {
                None
            };
            (population, sample)
        };

        Ok(ImageStatistics {
            total_samples: self.first.total_samples,
            usable_samples: self.first.usable_samples,
            masked_samples: self.first.masked_samples,
            non_finite_samples: self.first.non_finite_samples,
            minimum: canonical_zero(self.first.minimum),
            maximum: canonical_zero(self.first.maximum),
            mean,
            population_variance,
            sample_variance,
        })
    }
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

    #[test]
    fn streamed_passes_match_the_in_memory_oracle_across_chunk_boundaries() -> TestResult {
        let dimensions = Dimensions::new(7, 1, 1)?;
        let mut complete = ScientificImage::from_pixels(
            dimensions,
            vec![1.0e16, 1.0, -1.0e16, 4.0, f64::NAN, 6.0, 9.0],
        )?;
        complete.mask_mut().as_mut_slice()[3] = PixelFlags::REJECTED;
        let expected = image_statistics(&complete)?;

        let mut first_chunk = ScientificImage::from_pixels(
            Dimensions::new(3, 1, 1)?,
            complete.pixels()[..3].to_vec(),
        )?;
        first_chunk
            .mask_mut()
            .as_mut_slice()
            .copy_from_slice(&complete.mask().as_slice()[..3]);
        let mut second_chunk = ScientificImage::from_pixels(
            Dimensions::new(4, 1, 1)?,
            complete.pixels()[3..].to_vec(),
        )?;
        second_chunk
            .mask_mut()
            .as_mut_slice()
            .copy_from_slice(&complete.mask().as_slice()[3..]);

        let mut first = StatisticsFirstPass::new();
        first.observe_image(&first_chunk)?;
        first.observe_image(&second_chunk)?;

        // A FITS stream stores both masked and non-finite output as NaN. The
        // first pass has already retained the distinction needed for counts.
        let stored = [1.0e16, 1.0, -1.0e16, f64::NAN, f64::NAN, 6.0, 9.0];
        let mut mean = first.finish()?;
        mean.observe_values(&stored[..2])?;
        mean.observe_values(&stored[2..6])?;
        mean.observe_values(&stored[6..])?;
        let mut variance = mean.finish()?;
        variance.observe_values(&stored[..5])?;
        variance.observe_values(&stored[5..])?;
        let actual = variance.finish()?;

        assert_eq!(actual, expected);
        Ok(())
    }

    #[test]
    fn repeated_pass_detects_a_changed_finite_sample_count() -> TestResult {
        let input = image(vec![1.0, 2.0])?;
        let mut first = StatisticsFirstPass::new();
        first.observe_image(&input)?;
        let mut mean = first.finish()?;
        mean.observe_values(&[1.0, f64::NAN])?;

        assert_eq!(
            mean.finish().map(|_| ()),
            Err(StatisticsError::PassSampleMismatch {
                expected: 2,
                actual: 1,
            })
        );
        Ok(())
    }

    #[test]
    fn value_stream_first_pass_counts_non_finite_samples_without_masks() -> TestResult {
        let values = [1.0, f64::NAN, -2.0, f64::INFINITY, 5.0];
        let mut first = StatisticsFirstPass::new();
        first.observe_values(&values[..2])?;
        first.observe_values(&values[2..])?;
        let mut mean = first.finish()?;
        mean.observe_values(&values)?;
        let mut variance = mean.finish()?;
        variance.observe_values(&values)?;
        let statistics = variance.finish()?;

        assert_eq!(statistics.total_samples(), 5);
        assert_eq!(statistics.usable_samples(), 3);
        assert_eq!(statistics.masked_samples(), 0);
        assert_eq!(statistics.non_finite_samples(), 2);
        assert_eq!(statistics.minimum().to_bits(), (-2.0_f64).to_bits());
        assert_eq!(statistics.maximum().to_bits(), 5.0_f64.to_bits());
        assert!((statistics.mean() - (4.0 / 3.0)).abs() <= f64::EPSILON);
        Ok(())
    }
}
