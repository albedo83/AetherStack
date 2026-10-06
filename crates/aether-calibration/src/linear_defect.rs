use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_core::{CoreError, PixelFlags, ScientificImage};

use crate::{DefectMap, DefectMapError};

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

/// Complete accounting for one linear-defect detection pass.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LinearDefectDetectionEvidence {
    examined_lines: usize,
    supported_samples: usize,
    unavailable_samples: usize,
    insufficient_support_samples: usize,
    hot_lines: usize,
    cold_lines: usize,
    mapped_hot_samples: usize,
    mapped_cold_samples: usize,
}

impl LinearDefectDetectionEvidence {
    /// Plane-local rows or columns evaluated.
    #[must_use]
    pub const fn examined_lines(self) -> usize {
        self.examined_lines
    }
    /// Finite, clear samples with sufficient perpendicular support.
    #[must_use]
    pub const fn supported_samples(self) -> usize {
        self.supported_samples
    }
    /// Masked or non-finite centre samples excluded from decisions.
    #[must_use]
    pub const fn unavailable_samples(self) -> usize {
        self.unavailable_samples
    }
    /// Usable centres lacking the required clean perpendicular support.
    #[must_use]
    pub const fn insufficient_support_samples(self) -> usize {
        self.insufficient_support_samples
    }
    /// Lines meeting the positive coherence gate.
    #[must_use]
    pub const fn hot_lines(self) -> usize {
        self.hot_lines
    }
    /// Lines meeting the negative coherence gate.
    #[must_use]
    pub const fn cold_lines(self) -> usize {
        self.cold_lines
    }
    /// Samples retained as positive evidence after line gating.
    #[must_use]
    pub const fn mapped_hot_samples(self) -> usize {
        self.mapped_hot_samples
    }
    /// Samples retained as negative evidence after line gating.
    #[must_use]
    pub const fn mapped_cold_samples(self) -> usize {
        self.mapped_cold_samples
    }
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
    /// Stable map construction rejected generated evidence.
    DefectMap(DefectMapError),
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
            Self::DefectMap(error) => Display::fmt(error, formatter),
            Self::Core(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for LinearDefectError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::DefectMap(error) => Some(error),
            Self::Core(error) => Some(error),
            _ => None,
        }
    }
}

/// Detects coherent row or column outliers against perpendicular support.
///
/// Each sample is compared only with clear, finite samples at the same
/// coordinate along neighbouring lines. A stride of two preserves Bayer phase.
/// A line is accepted only when one polarity meets both the absolute count and
/// exact parts-per-million coverage gates. Candidate positions are recomputed
/// only for accepted lines, keeping auxiliary memory independent of image
/// width and height.
pub fn detect_linear_defects(
    reference: &ScientificImage,
    parameters: LinearDefectDetectionParameters,
) -> Result<(DefectMap, LinearDefectDetectionEvidence), LinearDefectError> {
    let dimensions = reference.dimensions();
    let (line_count, line_length) = match parameters.axis {
        LinearDefectAxis::Rows => (dimensions.height(), dimensions.width()),
        LinearDefectAxis::Columns => (dimensions.width(), dimensions.height()),
    };
    let mut map = DefectMap::clear(dimensions).map_err(LinearDefectError::DefectMap)?;
    let mut values = Vec::new();
    let mut deviations = Vec::new();
    let support_capacity = parameters.perpendicular_radius * 2;
    values.try_reserve_exact(support_capacity).map_err(|_| {
        LinearDefectError::Core(CoreError::AllocationFailed {
            elements: support_capacity,
        })
    })?;
    deviations
        .try_reserve_exact(support_capacity)
        .map_err(|_| {
            LinearDefectError::Core(CoreError::AllocationFailed {
                elements: support_capacity,
            })
        })?;
    let mut evidence = LinearDefectDetectionEvidence::default();

    for plane in 0..dimensions.planes() {
        for line in 0..line_count {
            evidence.examined_lines += 1;
            let mut supported = 0_usize;
            let mut hot = 0_usize;
            let mut cold = 0_usize;
            for position in 0..line_length {
                match classify_sample(
                    reference,
                    plane,
                    line,
                    position,
                    parameters,
                    &mut values,
                    &mut deviations,
                )? {
                    SampleDecision::Unavailable => evidence.unavailable_samples += 1,
                    SampleDecision::InsufficientSupport => {
                        evidence.insufficient_support_samples += 1;
                    }
                    SampleDecision::Clear => supported += 1,
                    SampleDecision::Hot => {
                        supported += 1;
                        hot += 1;
                    }
                    SampleDecision::Cold => {
                        supported += 1;
                        cold += 1;
                    }
                }
            }
            evidence.supported_samples += supported;
            let accept_hot = line_gate(hot, supported, parameters);
            let accept_cold = line_gate(cold, supported, parameters);
            evidence.hot_lines += usize::from(accept_hot);
            evidence.cold_lines += usize::from(accept_cold);
            if !accept_hot && !accept_cold {
                continue;
            }
            for position in 0..line_length {
                let decision = classify_sample(
                    reference,
                    plane,
                    line,
                    position,
                    parameters,
                    &mut values,
                    &mut deviations,
                )?;
                let flags = match decision {
                    SampleDecision::Hot if accept_hot => PixelFlags::HOT,
                    SampleDecision::Cold if accept_cold => PixelFlags::COLD,
                    _ => continue,
                };
                let (x, y) = line_coordinates(parameters.axis, line, position);
                map.insert(x, y, plane, flags)
                    .map_err(LinearDefectError::DefectMap)?;
                evidence.mapped_hot_samples += usize::from(flags.contains(PixelFlags::HOT));
                evidence.mapped_cold_samples += usize::from(flags.contains(PixelFlags::COLD));
            }
        }
    }
    Ok((map, evidence))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SampleDecision {
    Unavailable,
    InsufficientSupport,
    Clear,
    Hot,
    Cold,
}

#[allow(clippy::too_many_arguments)]
fn classify_sample(
    image: &ScientificImage,
    plane: usize,
    line: usize,
    position: usize,
    parameters: LinearDefectDetectionParameters,
    values: &mut Vec<f64>,
    deviations: &mut Vec<f64>,
) -> Result<SampleDecision, LinearDefectError> {
    let (x, y) = line_coordinates(parameters.axis, line, position);
    let dimensions = image.dimensions();
    let index = dimensions
        .linear_index(x, y, plane)
        .map_err(LinearDefectError::Core)?;
    let centre = image.pixels()[index];
    if !image.mask().as_slice()[index].is_clear() || !centre.is_finite() {
        return Ok(SampleDecision::Unavailable);
    }
    values.clear();
    let radius = isize::try_from(parameters.perpendicular_radius).map_err(|_| {
        LinearDefectError::InvalidRadius {
            radius: parameters.perpendicular_radius,
        }
    })?;
    let stride =
        isize::try_from(parameters.stride).map_err(|_| LinearDefectError::InvalidStride {
            stride: parameters.stride,
        })?;
    for offset in -radius..=radius {
        if offset == 0 {
            continue;
        }
        let Some(neighbour_line) = line.checked_add_signed(offset * stride) else {
            continue;
        };
        let (nx, ny) = line_coordinates(parameters.axis, neighbour_line, position);
        if nx >= dimensions.width() || ny >= dimensions.height() {
            continue;
        }
        let neighbour_index = dimensions
            .linear_index(nx, ny, plane)
            .map_err(LinearDefectError::Core)?;
        let value = image.pixels()[neighbour_index];
        if image.mask().as_slice()[neighbour_index].is_clear() && value.is_finite() {
            values.push(value);
        }
    }
    if values.len() < parameters.minimum_perpendicular_neighbours {
        return Ok(SampleDecision::InsufficientSupport);
    }
    let baseline = median(values);
    deviations.clear();
    deviations.extend(values.iter().map(|value| (value - baseline).abs()));
    let robust_sigma = median(deviations) * 1.482_602_218_505_602;
    let residual = centre - baseline;
    if residual.abs() <= parameters.minimum_absolute_deviation {
        return Ok(SampleDecision::Clear);
    }
    if residual > robust_sigma * parameters.hot_sigma {
        Ok(SampleDecision::Hot)
    } else if residual < -(robust_sigma * parameters.cold_sigma) {
        Ok(SampleDecision::Cold)
    } else {
        Ok(SampleDecision::Clear)
    }
}

const fn line_coordinates(axis: LinearDefectAxis, line: usize, position: usize) -> (usize, usize) {
    match axis {
        LinearDefectAxis::Rows => (position, line),
        LinearDefectAxis::Columns => (line, position),
    }
}

fn line_gate(
    affected: usize,
    supported: usize,
    parameters: LinearDefectDetectionParameters,
) -> bool {
    if affected < parameters.minimum_affected_samples || supported == 0 {
        return false;
    }
    let affected_ppm = (affected as u128) * u128::from(PARTS_PER_MILLION);
    let required = (supported as u128) * u128::from(parameters.minimum_affected_fraction_ppm);
    affected_ppm >= required
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_unstable_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        values[middle - 1] * 0.5 + values[middle] * 0.5
    } else {
        values[middle]
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
    use aether_core::Dimensions;

    type TestResult<T = ()> = Result<T, Box<dyn Error>>;

    fn image(width: usize, height: usize, value: f64) -> TestResult<ScientificImage> {
        Ok(ScientificImage::filled(
            Dimensions::new(width, height, 1)?,
            value,
        )?)
    }

    fn detection(
        axis: LinearDefectAxis,
    ) -> Result<LinearDefectDetectionParameters, LinearDefectError> {
        LinearDefectDetectionParameters::new(axis, 2, 1, 2, 5, 500_000, 5.0, 5.0, 1.0)
    }

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

    #[test]
    fn detects_coherent_hot_rows_and_cold_columns() -> TestResult {
        let mut hot = image(11, 9, 100.0)?;
        for x in 0..11 {
            hot.pixels_mut()[4 * 11 + x] = 500.0;
        }
        let (map, evidence) = detect_linear_defects(&hot, detection(LinearDefectAxis::Rows)?)?;
        assert_eq!(evidence.examined_lines(), 9);
        assert_eq!(evidence.hot_lines(), 1);
        assert_eq!(evidence.cold_lines(), 0);
        assert_eq!(evidence.mapped_hot_samples(), 11);
        assert_eq!(map.summary().hot_samples(), 11);

        let mut cold = image(9, 11, 100.0)?;
        for y in 0..11 {
            cold.pixels_mut()[y * 9 + 3] = -300.0;
        }
        let (map, evidence) = detect_linear_defects(&cold, detection(LinearDefectAxis::Columns)?)?;
        assert_eq!(evidence.cold_lines(), 1);
        assert_eq!(evidence.mapped_cold_samples(), 11);
        assert_eq!(map.summary().cold_samples(), 11);
        Ok(())
    }

    #[test]
    fn coherence_gate_rejects_isolated_outliers() -> TestResult {
        let mut source = image(11, 9, 100.0)?;
        for x in 0..4 {
            source.pixels_mut()[4 * 11 + x] = 500.0;
        }
        let (map, evidence) = detect_linear_defects(&source, detection(LinearDefectAxis::Rows)?)?;
        assert_eq!(evidence.hot_lines(), 0);
        assert_eq!(map.summary().defective_samples(), 0);
        Ok(())
    }

    #[test]
    fn bayer_stride_compares_only_the_same_detector_phase() -> TestResult {
        let dimensions = Dimensions::new(11, 9, 1)?;
        let mut pixels = Vec::with_capacity(dimensions.pixel_count());
        for y in 0..9 {
            for x in 0..11 {
                pixels.push(if (x + y) % 2 == 0 { 10.0 } else { 1_000.0 });
            }
        }
        let mut source = ScientificImage::from_pixels(dimensions, pixels)?;
        for x in 0..11 {
            source.pixels_mut()[4 * 11 + x] += 100.0;
        }
        let parameters = LinearDefectDetectionParameters::new(
            LinearDefectAxis::Rows,
            2,
            2,
            2,
            5,
            500_000,
            5.0,
            5.0,
            1.0,
        )?;
        let (map, evidence) = detect_linear_defects(&source, parameters)?;
        assert_eq!(evidence.hot_lines(), 1);
        assert_eq!(map.summary().hot_samples(), 11);
        Ok(())
    }
}
