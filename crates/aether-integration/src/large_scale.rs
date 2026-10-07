//! Source-specific spatial expansion of statistically rejected samples.

use std::error::Error;
use std::fmt::{Display, Formatter};

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
