use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_core::{CoreError, Dimensions, PixelFlags, ScientificImage};

/// Stable identifier for linear Rec. 709 luminance used for source detection.
pub const RGB_LUMINANCE_ALGORITHM_ID: &str = "linear-rec709-luminance-v1";

const RED_WEIGHT: f64 = 0.2126;
const GREEN_WEIGHT: f64 = 0.7152;
const BLUE_WEIGHT: f64 = 0.0722;

/// Canonical planar channel order required by [`RgbLuminanceBuilder`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RgbLuminanceChannel {
    /// First FITS plane.
    Red,
    /// Second FITS plane.
    Green,
    /// Third FITS plane.
    Blue,
}

impl RgbLuminanceChannel {
    const fn weight(self) -> f64 {
        match self {
            Self::Red => RED_WEIGHT,
            Self::Green => GREEN_WEIGHT,
            Self::Blue => BLUE_WEIGHT,
        }
    }

    const fn successor(self) -> Option<Self> {
        match self {
            Self::Red => Some(Self::Green),
            Self::Green => Some(Self::Blue),
            Self::Blue => None,
        }
    }
}

/// Failure to assemble a deterministic luminance detection plane.
#[derive(Debug)]
pub enum RgbDetectionError {
    /// The shared image model rejected dimensions or allocation.
    Core(CoreError),
    /// A supplied channel was not a single plane.
    RequiresSinglePlane {
        /// Actual plane count.
        planes: usize,
    },
    /// A channel did not match the builder dimensions.
    DimensionMismatch {
        /// Required single-plane dimensions.
        expected: Dimensions,
        /// Supplied dimensions.
        actual: Dimensions,
    },
    /// Channels were not supplied exactly once in red, green, blue order.
    ChannelOutOfOrder {
        /// Next required channel, or none after completion.
        expected: Option<RgbLuminanceChannel>,
        /// Channel supplied by the caller.
        actual: RgbLuminanceChannel,
    },
    /// Fewer than three channels were supplied.
    Incomplete,
}

impl Display for RgbDetectionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Core(error) => write!(formatter, "cannot construct RGB luminance: {error}"),
            Self::RequiresSinglePlane { planes } => write!(
                formatter,
                "RGB luminance requires one channel plane at a time, received {planes}"
            ),
            Self::DimensionMismatch { expected, actual } => write!(
                formatter,
                "RGB channel dimensions differ: expected {expected:?}, received {actual:?}"
            ),
            Self::ChannelOutOfOrder { expected, actual } => write!(
                formatter,
                "RGB channel order is invalid: expected {expected:?}, received {actual:?}"
            ),
            Self::Incomplete => {
                formatter.write_str("RGB luminance is missing one or more channels")
            }
        }
    }
}

impl Error for RgbDetectionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Core(error) => Some(error),
            Self::RequiresSinglePlane { .. }
            | Self::DimensionMismatch { .. }
            | Self::ChannelOutOfOrder { .. }
            | Self::Incomplete => None,
        }
    }
}

impl From<CoreError> for RgbDetectionError {
    fn from(value: CoreError) -> Self {
        Self::Core(value)
    }
}

/// Bounded assembler for one linear-light luminance plane.
///
/// Channels are consumed strictly in red, green, blue order. The caller can
/// therefore decode and drop one FITS plane at a time; peak source storage is
/// one channel plus the final luminance image instead of the complete RGB cube.
/// Any mask or non-finite sample invalidates the corresponding luminance sample.
#[derive(Debug)]
pub struct RgbLuminanceBuilder {
    dimensions: Dimensions,
    luminance: ScientificImage,
    corrections: Vec<f64>,
    next: Option<RgbLuminanceChannel>,
}

impl RgbLuminanceBuilder {
    /// Allocates one clear single-plane result initialized to zero.
    pub fn new(width: usize, height: usize) -> Result<Self, RgbDetectionError> {
        let dimensions = Dimensions::new(width, height, 1)?;
        let mut corrections = Vec::new();
        corrections
            .try_reserve_exact(dimensions.pixel_count())
            .map_err(|_| CoreError::AllocationFailed {
                elements: dimensions.pixel_count(),
            })?;
        corrections.resize(dimensions.pixel_count(), 0.0);
        Ok(Self {
            dimensions,
            luminance: ScientificImage::filled(dimensions, 0.0)?,
            corrections,
            next: Some(RgbLuminanceChannel::Red),
        })
    }

    /// Accumulates exactly one canonical RGB channel.
    pub fn push(
        &mut self,
        channel: RgbLuminanceChannel,
        source: &ScientificImage,
    ) -> Result<(), RgbDetectionError> {
        if self.next != Some(channel) {
            return Err(RgbDetectionError::ChannelOutOfOrder {
                expected: self.next,
                actual: channel,
            });
        }
        let actual = source.dimensions();
        if actual.planes() != 1 {
            return Err(RgbDetectionError::RequiresSinglePlane {
                planes: actual.planes(),
            });
        }
        if actual != self.dimensions {
            return Err(RgbDetectionError::DimensionMismatch {
                expected: self.dimensions,
                actual,
            });
        }

        let source_pixels = source.pixels();
        let source_mask = source.mask().as_slice();
        let (output_pixels, output_mask) = self.luminance.pixels_and_mask_mut();
        for index in 0..output_pixels.len() {
            let value = source_pixels[index];
            let mut flags = source_mask[index];
            if !value.is_finite() {
                flags |= PixelFlags::INVALID;
            }
            if !flags.is_clear() || !output_mask.as_slice()[index].is_clear() {
                output_pixels[index] = f64::NAN;
                output_mask.as_mut_slice()[index] |= flags;
                continue;
            }
            // Kahan compensation preserves low-order signal when channel
            // magnitudes differ substantially while retaining a fixed RGB
            // reduction order.
            let contribution = channel.weight() * value;
            let adjusted = contribution - self.corrections[index];
            let sum = output_pixels[index] + adjusted;
            let correction = (sum - output_pixels[index]) - adjusted;
            if contribution.is_finite()
                && adjusted.is_finite()
                && sum.is_finite()
                && correction.is_finite()
            {
                output_pixels[index] = sum;
                self.corrections[index] = correction;
            } else {
                output_pixels[index] = f64::NAN;
                output_mask.as_mut_slice()[index] |= PixelFlags::INVALID;
            }
        }
        self.next = channel.successor();
        Ok(())
    }

    /// Returns the complete luminance plane after all channels were consumed.
    pub fn finish(self) -> Result<ScientificImage, RgbDetectionError> {
        if self.next.is_some() {
            return Err(RgbDetectionError::Incomplete);
        }
        Ok(self.luminance)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn StdError>>;

    fn plane(width: usize, height: usize, pixels: Vec<f64>) -> TestResult<ScientificImage> {
        Ok(ScientificImage::from_pixels(
            Dimensions::new(width, height, 1)?,
            pixels,
        )?)
    }

    #[test]
    fn assembles_rec709_luminance_in_canonical_order() -> TestResult {
        let mut builder = RgbLuminanceBuilder::new(2, 1)?;
        builder.push(RgbLuminanceChannel::Red, &plane(2, 1, vec![10.0, 20.0])?)?;
        builder.push(RgbLuminanceChannel::Green, &plane(2, 1, vec![20.0, 30.0])?)?;
        builder.push(RgbLuminanceChannel::Blue, &plane(2, 1, vec![30.0, 40.0])?)?;
        let luminance = builder.finish()?;

        let expected = RED_WEIGHT * 10.0 + GREEN_WEIGHT * 20.0 + BLUE_WEIGHT * 30.0;
        assert_eq!(luminance.pixels()[0].to_bits(), expected.to_bits());
        assert!(
            luminance
                .mask()
                .as_slice()
                .iter()
                .all(|flags| flags.is_clear())
        );
        Ok(())
    }

    #[test]
    fn combines_invalid_evidence_without_inventing_color() -> TestResult {
        let mut red = plane(2, 1, vec![1.0, f64::NAN])?;
        red.mask_mut().as_mut_slice()[0] = PixelFlags::HOT;
        let clear = plane(2, 1, vec![2.0, 2.0])?;
        let mut builder = RgbLuminanceBuilder::new(2, 1)?;
        builder.push(RgbLuminanceChannel::Red, &red)?;
        builder.push(RgbLuminanceChannel::Green, &clear)?;
        builder.push(RgbLuminanceChannel::Blue, &clear)?;
        let luminance = builder.finish()?;

        assert!(luminance.pixels().iter().all(|value| value.is_nan()));
        assert!(luminance.mask().as_slice()[0].contains(PixelFlags::HOT));
        assert!(luminance.mask().as_slice()[1].contains(PixelFlags::INVALID));
        Ok(())
    }

    #[test]
    fn rejects_wrong_order_shape_and_incomplete_input() -> TestResult {
        let mut builder = RgbLuminanceBuilder::new(2, 1)?;
        assert!(matches!(
            builder.push(RgbLuminanceChannel::Green, &plane(2, 1, vec![0.0; 2])?),
            Err(RgbDetectionError::ChannelOutOfOrder { .. })
        ));
        assert!(matches!(
            builder.push(RgbLuminanceChannel::Red, &plane(1, 2, vec![0.0; 2])?),
            Err(RgbDetectionError::DimensionMismatch { .. })
        ));
        assert!(matches!(
            builder.finish(),
            Err(RgbDetectionError::Incomplete)
        ));
        Ok(())
    }
}
