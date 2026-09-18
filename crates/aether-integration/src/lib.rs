//! Deterministic strict-reference image integration.
//!
//! The first implemented estimator is an unweighted arithmetic mean. Robust
//! rejection and weighting belong to later versioned algorithms; this primitive
//! supplies the transparent CPU oracle for the initial vertical slice.

use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_core::{CompensatedSum, CoreError, Dimensions, PixelFlags, ScientificImage};

/// Per-pixel accounting for one mean integration.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PixelSupport {
    accepted: u32,
    masked: u32,
    non_finite: u32,
}

impl PixelSupport {
    /// Finite, clear samples included in the mean.
    #[must_use]
    pub const fn accepted(self) -> u32 {
        self.accepted
    }

    /// Samples excluded because at least one quality bit was set.
    #[must_use]
    pub const fn masked(self) -> u32 {
        self.masked
    }

    /// Unmasked samples excluded because they were NaN or infinite.
    #[must_use]
    pub const fn non_finite(self) -> u32 {
        self.non_finite
    }

    /// Total input samples represented by this accounting record.
    #[must_use]
    pub const fn total(self) -> u32 {
        self.accepted + self.masked + self.non_finite
    }
}

/// Integrated image and exact per-pixel contribution accounting.
#[derive(Clone, Debug, PartialEq)]
pub struct MeanIntegration {
    image: ScientificImage,
    support: Vec<PixelSupport>,
}

impl MeanIntegration {
    /// Strict mean image.
    #[must_use]
    pub const fn image(&self) -> &ScientificImage {
        &self.image
    }

    /// Support records in the same planar order as the image samples.
    #[must_use]
    pub fn support(&self) -> &[PixelSupport] {
        &self.support
    }

    /// Consumes the result and returns its image and support map.
    #[must_use]
    pub fn into_parts(self) -> (ScientificImage, Vec<PixelSupport>) {
        (self.image, self.support)
    }
}

/// Failure to construct a strict mean integration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IntegrationError {
    /// At least one input image is required to establish dimensions.
    NoInputImages,
    /// The number of inputs cannot be represented in a support record.
    TooManyInputImages {
        /// Received image count.
        count: usize,
        /// Maximum supported count.
        maximum: u32,
    },
    /// An input does not match the first image's dimensions.
    DimensionMismatch {
        /// Zero-based input position.
        input_index: usize,
        /// Required dimensions.
        expected: Dimensions,
        /// Received dimensions.
        actual: Dimensions,
    },
    /// A supposedly valid image violated its internal sample/mask length invariant.
    InternalImageInvariant {
        /// Zero-based input position.
        input_index: usize,
    },
    /// Per-pixel support categories did not account for every input.
    InternalAccountingInvariant {
        /// Expected number of categorized inputs.
        expected: u32,
        /// Observed category total.
        actual: u32,
    },
    /// Support-map allocation failed.
    SupportAllocationFailed {
        /// Number of per-pixel records requested.
        elements: usize,
    },
    /// Output image allocation or construction failed.
    Core(CoreError),
}

impl Display for IntegrationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoInputImages => formatter.write_str("mean integration requires an input image"),
            Self::TooManyInputImages { count, maximum } => write!(
                formatter,
                "mean integration received {count} images; support map maximum is {maximum}"
            ),
            Self::DimensionMismatch {
                input_index,
                expected,
                actual,
            } => write!(
                formatter,
                "input {input_index} dimensions {}x{}x{} do not match {}x{}x{}",
                actual.width(),
                actual.height(),
                actual.planes(),
                expected.width(),
                expected.height(),
                expected.planes()
            ),
            Self::InternalImageInvariant { input_index } => write!(
                formatter,
                "input {input_index} violates the image sample/mask length invariant"
            ),
            Self::InternalAccountingInvariant { expected, actual } => write!(
                formatter,
                "pixel support accounts for {actual} inputs; expected {expected}"
            ),
            Self::SupportAllocationFailed { elements } => write!(
                formatter,
                "cannot reserve memory for {elements} pixel support records"
            ),
            Self::Core(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for IntegrationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Core(error) => Some(error),
            Self::NoInputImages
            | Self::TooManyInputImages { .. }
            | Self::DimensionMismatch { .. }
            | Self::InternalImageInvariant { .. }
            | Self::InternalAccountingInvariant { .. }
            | Self::SupportAllocationFailed { .. } => None,
        }
    }
}

/// Integrates equal-sized images with a strict unweighted arithmetic mean.
///
/// For each pixel, the first pass classifies every input and determines the
/// largest absolute usable value. The second pass accumulates each normalized
/// value divided by the accepted count with Neumaier compensation. Scaling
/// before summation prevents equal large values from overflowing the mean.
///
/// A sample participates only when its mask is clear and its value is finite.
/// When at least one sample participates, the output mask is clear and support
/// records every exclusion. With no usable contribution, the output is NaN and
/// marked `MISSING`; all input mask bits are retained and `INVALID` is added if
/// an unmasked non-finite value was observed.
///
/// Input order is part of the strict execution contract and must follow stable
/// manifest order. Spatial tiling does not change a pixel's reduction order.
///
/// # Errors
///
/// Returns an error for empty input, excessive input count, mismatched
/// dimensions, or fallible output allocation.
pub fn integrate_mean(inputs: &[&ScientificImage]) -> Result<MeanIntegration, IntegrationError> {
    let Some(first) = inputs.first().copied() else {
        return Err(IntegrationError::NoInputImages);
    };
    let input_count =
        u32::try_from(inputs.len()).map_err(|_| IntegrationError::TooManyInputImages {
            count: inputs.len(),
            maximum: u32::MAX,
        })?;
    let dimensions = first.dimensions();
    for (input_index, input) in inputs.iter().enumerate().skip(1) {
        let actual = input.dimensions();
        if actual != dimensions {
            return Err(IntegrationError::DimensionMismatch {
                input_index,
                expected: dimensions,
                actual,
            });
        }
    }

    let mut output =
        ScientificImage::filled(dimensions, f64::NAN).map_err(IntegrationError::Core)?;
    let mut support = Vec::new();
    support
        .try_reserve_exact(dimensions.pixel_count())
        .map_err(|_| IntegrationError::SupportAllocationFailed {
            elements: dimensions.pixel_count(),
        })?;
    support.resize(dimensions.pixel_count(), PixelSupport::default());

    let (output_pixels, output_mask) = output.pixels_and_mask_mut();
    for (pixel_index, ((output, output_flags), output_support)) in output_pixels
        .iter_mut()
        .zip(output_mask.as_mut_slice())
        .zip(&mut support)
        .enumerate()
    {
        let mut scale = 0.0_f64;
        let mut minimum = f64::INFINITY;
        let mut maximum = f64::NEG_INFINITY;
        let mut combined_rejected_flags = PixelFlags::CLEAR;

        for (input_index, input) in inputs.iter().enumerate() {
            let (value, flags) = sample_at(input, input_index, pixel_index)?;
            if !flags.is_clear() {
                output_support.masked += 1;
                combined_rejected_flags |= flags;
            } else if !value.is_finite() {
                output_support.non_finite += 1;
            } else {
                output_support.accepted += 1;
                scale = scale.max(value.abs());
                minimum = minimum.min(value);
                maximum = maximum.max(value);
            }
        }

        // Conversion was validated once above; this protects future changes to
        // accounting branches from silently dropping an input category.
        if output_support.total() != input_count {
            return Err(IntegrationError::InternalAccountingInvariant {
                expected: input_count,
                actual: output_support.total(),
            });
        }

        if output_support.accepted == 0 {
            *output = f64::NAN;
            let mut flags = combined_rejected_flags | PixelFlags::MISSING;
            if output_support.non_finite > 0 {
                flags |= PixelFlags::INVALID;
            }
            *output_flags = flags;
            continue;
        }

        *output = if scale == 0.0 {
            0.0
        } else {
            let divisor = f64::from(output_support.accepted);
            let mut normalized_mean = CompensatedSum::new();
            for (input_index, input) in inputs.iter().enumerate() {
                let (value, flags) = sample_at(input, input_index, pixel_index)?;
                if flags.is_clear() && value.is_finite() {
                    normalized_mean.add((value / scale) / divisor);
                }
            }
            let normalized_mean = normalized_mean
                .total()
                .max(minimum / scale)
                .min(maximum / scale);
            canonical_zero(normalized_mean * scale)
        };
        *output_flags = PixelFlags::CLEAR;
    }

    Ok(MeanIntegration {
        image: output,
        support,
    })
}

fn sample_at(
    input: &ScientificImage,
    input_index: usize,
    pixel_index: usize,
) -> Result<(f64, PixelFlags), IntegrationError> {
    let value = input
        .pixels()
        .get(pixel_index)
        .copied()
        .ok_or(IntegrationError::InternalImageInvariant { input_index })?;
    let flags = input
        .mask()
        .as_slice()
        .get(pixel_index)
        .copied()
        .ok_or(IntegrationError::InternalImageInvariant { input_index })?;
    Ok((value, flags))
}

fn canonical_zero(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn StdError>>;

    fn image(values: Vec<f64>) -> TestResult<ScientificImage> {
        let dimensions = Dimensions::new(values.len(), 1, 1)?;
        Ok(ScientificImage::from_pixels(dimensions, values)?)
    }

    #[test]
    fn calculates_exact_means_and_support_in_stable_input_order() -> TestResult {
        let first = image(vec![1.0, 10.0])?;
        let second = image(vec![3.0, 20.0])?;
        let third = image(vec![5.0, 30.0])?;

        let result = integrate_mean(&[&first, &second, &third])?;

        assert_eq!(result.image().pixels(), &[3.0, 20.0]);
        assert_eq!(
            result.support(),
            &[
                PixelSupport {
                    accepted: 3,
                    masked: 0,
                    non_finite: 0,
                },
                PixelSupport {
                    accepted: 3,
                    masked: 0,
                    non_finite: 0,
                },
            ]
        );
        Ok(())
    }

    #[test]
    fn excludes_masked_and_non_finite_values_without_invalidating_valid_mean() -> TestResult {
        let first = image(vec![2.0])?;
        let mut masked = image(vec![100.0])?;
        masked.mask_mut().as_mut_slice()[0] = PixelFlags::SATURATED;
        let non_finite = image(vec![f64::NAN])?;

        let result = integrate_mean(&[&first, &masked, &non_finite])?;

        assert_eq!(result.image().pixels()[0].to_bits(), 2.0_f64.to_bits());
        assert!(result.image().mask().as_slice()[0].is_clear());
        assert_eq!(
            result.support()[0],
            PixelSupport {
                accepted: 1,
                masked: 1,
                non_finite: 1,
            }
        );
        Ok(())
    }

    #[test]
    fn marks_pixels_without_support_and_retains_rejection_reasons() -> TestResult {
        let mut saturated = image(vec![10.0])?;
        saturated.mask_mut().as_mut_slice()[0] = PixelFlags::SATURATED;
        let mut unknown = image(vec![20.0])?;
        unknown.mask_mut().as_mut_slice()[0] = PixelFlags::from_bits_retain(0b1000_0000);
        let non_finite = image(vec![f64::INFINITY])?;

        let result = integrate_mean(&[&saturated, &unknown, &non_finite])?;
        let flags = result.image().mask().as_slice()[0];

        assert!(result.image().pixels()[0].is_nan());
        assert!(flags.contains(PixelFlags::MISSING));
        assert!(flags.contains(PixelFlags::INVALID));
        assert!(flags.contains(PixelFlags::SATURATED));
        assert_eq!(flags.bits() & 0b1000_0000, 0b1000_0000);
        assert_eq!(result.support()[0].total(), 3);
        Ok(())
    }

    #[test]
    fn scaled_mean_avoids_overflow_for_large_equal_inputs() -> TestResult {
        let first = image(vec![f64::MAX])?;
        let second = image(vec![f64::MAX])?;

        let result = integrate_mean(&[&first, &second])?;

        assert_eq!(result.image().pixels()[0].to_bits(), f64::MAX.to_bits());
        assert!(result.image().mask().as_slice()[0].is_clear());
        Ok(())
    }

    #[test]
    fn compensated_normalized_mean_preserves_small_residual() -> TestResult {
        let positive = image(vec![1.0e16])?;
        let residual = image(vec![1.0])?;
        let negative = image(vec![-1.0e16])?;

        let result = integrate_mean(&[&positive, &residual, &negative])?;
        let expected = 1.0_f64 / 3.0;

        assert!((result.image().pixels()[0] - expected).abs() <= f64::EPSILON);
        Ok(())
    }

    #[test]
    fn rejects_empty_and_mismatched_inputs() -> TestResult {
        assert_eq!(integrate_mean(&[]), Err(IntegrationError::NoInputImages));

        let first = image(vec![1.0, 2.0])?;
        let second = image(vec![1.0])?;
        assert!(matches!(
            integrate_mean(&[&first, &second]),
            Err(IntegrationError::DimensionMismatch { input_index: 1, .. })
        ));
        Ok(())
    }

    #[test]
    fn preserves_planar_order_and_can_return_owned_parts() -> TestResult {
        let dimensions = Dimensions::new(2, 1, 2)?;
        let first = ScientificImage::from_pixels(dimensions, vec![1.0, 2.0, 3.0, 4.0])?;
        let second = ScientificImage::from_pixels(dimensions, vec![3.0, 4.0, 5.0, 6.0])?;

        let result = integrate_mean(&[&first, &second])?;
        let (image, support) = result.into_parts();

        assert_eq!(image.pixels(), &[2.0, 3.0, 4.0, 5.0]);
        assert!(support.iter().all(|entry| entry.accepted() == 2));
        Ok(())
    }
}
