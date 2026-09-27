use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_core::{CoreError, Dimensions};

use crate::{AffineTransform, CoordinateError, ImagePoint};

/// Stable identifier for exact discrete common-support scanning.
pub const COMMON_LANCZOS3_FOOTPRINT_ALGORITHM_ID: &str = "common-lanczos3-footprint-v1";
/// Maximum registered frames accepted by one common-footprint request.
pub const MAX_COMMON_FOOTPRINT_FRAMES: usize = 100_000;
/// Maximum output-pixel/frame support decisions in one request.
pub const MAX_COMMON_FOOTPRINT_EVALUATIONS: usize = 2_000_000_000;

const LANCZOS_RADIUS: f64 = 3.0;

/// Source extent and source-to-reference transform used for coverage only.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RegistrationFootprint {
    source_width: usize,
    source_height: usize,
    source_to_reference: AffineTransform,
}

impl RegistrationFootprint {
    /// Creates one validated geometric footprint.
    pub fn new(
        source_width: usize,
        source_height: usize,
        source_to_reference: AffineTransform,
    ) -> Result<Self, CommonFootprintError> {
        Dimensions::new(source_width, source_height, 1).map_err(CommonFootprintError::Core)?;
        validate_exact_coordinate_extent(source_width)?;
        validate_exact_coordinate_extent(source_height)?;
        Ok(Self {
            source_width,
            source_height,
            source_to_reference,
        })
    }

    /// Source width in pixels.
    #[must_use]
    pub const fn source_width(self) -> usize {
        self.source_width
    }

    /// Source height in pixels.
    #[must_use]
    pub const fn source_height(self) -> usize {
        self.source_height
    }

    /// Transform mapping this source into the common reference coordinates.
    #[must_use]
    pub const fn source_to_reference(self) -> AffineTransform {
        self.source_to_reference
    }
}

/// Inclusive-origin, exclusive-extent crop in reference pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReferenceRectangle {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

impl ReferenceRectangle {
    /// Leftmost included reference column.
    #[must_use]
    pub const fn x(self) -> usize {
        self.x
    }

    /// Topmost included reference row.
    #[must_use]
    pub const fn y(self) -> usize {
        self.y
    }

    /// Number of included columns.
    #[must_use]
    pub const fn width(self) -> usize {
        self.width
    }

    /// Number of included rows.
    #[must_use]
    pub const fn height(self) -> usize {
        self.height
    }

    /// Checked rectangle area established during construction.
    #[must_use]
    pub const fn area(self) -> usize {
        self.width * self.height
    }
}

/// Exact coverage evidence and the largest all-frame rectangular crop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CommonFootprintReport {
    reference_width: usize,
    reference_height: usize,
    frame_count: usize,
    covered_pixels: usize,
    crop: Option<ReferenceRectangle>,
}

impl CommonFootprintReport {
    /// Versioned coverage and crop policy.
    #[must_use]
    pub const fn algorithm_id(&self) -> &'static str {
        COMMON_LANCZOS3_FOOTPRINT_ALGORITHM_ID
    }

    /// Reference canvas width inspected.
    #[must_use]
    pub const fn reference_width(self) -> usize {
        self.reference_width
    }

    /// Reference canvas height inspected.
    #[must_use]
    pub const fn reference_height(self) -> usize {
        self.reference_height
    }

    /// Number of transformed source extents intersected.
    #[must_use]
    pub const fn frame_count(self) -> usize {
        self.frame_count
    }

    /// Discrete reference pixels having complete Lanczos support in every frame.
    #[must_use]
    pub const fn covered_pixels(self) -> usize {
        self.covered_pixels
    }

    /// Largest axis-aligned rectangle entirely inside the discrete common support.
    #[must_use]
    pub const fn crop(self) -> Option<ReferenceRectangle> {
        self.crop
    }
}

/// Failure to derive bounded exact common coverage.
#[derive(Clone, Debug, PartialEq)]
pub enum CommonFootprintError {
    /// At least one transformed frame is required.
    NoFrames,
    /// Frame count exceeds the documented bound.
    TooManyFrames {
        /// Maximum accepted count.
        maximum: usize,
        /// Requested count.
        actual: usize,
    },
    /// Exact output-pixel/frame evaluation bound would be exceeded.
    WorkBoundExceeded {
        /// Maximum accepted decisions.
        maximum: usize,
        /// Requested decisions.
        requested: usize,
    },
    /// An image extent violates the shared dimension contract.
    Core(CoreError),
    /// Integer pixel centers cannot be represented exactly in binary64.
    CoordinatePrecisionExceeded,
    /// A transform could not be inverted or evaluated.
    Coordinate(CoordinateError),
    /// Scratch storage could not be reserved.
    AllocationFailed,
    /// Checked support accounting overflowed.
    CountOverflow,
}

impl Display for CommonFootprintError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoFrames => formatter.write_str("common footprint requires at least one frame"),
            Self::TooManyFrames { maximum, actual } => write!(
                formatter,
                "common footprint accepts at most {maximum} frames, received {actual}"
            ),
            Self::WorkBoundExceeded { maximum, requested } => write!(
                formatter,
                "common footprint requires {requested} evaluations, limit is {maximum}"
            ),
            Self::Core(error) => write!(formatter, "invalid common-footprint dimensions: {error}"),
            Self::CoordinatePrecisionExceeded => formatter.write_str(
                "common-footprint pixel centers exceed exact binary64 integer coordinates",
            ),
            Self::Coordinate(error) => {
                write!(
                    formatter,
                    "cannot evaluate common-footprint transform: {error}"
                )
            }
            Self::AllocationFailed => {
                formatter.write_str("common-footprint scratch allocation failed")
            }
            Self::CountOverflow => formatter.write_str("common-footprint count overflow"),
        }
    }
}

impl Error for CommonFootprintError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Core(error) => Some(error),
            Self::Coordinate(error) => Some(error),
            Self::NoFrames
            | Self::TooManyFrames { .. }
            | Self::WorkBoundExceeded { .. }
            | Self::CoordinatePrecisionExceeded
            | Self::AllocationFailed
            | Self::CountOverflow => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct HistogramEntry {
    start_x: usize,
    height: usize,
}

/// Finds exact all-frame Lanczos support and its largest rectangular subset.
///
/// Coverage depends only on source extents and transforms. Pixel masks are
/// deliberately absent: isolated sensor defects may invalidate interpolated
/// samples, but they must never collapse the geometric integration footprint.
/// The scan retains one row histogram and one monotonic stack, so scratch memory
/// is proportional to reference width rather than image area.
///
/// Crop candidates prefer larger area, then smaller `y`, smaller `x`, larger
/// width, and larger height. These tie breakers make output independent of stack
/// implementation details.
pub fn derive_common_lanczos3_footprint(
    reference_width: usize,
    reference_height: usize,
    frames: &[RegistrationFootprint],
) -> Result<CommonFootprintReport, CommonFootprintError> {
    let reference_dimensions = Dimensions::new(reference_width, reference_height, 1)
        .map_err(CommonFootprintError::Core)?;
    validate_exact_coordinate_extent(reference_width)?;
    validate_exact_coordinate_extent(reference_height)?;
    if frames.is_empty() {
        return Err(CommonFootprintError::NoFrames);
    }
    if frames.len() > MAX_COMMON_FOOTPRINT_FRAMES {
        return Err(CommonFootprintError::TooManyFrames {
            maximum: MAX_COMMON_FOOTPRINT_FRAMES,
            actual: frames.len(),
        });
    }
    let requested = reference_dimensions
        .pixel_count()
        .checked_mul(frames.len())
        .ok_or(CommonFootprintError::CountOverflow)?;
    if requested > MAX_COMMON_FOOTPRINT_EVALUATIONS {
        return Err(CommonFootprintError::WorkBoundExceeded {
            maximum: MAX_COMMON_FOOTPRINT_EVALUATIONS,
            requested,
        });
    }

    let mut inverse_frames = Vec::new();
    inverse_frames
        .try_reserve_exact(frames.len())
        .map_err(|_| CommonFootprintError::AllocationFailed)?;
    for frame in frames {
        inverse_frames.push((
            frame
                .source_to_reference
                .inverse()
                .map_err(CommonFootprintError::Coordinate)?,
            frame.source_width,
            frame.source_height,
        ));
    }
    let mut heights = Vec::new();
    heights
        .try_reserve_exact(reference_width)
        .map_err(|_| CommonFootprintError::AllocationFailed)?;
    heights.resize(reference_width, 0_usize);
    let mut stack = Vec::new();
    stack
        .try_reserve_exact(reference_width)
        .map_err(|_| CommonFootprintError::AllocationFailed)?;
    let mut covered_pixels = 0_usize;
    let mut crop = None;

    for reference_y in 0..reference_height {
        for (reference_x, height) in heights.iter_mut().enumerate() {
            let point = ImagePoint::new(reference_x as f64, reference_y as f64)
                .map_err(CommonFootprintError::Coordinate)?;
            let mut covered = true;
            for (inverse, source_width, source_height) in &inverse_frames {
                let source = inverse
                    .apply(point)
                    .map_err(CommonFootprintError::Coordinate)?;
                if !axis_has_complete_support(source.x(), *source_width)
                    || !axis_has_complete_support(source.y(), *source_height)
                {
                    covered = false;
                    break;
                }
            }
            if covered {
                *height = height
                    .checked_add(1)
                    .ok_or(CommonFootprintError::CountOverflow)?;
                covered_pixels = covered_pixels
                    .checked_add(1)
                    .ok_or(CommonFootprintError::CountOverflow)?;
            } else {
                *height = 0;
            }
        }
        update_largest_rectangle(reference_y, &heights, &mut stack, &mut crop)?;
    }

    Ok(CommonFootprintReport {
        reference_width,
        reference_height,
        frame_count: frames.len(),
        covered_pixels,
        crop,
    })
}

fn update_largest_rectangle(
    bottom_y: usize,
    heights: &[usize],
    stack: &mut Vec<HistogramEntry>,
    best: &mut Option<ReferenceRectangle>,
) -> Result<(), CommonFootprintError> {
    stack.clear();
    for x in 0..=heights.len() {
        let current_height = heights.get(x).copied().unwrap_or(0);
        let mut start_x = x;
        while stack
            .last()
            .is_some_and(|entry| entry.height > current_height)
        {
            let Some(entry) = stack.pop() else {
                return Err(CommonFootprintError::CountOverflow);
            };
            start_x = entry.start_x;
            let width = x
                .checked_sub(entry.start_x)
                .ok_or(CommonFootprintError::CountOverflow)?;
            let y = bottom_y
                .checked_add(1)
                .and_then(|value| value.checked_sub(entry.height))
                .ok_or(CommonFootprintError::CountOverflow)?;
            let candidate = ReferenceRectangle {
                x: entry.start_x,
                y,
                width,
                height: entry.height,
            };
            if rectangle_is_better(candidate, *best)? {
                *best = Some(candidate);
            }
        }
        if current_height > 0
            && stack
                .last()
                .is_none_or(|entry| entry.height < current_height)
        {
            stack.push(HistogramEntry {
                start_x,
                height: current_height,
            });
        }
    }
    Ok(())
}

fn rectangle_is_better(
    candidate: ReferenceRectangle,
    current: Option<ReferenceRectangle>,
) -> Result<bool, CommonFootprintError> {
    let candidate_area = candidate
        .width
        .checked_mul(candidate.height)
        .ok_or(CommonFootprintError::CountOverflow)?;
    let Some(current) = current else {
        return Ok(candidate_area > 0);
    };
    let current_area = current
        .width
        .checked_mul(current.height)
        .ok_or(CommonFootprintError::CountOverflow)?;
    Ok(candidate_area > current_area
        || (candidate_area == current_area
            && (
                candidate.y,
                candidate.x,
                usize::MAX - candidate.width,
                usize::MAX - candidate.height,
            ) < (
                current.y,
                current.x,
                usize::MAX - current.width,
                usize::MAX - current.height,
            )))
}

#[allow(clippy::float_cmp)]
fn axis_has_complete_support(coordinate: f64, source_length: usize) -> bool {
    if !coordinate.is_finite() {
        return false;
    }
    let maximum = source_length.saturating_sub(1) as f64;
    if coordinate == coordinate.round() {
        return coordinate >= 0.0 && coordinate <= maximum;
    }
    let floor = coordinate.floor();
    floor - (LANCZOS_RADIUS - 1.0) >= 0.0 && floor + LANCZOS_RADIUS <= maximum
}

fn validate_exact_coordinate_extent(length: usize) -> Result<(), CommonFootprintError> {
    let maximum = length.saturating_sub(1);
    if maximum as f64 > 2_f64.powi(53) {
        return Err(CommonFootprintError::CoordinatePrecisionExceeded);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn StdError>>;

    fn frame(
        width: usize,
        height: usize,
        transform: AffineTransform,
    ) -> TestResult<RegistrationFootprint> {
        Ok(RegistrationFootprint::new(width, height, transform)?)
    }

    #[test]
    fn identical_frames_retain_the_complete_reference_canvas() -> TestResult {
        let frames = [
            frame(8, 6, AffineTransform::IDENTITY)?,
            frame(8, 6, AffineTransform::IDENTITY)?,
        ];

        let report = derive_common_lanczos3_footprint(8, 6, &frames)?;

        assert_eq!(
            report.algorithm_id(),
            COMMON_LANCZOS3_FOOTPRINT_ALGORITHM_ID
        );
        assert_eq!(report.covered_pixels(), 48);
        assert_eq!(report.frame_count(), 2);
        assert_eq!(
            report.crop(),
            Some(ReferenceRectangle {
                x: 0,
                y: 0,
                width: 8,
                height: 6,
            })
        );
        Ok(())
    }

    #[test]
    fn integer_translation_returns_the_exact_overlap() -> TestResult {
        let frames = [
            frame(5, 4, AffineTransform::IDENTITY)?,
            frame(5, 4, AffineTransform::new(1.0, 0.0, 0.0, 1.0, 1.0, 0.0)?)?,
        ];

        let report = derive_common_lanczos3_footprint(5, 4, &frames)?;

        assert_eq!(report.covered_pixels(), 16);
        let crop = report.crop().ok_or("missing translated overlap")?;
        assert_eq!(
            (crop.x(), crop.y(), crop.width(), crop.height()),
            (1, 0, 4, 4)
        );
        assert_eq!(crop.area(), 16);
        Ok(())
    }

    #[test]
    fn fractional_translation_accounts_for_every_nonzero_lanczos_tap() -> TestResult {
        let frames = [frame(
            10,
            10,
            AffineTransform::new(1.0, 0.0, 0.0, 1.0, 0.5, 0.0)?,
        )?];

        let report = derive_common_lanczos3_footprint(10, 10, &frames)?;

        assert_eq!(report.covered_pixels(), 50);
        let crop = report.crop().ok_or("missing fractional overlap")?;
        assert_eq!(
            (crop.x(), crop.y(), crop.width(), crop.height()),
            (3, 0, 5, 10)
        );
        Ok(())
    }

    #[test]
    fn disjoint_frames_report_absence_instead_of_inventing_a_crop() -> TestResult {
        let frames = [frame(
            5,
            5,
            AffineTransform::new(1.0, 0.0, 0.0, 1.0, 10.0, 0.0)?,
        )?];

        let report = derive_common_lanczos3_footprint(5, 5, &frames)?;

        assert_eq!(report.covered_pixels(), 0);
        assert_eq!(report.crop(), None);
        Ok(())
    }

    #[test]
    fn rectangle_ties_prefer_the_documented_top_left_crop() -> TestResult {
        let mut stack = Vec::new();
        let mut best = None;

        update_largest_rectangle(0, &[1, 1, 0, 1, 1], &mut stack, &mut best)?;
        update_largest_rectangle(1, &[2, 2, 0, 2, 2], &mut stack, &mut best)?;

        assert_eq!(
            best,
            Some(ReferenceRectangle {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            })
        );
        Ok(())
    }

    #[test]
    fn rejects_empty_requests_and_excessive_work() -> TestResult {
        assert_eq!(
            derive_common_lanczos3_footprint(5, 5, &[]),
            Err(CommonFootprintError::NoFrames)
        );
        let frames = [frame(1, 1, AffineTransform::IDENTITY)?];
        assert!(matches!(
            derive_common_lanczos3_footprint(MAX_COMMON_FOOTPRINT_EVALUATIONS, 2, &frames,),
            Err(CommonFootprintError::WorkBoundExceeded { .. })
                | Err(CommonFootprintError::Core(_))
        ));
        Ok(())
    }
}
