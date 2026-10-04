use std::error::Error;
use std::f64::consts::PI;
use std::fmt::{Display, Formatter};

use aether_core::{CompensatedSum, CoreError, Dimensions, PixelFlags, ScientificImage};

use crate::{AffineTransform, CoordinateError, ImagePoint, ProjectiveTransform};

/// Stable identifier for strict inverse-mapped, normalized Lanczos-3 sampling.
pub const LANCZOS3_RESAMPLING_ALGORITHM_ID: &str = "lanczos3-normalized-f64-v1";

const LANCZOS_RADIUS: i64 = 3;
const LANCZOS_TAPS: usize = 6;
const MINIMUM_WEIGHT_SUM: f64 = 1.0e-12;

/// Complete support accounting for one resampled image.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ResamplingStatistics {
    total_samples: usize,
    interpolated_samples: usize,
    outside_footprint_samples: usize,
    masked_support_samples: usize,
}

impl ResamplingStatistics {
    /// Output samples across all planes.
    #[must_use]
    pub const fn total_samples(self) -> usize {
        self.total_samples
    }

    /// Finite samples produced from complete clear support.
    #[must_use]
    pub const fn interpolated_samples(self) -> usize {
        self.interpolated_samples
    }

    /// Samples whose non-zero kernel support crossed a source boundary.
    #[must_use]
    pub const fn outside_footprint_samples(self) -> usize {
        self.outside_footprint_samples
    }

    /// Samples withheld because at least one non-zero source tap was unusable.
    #[must_use]
    pub const fn masked_support_samples(self) -> usize {
        self.masked_support_samples
    }

    /// Combines disjoint output regions with checked accounting.
    pub fn checked_add(self, other: Self) -> Result<Self, ResamplingError> {
        Ok(Self {
            total_samples: self
                .total_samples
                .checked_add(other.total_samples)
                .ok_or(ResamplingError::CountOverflow)?,
            interpolated_samples: self
                .interpolated_samples
                .checked_add(other.interpolated_samples)
                .ok_or(ResamplingError::CountOverflow)?,
            outside_footprint_samples: self
                .outside_footprint_samples
                .checked_add(other.outside_footprint_samples)
                .ok_or(ResamplingError::CountOverflow)?,
            masked_support_samples: self
                .masked_support_samples
                .checked_add(other.masked_support_samples)
                .ok_or(ResamplingError::CountOverflow)?,
        })
    }
}

/// A fully materialized strict CPU reference result.
#[derive(Clone, Debug, PartialEq)]
pub struct ResampledImage {
    image: ScientificImage,
    source_to_reference: AffineTransform,
    statistics: ResamplingStatistics,
}

/// Fully materialized projective scalar-oracle result.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectivelyResampledImage {
    image: ScientificImage,
    source_to_reference: ProjectiveTransform,
    statistics: ResamplingStatistics,
}

/// One bounded, reference-aligned output band.
#[derive(Clone, Debug, PartialEq)]
pub struct ResampledBand {
    reference_y: usize,
    image: ScientificImage,
    statistics: ResamplingStatistics,
}

/// Smallest rectangular source region containing every tap used by one band.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Lanczos3SourceWindow {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

impl Lanczos3SourceWindow {
    /// First included source column.
    #[must_use]
    pub const fn x(self) -> usize {
        self.x
    }

    /// First included source row.
    #[must_use]
    pub const fn y(self) -> usize {
        self.y
    }

    /// Number of included source columns.
    #[must_use]
    pub const fn width(self) -> usize {
        self.width
    }

    /// Number of included source rows.
    #[must_use]
    pub const fn height(self) -> usize {
        self.height
    }
}

/// Immutable, validated geometry for one window-backed resampling band.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lanczos3BandPlan {
    source_dimensions: Dimensions,
    output_width: usize,
    output_height: usize,
    reference_y: usize,
    band_height: usize,
    source_to_reference: AffineTransform,
    reference_to_source: AffineTransform,
    source_window: Option<Lanczos3SourceWindow>,
}

/// Immutable, validated projective geometry for one window-backed band.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectiveLanczos3BandPlan {
    source_dimensions: Dimensions,
    output_width: usize,
    output_height: usize,
    reference_y: usize,
    band_height: usize,
    source_to_reference: ProjectiveTransform,
    reference_to_source: ProjectiveTransform,
    source_window: Option<Lanczos3SourceWindow>,
}

impl Lanczos3BandPlan {
    /// Plans one band and its exact source read rectangle.
    pub fn new(
        source_dimensions: Dimensions,
        output_width: usize,
        output_height: usize,
        reference_y: usize,
        band_height: usize,
        source_to_reference: AffineTransform,
    ) -> Result<Self, ResamplingError> {
        let source_window = plan_lanczos3_source_window(
            source_dimensions.width(),
            source_dimensions.height(),
            output_width,
            output_height,
            reference_y,
            band_height,
            source_to_reference,
        )?;
        let reference_to_source = source_to_reference
            .inverse()
            .map_err(ResamplingError::Coordinate)?;
        Ok(Self {
            source_dimensions,
            output_width,
            output_height,
            reference_y,
            band_height,
            source_to_reference,
            reference_to_source,
            source_window,
        })
    }

    /// Complete source dimensions represented by the eventual window.
    #[must_use]
    pub const fn source_dimensions(self) -> Dimensions {
        self.source_dimensions
    }

    /// Complete reference-canvas width.
    #[must_use]
    pub const fn output_width(self) -> usize {
        self.output_width
    }

    /// Complete reference-canvas height.
    #[must_use]
    pub const fn output_height(self) -> usize {
        self.output_height
    }

    /// First output row produced by this plan.
    #[must_use]
    pub const fn reference_y(self) -> usize {
        self.reference_y
    }

    /// Number of output rows produced by this plan.
    #[must_use]
    pub const fn band_height(self) -> usize {
        self.band_height
    }

    /// Exact source-to-reference transform applied by this plan.
    #[must_use]
    pub const fn source_to_reference(self) -> AffineTransform {
        self.source_to_reference
    }

    /// Exact rectangular read required, or `None` for a fully disjoint band.
    #[must_use]
    pub const fn source_window(self) -> Option<Lanczos3SourceWindow> {
        self.source_window
    }

    /// Resamples this band from exactly the planned source window.
    ///
    /// A disjoint plan requires `None` and produces an all-missing band. Every
    /// intersecting plan requires an image whose width, height, and plane count
    /// exactly match the plan, preventing accidental coordinate rebasing.
    pub fn resample(
        self,
        source_window_image: Option<&ScientificImage>,
    ) -> Result<ResampledBand, ResamplingError> {
        match (self.source_window, source_window_image) {
            (None, None) => missing_band(
                self.output_width,
                self.band_height,
                self.reference_y,
                self.source_dimensions.planes(),
            ),
            (None, Some(_)) => Err(ResamplingError::UnexpectedSourceWindow),
            (Some(_), None) => Err(ResamplingError::MissingSourceWindow),
            (Some(window), Some(image)) => {
                validate_source_window(image, window, self.source_dimensions.planes())?;
                resample_lanczos3_planned_band(image, window, self)
            }
        }
    }
}

impl ProjectiveLanczos3BandPlan {
    /// Plans one projective band and its exact source read rectangle.
    pub fn new(
        source_dimensions: Dimensions,
        output_width: usize,
        output_height: usize,
        reference_y: usize,
        band_height: usize,
        source_to_reference: ProjectiveTransform,
    ) -> Result<Self, ResamplingError> {
        let source_window = plan_lanczos3_projective_source_window(
            source_dimensions.width(),
            source_dimensions.height(),
            output_width,
            output_height,
            reference_y,
            band_height,
            source_to_reference,
        )?;
        let reference_to_source = source_to_reference
            .inverse()
            .map_err(ResamplingError::Coordinate)?;
        Ok(Self {
            source_dimensions,
            output_width,
            output_height,
            reference_y,
            band_height,
            source_to_reference,
            reference_to_source,
            source_window,
        })
    }

    /// Complete source dimensions represented by the eventual window.
    #[must_use]
    pub const fn source_dimensions(self) -> Dimensions {
        self.source_dimensions
    }

    /// Complete reference-canvas width.
    #[must_use]
    pub const fn output_width(self) -> usize {
        self.output_width
    }

    /// Complete reference-canvas height.
    #[must_use]
    pub const fn output_height(self) -> usize {
        self.output_height
    }

    /// First output row produced by this plan.
    #[must_use]
    pub const fn reference_y(self) -> usize {
        self.reference_y
    }

    /// Number of output rows produced by this plan.
    #[must_use]
    pub const fn band_height(self) -> usize {
        self.band_height
    }

    /// Exact source-to-reference homography applied by this plan.
    #[must_use]
    pub const fn source_to_reference(self) -> ProjectiveTransform {
        self.source_to_reference
    }

    /// Exact rectangular read required, or `None` for a fully disjoint band.
    #[must_use]
    pub const fn source_window(self) -> Option<Lanczos3SourceWindow> {
        self.source_window
    }

    /// Resamples this band from exactly the planned source window.
    pub fn resample(
        self,
        source_window_image: Option<&ScientificImage>,
    ) -> Result<ResampledBand, ResamplingError> {
        match (self.source_window, source_window_image) {
            (None, None) => missing_band(
                self.output_width,
                self.band_height,
                self.reference_y,
                self.source_dimensions.planes(),
            ),
            (None, Some(_)) => Err(ResamplingError::UnexpectedSourceWindow),
            (Some(_), None) => Err(ResamplingError::MissingSourceWindow),
            (Some(window), Some(image)) => {
                validate_source_window(image, window, self.source_dimensions.planes())?;
                resample_lanczos3_planned_band_with(
                    image,
                    window,
                    self.source_dimensions,
                    self.output_width,
                    self.reference_y,
                    self.band_height,
                    |point| self.reference_to_source.apply(point),
                )
            }
        }
    }
}

impl ResampledBand {
    /// First reference row represented by this band.
    #[must_use]
    pub const fn reference_y(&self) -> usize {
        self.reference_y
    }

    /// Complete support accounting for this band only.
    #[must_use]
    pub const fn statistics(&self) -> ResamplingStatistics {
        self.statistics
    }

    /// Band pixels and conservative mask, stored plane-major.
    #[must_use]
    pub const fn image(&self) -> &ScientificImage {
        &self.image
    }

    /// Consumes the band and returns its image storage.
    #[must_use]
    pub fn into_image(self) -> ScientificImage {
        self.image
    }
}

/// Stateful bounded-memory traversal of a complete reference canvas.
#[derive(Debug)]
pub struct Lanczos3BandExecutor<'a> {
    source: &'a ScientificImage,
    output_dimensions: Dimensions,
    source_to_reference: AffineTransform,
    reference_to_source: AffineTransform,
    band_height: usize,
    next_reference_y: usize,
    statistics: ResamplingStatistics,
}

/// Stateful bounded-memory projective traversal of a reference canvas.
#[derive(Debug)]
pub struct ProjectiveLanczos3BandExecutor<'a> {
    source: &'a ScientificImage,
    output_dimensions: Dimensions,
    source_to_reference: ProjectiveTransform,
    reference_to_source: ProjectiveTransform,
    band_height: usize,
    next_reference_y: usize,
    statistics: ResamplingStatistics,
}

impl<'a> Lanczos3BandExecutor<'a> {
    /// Validates a deterministic top-to-bottom band traversal.
    pub fn new(
        source: &'a ScientificImage,
        output_width: usize,
        output_height: usize,
        band_height: usize,
        source_to_reference: AffineTransform,
    ) -> Result<Self, ResamplingError> {
        if band_height == 0 {
            return Err(ResamplingError::ZeroBandHeight);
        }
        let output_dimensions =
            Dimensions::new(output_width, output_height, source.dimensions().planes())?;
        let reference_to_source = source_to_reference
            .inverse()
            .map_err(ResamplingError::Coordinate)?;
        Ok(Self {
            source,
            output_dimensions,
            source_to_reference,
            reference_to_source,
            band_height,
            next_reference_y: 0,
            statistics: ResamplingStatistics::default(),
        })
    }

    /// Versioned interpolation and support policy shared with the scalar oracle.
    #[must_use]
    pub const fn algorithm_id(&self) -> &'static str {
        LANCZOS3_RESAMPLING_ALGORITHM_ID
    }

    /// Exact source-to-reference transform used by every band.
    #[must_use]
    pub const fn source_to_reference(&self) -> AffineTransform {
        self.source_to_reference
    }

    /// Complete output dimensions, independent of the chosen band height.
    #[must_use]
    pub const fn output_dimensions(&self) -> Dimensions {
        self.output_dimensions
    }

    /// Requested maximum number of output rows held by one returned band.
    #[must_use]
    pub const fn band_height(&self) -> usize {
        self.band_height
    }

    /// Support accounting for all bands returned so far.
    #[must_use]
    pub const fn statistics(&self) -> ResamplingStatistics {
        self.statistics
    }

    /// Returns whether every output row has been produced successfully.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.next_reference_y == self.output_dimensions.height()
    }

    /// Produces the next top-to-bottom band using global reference coordinates.
    ///
    /// Returned storage never exceeds `output_width * band_height * planes`.
    /// `Ok(None)` is stable after completion, allowing simple writer loops.
    pub fn next_band(&mut self) -> Result<Option<ResampledBand>, ResamplingError> {
        if self.is_complete() {
            return Ok(None);
        }
        let remaining = self
            .output_dimensions
            .height()
            .checked_sub(self.next_reference_y)
            .ok_or(ResamplingError::CountOverflow)?;
        let height = remaining.min(self.band_height);
        let band = resample_lanczos3_band(
            self.source,
            self.output_dimensions.width(),
            self.next_reference_y,
            height,
            self.reference_to_source,
        )?;
        self.statistics = self.statistics.checked_add(band.statistics)?;
        self.next_reference_y = self
            .next_reference_y
            .checked_add(height)
            .ok_or(ResamplingError::CountOverflow)?;
        Ok(Some(band))
    }
}

impl<'a> ProjectiveLanczos3BandExecutor<'a> {
    /// Validates a deterministic top-to-bottom projective traversal.
    pub fn new(
        source: &'a ScientificImage,
        output_width: usize,
        output_height: usize,
        band_height: usize,
        source_to_reference: ProjectiveTransform,
    ) -> Result<Self, ResamplingError> {
        if band_height == 0 {
            return Err(ResamplingError::ZeroBandHeight);
        }
        let output_dimensions =
            Dimensions::new(output_width, output_height, source.dimensions().planes())?;
        let reference_to_source = source_to_reference
            .inverse()
            .map_err(ResamplingError::Coordinate)?;
        Ok(Self {
            source,
            output_dimensions,
            source_to_reference,
            reference_to_source,
            band_height,
            next_reference_y: 0,
            statistics: ResamplingStatistics::default(),
        })
    }

    /// Versioned interpolation and support policy shared with the scalar oracle.
    #[must_use]
    pub const fn algorithm_id(&self) -> &'static str {
        LANCZOS3_RESAMPLING_ALGORITHM_ID
    }

    /// Exact source-to-reference homography used by every band.
    #[must_use]
    pub const fn source_to_reference(&self) -> ProjectiveTransform {
        self.source_to_reference
    }

    /// Complete output dimensions, independent of the chosen band height.
    #[must_use]
    pub const fn output_dimensions(&self) -> Dimensions {
        self.output_dimensions
    }

    /// Requested maximum number of output rows held by one returned band.
    #[must_use]
    pub const fn band_height(&self) -> usize {
        self.band_height
    }

    /// Support accounting for all bands returned so far.
    #[must_use]
    pub const fn statistics(&self) -> ResamplingStatistics {
        self.statistics
    }

    /// Returns whether every output row has been produced successfully.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.next_reference_y == self.output_dimensions.height()
    }

    /// Produces the next projective band using global reference coordinates.
    pub fn next_band(&mut self) -> Result<Option<ResampledBand>, ResamplingError> {
        if self.is_complete() {
            return Ok(None);
        }
        let remaining = self
            .output_dimensions
            .height()
            .checked_sub(self.next_reference_y)
            .ok_or(ResamplingError::CountOverflow)?;
        let height = remaining.min(self.band_height);
        let band = resample_lanczos3_band_with(
            self.source,
            self.output_dimensions.width(),
            self.next_reference_y,
            height,
            |point| self.reference_to_source.apply(point),
        )?;
        self.statistics = self.statistics.checked_add(band.statistics)?;
        self.next_reference_y = self
            .next_reference_y
            .checked_add(height)
            .ok_or(ResamplingError::CountOverflow)?;
        Ok(Some(band))
    }
}

impl ResampledImage {
    /// Versioned interpolation and support policy.
    #[must_use]
    pub const fn algorithm_id(&self) -> &'static str {
        LANCZOS3_RESAMPLING_ALGORITHM_ID
    }

    /// Exact transform whose inverse was evaluated at output pixel centers.
    #[must_use]
    pub const fn source_to_reference(&self) -> AffineTransform {
        self.source_to_reference
    }

    /// Complete output-support accounting.
    #[must_use]
    pub const fn statistics(&self) -> ResamplingStatistics {
        self.statistics
    }

    /// Immutable output image and conservative mask.
    #[must_use]
    pub const fn image(&self) -> &ScientificImage {
        &self.image
    }

    /// Consumes the report and returns the output image.
    #[must_use]
    pub fn into_image(self) -> ScientificImage {
        self.image
    }
}

impl ProjectivelyResampledImage {
    /// Versioned interpolation and support policy shared with affine sampling.
    #[must_use]
    pub const fn algorithm_id(&self) -> &'static str {
        LANCZOS3_RESAMPLING_ALGORITHM_ID
    }

    /// Exact homography whose inverse was evaluated at output pixel centers.
    #[must_use]
    pub const fn source_to_reference(&self) -> ProjectiveTransform {
        self.source_to_reference
    }

    /// Complete output-support accounting.
    #[must_use]
    pub const fn statistics(&self) -> ResamplingStatistics {
        self.statistics
    }

    /// Immutable output image and conservative mask.
    #[must_use]
    pub const fn image(&self) -> &ScientificImage {
        &self.image
    }

    /// Consumes the report and returns its image storage.
    #[must_use]
    pub fn into_image(self) -> ScientificImage {
        self.image
    }
}

/// Failure raised before a complete resampled image can be returned.
#[derive(Clone, Debug, PartialEq)]
pub enum ResamplingError {
    /// A bounded executor must make progress by at least one row.
    ZeroBandHeight,
    /// The requested reference rows do not fit inside the output canvas.
    ReferenceBandOutOfBounds {
        /// First requested reference row.
        y: usize,
        /// Requested row count.
        height: usize,
        /// Complete output height.
        output_height: usize,
    },
    /// A geometrically intersecting plan was executed without its source data.
    MissingSourceWindow,
    /// A fully disjoint plan was supplied an unnecessary source window.
    UnexpectedSourceWindow,
    /// Decoded source storage does not exactly match the planned rectangle.
    SourceWindowDimensions {
        /// Planned source-window width.
        expected_width: usize,
        /// Planned source-window height.
        expected_height: usize,
        /// Planned source plane count.
        expected_planes: usize,
        /// Supplied source-window width.
        actual_width: usize,
        /// Supplied source-window height.
        actual_height: usize,
        /// Supplied source plane count.
        actual_planes: usize,
    },
    /// An exact planned tap was absent from the supplied source window.
    IncompleteSourceWindow,
    /// Output dimensions or allocation violated the shared image contract.
    Core(CoreError),
    /// Transform inversion or application failed.
    Coordinate(CoordinateError),
    /// A coordinate or weighted calculation left the finite `f64` domain.
    NumericalOverflow,
    /// Support accounting exceeded `usize`.
    CountOverflow,
}

impl Display for ResamplingError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ZeroBandHeight => formatter.write_str("resampling band height must be positive"),
            Self::ReferenceBandOutOfBounds {
                y,
                height,
                output_height,
            } => write!(
                formatter,
                "reference band y={y}, height={height} exceeds output height {output_height}"
            ),
            Self::MissingSourceWindow => {
                formatter.write_str("resampling plan requires its exact source window")
            }
            Self::UnexpectedSourceWindow => {
                formatter.write_str("disjoint resampling plan requires no source window")
            }
            Self::SourceWindowDimensions {
                expected_width,
                expected_height,
                expected_planes,
                actual_width,
                actual_height,
                actual_planes,
            } => write!(
                formatter,
                "source window must be {expected_width}x{expected_height}x{expected_planes}, received {actual_width}x{actual_height}x{actual_planes}"
            ),
            Self::IncompleteSourceWindow => {
                formatter.write_str("planned source window does not contain every required tap")
            }
            Self::Core(error) => write!(formatter, "cannot construct resampled image: {error}"),
            Self::Coordinate(error) => {
                write!(formatter, "cannot map resampling coordinate: {error}")
            }
            Self::NumericalOverflow => {
                formatter.write_str("Lanczos resampling left the finite numerical domain")
            }
            Self::CountOverflow => formatter.write_str("resampling support count overflow"),
        }
    }
}

impl Error for ResamplingError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Core(error) => Some(error),
            Self::Coordinate(error) => Some(error),
            Self::ZeroBandHeight
            | Self::ReferenceBandOutOfBounds { .. }
            | Self::MissingSourceWindow
            | Self::UnexpectedSourceWindow
            | Self::SourceWindowDimensions { .. }
            | Self::IncompleteSourceWindow
            | Self::NumericalOverflow
            | Self::CountOverflow => None,
        }
    }
}

impl From<CoreError> for ResamplingError {
    fn from(value: CoreError) -> Self {
        Self::Core(value)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct AxisTap {
    index: usize,
    weight: f64,
}

#[derive(Clone, Copy, Debug)]
struct AxisKernel {
    taps: [AxisTap; LANCZOS_TAPS],
    count: usize,
    complete: bool,
}

impl AxisKernel {
    fn new(coordinate: f64, source_length: usize) -> Result<Self, ResamplingError> {
        if !coordinate.is_finite() {
            return Err(ResamplingError::NumericalOverflow);
        }
        let mut kernel = Self {
            taps: [AxisTap::default(); LANCZOS_TAPS],
            count: 0,
            complete: true,
        };
        let maximum = source_length.saturating_sub(1) as f64;
        if coordinate <= -(LANCZOS_RADIUS as f64) || coordinate >= maximum + LANCZOS_RADIUS as f64 {
            kernel.complete = false;
            return Ok(kernel);
        }
        let floor = coordinate.floor();
        if floor < i64::MIN as f64 || floor > i64::MAX as f64 {
            kernel.complete = false;
            return Ok(kernel);
        }
        let floor = floor as i64;
        for offset in (-LANCZOS_RADIUS + 1)..=LANCZOS_RADIUS {
            let source_index = floor
                .checked_add(offset)
                .ok_or(ResamplingError::NumericalOverflow)?;
            let distance = coordinate - source_index as f64;
            let weight = lanczos3(distance);
            if weight == 0.0 {
                continue;
            }
            let Ok(index) = usize::try_from(source_index) else {
                kernel.complete = false;
                continue;
            };
            if index >= source_length {
                kernel.complete = false;
                continue;
            }
            let Some(slot) = kernel.taps.get_mut(kernel.count) else {
                return Err(ResamplingError::CountOverflow);
            };
            *slot = AxisTap { index, weight };
            kernel.count = kernel
                .count
                .checked_add(1)
                .ok_or(ResamplingError::CountOverflow)?;
        }
        if kernel.count == 0 {
            kernel.complete = false;
        }
        Ok(kernel)
    }

    fn active(&self) -> &[AxisTap] {
        &self.taps[..self.count]
    }
}

/// Computes geometry-only normalization once for every linked image plane.
///
/// The tap order matches the historical per-plane loop exactly, preserving the
/// binary64 denominator while avoiding redundant work for RGB images.
fn kernel_weight_sum(x_kernel: &AxisKernel, y_kernel: &AxisKernel) -> Result<f64, ResamplingError> {
    let mut sum = CompensatedSum::new();
    for y_tap in y_kernel.active() {
        for x_tap in x_kernel.active() {
            let weight = x_tap.weight * y_tap.weight;
            if !weight.is_finite() {
                return Err(ResamplingError::NumericalOverflow);
            }
            sum.add(weight);
        }
    }
    Ok(sum.total())
}

/// Plans the exact source rectangle needed to resample one reference band.
///
/// Every discrete output center is inverse-mapped and evaluated with the same
/// analytical-zero Lanczos policy as the numerical oracle. Only taps belonging
/// to complete two-dimensional kernels contribute to the rectangle; samples
/// outside the source footprint need no source I/O because they become missing.
/// `None` therefore means the entire output band is outside the source.
pub fn plan_lanczos3_source_window(
    source_width: usize,
    source_height: usize,
    output_width: usize,
    output_height: usize,
    reference_y: usize,
    band_height: usize,
    source_to_reference: AffineTransform,
) -> Result<Option<Lanczos3SourceWindow>, ResamplingError> {
    let reference_to_source = source_to_reference
        .inverse()
        .map_err(ResamplingError::Coordinate)?;
    plan_lanczos3_source_window_with(
        source_width,
        source_height,
        output_width,
        output_height,
        reference_y,
        band_height,
        |point| reference_to_source.apply(point),
    )
}

/// Plans the exact source rectangle needed for one projective reference band.
///
/// Unlike a corner-only approximation, this evaluates every discrete output
/// center and its exact non-zero Lanczos support. That is deliberately more
/// conservative computationally and guarantees that a curved projective path
/// cannot escape the planned read rectangle between sampled corners.
pub fn plan_lanczos3_projective_source_window(
    source_width: usize,
    source_height: usize,
    output_width: usize,
    output_height: usize,
    reference_y: usize,
    band_height: usize,
    source_to_reference: ProjectiveTransform,
) -> Result<Option<Lanczos3SourceWindow>, ResamplingError> {
    let reference_to_source = source_to_reference
        .inverse()
        .map_err(ResamplingError::Coordinate)?;
    plan_lanczos3_source_window_with(
        source_width,
        source_height,
        output_width,
        output_height,
        reference_y,
        band_height,
        |point| reference_to_source.apply(point),
    )
}

fn plan_lanczos3_source_window_with<F>(
    source_width: usize,
    source_height: usize,
    output_width: usize,
    output_height: usize,
    reference_y: usize,
    band_height: usize,
    reference_to_source: F,
) -> Result<Option<Lanczos3SourceWindow>, ResamplingError>
where
    F: Fn(ImagePoint) -> Result<ImagePoint, CoordinateError>,
{
    if band_height == 0 {
        return Err(ResamplingError::ZeroBandHeight);
    }
    Dimensions::new(source_width, source_height, 1)?;
    Dimensions::new(output_width, output_height, 1)?;
    let band_bottom = reference_y
        .checked_add(band_height)
        .ok_or(ResamplingError::CountOverflow)?;
    if reference_y >= output_height || band_bottom > output_height {
        return Err(ResamplingError::ReferenceBandOutOfBounds {
            y: reference_y,
            height: band_height,
            output_height,
        });
    }
    let mut minimum_x = usize::MAX;
    let mut minimum_y = usize::MAX;
    let mut maximum_x = 0_usize;
    let mut maximum_y = 0_usize;
    let mut has_support = false;

    for output_y in reference_y..band_bottom {
        for output_x in 0..output_width {
            let reference_point = ImagePoint::new(output_x as f64, output_y as f64)
                .map_err(ResamplingError::Coordinate)?;
            let source_point =
                reference_to_source(reference_point).map_err(ResamplingError::Coordinate)?;
            let x_kernel = AxisKernel::new(source_point.x(), source_width)?;
            let y_kernel = AxisKernel::new(source_point.y(), source_height)?;
            if !x_kernel.complete || !y_kernel.complete {
                continue;
            }
            for tap in x_kernel.active() {
                minimum_x = minimum_x.min(tap.index);
                maximum_x = maximum_x.max(tap.index);
            }
            for tap in y_kernel.active() {
                minimum_y = minimum_y.min(tap.index);
                maximum_y = maximum_y.max(tap.index);
            }
            has_support = true;
        }
    }

    if !has_support {
        return Ok(None);
    }
    let width = maximum_x
        .checked_sub(minimum_x)
        .and_then(|span| span.checked_add(1))
        .ok_or(ResamplingError::CountOverflow)?;
    let height = maximum_y
        .checked_sub(minimum_y)
        .and_then(|span| span.checked_add(1))
        .ok_or(ResamplingError::CountOverflow)?;
    Ok(Some(Lanczos3SourceWindow {
        x: minimum_x,
        y: minimum_y,
        width,
        height,
    }))
}

/// Resamples every source plane into a reference-aligned output rectangle.
///
/// The supplied affine transform maps source coordinates to reference
/// coordinates. The implementation inverts it exactly once, evaluates that
/// inverse at each integer-centered output pixel, and applies a separable
/// normalized Lanczos-3 kernel in `f64`. Output is never clipped.
///
/// A sample is emitted only when every mathematically non-zero tap lies inside
/// the source and contains a finite, clear value. Boundary loss becomes
/// [`PixelFlags::MISSING`]; unusable source support preserves the union of its
/// flags, adding [`PixelFlags::INVALID`] for unflagged non-finite values. This
/// strict policy prevents interpolation from concealing defects.
///
/// This complete-image scalar implementation is the numerical oracle. A later
/// band executor may optimize traversal and memory use only after differential
/// tests prove agreement with it.
pub fn resample_lanczos3(
    source: &ScientificImage,
    output_width: usize,
    output_height: usize,
    source_to_reference: AffineTransform,
) -> Result<ResampledImage, ResamplingError> {
    let reference_to_source = source_to_reference
        .inverse()
        .map_err(ResamplingError::Coordinate)?;
    let (image, statistics) =
        resample_lanczos3_oracle(source, output_width, output_height, |point| {
            reference_to_source.apply(point)
        })?;
    Ok(ResampledImage {
        image,
        source_to_reference,
        statistics,
    })
}

/// Resamples through a finite source-to-reference homography.
///
/// This uses the identical scalar Lanczos-3 oracle, support policy, masks, and
/// output accounting as [`resample_lanczos3`]. The homography is inverted once;
/// a projective horizon at any evaluated output center is an explicit failure.
/// Production selection and bounded band execution remain separate concerns.
pub fn resample_lanczos3_projective(
    source: &ScientificImage,
    output_width: usize,
    output_height: usize,
    source_to_reference: ProjectiveTransform,
) -> Result<ProjectivelyResampledImage, ResamplingError> {
    let reference_to_source = source_to_reference
        .inverse()
        .map_err(ResamplingError::Coordinate)?;
    let (image, statistics) =
        resample_lanczos3_oracle(source, output_width, output_height, |point| {
            reference_to_source.apply(point)
        })?;
    Ok(ProjectivelyResampledImage {
        image,
        source_to_reference,
        statistics,
    })
}

fn resample_lanczos3_oracle<F>(
    source: &ScientificImage,
    output_width: usize,
    output_height: usize,
    reference_to_source: F,
) -> Result<(ScientificImage, ResamplingStatistics), ResamplingError>
where
    F: Fn(ImagePoint) -> Result<ImagePoint, CoordinateError>,
{
    let source_dimensions = source.dimensions();
    let output_dimensions =
        Dimensions::new(output_width, output_height, source_dimensions.planes())?;
    let mut output = ScientificImage::filled(output_dimensions, f64::NAN)?;
    let output_area = output_width
        .checked_mul(output_height)
        .ok_or(ResamplingError::CountOverflow)?;
    let source_area = source_dimensions
        .width()
        .checked_mul(source_dimensions.height())
        .ok_or(ResamplingError::CountOverflow)?;
    let mut statistics = ResamplingStatistics {
        total_samples: output_dimensions.pixel_count(),
        ..ResamplingStatistics::default()
    };

    for output_y in 0..output_height {
        for output_x in 0..output_width {
            let reference_point = ImagePoint::new(output_x as f64, output_y as f64)
                .map_err(ResamplingError::Coordinate)?;
            let source_point =
                reference_to_source(reference_point).map_err(ResamplingError::Coordinate)?;
            let x_kernel = AxisKernel::new(source_point.x(), source_dimensions.width())?;
            let y_kernel = AxisKernel::new(source_point.y(), source_dimensions.height())?;
            if !x_kernel.complete || !y_kernel.complete {
                for plane in 0..source_dimensions.planes() {
                    let output_index =
                        linear_index(output_area, output_width, output_x, output_y, plane)?;
                    output.mask_mut().as_mut_slice()[output_index] = PixelFlags::MISSING;
                    statistics.outside_footprint_samples =
                        checked_increment(statistics.outside_footprint_samples)?;
                }
                continue;
            }
            let denominator = kernel_weight_sum(&x_kernel, &y_kernel)?;

            for plane in 0..source_dimensions.planes() {
                let mut weighted_sum = CompensatedSum::new();
                let mut combined_flags = PixelFlags::CLEAR;
                for y_tap in y_kernel.active() {
                    for x_tap in x_kernel.active() {
                        let weight = x_tap.weight * y_tap.weight;
                        let source_index = linear_index(
                            source_area,
                            source_dimensions.width(),
                            x_tap.index,
                            y_tap.index,
                            plane,
                        )?;
                        let value = source.pixels()[source_index];
                        combined_flags |= source.mask().as_slice()[source_index];
                        if value.is_finite() {
                            weighted_sum.add(value * weight);
                        } else {
                            combined_flags |= PixelFlags::INVALID;
                        }
                    }
                }
                let output_index =
                    linear_index(output_area, output_width, output_x, output_y, plane)?;
                if !combined_flags.is_clear() {
                    output.mask_mut().as_mut_slice()[output_index] = combined_flags;
                    statistics.masked_support_samples =
                        checked_increment(statistics.masked_support_samples)?;
                    continue;
                }
                if !denominator.is_finite() || denominator.abs() < MINIMUM_WEIGHT_SUM {
                    return Err(ResamplingError::NumericalOverflow);
                }
                let value = weighted_sum.total() / denominator;
                if !value.is_finite() {
                    return Err(ResamplingError::NumericalOverflow);
                }
                output.pixels_mut()[output_index] = canonical_zero(value);
                statistics.interpolated_samples =
                    checked_increment(statistics.interpolated_samples)?;
            }
        }
    }

    let accounted = statistics
        .interpolated_samples
        .checked_add(statistics.outside_footprint_samples)
        .and_then(|value| value.checked_add(statistics.masked_support_samples))
        .ok_or(ResamplingError::CountOverflow)?;
    if accounted != statistics.total_samples {
        return Err(ResamplingError::CountOverflow);
    }
    Ok((output, statistics))
}

fn resample_lanczos3_band(
    source: &ScientificImage,
    output_width: usize,
    reference_y: usize,
    band_height: usize,
    reference_to_source: AffineTransform,
) -> Result<ResampledBand, ResamplingError> {
    resample_lanczos3_band_with(source, output_width, reference_y, band_height, |point| {
        reference_to_source.apply(point)
    })
}

fn resample_lanczos3_band_with<F>(
    source: &ScientificImage,
    output_width: usize,
    reference_y: usize,
    band_height: usize,
    reference_to_source: F,
) -> Result<ResampledBand, ResamplingError>
where
    F: Fn(ImagePoint) -> Result<ImagePoint, CoordinateError>,
{
    let source_dimensions = source.dimensions();
    let band_dimensions = Dimensions::new(output_width, band_height, source_dimensions.planes())?;
    let mut output = ScientificImage::filled(band_dimensions, f64::NAN)?;
    let band_area = output_width
        .checked_mul(band_height)
        .ok_or(ResamplingError::CountOverflow)?;
    let source_area = source_dimensions
        .width()
        .checked_mul(source_dimensions.height())
        .ok_or(ResamplingError::CountOverflow)?;
    let mut statistics = ResamplingStatistics {
        total_samples: band_dimensions.pixel_count(),
        ..ResamplingStatistics::default()
    };

    for band_y in 0..band_height {
        let output_y = reference_y
            .checked_add(band_y)
            .ok_or(ResamplingError::CountOverflow)?;
        for output_x in 0..output_width {
            let reference_point = ImagePoint::new(output_x as f64, output_y as f64)
                .map_err(ResamplingError::Coordinate)?;
            let source_point =
                reference_to_source(reference_point).map_err(ResamplingError::Coordinate)?;
            let x_kernel = AxisKernel::new(source_point.x(), source_dimensions.width())?;
            let y_kernel = AxisKernel::new(source_point.y(), source_dimensions.height())?;
            if !x_kernel.complete || !y_kernel.complete {
                for plane in 0..source_dimensions.planes() {
                    let output_index =
                        linear_index(band_area, output_width, output_x, band_y, plane)?;
                    output.mask_mut().as_mut_slice()[output_index] = PixelFlags::MISSING;
                    statistics.outside_footprint_samples =
                        checked_increment(statistics.outside_footprint_samples)?;
                }
                continue;
            }
            let denominator = kernel_weight_sum(&x_kernel, &y_kernel)?;

            for plane in 0..source_dimensions.planes() {
                let mut weighted_sum = CompensatedSum::new();
                let mut combined_flags = PixelFlags::CLEAR;
                for y_tap in y_kernel.active() {
                    for x_tap in x_kernel.active() {
                        let weight = x_tap.weight * y_tap.weight;
                        let source_index = linear_index(
                            source_area,
                            source_dimensions.width(),
                            x_tap.index,
                            y_tap.index,
                            plane,
                        )?;
                        let value = source.pixels()[source_index];
                        combined_flags |= source.mask().as_slice()[source_index];
                        if value.is_finite() {
                            weighted_sum.add(value * weight);
                        } else {
                            combined_flags |= PixelFlags::INVALID;
                        }
                    }
                }
                let output_index = linear_index(band_area, output_width, output_x, band_y, plane)?;
                if !combined_flags.is_clear() {
                    output.mask_mut().as_mut_slice()[output_index] = combined_flags;
                    statistics.masked_support_samples =
                        checked_increment(statistics.masked_support_samples)?;
                    continue;
                }
                if !denominator.is_finite() || denominator.abs() < MINIMUM_WEIGHT_SUM {
                    return Err(ResamplingError::NumericalOverflow);
                }
                let value = weighted_sum.total() / denominator;
                if !value.is_finite() {
                    return Err(ResamplingError::NumericalOverflow);
                }
                output.pixels_mut()[output_index] = canonical_zero(value);
                statistics.interpolated_samples =
                    checked_increment(statistics.interpolated_samples)?;
            }
        }
    }

    let accounted = statistics
        .interpolated_samples
        .checked_add(statistics.outside_footprint_samples)
        .and_then(|value| value.checked_add(statistics.masked_support_samples))
        .ok_or(ResamplingError::CountOverflow)?;
    if accounted != statistics.total_samples {
        return Err(ResamplingError::CountOverflow);
    }
    Ok(ResampledBand {
        reference_y,
        image: output,
        statistics,
    })
}

fn resample_lanczos3_planned_band(
    source: &ScientificImage,
    window: Lanczos3SourceWindow,
    plan: Lanczos3BandPlan,
) -> Result<ResampledBand, ResamplingError> {
    resample_lanczos3_planned_band_with(
        source,
        window,
        plan.source_dimensions,
        plan.output_width,
        plan.reference_y,
        plan.band_height,
        |point| plan.reference_to_source.apply(point),
    )
}

fn resample_lanczos3_planned_band_with<F>(
    source: &ScientificImage,
    window: Lanczos3SourceWindow,
    source_dimensions: Dimensions,
    output_width: usize,
    reference_y: usize,
    band_height: usize,
    reference_to_source: F,
) -> Result<ResampledBand, ResamplingError>
where
    F: Fn(ImagePoint) -> Result<ImagePoint, CoordinateError>,
{
    let output_dimensions = Dimensions::new(output_width, band_height, source_dimensions.planes())?;
    let mut output = ScientificImage::filled(output_dimensions, f64::NAN)?;
    let output_area = output_width
        .checked_mul(band_height)
        .ok_or(ResamplingError::CountOverflow)?;
    let source_area = window
        .width
        .checked_mul(window.height)
        .ok_or(ResamplingError::CountOverflow)?;
    let mut statistics = ResamplingStatistics {
        total_samples: output_dimensions.pixel_count(),
        ..ResamplingStatistics::default()
    };

    for band_y in 0..band_height {
        let output_y = reference_y
            .checked_add(band_y)
            .ok_or(ResamplingError::CountOverflow)?;
        for output_x in 0..output_width {
            let reference_point = ImagePoint::new(output_x as f64, output_y as f64)
                .map_err(ResamplingError::Coordinate)?;
            let source_point =
                reference_to_source(reference_point).map_err(ResamplingError::Coordinate)?;
            let x_kernel = AxisKernel::new(source_point.x(), source_dimensions.width())?;
            let y_kernel = AxisKernel::new(source_point.y(), source_dimensions.height())?;
            if !x_kernel.complete || !y_kernel.complete {
                for plane in 0..source_dimensions.planes() {
                    let output_index =
                        linear_index(output_area, output_width, output_x, band_y, plane)?;
                    output.mask_mut().as_mut_slice()[output_index] = PixelFlags::MISSING;
                    statistics.outside_footprint_samples =
                        checked_increment(statistics.outside_footprint_samples)?;
                }
                continue;
            }
            let denominator = kernel_weight_sum(&x_kernel, &y_kernel)?;

            for plane in 0..source_dimensions.planes() {
                let mut weighted_sum = CompensatedSum::new();
                let mut combined_flags = PixelFlags::CLEAR;
                for y_tap in y_kernel.active() {
                    let local_y = y_tap
                        .index
                        .checked_sub(window.y)
                        .filter(|&index| index < window.height)
                        .ok_or(ResamplingError::IncompleteSourceWindow)?;
                    for x_tap in x_kernel.active() {
                        let local_x = x_tap
                            .index
                            .checked_sub(window.x)
                            .filter(|&index| index < window.width)
                            .ok_or(ResamplingError::IncompleteSourceWindow)?;
                        let weight = x_tap.weight * y_tap.weight;
                        let source_index =
                            linear_index(source_area, window.width, local_x, local_y, plane)?;
                        let value = source.pixels()[source_index];
                        combined_flags |= source.mask().as_slice()[source_index];
                        if value.is_finite() {
                            weighted_sum.add(value * weight);
                        } else {
                            combined_flags |= PixelFlags::INVALID;
                        }
                    }
                }
                let output_index =
                    linear_index(output_area, output_width, output_x, band_y, plane)?;
                if !combined_flags.is_clear() {
                    output.mask_mut().as_mut_slice()[output_index] = combined_flags;
                    statistics.masked_support_samples =
                        checked_increment(statistics.masked_support_samples)?;
                    continue;
                }
                if !denominator.is_finite() || denominator.abs() < MINIMUM_WEIGHT_SUM {
                    return Err(ResamplingError::NumericalOverflow);
                }
                let value = weighted_sum.total() / denominator;
                if !value.is_finite() {
                    return Err(ResamplingError::NumericalOverflow);
                }
                output.pixels_mut()[output_index] = canonical_zero(value);
                statistics.interpolated_samples =
                    checked_increment(statistics.interpolated_samples)?;
            }
        }
    }

    let accounted = statistics
        .interpolated_samples
        .checked_add(statistics.outside_footprint_samples)
        .and_then(|value| value.checked_add(statistics.masked_support_samples))
        .ok_or(ResamplingError::CountOverflow)?;
    if accounted != statistics.total_samples {
        return Err(ResamplingError::CountOverflow);
    }
    Ok(ResampledBand {
        reference_y,
        image: output,
        statistics,
    })
}

fn validate_source_window(
    image: &ScientificImage,
    window: Lanczos3SourceWindow,
    expected_planes: usize,
) -> Result<(), ResamplingError> {
    let actual = image.dimensions();
    if actual.width() != window.width
        || actual.height() != window.height
        || actual.planes() != expected_planes
    {
        return Err(ResamplingError::SourceWindowDimensions {
            expected_width: window.width,
            expected_height: window.height,
            expected_planes,
            actual_width: actual.width(),
            actual_height: actual.height(),
            actual_planes: actual.planes(),
        });
    }
    Ok(())
}

fn missing_band(
    output_width: usize,
    band_height: usize,
    reference_y: usize,
    planes: usize,
) -> Result<ResampledBand, ResamplingError> {
    let dimensions = Dimensions::new(output_width, band_height, planes)?;
    let mut image = ScientificImage::filled(dimensions, f64::NAN)?;
    image.mask_mut().as_mut_slice().fill(PixelFlags::MISSING);
    Ok(ResampledBand {
        reference_y,
        image,
        statistics: ResamplingStatistics {
            total_samples: dimensions.pixel_count(),
            outside_footprint_samples: dimensions.pixel_count(),
            ..ResamplingStatistics::default()
        },
    })
}

fn linear_index(
    plane_area: usize,
    width: usize,
    x: usize,
    y: usize,
    plane: usize,
) -> Result<usize, ResamplingError> {
    plane
        .checked_mul(plane_area)
        .and_then(|offset| y.checked_mul(width).and_then(|row| offset.checked_add(row)))
        .and_then(|offset| offset.checked_add(x))
        .ok_or(ResamplingError::CountOverflow)
}

fn lanczos3(distance: f64) -> f64 {
    let absolute = distance.abs();
    if absolute >= LANCZOS_RADIUS as f64 {
        return 0.0;
    }
    sinc_pi(distance) * sinc_pi(distance / LANCZOS_RADIUS as f64)
}

#[allow(clippy::float_cmp)]
fn sinc_pi(value: f64) -> f64 {
    if value == 0.0 {
        return 1.0;
    }
    // Exact integer offsets are analytical zeros. Returning those zeros
    // explicitly makes identity transforms bit-exact and prevents irrelevant
    // neighboring masks from contaminating a direct sample.
    if value == value.round() {
        return 0.0;
    }
    let angle = PI * value;
    angle.sin() / angle
}

const fn checked_increment(value: usize) -> Result<usize, ResamplingError> {
    match value.checked_add(1) {
        Some(value) => Ok(value),
        None => Err(ResamplingError::CountOverflow),
    }
}

const fn canonical_zero(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn StdError>>;

    fn image(width: usize, height: usize, planes: usize) -> TestResult<ScientificImage> {
        let dimensions = Dimensions::new(width, height, planes)?;
        let pixels = (0..dimensions.pixel_count())
            .map(|index| index as f64 + 0.25)
            .collect();
        Ok(ScientificImage::from_pixels(dimensions, pixels)?)
    }

    fn extract_window(
        source: &ScientificImage,
        window: Lanczos3SourceWindow,
    ) -> TestResult<ScientificImage> {
        let source_dimensions = source.dimensions();
        let output_dimensions =
            Dimensions::new(window.width(), window.height(), source_dimensions.planes())?;
        let mut output = ScientificImage::filled(output_dimensions, f64::NAN)?;
        let source_area = source_dimensions.width() * source_dimensions.height();
        let output_area = output_dimensions.width() * output_dimensions.height();
        for plane in 0..source_dimensions.planes() {
            for local_y in 0..window.height() {
                let source_start = plane * source_area
                    + (window.y() + local_y) * source_dimensions.width()
                    + window.x();
                let output_start = plane * output_area + local_y * output_dimensions.width();
                let source_range = source_start..source_start + window.width();
                let output_range = output_start..output_start + window.width();
                output.pixels_mut()[output_range.clone()]
                    .copy_from_slice(&source.pixels()[source_range.clone()]);
                output.mask_mut().as_mut_slice()[output_range]
                    .copy_from_slice(&source.mask().as_slice()[source_range]);
            }
        }
        Ok(output)
    }

    fn copy_band_into(target: &mut ScientificImage, band: &ResampledBand) -> TestResult {
        let target_dimensions = target.dimensions();
        let band_dimensions = band.image().dimensions();
        let band_area = band_dimensions.width() * band_dimensions.height();
        let target_area = target_dimensions.width() * target_dimensions.height();
        for plane in 0..target_dimensions.planes() {
            for local_y in 0..band_dimensions.height() {
                let source_start = plane * band_area + local_y * band_dimensions.width();
                let target_start = plane * target_area
                    + (band.reference_y() + local_y) * target_dimensions.width();
                let source_range = source_start..source_start + band_dimensions.width();
                let target_range = target_start..target_start + target_dimensions.width();
                target.pixels_mut()[target_range.clone()]
                    .copy_from_slice(&band.image().pixels()[source_range.clone()]);
                target.mask_mut().as_mut_slice()[target_range]
                    .copy_from_slice(&band.image().mask().as_slice()[source_range]);
            }
        }
        Ok(())
    }

    #[test]
    fn identity_is_bit_exact_for_clear_pixels_and_preserves_mask_evidence() -> TestResult {
        let mut source = image(7, 6, 3)?;
        source.mark(0, 0, 0, PixelFlags::HOT)?;
        source.mark(6, 5, 2, PixelFlags::SATURATED)?;

        let result = resample_lanczos3(&source, 7, 6, AffineTransform::IDENTITY)?;

        assert_eq!(result.image().mask(), source.mask());
        for ((&input, &output), &flags) in source
            .pixels()
            .iter()
            .zip(result.image().pixels())
            .zip(source.mask().as_slice())
        {
            if flags.is_clear() {
                assert_eq!(output.to_bits(), input.to_bits());
            } else {
                assert!(output.is_nan());
            }
        }
        assert_eq!(result.statistics().interpolated_samples(), 124);
        assert_eq!(result.statistics().masked_support_samples(), 2);
        assert_eq!(result.statistics().outside_footprint_samples(), 0);
        Ok(())
    }

    #[test]
    fn affine_lift_agrees_with_projective_oracle_at_binary64_precision() -> TestResult {
        let mut source = image(13, 11, 2)?;
        source.mark(6, 5, 0, PixelFlags::HOT)?;
        source.pixels_mut()[13 * 11 + 7 * 13 + 8] = f64::NAN;
        let affine = AffineTransform::new(0.999, -0.012, 0.012, 0.999, 0.37, -0.28)?;

        let expected = resample_lanczos3(&source, 14, 12, affine)?;
        let actual = resample_lanczos3_projective(
            &source,
            14,
            12,
            ProjectiveTransform::from_affine(affine)?,
        )?;

        assert_eq!(actual.algorithm_id(), expected.algorithm_id());
        assert_eq!(actual.statistics(), expected.statistics());
        assert_eq!(actual.image().mask(), expected.image().mask());
        assert_eq!(
            actual.image().pixels().len(),
            expected.image().pixels().len()
        );
        for (&left, &right) in actual
            .image()
            .pixels()
            .iter()
            .zip(expected.image().pixels())
        {
            let tolerance = 1.0e-11 * right.abs().max(1.0);
            assert!(
                (left.is_nan() && right.is_nan()) || (left - right).abs() <= tolerance,
                "affine lift disagrees beyond binary64 roundoff"
            );
        }
        assert!(actual.source_to_reference().is_affine());
        Ok(())
    }

    #[test]
    fn projective_oracle_preserves_constant_complete_support() -> TestResult {
        let dimensions = Dimensions::new(31, 29, 1)?;
        let source = ScientificImage::filled(dimensions, 42.25)?;
        let transform = ProjectiveTransform::new([
            [1.0, 0.002, 0.1],
            [-0.001, 1.0, -0.15],
            [2.0e-5, -1.0e-5, 1.0],
        ])?;

        let result = resample_lanczos3_projective(&source, 31, 29, transform)?;

        assert!(!result.source_to_reference().is_affine());
        assert!(result.statistics().interpolated_samples() > 0);
        for (&value, &flags) in result
            .image()
            .pixels()
            .iter()
            .zip(result.image().mask().as_slice())
        {
            if flags.is_clear() {
                assert!((value - 42.25).abs() < 1.0e-12);
            } else {
                assert!(flags.contains(PixelFlags::MISSING));
                assert!(value.is_nan());
            }
        }
        Ok(())
    }

    #[test]
    fn projective_horizon_fails_instead_of_returning_partial_output() -> TestResult {
        let source = image(8, 8, 1)?;
        let transform =
            ProjectiveTransform::new([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.5, 0.0, 1.0]])?;

        assert!(matches!(
            resample_lanczos3_projective(&source, 8, 8, transform),
            Err(ResamplingError::Coordinate(
                CoordinateError::TransformOverflow
            ))
        ));
        Ok(())
    }

    #[test]
    fn fractional_translation_preserves_a_constant_on_complete_support() -> TestResult {
        let dimensions = Dimensions::new(12, 10, 1)?;
        let source = ScientificImage::filled(dimensions, 42.5)?;
        let transform = AffineTransform::new(1.0, 0.0, 0.0, 1.0, 0.25, -0.4)?;

        let result = resample_lanczos3(&source, 12, 10, transform)?;

        let statistics = result.statistics();
        assert!(statistics.interpolated_samples() > 0);
        assert!(statistics.outside_footprint_samples() > 0);
        for (&value, &flags) in result
            .image()
            .pixels()
            .iter()
            .zip(result.image().mask().as_slice())
        {
            if flags.is_clear() {
                assert!((value - 42.5).abs() < 1.0e-12);
            } else {
                assert!(value.is_nan());
                assert_eq!(flags, PixelFlags::MISSING);
            }
        }
        Ok(())
    }

    #[test]
    fn fractional_translation_preserves_isolated_source_flux_without_clipping() -> TestResult {
        let dimensions = Dimensions::new(31, 31, 1)?;
        let mut source = ScientificImage::filled(dimensions, 0.0)?;
        source.pixels_mut()[15 * 31 + 15] = 1.0;
        let transform = AffineTransform::new(1.0, 0.0, 0.0, 1.0, 0.37, -0.22)?;

        let result = resample_lanczos3(&source, 31, 31, transform)?;

        let mut flux = CompensatedSum::new();
        let mut minimum = f64::INFINITY;
        for (&value, &flags) in result
            .image()
            .pixels()
            .iter()
            .zip(result.image().mask().as_slice())
        {
            if flags.is_clear() {
                flux.add(value);
                minimum = minimum.min(value);
            }
        }
        assert!((flux.total() - 1.0).abs() < 1.0e-12);
        assert!(minimum < 0.0, "Lanczos negative lobes must not be clipped");
        Ok(())
    }

    #[test]
    fn nonzero_masked_support_is_conservative_but_zero_taps_are_ignored() -> TestResult {
        let mut source = ScientificImage::filled(Dimensions::new(9, 9, 1)?, 1.0)?;
        source.mark(4, 4, 0, PixelFlags::HOT)?;

        let identity = resample_lanczos3(&source, 9, 9, AffineTransform::IDENTITY)?;
        assert_eq!(identity.image().mask().get(4, 4, 0)?, PixelFlags::HOT);
        assert!(identity.image().mask().get(3, 4, 0)?.is_clear());

        let shifted = resample_lanczos3(
            &source,
            9,
            9,
            AffineTransform::new(1.0, 0.0, 0.0, 1.0, 0.5, 0.0)?,
        )?;
        assert!(shifted.statistics().masked_support_samples() > 1);
        assert!(
            shifted
                .image()
                .mask()
                .as_slice()
                .iter()
                .any(|flags| flags.contains(PixelFlags::HOT))
        );
        Ok(())
    }

    #[test]
    fn nonfinite_support_adds_invalid_without_losing_existing_flags() -> TestResult {
        let mut source = ScientificImage::filled(Dimensions::new(7, 7, 1)?, 1.0)?;
        source.pixels_mut()[24] = f64::NAN;
        source.mark(3, 3, 0, PixelFlags::REJECTED)?;

        let result = resample_lanczos3(&source, 7, 7, AffineTransform::IDENTITY)?;
        let flags = result.image().mask().get(3, 3, 0)?;
        assert!(flags.contains(PixelFlags::REJECTED));
        assert!(flags.contains(PixelFlags::INVALID));
        assert!(result.image().pixels()[24].is_nan());
        Ok(())
    }

    #[test]
    fn integer_translation_has_an_exact_missing_border() -> TestResult {
        let source = image(5, 4, 1)?;
        let transform = AffineTransform::new(1.0, 0.0, 0.0, 1.0, 1.0, 0.0)?;

        let result = resample_lanczos3(&source, 5, 4, transform)?;

        assert_eq!(result.statistics().outside_footprint_samples(), 4);
        for y in 0..4 {
            assert_eq!(result.image().mask().get(0, y, 0)?, PixelFlags::MISSING);
            for x in 1..5 {
                assert_eq!(
                    result.image().get(x, y, 0)?.to_bits(),
                    source.get(x - 1, y, 0)?.to_bits()
                );
            }
        }
        Ok(())
    }

    #[test]
    fn exact_quarter_turn_obeys_the_source_to_reference_convention() -> TestResult {
        let source = image(5, 5, 1)?;
        // In downward-positive image coordinates this maps
        // (x_source, y_source) to (4 - y_source, x_source).
        let transform = AffineTransform::new(0.0, -1.0, 1.0, 0.0, 4.0, 0.0)?;

        let result = resample_lanczos3(&source, 5, 5, transform)?;

        assert_eq!(result.statistics().outside_footprint_samples(), 0);
        for reference_y in 0..5 {
            for reference_x in 0..5 {
                let source_x = reference_y;
                let source_y = 4 - reference_x;
                assert_eq!(
                    result.image().get(reference_x, reference_y, 0)?.to_bits(),
                    source.get(source_x, source_y, 0)?.to_bits()
                );
            }
        }
        Ok(())
    }

    #[test]
    fn validates_output_dimensions_and_preserves_algorithm_identity() -> TestResult {
        let source = image(4, 4, 1)?;
        assert!(matches!(
            resample_lanczos3(&source, 0, 4, AffineTransform::IDENTITY),
            Err(ResamplingError::Core(CoreError::ZeroDimension { .. }))
        ));
        let result = resample_lanczos3(&source, 4, 4, AffineTransform::IDENTITY)?;
        assert_eq!(result.algorithm_id(), LANCZOS3_RESAMPLING_ALGORITHM_ID);
        assert_eq!(result.source_to_reference(), AffineTransform::IDENTITY);
        assert_eq!(result.statistics().total_samples(), 16);
        Ok(())
    }

    #[test]
    fn bounded_bands_are_bit_exact_against_the_complete_image_oracle() -> TestResult {
        let mut source = image(17, 15, 3)?;
        source.mark(8, 7, 0, PixelFlags::HOT)?;
        source.mark(9, 8, 2, PixelFlags::SATURATED)?;
        let nonfinite_index = 2 * 17 * 15 + 6 * 17 + 5;
        source.pixels_mut()[nonfinite_index] = f64::NAN;
        let transform = AffineTransform::new(0.999, -0.018, 0.021, 1.002, 0.37, -0.41)?;
        let oracle = resample_lanczos3(&source, 14, 13, transform)?;
        let output_dimensions = Dimensions::new(14, 13, 3)?;
        for band_height in [1, 4, 13, 32] {
            let mut assembled = ScientificImage::filled(output_dimensions, f64::NAN)?;
            let mut executor = Lanczos3BandExecutor::new(&source, 14, 13, band_height, transform)?;
            let mut expected_y = 0;

            while let Some(band) = executor.next_band()? {
                assert_eq!(band.reference_y(), expected_y);
                let band_dimensions = band.image().dimensions();
                assert_eq!(band_dimensions.width(), 14);
                assert!(band_dimensions.height() <= band_height);
                expected_y += band_dimensions.height();
                let band_area = band_dimensions.width() * band_dimensions.height();
                let output_area = output_dimensions.width() * output_dimensions.height();
                for plane in 0..output_dimensions.planes() {
                    for local_y in 0..band_dimensions.height() {
                        let source_start = plane * band_area + local_y * band_dimensions.width();
                        let target_start = plane * output_area
                            + (band.reference_y() + local_y) * output_dimensions.width();
                        let source_range = source_start..source_start + band_dimensions.width();
                        let target_range = target_start..target_start + output_dimensions.width();
                        assembled.pixels_mut()[target_range.clone()]
                            .copy_from_slice(&band.image().pixels()[source_range.clone()]);
                        assembled.mask_mut().as_mut_slice()[target_range]
                            .copy_from_slice(&band.image().mask().as_slice()[source_range]);
                    }
                }
            }

            assert_eq!(expected_y, output_dimensions.height());
            assert!(executor.is_complete());
            assert_eq!(executor.next_band()?, None);
            assert_eq!(executor.next_band()?, None);
            assert_eq!(executor.statistics(), oracle.statistics());
            assert_eq!(executor.algorithm_id(), oracle.algorithm_id());
            assert_eq!(executor.source_to_reference(), transform);
            assert_eq!(executor.output_dimensions(), output_dimensions);
            assert_eq!(executor.band_height(), band_height);
            assert_eq!(assembled.mask(), oracle.image().mask());
            for (&actual, &expected) in assembled.pixels().iter().zip(oracle.image().pixels()) {
                assert_eq!(actual.to_bits(), expected.to_bits());
            }
        }
        Ok(())
    }

    #[test]
    fn projective_bands_are_bit_exact_against_the_complete_image_oracle() -> TestResult {
        let mut source = image(19, 17, 2)?;
        source.mark(8, 7, 0, PixelFlags::HOT)?;
        source.pixels_mut()[19 * 17 + 9 * 19 + 10] = f64::NAN;
        let transform = ProjectiveTransform::new([
            [0.999, -0.014, 0.31],
            [0.017, 1.001, -0.27],
            [7.0e-5, -4.0e-5, 1.0],
        ])?;
        let oracle = resample_lanczos3_projective(&source, 16, 15, transform)?;
        let output_dimensions = Dimensions::new(16, 15, 2)?;

        for band_height in [1, 4, 15, 32] {
            let mut assembled = ScientificImage::filled(output_dimensions, f64::NAN)?;
            let mut executor =
                ProjectiveLanczos3BandExecutor::new(&source, 16, 15, band_height, transform)?;
            while let Some(band) = executor.next_band()? {
                copy_band_into(&mut assembled, &band)?;
            }

            assert!(executor.is_complete());
            assert_eq!(executor.next_band()?, None);
            assert_eq!(executor.statistics(), oracle.statistics());
            assert_eq!(executor.algorithm_id(), oracle.algorithm_id());
            assert_eq!(executor.source_to_reference(), transform);
            assert_eq!(executor.output_dimensions(), output_dimensions);
            assert_eq!(executor.band_height(), band_height);
            assert_eq!(assembled.mask(), oracle.image().mask());
            for (&actual, &expected) in assembled.pixels().iter().zip(oracle.image().pixels()) {
                assert_eq!(actual.to_bits(), expected.to_bits());
            }
        }
        Ok(())
    }

    #[test]
    fn band_executor_rejects_zero_height_and_invalid_output_dimensions() -> TestResult {
        let source = image(4, 4, 1)?;

        assert!(matches!(
            Lanczos3BandExecutor::new(&source, 4, 4, 0, AffineTransform::IDENTITY),
            Err(ResamplingError::ZeroBandHeight)
        ));
        assert!(matches!(
            Lanczos3BandExecutor::new(&source, 0, 4, 1, AffineTransform::IDENTITY),
            Err(ResamplingError::Core(CoreError::ZeroDimension { .. }))
        ));
        Ok(())
    }

    #[test]
    fn source_window_uses_exact_nonzero_taps_in_global_coordinates() -> TestResult {
        let identity = plan_lanczos3_source_window(20, 20, 5, 8, 3, 2, AffineTransform::IDENTITY)?
            .ok_or("identity band unexpectedly outside source")?;
        assert_eq!(
            identity,
            Lanczos3SourceWindow {
                x: 0,
                y: 3,
                width: 5,
                height: 2,
            }
        );

        let fractional = plan_lanczos3_source_window(
            20,
            20,
            5,
            8,
            2,
            2,
            AffineTransform::new(1.0, 0.0, 0.0, 1.0, -5.5, -4.25)?,
        )?
        .ok_or("fractional band unexpectedly outside source")?;
        assert_eq!(
            fractional,
            Lanczos3SourceWindow {
                x: 3,
                y: 4,
                width: 10,
                height: 7,
            }
        );
        Ok(())
    }

    #[test]
    fn source_window_reports_disjoint_and_invalid_bands_explicitly() -> TestResult {
        let disjoint = plan_lanczos3_source_window(
            5,
            5,
            5,
            5,
            0,
            5,
            AffineTransform::new(1.0, 0.0, 0.0, 1.0, 100.0, 0.0)?,
        )?;
        assert_eq!(disjoint, None);
        assert!(matches!(
            plan_lanczos3_source_window(5, 5, 5, 5, 4, 2, AffineTransform::IDENTITY),
            Err(ResamplingError::ReferenceBandOutOfBounds { .. })
        ));
        assert!(matches!(
            plan_lanczos3_source_window(5, 5, 5, 5, 0, 0, AffineTransform::IDENTITY),
            Err(ResamplingError::ZeroBandHeight)
        ));
        Ok(())
    }

    #[test]
    fn exact_source_windows_reconstruct_the_complete_image_oracle() -> TestResult {
        let mut source = image(17, 15, 3)?;
        source.mark(8, 7, 0, PixelFlags::HOT)?;
        source.mark(9, 8, 2, PixelFlags::SATURATED)?;
        source.pixels_mut()[2 * 17 * 15 + 6 * 17 + 5] = f64::NAN;
        let transform = AffineTransform::new(0.999, -0.018, 0.021, 1.002, 0.37, -0.41)?;
        let oracle = resample_lanczos3(&source, 14, 13, transform)?;
        let output_dimensions = Dimensions::new(14, 13, 3)?;
        let mut assembled = ScientificImage::filled(output_dimensions, f64::NAN)?;
        let mut statistics = ResamplingStatistics::default();

        for reference_y in (0..13).step_by(4) {
            let height = (13 - reference_y).min(4);
            let plan =
                Lanczos3BandPlan::new(source.dimensions(), 14, 13, reference_y, height, transform)?;
            assert_eq!(plan.source_dimensions(), source.dimensions());
            assert_eq!(plan.output_width(), 14);
            assert_eq!(plan.output_height(), 13);
            assert_eq!(plan.reference_y(), reference_y);
            assert_eq!(plan.band_height(), height);
            assert_eq!(plan.source_to_reference(), transform);
            let window_image = plan
                .source_window()
                .map(|window| extract_window(&source, window))
                .transpose()?;
            let band = plan.resample(window_image.as_ref())?;
            statistics = statistics.checked_add(band.statistics())?;
            let band_dimensions = band.image().dimensions();
            let band_area = band_dimensions.width() * band_dimensions.height();
            let output_area = output_dimensions.width() * output_dimensions.height();
            for plane in 0..output_dimensions.planes() {
                for local_y in 0..band_dimensions.height() {
                    let source_start = plane * band_area + local_y * band_dimensions.width();
                    let target_start =
                        plane * output_area + (reference_y + local_y) * output_dimensions.width();
                    let source_range = source_start..source_start + band_dimensions.width();
                    let target_range = target_start..target_start + output_dimensions.width();
                    assembled.pixels_mut()[target_range.clone()]
                        .copy_from_slice(&band.image().pixels()[source_range.clone()]);
                    assembled.mask_mut().as_mut_slice()[target_range]
                        .copy_from_slice(&band.image().mask().as_slice()[source_range]);
                }
            }
        }

        assert_eq!(statistics, oracle.statistics());
        assert_eq!(assembled.mask(), oracle.image().mask());
        for (&actual, &expected) in assembled.pixels().iter().zip(oracle.image().pixels()) {
            assert_eq!(actual.to_bits(), expected.to_bits());
        }
        Ok(())
    }

    #[test]
    fn exact_projective_windows_reconstruct_the_complete_image_oracle() -> TestResult {
        let mut source = image(19, 17, 2)?;
        source.mark(8, 7, 0, PixelFlags::HOT)?;
        source.mark(10, 9, 1, PixelFlags::SATURATED)?;
        source.pixels_mut()[19 * 17 + 6 * 19 + 7] = f64::NAN;
        let transform = ProjectiveTransform::new([
            [0.999, -0.014, 0.31],
            [0.017, 1.001, -0.27],
            [7.0e-5, -4.0e-5, 1.0],
        ])?;
        let oracle = resample_lanczos3_projective(&source, 16, 15, transform)?;
        let output_dimensions = Dimensions::new(16, 15, 2)?;
        let mut assembled = ScientificImage::filled(output_dimensions, f64::NAN)?;
        let mut statistics = ResamplingStatistics::default();

        for reference_y in (0..15).step_by(4) {
            let height = (15 - reference_y).min(4);
            let plan = ProjectiveLanczos3BandPlan::new(
                source.dimensions(),
                16,
                15,
                reference_y,
                height,
                transform,
            )?;
            assert_eq!(plan.source_dimensions(), source.dimensions());
            assert_eq!(plan.output_width(), 16);
            assert_eq!(plan.output_height(), 15);
            assert_eq!(plan.reference_y(), reference_y);
            assert_eq!(plan.band_height(), height);
            assert_eq!(plan.source_to_reference(), transform);
            let window_image = plan
                .source_window()
                .map(|window| extract_window(&source, window))
                .transpose()?;
            let band = plan.resample(window_image.as_ref())?;
            statistics = statistics.checked_add(band.statistics())?;
            copy_band_into(&mut assembled, &band)?;
        }

        assert_eq!(statistics, oracle.statistics());
        assert_eq!(assembled.mask(), oracle.image().mask());
        for (&actual, &expected) in assembled.pixels().iter().zip(oracle.image().pixels()) {
            assert_eq!(actual.to_bits(), expected.to_bits());
        }
        Ok(())
    }

    #[test]
    fn band_plan_rejects_missing_unexpected_and_misshaped_windows() -> TestResult {
        let dimensions = Dimensions::new(8, 8, 1)?;
        let intersecting =
            Lanczos3BandPlan::new(dimensions, 8, 8, 0, 2, AffineTransform::IDENTITY)?;
        assert!(matches!(
            intersecting.resample(None),
            Err(ResamplingError::MissingSourceWindow)
        ));
        let intersecting =
            Lanczos3BandPlan::new(dimensions, 8, 8, 0, 2, AffineTransform::IDENTITY)?;
        let wrong = image(8, 3, 1)?;
        assert!(matches!(
            intersecting.resample(Some(&wrong)),
            Err(ResamplingError::SourceWindowDimensions { .. })
        ));

        let disjoint = Lanczos3BandPlan::new(
            dimensions,
            8,
            8,
            0,
            2,
            AffineTransform::new(1.0, 0.0, 0.0, 1.0, 100.0, 0.0)?,
        )?;
        assert_eq!(disjoint.source_window(), None);
        let unnecessary = image(1, 1, 1)?;
        assert!(matches!(
            disjoint.resample(Some(&unnecessary)),
            Err(ResamplingError::UnexpectedSourceWindow)
        ));
        let disjoint = Lanczos3BandPlan::new(
            dimensions,
            8,
            8,
            0,
            2,
            AffineTransform::new(1.0, 0.0, 0.0, 1.0, 100.0, 0.0)?,
        )?;
        let missing = disjoint.resample(None)?;
        assert_eq!(missing.statistics().outside_footprint_samples(), 16);
        assert!(
            missing
                .image()
                .mask()
                .as_slice()
                .iter()
                .all(|flags| *flags == PixelFlags::MISSING)
        );
        Ok(())
    }
}
