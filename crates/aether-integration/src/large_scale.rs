//! Source-specific spatial expansion of statistically rejected samples.

use std::error::Error;
use std::fmt::{Display, Formatter};

use crate::{RejectionAttribution, RejectionAttributionError, SampleDisposition};

/// Stable identity for the first source-owned large-scale rejection contract.
pub const LARGE_SCALE_REJECTION_ALGORITHM_ID: &str = "source-large-scale-rejection-v1";

/// Smallest supported dyadic detection layer count.
pub const MINIMUM_LARGE_SCALE_LAYERS: u8 = 1;

/// Largest supported dyadic detection layer count.
pub const MAXIMUM_LARGE_SCALE_LAYERS: u8 = 12;

/// Largest supported Chebyshev growth radius in pixels.
pub const MAXIMUM_LARGE_SCALE_GROWTH: u16 = 256;

/// Invalid source-owned large-scale rejection controls.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LargeScaleRejectionParameterError {
    /// Neither rejection tail was enabled.
    NoTailEnabled,
    /// A layer count was outside the supported dyadic range.
    LayersOutsideRange {
        /// Rejected layer count.
        layers: u8,
    },
    /// A growth radius exceeded the bounded implementation limit.
    GrowthOutsideRange {
        /// Rejected growth radius.
        growth: u16,
    },
}

impl Display for LargeScaleRejectionParameterError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoTailEnabled => {
                formatter.write_str("large-scale rejection requires at least one tail")
            }
            Self::LayersOutsideRange { layers } => write!(
                formatter,
                "large-scale rejection layers {layers} are outside {MINIMUM_LARGE_SCALE_LAYERS}..={MAXIMUM_LARGE_SCALE_LAYERS}"
            ),
            Self::GrowthOutsideRange { growth } => write!(
                formatter,
                "large-scale rejection growth {growth} exceeds {MAXIMUM_LARGE_SCALE_GROWTH} pixels"
            ),
        }
    }
}

impl Error for LargeScaleRejectionParameterError {}

/// Failure while classifying or expanding source-owned spatial rejection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LargeScaleRejectionError {
    /// The attribution cube could not be read safely.
    Attribution(RejectionAttributionError),
    /// A derived image, source, or summed-area size overflowed `usize`.
    SizeOverflow,
    /// A bounded working buffer could not be reserved.
    AllocationFailed {
        /// Number of requested elements.
        elements: usize,
    },
}

impl Display for LargeScaleRejectionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Attribution(error) => write!(formatter, "invalid rejection attribution: {error}"),
            Self::SizeOverflow => formatter.write_str("large-scale rejection size overflowed"),
            Self::AllocationFailed { elements } => write!(
                formatter,
                "cannot reserve {elements} large-scale rejection elements"
            ),
        }
    }
}

impl Error for LargeScaleRejectionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Attribution(error) => Some(error),
            Self::SizeOverflow | Self::AllocationFailed { .. } => None,
        }
    }
}

impl From<RejectionAttributionError> for LargeScaleRejectionError {
    fn from(value: RejectionAttributionError) -> Self {
        Self::Attribution(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Statistical rejection tail examined by the spatial classifier.
pub enum LargeScaleRejectionTail {
    /// Low-valued statistical rejects.
    Low,
    /// High-valued statistical rejects.
    High,
}

impl LargeScaleRejectionTail {
    const fn disposition(self) -> SampleDisposition {
        match self {
            Self::Low => SampleDisposition::RejectedLow,
            Self::High => SampleDisposition::RejectedHigh,
        }
    }
}

/// Validated spatial controls for one rejection tail.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LargeScaleTailParameters {
    layers: u8,
    growth: u16,
}

impl LargeScaleTailParameters {
    /// Validates dyadic detection layers and a Chebyshev growth radius.
    pub fn new(layers: u8, growth: u16) -> Result<Self, LargeScaleRejectionParameterError> {
        if !(MINIMUM_LARGE_SCALE_LAYERS..=MAXIMUM_LARGE_SCALE_LAYERS).contains(&layers) {
            return Err(LargeScaleRejectionParameterError::LayersOutsideRange { layers });
        }
        if growth > MAXIMUM_LARGE_SCALE_GROWTH {
            return Err(LargeScaleRejectionParameterError::GrowthOutsideRange { growth });
        }
        Ok(Self { layers, growth })
    }

    /// Number of dyadic structure-detection layers.
    #[must_use]
    pub const fn layers(self) -> u8 {
        self.layers
    }

    /// Chebyshev growth radius in pixels.
    #[must_use]
    pub const fn growth(self) -> u16 {
        self.growth
    }

    /// Radius of the square structure-detection neighbourhood.
    #[must_use]
    pub const fn detection_radius(self) -> usize {
        1_usize << (self.layers - 1)
    }
}

/// Independent low-tail and high-tail large-scale rejection policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LargeScaleRejectionParameters {
    low: Option<LargeScaleTailParameters>,
    high: Option<LargeScaleTailParameters>,
}

impl LargeScaleRejectionParameters {
    /// Requires at least one explicitly configured rejection tail.
    pub fn new(
        low: Option<LargeScaleTailParameters>,
        high: Option<LargeScaleTailParameters>,
    ) -> Result<Self, LargeScaleRejectionParameterError> {
        if low.is_none() && high.is_none() {
            return Err(LargeScaleRejectionParameterError::NoTailEnabled);
        }
        Ok(Self { low, high })
    }

    /// Low-tail spatial controls when enabled.
    #[must_use]
    pub const fn low(self) -> Option<LargeScaleTailParameters> {
        self.low
    }

    /// High-tail spatial controls when enabled.
    #[must_use]
    pub const fn high(self) -> Option<LargeScaleTailParameters> {
        self.high
    }
}

/// Classifies source-owned large-scale seeds in sample-major, source-minor order.
///
/// Each returned byte is zero or one. A rejected sample becomes a seed only
/// when its square dyadic neighbourhood contains at least one full detection
/// diameter of same-tail rejected samples. Original attribution is never
/// modified by this operation.
pub fn classify_large_scale_seeds(
    attribution: &RejectionAttribution,
    tail: LargeScaleRejectionTail,
    parameters: LargeScaleTailParameters,
) -> Result<Vec<u8>, LargeScaleRejectionError> {
    let dimensions = attribution.dimensions();
    let width = dimensions.width();
    let height = dimensions.height();
    let source_count = attribution.source_count();
    let mask_elements = dimensions
        .pixel_count()
        .checked_mul(source_count)
        .ok_or(LargeScaleRejectionError::SizeOverflow)?;
    let mut seeds = zeroed_u8(mask_elements)?;
    let integral_width = width
        .checked_add(1)
        .ok_or(LargeScaleRejectionError::SizeOverflow)?;
    let integral_height = height
        .checked_add(1)
        .ok_or(LargeScaleRejectionError::SizeOverflow)?;
    let integral_elements = integral_width
        .checked_mul(integral_height)
        .ok_or(LargeScaleRejectionError::SizeOverflow)?;
    let mut integral = zeroed_usize(integral_elements)?;
    let radius = parameters.detection_radius();
    let full_diameter = radius
        .checked_mul(2)
        .and_then(|value| value.checked_add(1))
        .ok_or(LargeScaleRejectionError::SizeOverflow)?;
    let plane_pixels = width
        .checked_mul(height)
        .ok_or(LargeScaleRejectionError::SizeOverflow)?;

    for plane in 0..dimensions.planes() {
        let plane_offset = plane
            .checked_mul(plane_pixels)
            .ok_or(LargeScaleRejectionError::SizeOverflow)?;
        for source_index in 0..source_count {
            integral.fill(0);
            for y in 0..height {
                let mut row_sum = 0_usize;
                for x in 0..width {
                    let sample_index = plane_offset + y * width + x;
                    if attribution.disposition(sample_index, source_index)? == tail.disposition() {
                        row_sum += 1;
                    }
                    let integral_index = (y + 1) * integral_width + x + 1;
                    integral[integral_index] = integral[y * integral_width + x + 1] + row_sum;
                }
            }

            for y in 0..height {
                let top = y.saturating_sub(radius);
                let bottom = y.saturating_add(radius).saturating_add(1).min(height);
                for x in 0..width {
                    let sample_index = plane_offset + y * width + x;
                    if attribution.disposition(sample_index, source_index)? != tail.disposition() {
                        continue;
                    }
                    let left = x.saturating_sub(radius);
                    let right = x.saturating_add(radius).saturating_add(1).min(width);
                    let count = rectangle_sum(&integral, integral_width, left, top, right, bottom);
                    if count >= full_diameter {
                        seeds[sample_index * source_count + source_index] = 1;
                    }
                }
            }
        }
    }
    Ok(seeds)
}

fn rectangle_sum(
    integral: &[usize],
    stride: usize,
    left: usize,
    top: usize,
    right: usize,
    bottom: usize,
) -> usize {
    integral[bottom * stride + right] + integral[top * stride + left]
        - integral[top * stride + right]
        - integral[bottom * stride + left]
}

fn zeroed_u8(elements: usize) -> Result<Vec<u8>, LargeScaleRejectionError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(elements)
        .map_err(|_| LargeScaleRejectionError::AllocationFailed { elements })?;
    values.resize(elements, 0);
    Ok(values)
}

fn zeroed_usize(elements: usize) -> Result<Vec<usize>, LargeScaleRejectionError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(elements)
        .map_err(|_| LargeScaleRejectionError::AllocationFailed { elements })?;
    values.resize(elements, 0);
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::Dimensions;

    type TestResult = Result<(), Box<dyn Error>>;

    #[test]
    fn validates_tail_controls_and_derived_scale() -> TestResult {
        let tail = LargeScaleTailParameters::new(4, 17)?;
        assert_eq!(tail.layers(), 4);
        assert_eq!(tail.growth(), 17);
        assert_eq!(tail.detection_radius(), 8);
        assert!(matches!(
            LargeScaleTailParameters::new(0, 0),
            Err(LargeScaleRejectionParameterError::LayersOutsideRange { layers: 0 })
        ));
        assert!(matches!(
            LargeScaleTailParameters::new(1, 257),
            Err(LargeScaleRejectionParameterError::GrowthOutsideRange { growth: 257 })
        ));
        Ok(())
    }

    #[test]
    fn requires_an_explicit_tail() -> TestResult {
        assert_eq!(
            LargeScaleRejectionParameters::new(None, None),
            Err(LargeScaleRejectionParameterError::NoTailEnabled)
        );
        let high = LargeScaleTailParameters::new(2, 2)?;
        assert_eq!(
            LargeScaleRejectionParameters::new(None, Some(high))?.high(),
            Some(high)
        );
        Ok(())
    }

    #[test]
    fn summed_area_classifier_keeps_lines_and_discards_isolated_rejects() -> TestResult {
        let dimensions = Dimensions::new(9, 7, 1)?;
        let mut attribution = RejectionAttribution::new(dimensions, 1)?;
        for x in 1..=7 {
            attribution.set_disposition(3 * 9 + x, 0, SampleDisposition::RejectedHigh)?;
        }
        attribution.set_disposition(9 + 1, 0, SampleDisposition::RejectedHigh)?;
        let parameters = LargeScaleTailParameters::new(2, 0)?;

        let seeds =
            classify_large_scale_seeds(&attribution, LargeScaleRejectionTail::High, parameters)?;
        let low_seeds =
            classify_large_scale_seeds(&attribution, LargeScaleRejectionTail::Low, parameters)?;

        assert_eq!(seeds[3 * 9 + 3], 1);
        assert_eq!(seeds[3 * 9 + 4], 1);
        assert_eq!(seeds[3 * 9 + 5], 1);
        assert_eq!(seeds[9 + 1], 0);
        assert_eq!(seeds.iter().filter(|value| **value != 0).count(), 4);
        assert!(low_seeds.iter().all(|value| *value == 0));
        Ok(())
    }
}
