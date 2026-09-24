use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_core::{CompensatedSum, CoreError, Dimensions, PixelFlags, ScientificImage};

/// Stable identifier for the phase-neutral Bayer-cell detection transform.
pub const CFA_CELL_MEAN_ALGORITHM_ID: &str = "cfa-cell-mean-v1";

/// Failure to prepare a raw Bayer mosaic for deterministic source detection.
#[derive(Debug)]
pub enum CfaDetectionError {
    /// The shared image model rejected an allocation or derived dimension.
    Core(CoreError),
    /// A raw CFA mosaic must contain exactly one stored plane.
    RequiresSinglePlane {
        /// Number of planes supplied by the caller.
        planes: usize,
    },
    /// A complete Bayer cell requires at least two pixels on each axis.
    ImageTooSmall {
        /// Source width in pixels.
        width: usize,
        /// Source height in pixels.
        height: usize,
    },
    /// Partial cells are rejected instead of being cropped implicitly.
    OddDimensions {
        /// Source width in pixels.
        width: usize,
        /// Source height in pixels.
        height: usize,
    },
}

impl Display for CfaDetectionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Core(error) => write!(formatter, "cannot construct CFA detection plane: {error}"),
            Self::RequiresSinglePlane { planes } => write!(
                formatter,
                "CFA detection requires exactly one source plane, received {planes}"
            ),
            Self::ImageTooSmall { width, height } => write!(
                formatter,
                "CFA detection requires at least a 2x2 source, received {width}x{height}"
            ),
            Self::OddDimensions { width, height } => write!(
                formatter,
                "CFA detection requires complete 2x2 cells, received {width}x{height}"
            ),
        }
    }
}

impl Error for CfaDetectionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Core(error) => Some(error),
            Self::RequiresSinglePlane { .. }
            | Self::ImageTooSmall { .. }
            | Self::OddDimensions { .. } => None,
        }
    }
}

impl From<CoreError> for CfaDetectionError {
    fn from(value: CoreError) -> Self {
        Self::Core(value)
    }
}

/// Collapses complete 2x2 Bayer cells into a phase-neutral detection plane.
///
/// Every output sample is the compensated arithmetic mean of the four source
/// samples in canonical row-major order. All standard Bayer patterns contain
/// the same two green, one red, and one blue samples in each aligned cell, so
/// their ordering cannot introduce alternating CFA structure into detection.
/// The output coordinates are therefore spaced two source pixels apart.
///
/// A cell contributes only when all four inputs are clear and finite. Otherwise
/// its output is `NaN` and carries the union of source flags plus
/// [`PixelFlags::INVALID`] for any unflagged non-finite value. Requiring complete
/// support prevents a damaged sample from changing the effective color weights.
///
/// # Errors
///
/// Returns a typed error for multi-plane, undersized, or odd-sized inputs and
/// for checked dimension or allocation failures. The caller must separately
/// prove that the source is an origin-aligned standard Bayer mosaic.
pub fn prepare_cfa_cell_mean(
    source: ScientificImage,
) -> Result<ScientificImage, CfaDetectionError> {
    let source_dimensions = source.dimensions();
    if source_dimensions.planes() != 1 {
        return Err(CfaDetectionError::RequiresSinglePlane {
            planes: source_dimensions.planes(),
        });
    }
    let width = source_dimensions.width();
    let height = source_dimensions.height();
    if width < 2 || height < 2 {
        return Err(CfaDetectionError::ImageTooSmall { width, height });
    }
    if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err(CfaDetectionError::OddDimensions { width, height });
    }

    let output_width = width / 2;
    let output_height = height / 2;
    let output_dimensions = Dimensions::new(output_width, output_height, 1)?;
    let mut output = ScientificImage::filled(output_dimensions, f64::NAN)?;
    let source_pixels = source.pixels();
    let source_flags = source.mask().as_slice();
    let (output_pixels, output_mask) = output.pixels_and_mask_mut();

    for output_y in 0..output_height {
        let source_y = output_y * 2;
        for output_x in 0..output_width {
            let source_x = output_x * 2;
            let first = source_y * width + source_x;
            let source_indices = [first, first + 1, first + width, first + width + 1];
            let output_index = output_y * output_width + output_x;
            let mut combined_flags = PixelFlags::CLEAR;
            let mut sum = CompensatedSum::new();

            for source_index in source_indices {
                let value = source_pixels[source_index];
                combined_flags |= source_flags[source_index];
                if value.is_finite() {
                    sum.add(value);
                } else {
                    combined_flags |= PixelFlags::INVALID;
                }
            }

            if combined_flags.is_clear() {
                output_pixels[output_index] = sum.total() * 0.25;
            } else {
                output_mask.as_mut_slice()[output_index] = combined_flags;
            }
        }
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn StdError>>;

    fn image(width: usize, height: usize, pixels: Vec<f64>) -> TestResult<ScientificImage> {
        Ok(ScientificImage::from_pixels(
            Dimensions::new(width, height, 1)?,
            pixels,
        )?)
    }

    #[test]
    fn standard_bayer_phase_permutations_produce_the_same_cells() -> TestResult {
        let patterns = [
            vec![10.0, 20.0, 30.0, 40.0],
            vec![40.0, 30.0, 20.0, 10.0],
            vec![20.0, 10.0, 40.0, 30.0],
            vec![30.0, 40.0, 10.0, 20.0],
        ];

        for pixels in patterns {
            let plane = prepare_cfa_cell_mean(image(2, 2, pixels)?)?;
            assert_eq!(plane.dimensions(), Dimensions::new(1, 1, 1)?);
            assert_eq!(plane.pixels()[0].to_bits(), 25.0_f64.to_bits());
            assert!(plane.mask().as_slice()[0].is_clear());
        }
        Ok(())
    }

    #[test]
    fn compensation_preserves_small_signal_inside_a_cell() -> TestResult {
        let plane = prepare_cfa_cell_mean(image(2, 2, vec![1.0e16, 1.0, -1.0e16, 3.0])?)?;

        assert_eq!(plane.pixels()[0].to_bits(), 1.0_f64.to_bits());
        Ok(())
    }

    #[test]
    fn invalid_or_masked_input_invalidates_the_complete_cell() -> TestResult {
        let mut source = image(4, 2, vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, f64::NAN, 8.0])?;
        source.mask_mut().as_mut_slice()[0] = PixelFlags::HOT;

        let plane = prepare_cfa_cell_mean(source)?;

        assert!(plane.pixels()[0].is_nan());
        assert!(plane.mask().as_slice()[0].contains(PixelFlags::HOT));
        assert!(plane.pixels()[1].is_nan());
        assert!(plane.mask().as_slice()[1].contains(PixelFlags::INVALID));
        Ok(())
    }

    #[test]
    fn rejects_partial_cells_and_multiple_planes() -> TestResult {
        assert!(matches!(
            prepare_cfa_cell_mean(image(3, 2, vec![0.0; 6])?),
            Err(CfaDetectionError::OddDimensions {
                width: 3,
                height: 2
            })
        ));
        let multiple = ScientificImage::from_pixels(Dimensions::new(2, 2, 2)?, vec![0.0; 8])?;
        assert!(matches!(
            prepare_cfa_cell_mean(multiple),
            Err(CfaDetectionError::RequiresSinglePlane { planes: 2 })
        ));
        Ok(())
    }
}
