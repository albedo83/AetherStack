use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_core::{CoreError, PixelFlags, ScientificImage};

/// Explicit guards for robust flat normalization.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlatNormalizationParameters {
    minimum_valid_samples: usize,
    minimum_normalization: f64,
}

impl FlatNormalizationParameters {
    /// Builds flat-normalization guards without hidden defaults.
    ///
    /// The normalization scalar must be strictly greater than
    /// `minimum_normalization`. Only clear, finite, strictly positive samples
    /// contribute to the exact median and to `minimum_valid_samples`.
    ///
    /// # Errors
    ///
    /// Returns a typed error for zero support or a negative/non-finite scalar
    /// threshold.
    pub fn new(
        minimum_valid_samples: usize,
        minimum_normalization: f64,
    ) -> Result<Self, FlatNormalizationError> {
        if minimum_valid_samples == 0 {
            return Err(FlatNormalizationError::ZeroMinimumValidSamples);
        }
        if !minimum_normalization.is_finite() || minimum_normalization < 0.0 {
            return Err(FlatNormalizationError::InvalidMinimumNormalization {
                value: minimum_normalization,
            });
        }
        Ok(Self {
            minimum_valid_samples,
            minimum_normalization: canonical_zero(minimum_normalization),
        })
    }

    /// Required count of clear, finite, strictly positive samples.
    #[must_use]
    pub const fn minimum_valid_samples(self) -> usize {
        self.minimum_valid_samples
    }

    /// Exclusive lower bound for the median normalization scalar.
    #[must_use]
    pub const fn minimum_normalization(self) -> f64 {
        self.minimum_normalization
    }
}

/// Exact sample accounting used to derive a flat normalization scalar.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FlatNormalizationSupport {
    accepted: usize,
    masked: usize,
    non_finite: usize,
    non_positive: usize,
}

impl FlatNormalizationSupport {
    /// Clear, finite, positive samples included in the median.
    #[must_use]
    pub const fn accepted(self) -> usize {
        self.accepted
    }

    /// Samples excluded because at least one quality bit was set.
    #[must_use]
    pub const fn masked(self) -> usize {
        self.masked
    }

    /// Unmasked NaN or infinite samples.
    #[must_use]
    pub const fn non_finite(self) -> usize {
        self.non_finite
    }

    /// Unmasked finite zero or negative samples.
    #[must_use]
    pub const fn non_positive(self) -> usize {
        self.non_positive
    }

    /// Total pixels accounted for by the four exclusive categories.
    #[must_use]
    pub const fn total(self) -> usize {
        self.accepted + self.masked + self.non_finite + self.non_positive
    }
}

/// Normalized flat image and the evidence used to calculate its scale.
#[derive(Clone, Debug, PartialEq)]
pub struct NormalizedFlat {
    image: ScientificImage,
    normalization: f64,
    support: FlatNormalizationSupport,
}

impl NormalizedFlat {
    /// Flat divided by the exact robust normalization scalar.
    #[must_use]
    pub const fn image(&self) -> &ScientificImage {
        &self.image
    }

    /// Exact median of clear, finite, positive input pixels.
    #[must_use]
    pub const fn normalization(&self) -> f64 {
        self.normalization
    }

    /// Complete input sample accounting.
    #[must_use]
    pub const fn support(&self) -> FlatNormalizationSupport {
        self.support
    }

    /// Consumes the result into its image, scalar, and support accounting.
    #[must_use]
    pub fn into_parts(self) -> (ScientificImage, f64, FlatNormalizationSupport) {
        (self.image, self.normalization, self.support)
    }
}

/// Failure to derive or apply a robust flat normalization.
#[derive(Clone, Debug, PartialEq)]
pub enum FlatNormalizationError {
    /// At least one valid sample must be required.
    ZeroMinimumValidSamples,
    /// The normalization lower bound is negative, NaN, or infinite.
    InvalidMinimumNormalization {
        /// Rejected threshold.
        value: f64,
    },
    /// Too few clear, finite, positive samples remain.
    InsufficientSupport {
        /// Required valid samples.
        required: usize,
        /// Observed accounting for all input pixels.
        support: FlatNormalizationSupport,
    },
    /// The exact median is not safely above the explicit lower bound.
    UnsafeNormalization {
        /// Calculated median.
        normalization: f64,
        /// Exclusive lower bound supplied by the caller.
        minimum: f64,
    },
    /// Scratch allocation for exact median selection failed.
    ScratchAllocationFailed {
        /// Number of positive samples requested.
        elements: usize,
    },
    /// Output image allocation or construction failed.
    Core(CoreError),
}

impl Display for FlatNormalizationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ZeroMinimumValidSamples => {
                formatter.write_str("flat normalization requires at least one valid sample")
            }
            Self::InvalidMinimumNormalization { value } => write!(
                formatter,
                "minimum flat normalization must be finite and non-negative, received {value}"
            ),
            Self::InsufficientSupport { required, support } => write!(
                formatter,
                "flat normalization has {} valid samples; {required} required ({} masked, {} non-finite, {} non-positive)",
                support.accepted, support.masked, support.non_finite, support.non_positive
            ),
            Self::UnsafeNormalization {
                normalization,
                minimum,
            } => write!(
                formatter,
                "flat normalization {normalization} is not greater than minimum {minimum}"
            ),
            Self::ScratchAllocationFailed { elements } => write!(
                formatter,
                "cannot reserve {elements} samples for exact flat median"
            ),
            Self::Core(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for FlatNormalizationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Core(error) => Some(error),
            Self::ZeroMinimumValidSamples
            | Self::InvalidMinimumNormalization { .. }
            | Self::InsufficientSupport { .. }
            | Self::UnsafeNormalization { .. }
            | Self::ScratchAllocationFailed { .. } => None,
        }
    }
}

/// Normalizes a pedestal-corrected flat by its exact positive-sample median.
///
/// The algorithm makes two read-only accounting passes, selects the exact median
/// in expected linear time, then writes one independent normalized output. The
/// scratch vector contains only clear, finite, strictly positive values and uses
/// a fallible exact reservation. It never sorts or mutates the source image.
///
/// Flagged samples remain flagged and become canonical NaN. Unflagged non-finite
/// or non-positive samples become NaN with [`PixelFlags::INVALID`]. A finite
/// positive division result is retained; overflow is retained and marked
/// invalid. Existing unknown quality bits are never erased.
///
/// # Errors
///
/// Returns a support, unsafe-scalar, scratch-allocation, or output-allocation
/// error. No partially normalized image is returned.
pub fn normalize_flat(
    flat: &ScientificImage,
    parameters: FlatNormalizationParameters,
) -> Result<NormalizedFlat, FlatNormalizationError> {
    let support = classify_support(flat);
    if support.accepted < parameters.minimum_valid_samples {
        return Err(FlatNormalizationError::InsufficientSupport {
            required: parameters.minimum_valid_samples,
            support,
        });
    }

    let mut positive_samples = Vec::new();
    positive_samples
        .try_reserve_exact(support.accepted)
        .map_err(|_| FlatNormalizationError::ScratchAllocationFailed {
            elements: support.accepted,
        })?;
    positive_samples.extend(flat.pixels().iter().zip(flat.mask().as_slice()).filter_map(
        |(value, flags)| (flags.is_clear() && value.is_finite() && *value > 0.0).then_some(*value),
    ));
    if positive_samples.len() != support.accepted {
        return Err(FlatNormalizationError::InsufficientSupport {
            required: support.accepted,
            support: classify_support(flat),
        });
    }

    let normalization = exact_median(&mut positive_samples);
    if !normalization.is_finite() || normalization <= parameters.minimum_normalization {
        return Err(FlatNormalizationError::UnsafeNormalization {
            normalization,
            minimum: parameters.minimum_normalization,
        });
    }

    let mut image = ScientificImage::filled(flat.dimensions(), f64::NAN)
        .map_err(FlatNormalizationError::Core)?;
    let (output_pixels, output_mask) = image.pixels_and_mask_mut();
    for (((output, output_flags), value), input_flags) in output_pixels
        .iter_mut()
        .zip(output_mask.as_mut_slice())
        .zip(flat.pixels())
        .zip(flat.mask().as_slice())
    {
        let mut flags = *input_flags;
        if !flags.is_clear() {
            *output = f64::NAN;
            *output_flags = flags;
            continue;
        }
        if !value.is_finite() || *value <= 0.0 {
            flags |= PixelFlags::INVALID;
            *output = f64::NAN;
            *output_flags = flags;
            continue;
        }

        let normalized = value / normalization;
        if !normalized.is_finite() {
            flags |= PixelFlags::INVALID;
        }
        *output = canonical_zero(normalized);
        *output_flags = flags;
    }

    Ok(NormalizedFlat {
        image,
        normalization,
        support,
    })
}

fn classify_support(flat: &ScientificImage) -> FlatNormalizationSupport {
    let mut support = FlatNormalizationSupport::default();
    for (value, flags) in flat.pixels().iter().zip(flat.mask().as_slice()) {
        if !flags.is_clear() {
            support.masked += 1;
        } else if !value.is_finite() {
            support.non_finite += 1;
        } else if *value <= 0.0 {
            support.non_positive += 1;
        } else {
            support.accepted += 1;
        }
    }
    support
}

fn exact_median(values: &mut [f64]) -> f64 {
    let length = values.len();
    let middle = length / 2;
    let (lower, upper, _) = values.select_nth_unstable_by(middle, f64::total_cmp);
    if length % 2 == 1 {
        *upper
    } else {
        let lower = lower
            .iter()
            .copied()
            .max_by(f64::total_cmp)
            .unwrap_or(*upper);
        lower + (*upper - lower) * 0.5
    }
}

fn canonical_zero(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use aether_core::Dimensions;

    use super::*;
    use crate::subtract_pedestal;

    type TestResult<T = ()> = Result<T, Box<dyn StdError>>;

    fn image(values: Vec<f64>) -> TestResult<ScientificImage> {
        let dimensions = Dimensions::new(values.len(), 1, 1)?;
        Ok(ScientificImage::from_pixels(dimensions, values)?)
    }

    fn parameters(minimum_valid_samples: usize) -> TestResult<FlatNormalizationParameters> {
        Ok(FlatNormalizationParameters::new(
            minimum_valid_samples,
            1.0e-12,
        )?)
    }

    #[test]
    fn uses_exact_median_instead_of_outlier_sensitive_mean() -> TestResult {
        let flat = image(vec![1.0, 1.0, 1.0, 1_000.0])?;

        let normalized = normalize_flat(&flat, parameters(4)?)?;

        assert_eq!(normalized.normalization().to_bits(), 1.0_f64.to_bits());
        assert_eq!(normalized.image().pixels(), &[1.0, 1.0, 1.0, 1_000.0]);
        assert_eq!(
            normalized.support(),
            FlatNormalizationSupport {
                accepted: 4,
                masked: 0,
                non_finite: 0,
                non_positive: 0,
            }
        );
        Ok(())
    }

    #[test]
    fn pedestal_subtraction_then_normalization_recovers_known_flat_shape() -> TestResult {
        let raw_flat = image(vec![12.0, 14.0, 16.0])?;
        let pedestal = image(vec![10.0, 10.0, 10.0])?;

        let corrected = subtract_pedestal(&raw_flat, &pedestal)?;
        let normalized = normalize_flat(&corrected, parameters(3)?)?;

        assert_eq!(normalized.normalization().to_bits(), 4.0_f64.to_bits());
        assert_eq!(normalized.image().pixels(), &[0.5, 1.0, 1.5]);
        assert!(
            normalized
                .image()
                .mask()
                .as_slice()
                .iter()
                .all(|flags| flags.is_clear())
        );
        Ok(())
    }

    #[test]
    fn even_median_uses_overflow_safe_midpoint() -> TestResult {
        let flat = image(vec![f64::MAX, f64::MAX])?;

        let normalized = normalize_flat(&flat, parameters(2)?)?;

        assert_eq!(normalized.normalization().to_bits(), f64::MAX.to_bits());
        assert_eq!(normalized.image().pixels(), &[1.0, 1.0]);
        Ok(())
    }

    #[test]
    fn excludes_and_accounts_for_every_unusable_category() -> TestResult {
        let mut flat = image(vec![2.0, 4.0, 100.0, f64::NAN, 0.0, -1.0])?;
        flat.mask_mut().as_mut_slice()[2] = PixelFlags::SATURATED;

        let normalized = normalize_flat(&flat, parameters(2)?)?;

        assert_eq!(normalized.normalization().to_bits(), 3.0_f64.to_bits());
        assert_eq!(normalized.support().total(), 6);
        assert_eq!(normalized.support().accepted(), 2);
        assert_eq!(normalized.support().masked(), 1);
        assert_eq!(normalized.support().non_finite(), 1);
        assert_eq!(normalized.support().non_positive(), 2);
        assert_eq!(
            normalized.image().pixels()[0].to_bits(),
            (2.0_f64 / 3.0).to_bits()
        );
        assert_eq!(
            normalized.image().pixels()[1].to_bits(),
            (4.0_f64 / 3.0).to_bits()
        );
        assert!(
            normalized.image().pixels()[2..]
                .iter()
                .all(|value| value.is_nan())
        );
        assert_eq!(
            normalized.image().mask().as_slice()[2],
            PixelFlags::SATURATED
        );
        assert!(
            normalized.image().mask().as_slice()[3..]
                .iter()
                .all(|flags| flags.contains(PixelFlags::INVALID))
        );
        Ok(())
    }

    #[test]
    fn rejects_insufficient_support_with_complete_accounting() -> TestResult {
        let mut flat = image(vec![1.0, 2.0, f64::INFINITY, 0.0])?;
        flat.mask_mut().as_mut_slice()[1] = PixelFlags::HOT;

        let result = normalize_flat(&flat, parameters(2)?);

        assert!(matches!(
            result,
            Err(FlatNormalizationError::InsufficientSupport {
                required: 2,
                support: FlatNormalizationSupport {
                    accepted: 1,
                    masked: 1,
                    non_finite: 1,
                    non_positive: 1,
                }
            })
        ));
        Ok(())
    }

    #[test]
    fn rejects_median_at_the_exclusive_safety_floor() -> TestResult {
        let flat = image(vec![0.5, 0.5, 10.0])?;
        let parameters = FlatNormalizationParameters::new(2, 0.5)?;

        assert!(matches!(
            normalize_flat(&flat, parameters),
            Err(FlatNormalizationError::UnsafeNormalization {
                normalization: 0.5,
                minimum: 0.5,
            })
        ));
        Ok(())
    }

    #[test]
    fn validates_parameters_and_canonicalizes_negative_zero() {
        assert!(matches!(
            FlatNormalizationParameters::new(0, 0.0),
            Err(FlatNormalizationError::ZeroMinimumValidSamples)
        ));
        for value in [-1.0, f64::NAN, f64::INFINITY] {
            assert!(matches!(
                FlatNormalizationParameters::new(1, value),
                Err(FlatNormalizationError::InvalidMinimumNormalization { .. })
            ));
        }
        let negative_zero = FlatNormalizationParameters::new(1, -0.0);
        assert!(matches!(
            negative_zero,
            Ok(value) if value.minimum_normalization().to_bits() == 0.0_f64.to_bits()
        ));
    }

    #[test]
    fn preserves_unknown_mask_bits_and_planar_dimensions() -> TestResult {
        let dimensions = Dimensions::new(2, 1, 2)?;
        let mut flat = ScientificImage::from_pixels(dimensions, vec![1.0, 2.0, 3.0, 4.0])?;
        flat.mask_mut().as_mut_slice()[1] = PixelFlags::from_bits_retain(0b1000_0000);

        let normalized = normalize_flat(&flat, parameters(3)?)?;

        assert_eq!(normalized.image().dimensions(), dimensions);
        assert!(normalized.image().pixels()[1].is_nan());
        assert_eq!(normalized.image().mask().as_slice()[1].bits(), 0b1000_0000);
        Ok(())
    }
}
