//! Deterministic detector-pixel footprint projection for strict CPU Drizzle.
//!
//! The crate starts from original detector pixels rather than resampled images.
//! It maps a shrunken pixel quadrilateral through the accepted source-to-reference
//! homography, converts it to the finer output grid, and deposits normalized
//! overlap fractions. Later tiled accumulation can therefore remain bounded
//! without changing the geometric or flux convention established here.

use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_core::CompensatedSum;
use aether_metadata::BayerPattern;
use aether_registration::{CoordinateError, ImagePoint, ProjectiveTransform};

mod accumulation;
mod sample;

pub use accumulation::{
    DrizzleAccumulationError, DrizzleTileAccumulator, DrizzleTileBounds, DrizzleTileEvidence,
    DrizzleTileResult,
};
pub use sample::{
    DetectorSample, DrizzleOutputBounds, DrizzleSampleExclusion, DrizzleSampleOutcome,
    deposit_cfa_sample,
};

/// Stable identity of the strict detector-footprint projection contract.
pub const DRIZZLE_FOOTPRINT_ALGORITHM_ID: &str = "drizzle-footprint-projective-f64-v1";

const MAX_SCALE: u32 = 8;
const MAX_POLYGON_POINTS: usize = 8;

/// Output scale and detector-drop controls shared by planning and execution.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrizzleParameters {
    scale: u32,
    drop_shrink: f64,
}

impl DrizzleParameters {
    /// Builds bounded Drizzle controls.
    pub fn new(scale: u32, drop_shrink: f64) -> Result<Self, DrizzleError> {
        if !(1..=MAX_SCALE).contains(&scale)
            || !drop_shrink.is_finite()
            || !(0.0..=1.0).contains(&drop_shrink)
            || drop_shrink == 0.0
        {
            return Err(DrizzleError::InvalidParameters);
        }
        Ok(Self { scale, drop_shrink })
    }

    /// Integer output sampling multiplier.
    #[must_use]
    pub const fn scale(self) -> u32 {
        self.scale
    }

    /// Detector-pixel side length retained before projection, in `(0, 1]`.
    #[must_use]
    pub const fn drop_shrink(self) -> f64 {
        self.drop_shrink
    }
}

/// One finite vertex in output pixel-center coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FootprintPoint {
    x: f64,
    y: f64,
}

impl FootprintPoint {
    /// Horizontal output coordinate.
    #[must_use]
    pub const fn x(self) -> f64 {
        self.x
    }

    /// Vertical output coordinate.
    #[must_use]
    pub const fn y(self) -> f64 {
        self.y
    }
}

/// Projected quadrilateral for one original detector pixel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetectorFootprint {
    vertices: [FootprintPoint; 4],
    area: f64,
}

impl DetectorFootprint {
    /// Vertices in detector-corner traversal order.
    #[must_use]
    pub const fn vertices(self) -> [FootprintPoint; 4] {
        self.vertices
    }

    /// Positive area in output-pixel units squared.
    #[must_use]
    pub const fn area(self) -> f64 {
        self.area
    }
}

/// Normalized deposition from one detector pixel into one output pixel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrizzleContribution {
    x: u32,
    y: u32,
    area_fraction: f64,
    weighted_flux: f64,
    weight: f64,
}

impl DrizzleContribution {
    /// Output column.
    #[must_use]
    pub const fn x(self) -> u32 {
        self.x
    }

    /// Output row.
    #[must_use]
    pub const fn y(self) -> u32 {
        self.y
    }

    /// Fraction of the complete projected drop deposited into this pixel.
    #[must_use]
    pub const fn area_fraction(self) -> f64 {
        self.area_fraction
    }

    /// Input value multiplied by frame weight and normalized overlap.
    #[must_use]
    pub const fn weighted_flux(self) -> f64 {
        self.weighted_flux
    }

    /// Frame weight multiplied by normalized overlap.
    #[must_use]
    pub const fn weight(self) -> f64 {
        self.weight
    }
}

/// Complete bounded deposition evidence for one detector pixel.
#[derive(Clone, Debug, PartialEq)]
pub struct DrizzleDeposition {
    contributions: Vec<DrizzleContribution>,
    deposited_fraction: f64,
}

/// Canonical planar destination for one original Bayer photosite.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CfaChannel {
    /// Red detector samples.
    Red,
    /// Either green detector phase, accumulated into one green plane.
    Green,
    /// Blue detector samples.
    Blue,
}

impl CfaChannel {
    /// Canonical planar RGB index.
    #[must_use]
    pub const fn plane(self) -> usize {
        match self {
            Self::Red => 0,
            Self::Green => 1,
            Self::Blue => 2,
        }
    }
}

/// One CFA-routed deposition that never invents interpolated colors.
#[derive(Clone, Debug, PartialEq)]
pub struct CfaDrizzleDeposition {
    channel: CfaChannel,
    deposition: DrizzleDeposition,
}

impl CfaDrizzleDeposition {
    /// Destination color plane selected from the original detector coordinate.
    #[must_use]
    pub const fn channel(&self) -> CfaChannel {
        self.channel
    }

    /// Geometric contributions shared with monochrome Drizzle.
    #[must_use]
    pub const fn deposition(&self) -> &DrizzleDeposition {
        &self.deposition
    }

    /// Returns the routed deposition as owned parts.
    #[must_use]
    pub fn into_parts(self) -> (CfaChannel, DrizzleDeposition) {
        (self.channel, self.deposition)
    }
}

impl DrizzleDeposition {
    /// Contributions in stable row-major output order.
    #[must_use]
    pub fn contributions(&self) -> &[DrizzleContribution] {
        &self.contributions
    }

    /// Fraction retained inside the requested output rectangle.
    #[must_use]
    pub const fn deposited_fraction(&self) -> f64 {
        self.deposited_fraction
    }
}

/// Failure to validate, project, bound, or deposit one footprint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DrizzleError {
    /// Scale or drop shrink is outside the supported strict profile.
    InvalidParameters,
    /// Pixel coordinates or output dimensions cannot be represented safely.
    InvalidGeometry,
    /// The accepted transform failed at one detector-pixel corner.
    TransformFailed,
    /// The projected quadrilateral is degenerate or non-finite.
    DegenerateFootprint,
    /// The explicit contribution ceiling is zero or would be exceeded.
    ContributionLimitExceeded,
    /// A clipped convex quadrilateral exceeded its mathematical vertex bound.
    GeometryComplexityExceeded,
    /// Contribution storage could not be reserved.
    AllocationFailed,
    /// The declared CFA pattern is not one of the four supported Bayer phases.
    UnsupportedCfaPattern,
}

impl Display for DrizzleError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidParameters => "Drizzle parameters are invalid",
            Self::InvalidGeometry => "Drizzle geometry is invalid or unrepresentable",
            Self::TransformFailed => "detector footprint transform failed",
            Self::DegenerateFootprint => "projected detector footprint is degenerate",
            Self::ContributionLimitExceeded => "Drizzle contribution ceiling would be exceeded",
            Self::GeometryComplexityExceeded => "clipped detector footprint is too complex",
            Self::AllocationFailed => "Drizzle contribution allocation failed",
            Self::UnsupportedCfaPattern => "CFA Drizzle requires a supported Bayer pattern",
        })
    }
}

impl Error for DrizzleError {}

/// Projects one original detector-pixel drop into output pixel-center coordinates.
///
/// Reference coordinate `r` maps to output coordinate `(r + 0.5) * scale - 0.5`.
/// This maps reference pixel edges exactly onto output pixel edges and avoids a
/// half-pixel drift when the scale changes.
pub fn project_detector_footprint(
    source_x: u32,
    source_y: u32,
    transform: ProjectiveTransform,
    parameters: DrizzleParameters,
) -> Result<DetectorFootprint, DrizzleError> {
    let half_drop = parameters.drop_shrink * 0.5;
    let x = f64::from(source_x);
    let y = f64::from(source_y);
    let source_vertices = [
        (x - half_drop, y - half_drop),
        (x + half_drop, y - half_drop),
        (x + half_drop, y + half_drop),
        (x - half_drop, y + half_drop),
    ];
    let matrix = transform.coefficients();
    let mut denominator_sign = None;
    for (corner_x, corner_y) in source_vertices {
        let denominator =
            matrix[2][0].mul_add(corner_x, matrix[2][1].mul_add(corner_y, matrix[2][2]));
        if !denominator.is_finite() || denominator == 0.0 {
            return Err(DrizzleError::TransformFailed);
        }
        let sign = denominator.is_sign_positive();
        if denominator_sign.is_some_and(|expected| expected != sign) {
            return Err(DrizzleError::TransformFailed);
        }
        denominator_sign = Some(sign);
    }
    let mut vertices = [FootprintPoint::default(); 4];
    for (output, (corner_x, corner_y)) in vertices.iter_mut().zip(source_vertices) {
        let point = ImagePoint::new(corner_x, corner_y)
            .and_then(|point| transform.apply(point))
            .map_err(map_coordinate_error)?;
        *output = FootprintPoint {
            x: scale_coordinate(point.x(), parameters.scale)?,
            y: scale_coordinate(point.y(), parameters.scale)?,
        };
    }
    let area = polygon_area(&vertices);
    if !area.is_finite() || area <= 0.0 {
        return Err(DrizzleError::DegenerateFootprint);
    }
    Ok(DetectorFootprint { vertices, area })
}

/// Deposits one projected drop into an output rectangle with normalized flux.
///
/// The contribution ceiling is checked from the integer bounding box before
/// allocation or polygon clipping. Pixels outside the output rectangle are
/// omitted and reported through [`DrizzleDeposition::deposited_fraction`].
pub fn deposit_detector_footprint(
    footprint: DetectorFootprint,
    value: f64,
    frame_weight: f64,
    output_width: u32,
    output_height: u32,
    maximum_contributions: usize,
) -> Result<DrizzleDeposition, DrizzleError> {
    if !value.is_finite()
        || !frame_weight.is_finite()
        || frame_weight <= 0.0
        || output_width == 0
        || output_height == 0
        || maximum_contributions == 0
    {
        return Err(DrizzleError::InvalidGeometry);
    }
    let (minimum_x, maximum_x, minimum_y, maximum_y) = footprint_bounds(footprint)?;
    let first_x = minimum_center_index(minimum_x)?.max(0);
    let last_x = maximum_center_index(maximum_x)?.min(i64::from(output_width) - 1);
    let first_y = minimum_center_index(minimum_y)?.max(0);
    let last_y = maximum_center_index(maximum_y)?.min(i64::from(output_height) - 1);
    if first_x > last_x || first_y > last_y {
        return Ok(DrizzleDeposition {
            contributions: Vec::new(),
            deposited_fraction: 0.0,
        });
    }
    let columns =
        usize::try_from(last_x - first_x + 1).map_err(|_| DrizzleError::InvalidGeometry)?;
    let rows = usize::try_from(last_y - first_y + 1).map_err(|_| DrizzleError::InvalidGeometry)?;
    let candidates = columns
        .checked_mul(rows)
        .ok_or(DrizzleError::InvalidGeometry)?;
    if candidates > maximum_contributions {
        return Err(DrizzleError::ContributionLimitExceeded);
    }
    let mut contributions = Vec::new();
    contributions
        .try_reserve_exact(candidates)
        .map_err(|_| DrizzleError::AllocationFailed)?;
    let polygon = SmallPolygon::from_quad(footprint.vertices);
    let mut deposited_fraction = CompensatedSum::new();
    for y in first_y..=last_y {
        for x in first_x..=last_x {
            let clipped = polygon.clip_to_pixel(x as f64, y as f64)?;
            let overlap = clipped.area();
            if overlap <= 0.0 {
                continue;
            }
            let area_fraction = overlap / footprint.area;
            if !area_fraction.is_finite() || area_fraction <= 0.0 {
                return Err(DrizzleError::DegenerateFootprint);
            }
            let weight = frame_weight * area_fraction;
            let weighted_flux = value * weight;
            if !weight.is_finite() || !weighted_flux.is_finite() {
                return Err(DrizzleError::InvalidGeometry);
            }
            contributions.push(DrizzleContribution {
                x: u32::try_from(x).map_err(|_| DrizzleError::InvalidGeometry)?,
                y: u32::try_from(y).map_err(|_| DrizzleError::InvalidGeometry)?,
                area_fraction,
                weighted_flux,
                weight,
            });
            deposited_fraction.add(area_fraction);
        }
    }
    let deposited_fraction = deposited_fraction.total();
    if !deposited_fraction.is_finite() {
        return Err(DrizzleError::DegenerateFootprint);
    }
    Ok(DrizzleDeposition {
        contributions,
        deposited_fraction,
    })
}

/// Returns the measured color at one original detector coordinate.
///
/// Routing deliberately uses source parity before any transform. Geometric
/// shifts, rotations, reflections, and output scaling therefore cannot change
/// the physical filter that measured the sample.
pub fn cfa_channel(
    pattern: &BayerPattern,
    source_x: u32,
    source_y: u32,
) -> Result<CfaChannel, DrizzleError> {
    let phase = ((source_y & 1) << 1) | (source_x & 1);
    let channels = match pattern {
        BayerPattern::Rggb => [
            CfaChannel::Red,
            CfaChannel::Green,
            CfaChannel::Green,
            CfaChannel::Blue,
        ],
        BayerPattern::Bggr => [
            CfaChannel::Blue,
            CfaChannel::Green,
            CfaChannel::Green,
            CfaChannel::Red,
        ],
        BayerPattern::Grbg => [
            CfaChannel::Green,
            CfaChannel::Red,
            CfaChannel::Blue,
            CfaChannel::Green,
        ],
        BayerPattern::Gbrg => [
            CfaChannel::Green,
            CfaChannel::Blue,
            CfaChannel::Red,
            CfaChannel::Green,
        ],
        BayerPattern::Other(_) => return Err(DrizzleError::UnsupportedCfaPattern),
    };
    Ok(channels[phase as usize])
}

/// Projects and deposits one original CFA photosite into exactly one RGB plane.
#[allow(clippy::too_many_arguments)]
pub fn deposit_cfa_detector_pixel(
    source_x: u32,
    source_y: u32,
    transform: ProjectiveTransform,
    parameters: DrizzleParameters,
    pattern: &BayerPattern,
    value: f64,
    frame_weight: f64,
    output_width: u32,
    output_height: u32,
    maximum_contributions: usize,
) -> Result<CfaDrizzleDeposition, DrizzleError> {
    let channel = cfa_channel(pattern, source_x, source_y)?;
    let footprint = project_detector_footprint(source_x, source_y, transform, parameters)?;
    let deposition = deposit_detector_footprint(
        footprint,
        value,
        frame_weight,
        output_width,
        output_height,
        maximum_contributions,
    )?;
    Ok(CfaDrizzleDeposition {
        channel,
        deposition,
    })
}

fn map_coordinate_error(_error: CoordinateError) -> DrizzleError {
    DrizzleError::TransformFailed
}

fn scale_coordinate(coordinate: f64, scale: u32) -> Result<f64, DrizzleError> {
    let output = (coordinate + 0.5).mul_add(f64::from(scale), -0.5);
    if output.is_finite() {
        Ok(output)
    } else {
        Err(DrizzleError::InvalidGeometry)
    }
}

fn polygon_area<const N: usize>(vertices: &[FootprintPoint; N]) -> f64 {
    let mut twice_area = 0.0;
    for index in 0..N {
        let current = vertices[index];
        let next = vertices[(index + 1) % N];
        twice_area = current.x.mul_add(next.y, twice_area - current.y * next.x);
    }
    (twice_area * 0.5).abs()
}

fn footprint_bounds(footprint: DetectorFootprint) -> Result<(f64, f64, f64, f64), DrizzleError> {
    let mut minimum_x = f64::INFINITY;
    let mut maximum_x = f64::NEG_INFINITY;
    let mut minimum_y = f64::INFINITY;
    let mut maximum_y = f64::NEG_INFINITY;
    for vertex in footprint.vertices {
        minimum_x = minimum_x.min(vertex.x);
        maximum_x = maximum_x.max(vertex.x);
        minimum_y = minimum_y.min(vertex.y);
        maximum_y = maximum_y.max(vertex.y);
    }
    if [minimum_x, maximum_x, minimum_y, maximum_y]
        .iter()
        .all(|value| value.is_finite())
    {
        Ok((minimum_x, maximum_x, minimum_y, maximum_y))
    } else {
        Err(DrizzleError::InvalidGeometry)
    }
}

fn minimum_center_index(coordinate: f64) -> Result<i64, DrizzleError> {
    let shifted = (coordinate + 0.5).floor();
    finite_i64(shifted)
}

fn maximum_center_index(coordinate: f64) -> Result<i64, DrizzleError> {
    let shifted = (coordinate + 0.5).ceil() - 1.0;
    finite_i64(shifted)
}

fn finite_i64(value: f64) -> Result<i64, DrizzleError> {
    if !value.is_finite() || value < i64::MIN as f64 || value > i64::MAX as f64 {
        return Err(DrizzleError::InvalidGeometry);
    }
    Ok(value as i64)
}

#[derive(Clone, Copy)]
struct SmallPolygon {
    points: [FootprintPoint; MAX_POLYGON_POINTS],
    len: usize,
}

impl SmallPolygon {
    fn from_quad(points: [FootprintPoint; 4]) -> Self {
        let mut polygon = Self {
            points: [FootprintPoint::default(); MAX_POLYGON_POINTS],
            len: points.len(),
        };
        polygon.points[..points.len()].copy_from_slice(&points);
        polygon
    }

    fn clip_to_pixel(self, x: f64, y: f64) -> Result<Self, DrizzleError> {
        self.clip(Axis::X, x - 0.5, Keep::Greater)?
            .clip(Axis::X, x + 0.5, Keep::Less)?
            .clip(Axis::Y, y - 0.5, Keep::Greater)?
            .clip(Axis::Y, y + 0.5, Keep::Less)
    }

    fn clip(self, axis: Axis, boundary: f64, keep: Keep) -> Result<Self, DrizzleError> {
        if self.len == 0 {
            return Ok(self);
        }
        let mut output = Self {
            points: [FootprintPoint::default(); MAX_POLYGON_POINTS],
            len: 0,
        };
        let mut previous = self.points[self.len - 1];
        let mut previous_inside = inside(previous, axis, boundary, keep);
        for current in self.points[..self.len].iter().copied() {
            let current_inside = inside(current, axis, boundary, keep);
            if current_inside != previous_inside {
                output.push(intersection(previous, current, axis, boundary)?)?;
            }
            if current_inside {
                output.push(current)?;
            }
            previous = current;
            previous_inside = current_inside;
        }
        Ok(output)
    }

    fn push(&mut self, point: FootprintPoint) -> Result<(), DrizzleError> {
        if self.len == MAX_POLYGON_POINTS {
            return Err(DrizzleError::GeometryComplexityExceeded);
        }
        self.points[self.len] = point;
        self.len += 1;
        Ok(())
    }

    fn area(self) -> f64 {
        if self.len < 3 {
            return 0.0;
        }
        let mut twice_area = 0.0;
        for index in 0..self.len {
            let current = self.points[index];
            let next = self.points[(index + 1) % self.len];
            twice_area = current.x.mul_add(next.y, twice_area - current.y * next.x);
        }
        (twice_area * 0.5).abs()
    }
}

#[derive(Clone, Copy)]
enum Axis {
    X,
    Y,
}

#[derive(Clone, Copy)]
enum Keep {
    Greater,
    Less,
}

fn coordinate(point: FootprintPoint, axis: Axis) -> f64 {
    match axis {
        Axis::X => point.x,
        Axis::Y => point.y,
    }
}

fn inside(point: FootprintPoint, axis: Axis, boundary: f64, keep: Keep) -> bool {
    match keep {
        Keep::Greater => coordinate(point, axis) >= boundary,
        Keep::Less => coordinate(point, axis) <= boundary,
    }
}

fn intersection(
    from: FootprintPoint,
    to: FootprintPoint,
    axis: Axis,
    boundary: f64,
) -> Result<FootprintPoint, DrizzleError> {
    let from_coordinate = coordinate(from, axis);
    let denominator = coordinate(to, axis) - from_coordinate;
    if !denominator.is_finite() || denominator == 0.0 {
        return Err(DrizzleError::DegenerateFootprint);
    }
    let fraction = (boundary - from_coordinate) / denominator;
    let point = FootprintPoint {
        x: (to.x - from.x).mul_add(fraction, from.x),
        y: (to.y - from.y).mul_add(fraction, from.y),
    };
    if point.x.is_finite() && point.y.is_finite() {
        Ok(point)
    } else {
        Err(DrizzleError::DegenerateFootprint)
    }
}

#[cfg(test)]
mod tests {
    use aether_registration::AffineTransform;

    use super::*;

    type TestResult = Result<(), Box<dyn Error>>;

    fn sum(deposition: &DrizzleDeposition) -> (f64, f64, f64) {
        let mut fraction = CompensatedSum::new();
        let mut flux = CompensatedSum::new();
        let mut weight = CompensatedSum::new();
        for contribution in deposition.contributions() {
            fraction.add(contribution.area_fraction());
            flux.add(contribution.weighted_flux());
            weight.add(contribution.weight());
        }
        (fraction.total(), flux.total(), weight.total())
    }

    fn assert_close(actual: f64, expected: f64, tolerance: f64) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "expected {expected}, actual {actual}, tolerance {tolerance}"
        );
    }

    #[test]
    fn parameters_reject_unsupported_scale_and_drop_values() {
        assert_eq!(
            DrizzleParameters::new(0, 1.0),
            Err(DrizzleError::InvalidParameters)
        );
        assert_eq!(
            DrizzleParameters::new(9, 1.0),
            Err(DrizzleError::InvalidParameters)
        );
        assert_eq!(
            DrizzleParameters::new(2, 0.0),
            Err(DrizzleError::InvalidParameters)
        );
        assert_eq!(
            DrizzleParameters::new(2, f64::NAN),
            Err(DrizzleError::InvalidParameters)
        );
    }

    #[test]
    fn identity_scale_two_maps_edges_and_conserves_weighted_flux() -> TestResult {
        let footprint = project_detector_footprint(
            0,
            0,
            ProjectiveTransform::IDENTITY,
            DrizzleParameters::new(2, 1.0)?,
        )?;
        assert_eq!(
            footprint.vertices(),
            [
                FootprintPoint { x: -0.5, y: -0.5 },
                FootprintPoint { x: 1.5, y: -0.5 },
                FootprintPoint { x: 1.5, y: 1.5 },
                FootprintPoint { x: -0.5, y: 1.5 },
            ]
        );
        assert_close(footprint.area(), 4.0, f64::EPSILON);
        let deposition = deposit_detector_footprint(footprint, 42.0, 2.5, 4, 4, 16)?;
        assert_eq!(deposition.contributions().len(), 4);
        assert!(
            deposition
                .contributions()
                .iter()
                .all(|contribution| (contribution.area_fraction() - 0.25).abs() <= f64::EPSILON)
        );
        let (fraction, flux, weight) = sum(&deposition);
        assert_close(fraction, 1.0, f64::EPSILON);
        assert_close(deposition.deposited_fraction(), 1.0, f64::EPSILON);
        assert_close(flux, 105.0, 8.0 * f64::EPSILON);
        assert_close(weight, 2.5, 4.0 * f64::EPSILON);
        assert_close(flux / weight, 42.0, 8.0 * f64::EPSILON);
        Ok(())
    }

    #[test]
    fn drop_shrink_changes_support_without_changing_total_flux() -> TestResult {
        let footprint = project_detector_footprint(
            0,
            0,
            ProjectiveTransform::IDENTITY,
            DrizzleParameters::new(2, 0.5)?,
        )?;
        assert_close(footprint.area(), 1.0, f64::EPSILON);
        let deposition = deposit_detector_footprint(footprint, 8.0, 3.0, 4, 4, 16)?;
        let (fraction, flux, weight) = sum(&deposition);
        assert_close(fraction, 1.0, f64::EPSILON);
        assert_close(flux, 24.0, 8.0 * f64::EPSILON);
        assert_close(weight, 3.0, 4.0 * f64::EPSILON);
        Ok(())
    }

    #[test]
    fn rotated_footprint_is_clipped_deterministically_and_conserves_flux() -> TestResult {
        let cosine = std::f64::consts::FRAC_1_SQRT_2;
        let affine = AffineTransform::new(cosine, -cosine, cosine, cosine, 4.0, 4.0)?;
        let transform = ProjectiveTransform::from_affine(affine)?;
        let footprint =
            project_detector_footprint(0, 0, transform, DrizzleParameters::new(2, 1.0)?)?;
        let deposition = deposit_detector_footprint(footprint, 17.0, 1.5, 16, 16, 64)?;
        let (fraction, flux, weight) = sum(&deposition);
        assert!((fraction - 1.0).abs() <= 1.0e-14);
        assert!((flux - 25.5).abs() <= 1.0e-13);
        assert!((weight - 1.5).abs() <= 1.0e-14);
        assert!(
            deposition
                .contributions()
                .windows(2)
                .all(|pair| (pair[0].y(), pair[0].x()) < (pair[1].y(), pair[1].x()))
        );
        Ok(())
    }

    #[test]
    fn boundary_clipping_reports_lost_fraction() -> TestResult {
        let translated = ProjectiveTransform::from_affine(AffineTransform::new(
            1.0, 0.0, 0.0, 1.0, -0.25, 0.0,
        )?)?;
        let footprint =
            project_detector_footprint(0, 0, translated, DrizzleParameters::new(1, 1.0)?)?;
        let deposition = deposit_detector_footprint(footprint, 1.0, 1.0, 4, 4, 8)?;
        assert_eq!(deposition.contributions().len(), 1);
        assert_close(deposition.deposited_fraction(), 0.75, f64::EPSILON);
        assert_close(
            deposition.contributions()[0].area_fraction(),
            0.75,
            f64::EPSILON,
        );
        Ok(())
    }

    #[test]
    fn contribution_ceiling_fails_before_clipping_or_allocation() -> TestResult {
        let footprint = project_detector_footprint(
            0,
            0,
            ProjectiveTransform::IDENTITY,
            DrizzleParameters::new(8, 1.0)?,
        )?;
        assert_eq!(
            deposit_detector_footprint(footprint, 1.0, 1.0, 16, 16, 63),
            Err(DrizzleError::ContributionLimitExceeded)
        );
        Ok(())
    }

    #[test]
    fn projective_pole_crossing_the_drop_is_rejected() -> TestResult {
        let transform =
            ProjectiveTransform::new([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [2.0, 0.0, 1.0]])?;
        assert_eq!(
            project_detector_footprint(0, 0, transform, DrizzleParameters::new(2, 1.0)?),
            Err(DrizzleError::TransformFailed)
        );
        Ok(())
    }

    #[test]
    fn every_bayer_phase_routes_source_photosites_without_demosaicing() -> TestResult {
        let cases = [
            (
                BayerPattern::Rggb,
                [
                    CfaChannel::Red,
                    CfaChannel::Green,
                    CfaChannel::Green,
                    CfaChannel::Blue,
                ],
            ),
            (
                BayerPattern::Bggr,
                [
                    CfaChannel::Blue,
                    CfaChannel::Green,
                    CfaChannel::Green,
                    CfaChannel::Red,
                ],
            ),
            (
                BayerPattern::Grbg,
                [
                    CfaChannel::Green,
                    CfaChannel::Red,
                    CfaChannel::Blue,
                    CfaChannel::Green,
                ],
            ),
            (
                BayerPattern::Gbrg,
                [
                    CfaChannel::Green,
                    CfaChannel::Blue,
                    CfaChannel::Red,
                    CfaChannel::Green,
                ],
            ),
        ];
        for (pattern, expected) in cases {
            assert_eq!(cfa_channel(&pattern, 0, 0)?, expected[0]);
            assert_eq!(cfa_channel(&pattern, 1, 0)?, expected[1]);
            assert_eq!(cfa_channel(&pattern, 0, 1)?, expected[2]);
            assert_eq!(cfa_channel(&pattern, 1, 1)?, expected[3]);
        }
        assert_eq!(
            cfa_channel(&BayerPattern::Other("CYGM".to_owned()), 0, 0),
            Err(DrizzleError::UnsupportedCfaPattern)
        );
        Ok(())
    }

    #[test]
    fn cfa_channel_is_bound_before_geometric_projection() -> TestResult {
        let transform =
            ProjectiveTransform::from_affine(AffineTransform::new(0.0, -1.0, 1.0, 0.0, 8.0, 4.0)?)?;
        let routed = deposit_cfa_detector_pixel(
            0,
            0,
            transform,
            DrizzleParameters::new(2, 0.8)?,
            &BayerPattern::Rggb,
            120.0,
            0.75,
            32,
            32,
            16,
        )?;
        assert_eq!(routed.channel(), CfaChannel::Red);
        let (fraction, flux, weight) = sum(routed.deposition());
        assert_close(fraction, 1.0, 8.0 * f64::EPSILON);
        assert_close(flux, 90.0, 64.0 * f64::EPSILON);
        assert_close(weight, 0.75, 8.0 * f64::EPSILON);
        assert_eq!(routed.channel().plane(), 0);
        Ok(())
    }
}
