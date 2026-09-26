//! Deterministic Bayer demosaicing for calibrated astronomical images.
//!
//! The strict implementation is a double-precision Malvar-He-Cutler 5x5
//! gradient-corrected linear filter. It is intentionally a small, scalar CPU
//! oracle: future tiled, SIMD, or GPU implementations must agree with this
//! traversal and its documented numerical contract before replacing it.

use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_core::{CoreError, Dimensions, PixelFlags, ScientificImage};
use aether_metadata::BayerPattern;

/// Stable identifier included in cache keys and processing provenance.
pub const MALVAR_HE_CUTLER_ALGORITHM_ID: &str = "malvar-he-cutler-f64-v1";

const RED: usize = 0;
const GREEN: usize = 1;
const BLUE: usize = 2;

// Coefficients are integer numerators over a power-of-two denominator. This
// preserves the published filters exactly in binary floating point and avoids
// decimal constants that would obscure review of the kernel.
const GREEN_AT_RED_OR_BLUE: Kernel = Kernel {
    denominator: 8.0,
    taps: &[
        (0, -2, -1),
        (0, -1, 2),
        (-2, 0, -1),
        (-1, 0, 2),
        (0, 0, 4),
        (1, 0, 2),
        (2, 0, -1),
        (0, 1, 2),
        (0, 2, -1),
    ],
};

const OPPOSITE_AT_RED_OR_BLUE: Kernel = Kernel {
    denominator: 16.0,
    taps: &[
        (0, -2, -3),
        (-1, -1, 4),
        (1, -1, 4),
        (-2, 0, -3),
        (0, 0, 12),
        (2, 0, -3),
        (-1, 1, 4),
        (1, 1, 4),
        (0, 2, -3),
    ],
};

const COLOR_AT_GREEN_HORIZONTAL: Kernel = Kernel {
    denominator: 16.0,
    taps: &[
        (0, -2, 1),
        (-1, -1, -2),
        (1, -1, -2),
        (-2, 0, -2),
        (-1, 0, 8),
        (0, 0, 10),
        (1, 0, 8),
        (2, 0, -2),
        (-1, 1, -2),
        (1, 1, -2),
        (0, 2, 1),
    ],
};

#[derive(Clone, Copy)]
struct Kernel {
    denominator: f64,
    taps: &'static [(i8, i8, i16)],
}

/// Failure produced before a complete RGB image can be returned.
#[derive(Clone, Debug, PartialEq)]
pub enum DemosaicError {
    /// The source must be a single-plane CFA mosaic.
    PlaneCount {
        /// Number of received source planes.
        actual: usize,
    },
    /// Reflection at the 5x5 boundary requires at least two samples per axis.
    ImageTooSmall {
        /// Source width.
        width: usize,
        /// Source height.
        height: usize,
    },
    /// Only the four standard Bayer phases have a defined reconstruction.
    UnsupportedPattern(String),
    /// A checked image or mask invariant failed.
    Core(CoreError),
}

impl Display for DemosaicError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PlaneCount { actual } => write!(
                formatter,
                "Bayer demosaicing requires one source plane, received {actual}"
            ),
            Self::ImageTooSmall { width, height } => write!(
                formatter,
                "Bayer demosaicing requires at least 2x2 samples, received {width}x{height}"
            ),
            Self::UnsupportedPattern(pattern) => {
                write!(formatter, "unsupported Bayer pattern `{pattern}`")
            }
            Self::Core(error) => write!(formatter, "cannot construct demosaiced image: {error}"),
        }
    }
}

impl Error for DemosaicError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Core(error) => Some(error),
            Self::PlaneCount { .. } | Self::ImageTooSmall { .. } | Self::UnsupportedPattern(_) => {
                None
            }
        }
    }
}

impl From<CoreError> for DemosaicError {
    fn from(error: CoreError) -> Self {
        Self::Core(error)
    }
}

/// Reconstructs planar linear RGB from one Bayer CFA plane.
///
/// Samples are evaluated in plane-row-column order with the published 5x5
/// gradient-corrected filters. Measured CFA samples are copied bit-for-bit into
/// their corresponding output channel. Missing channels use whole-sample
/// symmetric reflection at image boundaries. Values are never clipped, because
/// negative values and overshoot remain meaningful after calibration.
///
/// Every non-zero interpolation tap must be finite and unmasked. Otherwise the
/// affected output channel becomes NaN and receives the union of supporting
/// flags plus [`PixelFlags::MISSING`]. A non-finite supporting value additionally
/// contributes [`PixelFlags::INVALID`]. This conservative rule prevents a hot,
/// saturated, rejected, or missing input from being hidden by interpolation.
///
/// # Errors
///
/// Returns a typed error for a multi-plane source, a source smaller than 2x2,
/// an unknown Bayer declaration, checked-size overflow, or allocation failure.
pub fn demosaic_malvar_he_cutler(
    source: &ScientificImage,
    pattern: &BayerPattern,
) -> Result<ScientificImage, DemosaicError> {
    let source_dimensions = source.dimensions();
    if source_dimensions.planes() != 1 {
        return Err(DemosaicError::PlaneCount {
            actual: source_dimensions.planes(),
        });
    }
    let width = source_dimensions.width();
    let height = source_dimensions.height();
    if width < 2 || height < 2 {
        return Err(DemosaicError::ImageTooSmall { width, height });
    }
    if let BayerPattern::Other(name) = pattern {
        return Err(DemosaicError::UnsupportedPattern(name.clone()));
    }

    let output_dimensions = Dimensions::new(width, height, 3)?;
    let mut output = ScientificImage::filled(output_dimensions, 0.0)?;
    let area = width
        .checked_mul(height)
        .ok_or(CoreError::PixelCountOverflow {
            width,
            height,
            planes: 1,
        })?;
    let source_pixels = source.pixels();
    let source_flags = source.mask().as_slice();
    let (output_pixels, output_mask) = output.pixels_and_mask_mut();
    let output_flags = output_mask.as_mut_slice();

    for channel in [RED, GREEN, BLUE] {
        for y in 0..height {
            for x in 0..width {
                let output_index = channel * area + y * width + x;
                let sampled_channel = sampled_channel(pattern, x, y);
                let (value, flags) = if channel == sampled_channel {
                    direct_sample(source_pixels, source_flags, y * width + x)
                } else {
                    let (kernel, transpose) =
                        select_kernel(pattern, x, y, sampled_channel, channel);
                    interpolate(
                        source_pixels,
                        source_flags,
                        width,
                        height,
                        x,
                        y,
                        kernel,
                        transpose,
                    )
                };
                output_pixels[output_index] = value;
                output_flags[output_index] = flags;
            }
        }
    }
    Ok(output)
}

fn direct_sample(pixels: &[f64], flags: &[PixelFlags], index: usize) -> (f64, PixelFlags) {
    let value = pixels[index];
    let mut output_flags = flags[index];
    if !value.is_finite() {
        output_flags |= PixelFlags::INVALID | PixelFlags::MISSING;
    }
    (value, output_flags)
}

#[allow(clippy::too_many_arguments)]
fn interpolate(
    pixels: &[f64],
    flags: &[PixelFlags],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    kernel: Kernel,
    transpose: bool,
) -> (f64, PixelFlags) {
    let mut numerator = 0.0_f64;
    let mut combined_flags = PixelFlags::CLEAR;
    let mut usable = true;
    for &(kernel_x, kernel_y, coefficient) in kernel.taps {
        let (offset_x, offset_y) = if transpose {
            (kernel_y, kernel_x)
        } else {
            (kernel_x, kernel_y)
        };
        let sample_x = reflect_coordinate(x, offset_x, width);
        let sample_y = reflect_coordinate(y, offset_y, height);
        let index = sample_y * width + sample_x;
        let value = pixels[index];
        let sample_flags = flags[index];
        combined_flags |= sample_flags;
        if !sample_flags.is_clear() {
            usable = false;
        }
        if value.is_finite() {
            numerator += f64::from(coefficient) * value;
        } else {
            usable = false;
            combined_flags |= PixelFlags::INVALID;
        }
    }
    if usable {
        (numerator / kernel.denominator, PixelFlags::CLEAR)
    } else {
        (f64::NAN, combined_flags | PixelFlags::MISSING)
    }
}

fn select_kernel(
    pattern: &BayerPattern,
    x: usize,
    y: usize,
    sampled: usize,
    target: usize,
) -> (Kernel, bool) {
    if target == GREEN {
        return (GREEN_AT_RED_OR_BLUE, false);
    }
    if sampled != GREEN {
        return (OPPOSITE_AT_RED_OR_BLUE, false);
    }

    // At green locations the two chromatic filters lie either horizontally or
    // vertically. The horizontal form is transposed when the target color is
    // found on the vertical axis.
    let horizontal_color = sampled_channel(pattern, x + 1, y);
    (COLOR_AT_GREEN_HORIZONTAL, target != horizontal_color)
}

fn sampled_channel(pattern: &BayerPattern, x: usize, y: usize) -> usize {
    let phase = ((y & 1) << 1) | (x & 1);
    match pattern {
        BayerPattern::Rggb => [RED, GREEN, GREEN, BLUE][phase],
        BayerPattern::Bggr => [BLUE, GREEN, GREEN, RED][phase],
        BayerPattern::Grbg => [GREEN, RED, BLUE, GREEN][phase],
        BayerPattern::Gbrg => [GREEN, BLUE, RED, GREEN][phase],
        // Rejected by the public boundary before this helper is called.
        BayerPattern::Other(_) => GREEN,
    }
}

fn reflect_coordinate(origin: usize, offset: i8, length: usize) -> usize {
    let period = 2 * (length - 1);
    let shifted = origin as i128 + i128::from(offset);
    let position = shifted.rem_euclid(period as i128) as usize;
    if position < length {
        position
    } else {
        period - position
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn Error>>;

    fn image(width: usize, height: usize, pixels: Vec<f64>) -> Result<ScientificImage, CoreError> {
        ScientificImage::from_pixels(Dimensions::new(width, height, 1)?, pixels)
    }

    #[test]
    fn preserves_uniform_signal_for_every_standard_phase() -> TestResult {
        for pattern in [
            BayerPattern::Rggb,
            BayerPattern::Bggr,
            BayerPattern::Grbg,
            BayerPattern::Gbrg,
        ] {
            let source = image(6, 4, vec![42.5; 24])?;
            let rgb = demosaic_malvar_he_cutler(&source, &pattern)?;
            assert!(
                rgb.pixels()
                    .iter()
                    .all(|value| value.to_bits() == 42.5_f64.to_bits())
            );
            assert!(rgb.mask().as_slice().iter().all(|flags| flags.is_clear()));
        }
        Ok(())
    }

    #[test]
    fn preserves_measured_samples_in_their_exact_rgb_plane() -> TestResult {
        for pattern in [
            BayerPattern::Rggb,
            BayerPattern::Bggr,
            BayerPattern::Grbg,
            BayerPattern::Gbrg,
        ] {
            let pixels = (0_u32..16).map(f64::from).collect();
            let source = image(4, 4, pixels)?;
            let rgb = demosaic_malvar_he_cutler(&source, &pattern)?;
            let area = 16;
            for y in 0..4 {
                for x in 0..4 {
                    let source_index = y * 4 + x;
                    let channel = sampled_channel(&pattern, x, y);
                    assert_eq!(
                        rgb.pixels()[channel * area + source_index].to_bits(),
                        (source_index as f64).to_bits()
                    );
                }
            }
        }
        Ok(())
    }

    #[test]
    fn locks_published_center_coefficients() -> TestResult {
        let mut pixels = vec![0.0; 25];
        pixels[2 * 5 + 2] = 8.0;
        let source = image(5, 5, pixels)?;
        let rgb = demosaic_malvar_he_cutler(&source, &BayerPattern::Rggb)?;
        let center = 2 * 5 + 2;
        assert_eq!(rgb.pixels()[center].to_bits(), 8.0_f64.to_bits());
        assert_eq!(rgb.pixels()[25 + center].to_bits(), 4.0_f64.to_bits());
        assert_eq!(rgb.pixels()[50 + center].to_bits(), 6.0_f64.to_bits());
        Ok(())
    }

    #[test]
    fn orients_the_green_site_chromatic_kernels_by_cfa_phase() -> TestResult {
        let mut pixels = vec![0.0; 35];
        // (3, 2) is green on a red row in RGGB. Its right-hand neighbor is red,
        // so that sample contributes to red but not to the transposed blue
        // reconstruction at the selected green site.
        pixels[2 * 7 + 4] = 16.0;
        let source = image(7, 5, pixels)?;
        let rgb = demosaic_malvar_he_cutler(&source, &BayerPattern::Rggb)?;
        let center = 2 * 7 + 3;
        assert_eq!(rgb.pixels()[center].to_bits(), 8.0_f64.to_bits());
        assert_eq!(rgb.pixels()[70 + center].to_bits(), 0.0_f64.to_bits());
        Ok(())
    }

    #[test]
    fn reflects_whole_samples_at_every_border() {
        assert_eq!(reflect_coordinate(0, -2, 5), 2);
        assert_eq!(reflect_coordinate(0, -1, 5), 1);
        assert_eq!(reflect_coordinate(4, 1, 5), 3);
        assert_eq!(reflect_coordinate(4, 2, 5), 2);
        assert_eq!(reflect_coordinate(0, -2, 2), 0);
    }

    #[test]
    fn propagates_a_flagged_support_sample_without_hiding_it() -> TestResult {
        let mut source = image(5, 5, vec![1.0; 25])?;
        source.mark(3, 2, 0, PixelFlags::HOT)?;
        let rgb = demosaic_malvar_he_cutler(&source, &BayerPattern::Rggb)?;
        let center = 2 * 5 + 2;
        let green = rgb.pixels()[25 + center];
        let flags = rgb.mask().as_slice()[25 + center];
        assert!(green.is_nan());
        assert!(flags.contains(PixelFlags::HOT));
        assert!(flags.contains(PixelFlags::MISSING));
        assert_eq!(rgb.pixels()[center].to_bits(), 1.0_f64.to_bits());
        assert!(rgb.mask().as_slice()[center].is_clear());
        Ok(())
    }

    #[test]
    fn marks_non_finite_direct_and_interpolated_samples() -> TestResult {
        let mut source = image(5, 5, vec![1.0; 25])?;
        source.pixels_mut()[2 * 5 + 2] = f64::NAN;
        let rgb = demosaic_malvar_he_cutler(&source, &BayerPattern::Rggb)?;
        for channel in 0..3 {
            let flags = rgb.mask().as_slice()[channel * 25 + 12];
            assert!(rgb.pixels()[channel * 25 + 12].is_nan());
            assert!(flags.contains(PixelFlags::INVALID));
            assert!(flags.contains(PixelFlags::MISSING));
        }
        Ok(())
    }

    #[test]
    fn retains_unclipped_filter_overshoot() -> TestResult {
        let mut pixels = vec![0.0; 25];
        pixels[2 * 5] = 80.0;
        let source = image(5, 5, pixels)?;
        let rgb = demosaic_malvar_he_cutler(&source, &BayerPattern::Rggb)?;
        assert!(rgb.pixels()[25 + 2 * 5 + 2] < 0.0);
        Ok(())
    }

    #[test]
    fn rejects_invalid_shape_and_unknown_pattern() -> TestResult {
        let three_plane = ScientificImage::filled(Dimensions::new(2, 2, 3)?, 0.0)?;
        assert_eq!(
            demosaic_malvar_he_cutler(&three_plane, &BayerPattern::Rggb),
            Err(DemosaicError::PlaneCount { actual: 3 })
        );
        let narrow = image(1, 2, vec![0.0; 2])?;
        assert_eq!(
            demosaic_malvar_he_cutler(&narrow, &BayerPattern::Rggb),
            Err(DemosaicError::ImageTooSmall {
                width: 1,
                height: 2
            })
        );
        let source = image(2, 2, vec![0.0; 4])?;
        assert_eq!(
            demosaic_malvar_he_cutler(&source, &BayerPattern::Other("CYGM".to_owned())),
            Err(DemosaicError::UnsupportedPattern("CYGM".to_owned()))
        );
        Ok(())
    }
}
