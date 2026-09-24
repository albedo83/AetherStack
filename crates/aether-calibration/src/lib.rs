//! Strict double-precision calibration primitives.
//!
//! The strict CPU primitives cover exclusive bias-or-dark pedestal subtraction,
//! robust flat normalization, and dark-plus-normalized-flat light calibration.
//! Master orchestration, dark scaling, rejection, and uncertainty propagation
//! remain separate versioned policies and are never guessed here.

mod flat;
mod master;
mod pedestal;

use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_core::{CoreError, Dimensions, PixelFlags, ScientificImage};

pub use flat::{
    FlatNormalizationError, FlatNormalizationParameters, FlatNormalizationSupport, NormalizedFlat,
    normalize_flat,
};
pub use master::{
    CalibrationMasterKind, MasterIntegrationAlgorithm, StrictMeanMaster,
    construct_strict_mean_master,
};
pub use pedestal::{PedestalSubtractionError, subtract_pedestal};

/// Validated parameters for dark-and-flat calibration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CalibrationParameters {
    minimum_absolute_flat: f64,
}

impl CalibrationParameters {
    /// Creates calibration parameters with an explicit flat divisor floor.
    ///
    /// A flat sample is unusable when its absolute value is less than or equal
    /// to this threshold. No default is provided because changing the threshold
    /// can change scientific results and must remain an explicit policy choice.
    ///
    /// # Errors
    ///
    /// Returns [`CalibrationError::InvalidFlatThreshold`] for a negative or
    /// non-finite threshold.
    pub fn new(minimum_absolute_flat: f64) -> Result<Self, CalibrationError> {
        if !minimum_absolute_flat.is_finite() || minimum_absolute_flat < 0.0 {
            return Err(CalibrationError::InvalidFlatThreshold {
                value: minimum_absolute_flat,
            });
        }
        Ok(Self {
            minimum_absolute_flat: if minimum_absolute_flat == 0.0 {
                0.0
            } else {
                minimum_absolute_flat
            },
        })
    }

    /// Smallest accepted absolute flat value.
    #[must_use]
    pub const fn minimum_absolute_flat(self) -> f64 {
        self.minimum_absolute_flat
    }
}

/// Input role associated with a dimension mismatch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CalibrationInput {
    /// Dark master.
    Dark,
    /// Normalized flat master.
    Flat,
}

impl Display for CalibrationInput {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dark => formatter.write_str("dark"),
            Self::Flat => formatter.write_str("flat"),
        }
    }
}

/// Failure raised before a calibrated tile can be produced.
#[derive(Clone, Debug, PartialEq)]
pub enum CalibrationError {
    /// Flat threshold is negative, NaN, or infinite.
    InvalidFlatThreshold {
        /// Rejected threshold.
        value: f64,
    },
    /// A calibration master does not have the signal dimensions.
    DimensionMismatch {
        /// Mismatched input role.
        input: CalibrationInput,
        /// Required dimensions.
        expected: Dimensions,
        /// Received dimensions.
        actual: Dimensions,
    },
    /// Output image allocation or construction failed.
    Core(CoreError),
}

impl Display for CalibrationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidFlatThreshold { value } => {
                write!(
                    formatter,
                    "flat threshold must be finite and non-negative, received {value}"
                )
            }
            Self::DimensionMismatch {
                input,
                expected,
                actual,
            } => write!(
                formatter,
                "{input} dimensions {}x{}x{} do not match signal dimensions {}x{}x{}",
                actual.width(),
                actual.height(),
                actual.planes(),
                expected.width(),
                expected.height(),
                expected.planes()
            ),
            Self::Core(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for CalibrationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Core(error) => Some(error),
            Self::InvalidFlatThreshold { .. } | Self::DimensionMismatch { .. } => None,
        }
    }
}

/// Applies `(signal - dark) / flat` in deterministic planar order using `f64`.
///
/// All images must have identical dimensions. Input flags are combined exactly.
/// Any flagged or non-finite input produces a canonical NaN output; non-finite
/// inputs additionally set [`PixelFlags::INVALID`]. A flat at or below the
/// configured absolute threshold also produces NaN and `INVALID`. A finite-input
/// calculation that overflows retains its IEEE 754 result and sets `INVALID`.
///
/// The function never mutates an input and never silently substitutes a missing
/// calibration master or a flat value.
///
/// # Errors
///
/// Returns a dimension mismatch or an output allocation failure.
pub fn calibrate_dark_flat(
    signal: &ScientificImage,
    dark: &ScientificImage,
    flat: &ScientificImage,
    parameters: CalibrationParameters,
) -> Result<ScientificImage, CalibrationError> {
    validate_dimensions(signal.dimensions(), dark, CalibrationInput::Dark)?;
    validate_dimensions(signal.dimensions(), flat, CalibrationInput::Flat)?;

    let mut output =
        ScientificImage::filled(signal.dimensions(), f64::NAN).map_err(CalibrationError::Core)?;
    let (output_pixels, output_mask) = output.pixels_and_mask_mut();

    let samples = output_pixels
        .iter_mut()
        .zip(output_mask.as_mut_slice())
        .zip(signal.pixels().iter().zip(signal.mask().as_slice()))
        .zip(dark.pixels().iter().zip(dark.mask().as_slice()))
        .zip(flat.pixels().iter().zip(flat.mask().as_slice()));

    for (
        (((output, output_flags), (signal, signal_flags)), (dark, dark_flags)),
        (flat, flat_flags),
    ) in samples
    {
        let mut flags = *signal_flags | *dark_flags | *flat_flags;
        if !flags.is_clear() {
            *output = f64::NAN;
            *output_flags = flags;
            continue;
        }
        if !signal.is_finite() || !dark.is_finite() || !flat.is_finite() {
            flags |= PixelFlags::INVALID;
            *output = f64::NAN;
            *output_flags = flags;
            continue;
        }
        if flat.abs() <= parameters.minimum_absolute_flat {
            flags |= PixelFlags::INVALID;
            *output = f64::NAN;
            *output_flags = flags;
            continue;
        }

        let calibrated = (signal - dark) / flat;
        if !calibrated.is_finite() {
            flags |= PixelFlags::INVALID;
        }
        *output = calibrated;
        *output_flags = flags;
    }

    Ok(output)
}

fn validate_dimensions(
    expected: Dimensions,
    input: &ScientificImage,
    role: CalibrationInput,
) -> Result<(), CalibrationError> {
    let actual = input.dimensions();
    if actual != expected {
        return Err(CalibrationError::DimensionMismatch {
            input: role,
            expected,
            actual,
        });
    }
    Ok(())
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

    fn parameters() -> TestResult<CalibrationParameters> {
        Ok(CalibrationParameters::new(1.0e-12)?)
    }

    #[test]
    fn applies_dark_subtraction_then_normalized_flat_division() -> TestResult {
        let signal = image(vec![10.0, 20.0, 30.0])?;
        let dark = image(vec![2.0, 4.0, 6.0])?;
        let flat = image(vec![0.5, 1.0, 2.0])?;

        let output = calibrate_dark_flat(&signal, &dark, &flat, parameters()?)?;

        assert_eq!(output.pixels(), &[16.0, 16.0, 12.0]);
        assert!(
            output
                .mask()
                .as_slice()
                .iter()
                .all(|flags| flags.is_clear())
        );
        assert_eq!(signal.pixels(), &[10.0, 20.0, 30.0]);
        Ok(())
    }

    #[test]
    fn rejects_each_mismatched_master() -> TestResult {
        let signal = image(vec![1.0, 2.0])?;
        let dark = image(vec![0.0])?;
        let flat = image(vec![1.0])?;
        assert!(matches!(
            calibrate_dark_flat(&signal, &dark, &signal, parameters()?),
            Err(CalibrationError::DimensionMismatch {
                input: CalibrationInput::Dark,
                ..
            })
        ));
        assert!(matches!(
            calibrate_dark_flat(&signal, &signal, &flat, parameters()?),
            Err(CalibrationError::DimensionMismatch {
                input: CalibrationInput::Flat,
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn combines_input_masks_without_erasing_unknown_bits() -> TestResult {
        let mut signal = image(vec![10.0, 10.0, 10.0, 10.0])?;
        let mut dark = image(vec![1.0, 1.0, 1.0, 1.0])?;
        let mut flat = image(vec![1.0, 1.0, 1.0, 1.0])?;
        signal.mask_mut().as_mut_slice()[0] = PixelFlags::SATURATED;
        dark.mask_mut().as_mut_slice()[1] = PixelFlags::HOT;
        flat.mask_mut().as_mut_slice()[2] = PixelFlags::COLD;
        flat.mask_mut().as_mut_slice()[3] = PixelFlags::from_bits_retain(0b1000_0000);

        let output = calibrate_dark_flat(&signal, &dark, &flat, parameters()?)?;

        assert!(output.pixels().iter().all(|value| value.is_nan()));
        assert_eq!(
            output.mask().as_slice(),
            &[
                PixelFlags::SATURATED,
                PixelFlags::HOT,
                PixelFlags::COLD,
                PixelFlags::from_bits_retain(0b1000_0000),
            ]
        );
        Ok(())
    }

    #[test]
    fn marks_non_finite_inputs_and_unsafe_flat_divisors() -> TestResult {
        let signal = image(vec![f64::NAN, 1.0, 1.0, f64::MAX])?;
        let dark = image(vec![0.0, f64::INFINITY, 0.0, -f64::MAX])?;
        let flat = image(vec![1.0, 1.0, 1.0e-13, 0.5])?;

        let output = calibrate_dark_flat(&signal, &dark, &flat, parameters()?)?;

        assert!(output.pixels()[0].is_nan());
        assert!(output.pixels()[1].is_nan());
        assert!(output.pixels()[2].is_nan());
        assert!(output.pixels()[3].is_infinite());
        assert!(
            output
                .mask()
                .as_slice()
                .iter()
                .all(|flags| flags.contains(PixelFlags::INVALID))
        );
        Ok(())
    }

    #[test]
    fn validates_and_canonicalizes_flat_threshold() {
        assert!(matches!(
            CalibrationParameters::new(-1.0),
            Err(CalibrationError::InvalidFlatThreshold { .. })
        ));
        assert!(matches!(
            CalibrationParameters::new(f64::NAN),
            Err(CalibrationError::InvalidFlatThreshold { .. })
        ));
        assert!(matches!(
            CalibrationParameters::new(f64::INFINITY),
            Err(CalibrationError::InvalidFlatThreshold { .. })
        ));
        let negative_zero = CalibrationParameters::new(-0.0);
        assert!(negative_zero.is_ok());
        assert_eq!(
            negative_zero.map(CalibrationParameters::minimum_absolute_flat),
            Ok(0.0)
        );
    }

    #[test]
    fn preserves_planar_order_across_multiple_planes() -> TestResult {
        let dimensions = Dimensions::new(2, 1, 2)?;
        let signal = ScientificImage::from_pixels(dimensions, vec![10.0, 20.0, 30.0, 40.0])?;
        let dark = ScientificImage::from_pixels(dimensions, vec![1.0, 2.0, 3.0, 4.0])?;
        let flat = ScientificImage::from_pixels(dimensions, vec![1.0, 2.0, 3.0, 4.0])?;

        let output = calibrate_dark_flat(&signal, &dark, &flat, parameters()?)?;

        assert_eq!(output.pixels(), &[9.0, 9.0, 9.0, 9.0]);
        Ok(())
    }
}
