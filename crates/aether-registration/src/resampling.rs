use std::error::Error;
use std::f64::consts::PI;
use std::fmt::{Display, Formatter};

use aether_core::{CompensatedSum, CoreError, Dimensions, PixelFlags, ScientificImage};

use crate::{AffineTransform, CoordinateError, ImagePoint};

/// Stable identifier for strict inverse-mapped, normalized Lanczos-3 sampling.
pub const LANCZOS3_RESAMPLING_ALGORITHM_ID: &str = "lanczos3-normalized-f64-v1";

const LANCZOS_RADIUS: i64 = 3;
const LANCZOS_TAPS: usize = 6;
const MINIMUM_WEIGHT_SUM: f64 = 1.0e-12;

/// Complete support accounting for one resampled image.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ResamplingStatistics {
    total_samples: usize,
    interpolated_samples: usize,
    outside_footprint_samples: usize,
    masked_support_samples: usize,
}

impl ResamplingStatistics {
    /// Output samples across all planes.
    #[must_use]
    pub const fn total_samples(self) -> usize {
        self.total_samples
    }

    /// Finite samples produced from complete clear support.
    #[must_use]
    pub const fn interpolated_samples(self) -> usize {
        self.interpolated_samples
    }

    /// Samples whose non-zero kernel support crossed a source boundary.
    #[must_use]
    pub const fn outside_footprint_samples(self) -> usize {
        self.outside_footprint_samples
    }

    /// Samples withheld because at least one non-zero source tap was unusable.
    #[must_use]
    pub const fn masked_support_samples(self) -> usize {
        self.masked_support_samples
    }
}

/// A fully materialized strict CPU reference result.
#[derive(Clone, Debug, PartialEq)]
pub struct ResampledImage {
    image: ScientificImage,
    source_to_reference: AffineTransform,
    statistics: ResamplingStatistics,
}

impl ResampledImage {
    /// Versioned interpolation and support policy.
    #[must_use]
    pub const fn algorithm_id(&self) -> &'static str {
        LANCZOS3_RESAMPLING_ALGORITHM_ID
    }

    /// Exact transform whose inverse was evaluated at output pixel centers.
    #[must_use]
    pub const fn source_to_reference(&self) -> AffineTransform {
        self.source_to_reference
    }

    /// Complete output-support accounting.
    #[must_use]
    pub const fn statistics(&self) -> ResamplingStatistics {
        self.statistics
    }

    /// Immutable output image and conservative mask.
    #[must_use]
    pub const fn image(&self) -> &ScientificImage {
        &self.image
    }

    /// Consumes the report and returns the output image.
    #[must_use]
    pub fn into_image(self) -> ScientificImage {
        self.image
    }
}

/// Failure raised before a complete resampled image can be returned.
#[derive(Clone, Debug, PartialEq)]
pub enum ResamplingError {
    /// Output dimensions or allocation violated the shared image contract.
    Core(CoreError),
    /// Transform inversion or application failed.
    Coordinate(CoordinateError),
    /// A coordinate or weighted calculation left the finite `f64` domain.
    NumericalOverflow,
    /// Support accounting exceeded `usize`.
    CountOverflow,
}

impl Display for ResamplingError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Core(error) => write!(formatter, "cannot construct resampled image: {error}"),
            Self::Coordinate(error) => {
                write!(formatter, "cannot map resampling coordinate: {error}")
            }
            Self::NumericalOverflow => {
                formatter.write_str("Lanczos resampling left the finite numerical domain")
            }
            Self::CountOverflow => formatter.write_str("resampling support count overflow"),
        }
    }
}

impl Error for ResamplingError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Core(error) => Some(error),
            Self::Coordinate(error) => Some(error),
            Self::NumericalOverflow | Self::CountOverflow => None,
        }
    }
}

impl From<CoreError> for ResamplingError {
    fn from(value: CoreError) -> Self {
        Self::Core(value)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct AxisTap {
    index: usize,
    weight: f64,
}

#[derive(Clone, Copy, Debug)]
struct AxisKernel {
    taps: [AxisTap; LANCZOS_TAPS],
    count: usize,
    complete: bool,
}

impl AxisKernel {
    fn new(coordinate: f64, source_length: usize) -> Result<Self, ResamplingError> {
        if !coordinate.is_finite() {
            return Err(ResamplingError::NumericalOverflow);
        }
        let mut kernel = Self {
            taps: [AxisTap::default(); LANCZOS_TAPS],
            count: 0,
            complete: true,
        };
        let maximum = source_length.saturating_sub(1) as f64;
        if coordinate <= -(LANCZOS_RADIUS as f64) || coordinate >= maximum + LANCZOS_RADIUS as f64 {
            kernel.complete = false;
            return Ok(kernel);
        }
        let floor = coordinate.floor();
        if floor < i64::MIN as f64 || floor > i64::MAX as f64 {
            kernel.complete = false;
            return Ok(kernel);
        }
        let floor = floor as i64;
        for offset in (-LANCZOS_RADIUS + 1)..=LANCZOS_RADIUS {
            let source_index = floor
                .checked_add(offset)
                .ok_or(ResamplingError::NumericalOverflow)?;
            let distance = coordinate - source_index as f64;
            let weight = lanczos3(distance);
            if weight == 0.0 {
                continue;
            }
            let Ok(index) = usize::try_from(source_index) else {
                kernel.complete = false;
                continue;
            };
            if index >= source_length {
                kernel.complete = false;
                continue;
            }
            let Some(slot) = kernel.taps.get_mut(kernel.count) else {
                return Err(ResamplingError::CountOverflow);
            };
            *slot = AxisTap { index, weight };
            kernel.count = kernel
                .count
                .checked_add(1)
                .ok_or(ResamplingError::CountOverflow)?;
        }
        if kernel.count == 0 {
            kernel.complete = false;
        }
        Ok(kernel)
    }

    fn active(&self) -> &[AxisTap] {
        &self.taps[..self.count]
    }
}

/// Resamples every source plane into a reference-aligned output rectangle.
///
/// The supplied affine transform maps source coordinates to reference
/// coordinates. The implementation inverts it exactly once, evaluates that
/// inverse at each integer-centered output pixel, and applies a separable
/// normalized Lanczos-3 kernel in `f64`. Output is never clipped.
///
/// A sample is emitted only when every mathematically non-zero tap lies inside
/// the source and contains a finite, clear value. Boundary loss becomes
/// [`PixelFlags::MISSING`]; unusable source support preserves the union of its
/// flags, adding [`PixelFlags::INVALID`] for unflagged non-finite values. This
/// strict policy prevents interpolation from concealing defects.
///
/// This complete-image scalar implementation is the numerical oracle. A later
/// band executor may optimize traversal and memory use only after differential
/// tests prove agreement with it.
pub fn resample_lanczos3(
    source: &ScientificImage,
    output_width: usize,
    output_height: usize,
    source_to_reference: AffineTransform,
) -> Result<ResampledImage, ResamplingError> {
    let source_dimensions = source.dimensions();
    let output_dimensions =
        Dimensions::new(output_width, output_height, source_dimensions.planes())?;
    let reference_to_source = source_to_reference
        .inverse()
        .map_err(ResamplingError::Coordinate)?;
    let mut output = ScientificImage::filled(output_dimensions, f64::NAN)?;
    let output_area = output_width
        .checked_mul(output_height)
        .ok_or(ResamplingError::CountOverflow)?;
    let source_area = source_dimensions
        .width()
        .checked_mul(source_dimensions.height())
        .ok_or(ResamplingError::CountOverflow)?;
    let mut statistics = ResamplingStatistics {
        total_samples: output_dimensions.pixel_count(),
        ..ResamplingStatistics::default()
    };

    for output_y in 0..output_height {
        for output_x in 0..output_width {
            let reference_point = ImagePoint::new(output_x as f64, output_y as f64)
                .map_err(ResamplingError::Coordinate)?;
            let source_point = reference_to_source
                .apply(reference_point)
                .map_err(ResamplingError::Coordinate)?;
            let x_kernel = AxisKernel::new(source_point.x(), source_dimensions.width())?;
            let y_kernel = AxisKernel::new(source_point.y(), source_dimensions.height())?;
            if !x_kernel.complete || !y_kernel.complete {
                for plane in 0..source_dimensions.planes() {
                    let output_index =
                        linear_index(output_area, output_width, output_x, output_y, plane)?;
                    output.mask_mut().as_mut_slice()[output_index] = PixelFlags::MISSING;
                    statistics.outside_footprint_samples =
                        checked_increment(statistics.outside_footprint_samples)?;
                }
                continue;
            }

            for plane in 0..source_dimensions.planes() {
                let mut weighted_sum = CompensatedSum::new();
                let mut weight_sum = CompensatedSum::new();
                let mut combined_flags = PixelFlags::CLEAR;
                for y_tap in y_kernel.active() {
                    for x_tap in x_kernel.active() {
                        let weight = x_tap.weight * y_tap.weight;
                        if !weight.is_finite() {
                            return Err(ResamplingError::NumericalOverflow);
                        }
                        let source_index = linear_index(
                            source_area,
                            source_dimensions.width(),
                            x_tap.index,
                            y_tap.index,
                            plane,
                        )?;
                        let value = source.pixels()[source_index];
                        combined_flags |= source.mask().as_slice()[source_index];
                        if value.is_finite() {
                            weighted_sum.add(value * weight);
                        } else {
                            combined_flags |= PixelFlags::INVALID;
                        }
                        weight_sum.add(weight);
                    }
                }
                let output_index =
                    linear_index(output_area, output_width, output_x, output_y, plane)?;
                if !combined_flags.is_clear() {
                    output.mask_mut().as_mut_slice()[output_index] = combined_flags;
                    statistics.masked_support_samples =
                        checked_increment(statistics.masked_support_samples)?;
                    continue;
                }
                let denominator = weight_sum.total();
                if !denominator.is_finite() || denominator.abs() < MINIMUM_WEIGHT_SUM {
                    return Err(ResamplingError::NumericalOverflow);
                }
                let value = weighted_sum.total() / denominator;
                if !value.is_finite() {
                    return Err(ResamplingError::NumericalOverflow);
                }
                output.pixels_mut()[output_index] = canonical_zero(value);
                statistics.interpolated_samples =
                    checked_increment(statistics.interpolated_samples)?;
            }
        }
    }

    let accounted = statistics
        .interpolated_samples
        .checked_add(statistics.outside_footprint_samples)
        .and_then(|value| value.checked_add(statistics.masked_support_samples))
        .ok_or(ResamplingError::CountOverflow)?;
    if accounted != statistics.total_samples {
        return Err(ResamplingError::CountOverflow);
    }
    Ok(ResampledImage {
        image: output,
        source_to_reference,
        statistics,
    })
}

fn linear_index(
    plane_area: usize,
    width: usize,
    x: usize,
    y: usize,
    plane: usize,
) -> Result<usize, ResamplingError> {
    plane
        .checked_mul(plane_area)
        .and_then(|offset| y.checked_mul(width).and_then(|row| offset.checked_add(row)))
        .and_then(|offset| offset.checked_add(x))
        .ok_or(ResamplingError::CountOverflow)
}

fn lanczos3(distance: f64) -> f64 {
    let absolute = distance.abs();
    if absolute >= LANCZOS_RADIUS as f64 {
        return 0.0;
    }
    sinc_pi(distance) * sinc_pi(distance / LANCZOS_RADIUS as f64)
}

#[allow(clippy::float_cmp)]
fn sinc_pi(value: f64) -> f64 {
    if value == 0.0 {
        return 1.0;
    }
    // Exact integer offsets are analytical zeros. Returning those zeros
    // explicitly makes identity transforms bit-exact and prevents irrelevant
    // neighboring masks from contaminating a direct sample.
    if value == value.round() {
        return 0.0;
    }
    let angle = PI * value;
    angle.sin() / angle
}

const fn checked_increment(value: usize) -> Result<usize, ResamplingError> {
    match value.checked_add(1) {
        Some(value) => Ok(value),
        None => Err(ResamplingError::CountOverflow),
    }
}

const fn canonical_zero(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn StdError>>;

    fn image(width: usize, height: usize, planes: usize) -> TestResult<ScientificImage> {
        let dimensions = Dimensions::new(width, height, planes)?;
        let pixels = (0..dimensions.pixel_count())
            .map(|index| index as f64 + 0.25)
            .collect();
        Ok(ScientificImage::from_pixels(dimensions, pixels)?)
    }

    #[test]
    fn identity_is_bit_exact_for_clear_pixels_and_preserves_mask_evidence() -> TestResult {
        let mut source = image(7, 6, 3)?;
        source.mark(0, 0, 0, PixelFlags::HOT)?;
        source.mark(6, 5, 2, PixelFlags::SATURATED)?;

        let result = resample_lanczos3(&source, 7, 6, AffineTransform::IDENTITY)?;

        assert_eq!(result.image().mask(), source.mask());
        for ((&input, &output), &flags) in source
            .pixels()
            .iter()
            .zip(result.image().pixels())
            .zip(source.mask().as_slice())
        {
            if flags.is_clear() {
                assert_eq!(output.to_bits(), input.to_bits());
            } else {
                assert!(output.is_nan());
            }
        }
        assert_eq!(result.statistics().interpolated_samples(), 124);
        assert_eq!(result.statistics().masked_support_samples(), 2);
        assert_eq!(result.statistics().outside_footprint_samples(), 0);
        Ok(())
    }

    #[test]
    fn fractional_translation_preserves_a_constant_on_complete_support() -> TestResult {
        let dimensions = Dimensions::new(12, 10, 1)?;
        let source = ScientificImage::filled(dimensions, 42.5)?;
        let transform = AffineTransform::new(1.0, 0.0, 0.0, 1.0, 0.25, -0.4)?;

        let result = resample_lanczos3(&source, 12, 10, transform)?;

        let statistics = result.statistics();
        assert!(statistics.interpolated_samples() > 0);
        assert!(statistics.outside_footprint_samples() > 0);
        for (&value, &flags) in result
            .image()
            .pixels()
            .iter()
            .zip(result.image().mask().as_slice())
        {
            if flags.is_clear() {
                assert!((value - 42.5).abs() < 1.0e-12);
            } else {
                assert!(value.is_nan());
                assert_eq!(flags, PixelFlags::MISSING);
            }
        }
        Ok(())
    }

    #[test]
    fn fractional_translation_preserves_isolated_source_flux_without_clipping() -> TestResult {
        let dimensions = Dimensions::new(31, 31, 1)?;
        let mut source = ScientificImage::filled(dimensions, 0.0)?;
        source.pixels_mut()[15 * 31 + 15] = 1.0;
        let transform = AffineTransform::new(1.0, 0.0, 0.0, 1.0, 0.37, -0.22)?;

        let result = resample_lanczos3(&source, 31, 31, transform)?;

        let mut flux = CompensatedSum::new();
        let mut minimum = f64::INFINITY;
        for (&value, &flags) in result
            .image()
            .pixels()
            .iter()
            .zip(result.image().mask().as_slice())
        {
            if flags.is_clear() {
                flux.add(value);
                minimum = minimum.min(value);
            }
        }
        assert!((flux.total() - 1.0).abs() < 1.0e-12);
        assert!(minimum < 0.0, "Lanczos negative lobes must not be clipped");
        Ok(())
    }

    #[test]
    fn nonzero_masked_support_is_conservative_but_zero_taps_are_ignored() -> TestResult {
        let mut source = ScientificImage::filled(Dimensions::new(9, 9, 1)?, 1.0)?;
        source.mark(4, 4, 0, PixelFlags::HOT)?;

        let identity = resample_lanczos3(&source, 9, 9, AffineTransform::IDENTITY)?;
        assert_eq!(identity.image().mask().get(4, 4, 0)?, PixelFlags::HOT);
        assert!(identity.image().mask().get(3, 4, 0)?.is_clear());

        let shifted = resample_lanczos3(
            &source,
            9,
            9,
            AffineTransform::new(1.0, 0.0, 0.0, 1.0, 0.5, 0.0)?,
        )?;
        assert!(shifted.statistics().masked_support_samples() > 1);
        assert!(
            shifted
                .image()
                .mask()
                .as_slice()
                .iter()
                .any(|flags| flags.contains(PixelFlags::HOT))
        );
        Ok(())
    }

    #[test]
    fn nonfinite_support_adds_invalid_without_losing_existing_flags() -> TestResult {
        let mut source = ScientificImage::filled(Dimensions::new(7, 7, 1)?, 1.0)?;
        source.pixels_mut()[24] = f64::NAN;
        source.mark(3, 3, 0, PixelFlags::REJECTED)?;

        let result = resample_lanczos3(&source, 7, 7, AffineTransform::IDENTITY)?;
        let flags = result.image().mask().get(3, 3, 0)?;
        assert!(flags.contains(PixelFlags::REJECTED));
        assert!(flags.contains(PixelFlags::INVALID));
        assert!(result.image().pixels()[24].is_nan());
        Ok(())
    }

    #[test]
    fn integer_translation_has_an_exact_missing_border() -> TestResult {
        let source = image(5, 4, 1)?;
        let transform = AffineTransform::new(1.0, 0.0, 0.0, 1.0, 1.0, 0.0)?;

        let result = resample_lanczos3(&source, 5, 4, transform)?;

        assert_eq!(result.statistics().outside_footprint_samples(), 4);
        for y in 0..4 {
            assert_eq!(result.image().mask().get(0, y, 0)?, PixelFlags::MISSING);
            for x in 1..5 {
                assert_eq!(
                    result.image().get(x, y, 0)?.to_bits(),
                    source.get(x - 1, y, 0)?.to_bits()
                );
            }
        }
        Ok(())
    }

    #[test]
    fn exact_quarter_turn_obeys_the_source_to_reference_convention() -> TestResult {
        let source = image(5, 5, 1)?;
        // In downward-positive image coordinates this maps
        // (x_source, y_source) to (4 - y_source, x_source).
        let transform = AffineTransform::new(0.0, -1.0, 1.0, 0.0, 4.0, 0.0)?;

        let result = resample_lanczos3(&source, 5, 5, transform)?;

        assert_eq!(result.statistics().outside_footprint_samples(), 0);
        for reference_y in 0..5 {
            for reference_x in 0..5 {
                let source_x = reference_y;
                let source_y = 4 - reference_x;
                assert_eq!(
                    result.image().get(reference_x, reference_y, 0)?.to_bits(),
                    source.get(source_x, source_y, 0)?.to_bits()
                );
            }
        }
        Ok(())
    }

    #[test]
    fn validates_output_dimensions_and_preserves_algorithm_identity() -> TestResult {
        let source = image(4, 4, 1)?;
        assert!(matches!(
            resample_lanczos3(&source, 0, 4, AffineTransform::IDENTITY),
            Err(ResamplingError::Core(CoreError::ZeroDimension { .. }))
        ));
        let result = resample_lanczos3(&source, 4, 4, AffineTransform::IDENTITY)?;
        assert_eq!(result.algorithm_id(), LANCZOS3_RESAMPLING_ALGORITHM_ID);
        assert_eq!(result.source_to_reference(), AffineTransform::IDENTITY);
        assert_eq!(result.statistics().total_samples(), 16);
        Ok(())
    }
}
