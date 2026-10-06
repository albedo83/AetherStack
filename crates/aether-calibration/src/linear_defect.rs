use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_core::CoreError;

const MAXIMUM_PERPENDICULAR_RADIUS: usize = 8;
const PARTS_PER_MILLION: u32 = 1_000_000;

/// Detector-line orientation evaluated by the strict correction oracle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinearDefectAxis {
    /// Horizontal detector lines; support is sampled above and below.
    Rows,
    /// Vertical detector lines; support is sampled to the left and right.
    Columns,
}

impl Display for LinearDefectAxis {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rows => formatter.write_str("rows"),
            Self::Columns => formatter.write_str("columns"),
        }
    }
}

/// Explicit controls for detecting coherent row or column defects.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinearDefectDetectionParameters {
    axis: LinearDefectAxis,
    perpendicular_radius: usize,
    stride: usize,
    minimum_perpendicular_neighbours: usize,
    minimum_affected_samples: usize,
    minimum_affected_fraction_ppm: u32,
    hot_sigma: f64,
    cold_sigma: f64,
    minimum_absolute_deviation: f64,
}

impl LinearDefectDetectionParameters {
    /// Validates a fully explicit, bounded line-detection policy.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        axis: LinearDefectAxis,
        perpendicular_radius: usize,
        stride: usize,
        minimum_perpendicular_neighbours: usize,
        minimum_affected_samples: usize,
        minimum_affected_fraction_ppm: u32,
        hot_sigma: f64,
        cold_sigma: f64,
        minimum_absolute_deviation: f64,
    ) -> Result<Self, LinearDefectError> {
        if perpendicular_radius == 0 || perpendicular_radius > MAXIMUM_PERPENDICULAR_RADIUS {
            return Err(LinearDefectError::InvalidRadius {
                radius: perpendicular_radius,
            });
        }
        if !(1..=2).contains(&stride) {
            return Err(LinearDefectError::InvalidStride { stride });
        }
        let maximum_neighbours = perpendicular_radius * 2;
        if minimum_perpendicular_neighbours == 0
            || minimum_perpendicular_neighbours > maximum_neighbours
        {
            return Err(LinearDefectError::InvalidMinimumNeighbours {
                minimum: minimum_perpendicular_neighbours,
                maximum: maximum_neighbours,
            });
        }
        if minimum_affected_samples == 0 {
            return Err(LinearDefectError::InvalidMinimumAffectedSamples);
        }
        if minimum_affected_fraction_ppm == 0 || minimum_affected_fraction_ppm > PARTS_PER_MILLION {
            return Err(LinearDefectError::InvalidAffectedFraction {
                parts_per_million: minimum_affected_fraction_ppm,
            });
        }
        validate_sigma(hot_sigma, LinearDefectPolarity::Hot)?;
        validate_sigma(cold_sigma, LinearDefectPolarity::Cold)?;
        if !minimum_absolute_deviation.is_finite() || minimum_absolute_deviation < 0.0 {
            return Err(LinearDefectError::InvalidDeviationFloor {
                value: minimum_absolute_deviation,
            });
        }
        Ok(Self {
            axis,
            perpendicular_radius,
            stride,
            minimum_perpendicular_neighbours,
            minimum_affected_samples,
            minimum_affected_fraction_ppm,
            hot_sigma,
            cold_sigma,
            minimum_absolute_deviation,
        })
    }

    /// Line orientation evaluated by this policy.
    #[must_use]
    pub const fn axis(self) -> LinearDefectAxis {
        self.axis
    }

    /// Maximum number of same-phase lines sampled on each side.
    #[must_use]
    pub const fn perpendicular_radius(self) -> usize {
        self.perpendicular_radius
    }

    /// Detector-pixel spacing between support lines.
    #[must_use]
    pub const fn stride(self) -> usize {
        self.stride
    }

    /// Minimum clean perpendicular support for one sample decision.
    #[must_use]
    pub const fn minimum_perpendicular_neighbours(self) -> usize {
        self.minimum_perpendicular_neighbours
    }

    /// Minimum outlying samples needed to classify one line.
    #[must_use]
    pub const fn minimum_affected_samples(self) -> usize {
        self.minimum_affected_samples
    }

    /// Minimum outlier fraction expressed exactly in parts per million.
    #[must_use]
    pub const fn minimum_affected_fraction_ppm(self) -> u32 {
        self.minimum_affected_fraction_ppm
    }

    /// Positive robust-sigma threshold for a hot line.
    #[must_use]
    pub const fn hot_sigma(self) -> f64 {
        self.hot_sigma
    }

    /// Positive robust-sigma threshold for a cold line.
    #[must_use]
    pub const fn cold_sigma(self) -> f64 {
        self.cold_sigma
    }

    /// Smallest absolute residual eligible for line evidence.
    #[must_use]
    pub const fn minimum_absolute_deviation(self) -> f64 {
        self.minimum_absolute_deviation
    }
}

/// Positive or negative line polarity used by validation diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinearDefectPolarity {
    /// Positive residual relative to perpendicular support.
    Hot,
    /// Negative residual relative to perpendicular support.
    Cold,
}

impl Display for LinearDefectPolarity {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hot => formatter.write_str("hot"),
            Self::Cold => formatter.write_str("cold"),
        }
    }
}

/// Failure raised before a defensible line result can be produced.
#[derive(Clone, Debug, PartialEq)]
pub enum LinearDefectError {
    /// Perpendicular radius falls outside the bounded implementation range.
    InvalidRadius {
        /// Rejected perpendicular radius.
        radius: usize,
    },
    /// Only monochrome or same-Bayer-phase sampling is supported.
    InvalidStride {
        /// Rejected detector-pixel stride.
        stride: usize,
    },
    /// Requested support cannot fit inside the perpendicular neighbourhood.
    InvalidMinimumNeighbours {
        /// Requested minimum support.
        minimum: usize,
        /// Largest possible support for the selected radius.
        maximum: usize,
    },
    /// At least one affected sample is required.
    InvalidMinimumAffectedSamples,
    /// Required line coverage is outside one to one million ppm.
    InvalidAffectedFraction {
        /// Rejected exact line-coverage threshold.
        parts_per_million: u32,
    },
    /// A robust-sigma threshold is non-finite or non-positive.
    InvalidSigma {
        /// Rejected line polarity.
        polarity: LinearDefectPolarity,
        /// Rejected threshold.
        value: f64,
    },
    /// The absolute residual floor is negative or non-finite.
    InvalidDeviationFloor {
        /// Rejected absolute floor.
        value: f64,
    },
    /// Allocation or coordinate failure from the shared image core.
    Core(CoreError),
}

impl Display for LinearDefectError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRadius { radius } => write!(
                formatter,
                "linear-defect radius {radius} is outside 1..={MAXIMUM_PERPENDICULAR_RADIUS}"
            ),
            Self::InvalidStride { stride } => {
                write!(formatter, "linear-defect stride {stride} is not 1 or 2")
            }
            Self::InvalidMinimumNeighbours { minimum, maximum } => write!(
                formatter,
                "minimum perpendicular neighbour count {minimum} is outside 1..={maximum}"
            ),
            Self::InvalidMinimumAffectedSamples => {
                formatter.write_str("minimum affected sample count must be positive")
            }
            Self::InvalidAffectedFraction { parts_per_million } => write!(
                formatter,
                "minimum affected fraction {parts_per_million} ppm is outside 1..={PARTS_PER_MILLION}"
            ),
            Self::InvalidSigma { polarity, value } => write!(
                formatter,
                "linear {polarity} sigma must be finite and positive, received {value}"
            ),
            Self::InvalidDeviationFloor { value } => write!(
                formatter,
                "linear minimum absolute deviation must be finite and non-negative, received {value}"
            ),
            Self::Core(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for LinearDefectError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Core(error) => Some(error),
            _ => None,
        }
    }
}

fn validate_sigma(value: f64, polarity: LinearDefectPolarity) -> Result<(), LinearDefectError> {
    if !value.is_finite() || value <= 0.0 {
        return Err(LinearDefectError::InvalidSigma { polarity, value });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameters_reject_every_unbounded_or_ambiguous_control() {
        let valid = || {
            LinearDefectDetectionParameters::new(
                LinearDefectAxis::Rows,
                2,
                2,
                3,
                16,
                500_000,
                5.0,
                5.0,
                1.0,
            )
        };
        assert!(valid().is_ok());
        assert!(matches!(
            LinearDefectDetectionParameters::new(
                LinearDefectAxis::Rows,
                0,
                2,
                3,
                16,
                500_000,
                5.0,
                5.0,
                1.0,
            ),
            Err(LinearDefectError::InvalidRadius { .. })
        ));
        assert!(matches!(
            LinearDefectDetectionParameters::new(
                LinearDefectAxis::Columns,
                2,
                3,
                3,
                16,
                500_000,
                5.0,
                5.0,
                1.0,
            ),
            Err(LinearDefectError::InvalidStride { .. })
        ));
        assert!(matches!(
            LinearDefectDetectionParameters::new(
                LinearDefectAxis::Rows,
                2,
                1,
                5,
                16,
                500_000,
                5.0,
                5.0,
                1.0,
            ),
            Err(LinearDefectError::InvalidMinimumNeighbours { .. })
        ));
        assert!(matches!(
            LinearDefectDetectionParameters::new(
                LinearDefectAxis::Rows,
                2,
                1,
                2,
                0,
                500_000,
                5.0,
                5.0,
                1.0,
            ),
            Err(LinearDefectError::InvalidMinimumAffectedSamples)
        ));
        assert!(matches!(
            LinearDefectDetectionParameters::new(
                LinearDefectAxis::Rows,
                2,
                1,
                2,
                16,
                0,
                5.0,
                5.0,
                1.0,
            ),
            Err(LinearDefectError::InvalidAffectedFraction { .. })
        ));
        assert!(matches!(
            LinearDefectDetectionParameters::new(
                LinearDefectAxis::Rows,
                2,
                1,
                2,
                16,
                500_000,
                f64::NAN,
                5.0,
                1.0,
            ),
            Err(LinearDefectError::InvalidSigma { .. })
        ));
    }
}
