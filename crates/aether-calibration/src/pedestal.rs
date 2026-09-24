use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_core::{CoreError, Dimensions, PixelFlags, ScientificImage};

/// Failure raised while subtracting a bias or dark pedestal image.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PedestalSubtractionError {
    /// The pedestal does not have the signal dimensions.
    DimensionMismatch {
        /// Required dimensions.
        expected: Dimensions,
        /// Received pedestal dimensions.
        actual: Dimensions,
    },
    /// Output image allocation or construction failed.
    Core(CoreError),
}

impl Display for PedestalSubtractionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DimensionMismatch { expected, actual } => write!(
                formatter,
                "pedestal dimensions {}x{}x{} do not match signal dimensions {}x{}x{}",
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

impl Error for PedestalSubtractionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Core(error) => Some(error),
            Self::DimensionMismatch { .. } => None,
        }
    }
}

/// Subtracts one bias or dark pedestal image from a signal image in `f64`.
///
/// The operation is identical for a true bias and a matched short dark; the
/// versioned master plan decides which exclusive source is scientifically
/// appropriate. Both images must have identical dimensions. Existing mask bits
/// are combined without discarding unknown flags. A flagged or unflagged
/// non-finite input produces canonical NaN, with unflagged non-finite inputs
/// adding [`PixelFlags::INVALID`]. Finite overflow is retained and marked
/// invalid. Exact zero is canonicalized to positive zero.
///
/// The function never mutates either input and visits samples once in planar
/// row-major order.
///
/// # Errors
///
/// Returns a dimension mismatch or output allocation failure.
pub fn subtract_pedestal(
    signal: &ScientificImage,
    pedestal: &ScientificImage,
) -> Result<ScientificImage, PedestalSubtractionError> {
    let expected = signal.dimensions();
    let actual = pedestal.dimensions();
    if actual != expected {
        return Err(PedestalSubtractionError::DimensionMismatch { expected, actual });
    }

    let mut output =
        ScientificImage::filled(expected, f64::NAN).map_err(PedestalSubtractionError::Core)?;
    let (output_pixels, output_mask) = output.pixels_and_mask_mut();

    for ((((output, output_flags), signal), signal_flags), (pedestal, pedestal_flags)) in
        output_pixels
            .iter_mut()
            .zip(output_mask.as_mut_slice())
            .zip(signal.pixels())
            .zip(signal.mask().as_slice())
            .zip(pedestal.pixels().iter().zip(pedestal.mask().as_slice()))
    {
        let mut flags = *signal_flags | *pedestal_flags;
        if !flags.is_clear() {
            *output = f64::NAN;
            *output_flags = flags;
            continue;
        }
        if !signal.is_finite() || !pedestal.is_finite() {
            flags |= PixelFlags::INVALID;
            *output = f64::NAN;
            *output_flags = flags;
            continue;
        }

        let difference = signal - pedestal;
        if !difference.is_finite() {
            flags |= PixelFlags::INVALID;
        }
        *output = canonical_zero(difference);
        *output_flags = flags;
    }
    Ok(output)
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
    fn subtracts_in_exact_planar_order_and_canonicalizes_zero() -> TestResult {
        let dimensions = Dimensions::new(2, 1, 2)?;
        let signal = ScientificImage::from_pixels(dimensions, vec![5.0, 4.0, 3.0, -0.0])?;
        let pedestal = ScientificImage::from_pixels(dimensions, vec![1.0, 2.0, 3.0, 0.0])?;

        let output = subtract_pedestal(&signal, &pedestal)?;

        assert_eq!(output.pixels(), &[4.0, 2.0, 0.0, 0.0]);
        assert_eq!(output.pixels()[3].to_bits(), 0.0_f64.to_bits());
        assert!(
            output
                .mask()
                .as_slice()
                .iter()
                .all(|flags| flags.is_clear())
        );
        Ok(())
    }

    #[test]
    fn combines_masks_and_retains_unknown_bits() -> TestResult {
        let mut signal = image(vec![10.0, 20.0])?;
        let mut pedestal = image(vec![1.0, 2.0])?;
        signal.mask_mut().as_mut_slice()[0] = PixelFlags::SATURATED;
        pedestal.mask_mut().as_mut_slice()[0] = PixelFlags::HOT;
        pedestal.mask_mut().as_mut_slice()[1] = PixelFlags::from_bits_retain(0b1000_0000);

        let output = subtract_pedestal(&signal, &pedestal)?;

        assert!(output.pixels().iter().all(|value| value.is_nan()));
        assert_eq!(
            output.mask().as_slice(),
            &[
                PixelFlags::SATURATED | PixelFlags::HOT,
                PixelFlags::from_bits_retain(0b1000_0000),
            ]
        );
        Ok(())
    }

    #[test]
    fn distinguishes_non_finite_input_from_finite_overflow() -> TestResult {
        let signal = image(vec![f64::NAN, f64::MAX])?;
        let pedestal = image(vec![1.0, -f64::MAX])?;

        let output = subtract_pedestal(&signal, &pedestal)?;

        assert!(output.pixels()[0].is_nan());
        assert!(output.pixels()[1].is_infinite());
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
    fn rejects_mismatched_dimensions_without_mutating_inputs() -> TestResult {
        let signal = image(vec![2.0, 3.0])?;
        let pedestal = image(vec![1.0])?;

        assert!(matches!(
            subtract_pedestal(&signal, &pedestal),
            Err(PedestalSubtractionError::DimensionMismatch { .. })
        ));
        assert_eq!(signal.pixels(), &[2.0, 3.0]);
        assert_eq!(pedestal.pixels(), &[1.0]);
        Ok(())
    }
}
