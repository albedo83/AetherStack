use std::collections::HashSet;
use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_review::FrameId;

use crate::{FeatureCatalog, ImagePoint, RegistrationFeature};

/// Versioned local triangle descriptor policy.
pub const TRIANGLE_DESCRIPTOR_ALGORITHM_ID: &str = "local-triangle-ratios-v1";

/// Maximum number of high-ranked features used as triangle anchors.
pub const MAX_DESCRIPTOR_ANCHORS: usize = 4_096;

/// Maximum nearest neighbors considered around each anchor.
pub const MAX_DESCRIPTOR_NEIGHBORS: usize = 32;

/// Maximum unique triangle descriptors emitted for one frame.
pub const MAX_TRIANGLE_DESCRIPTORS: usize = 1_000_000;

const MAX_TRIANGLE_ATTEMPTS: usize = 2_100_000;

/// Validated controls for bounded local triangle construction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TriangleDescriptorParameters {
    maximum_anchors: usize,
    neighbors_per_anchor: usize,
    minimum_long_side_pixels: f64,
    minimum_normalized_area: f64,
    maximum_descriptors: usize,
}

impl TriangleDescriptorParameters {
    /// Creates explicit local-neighborhood and degeneracy limits.
    pub fn new(
        maximum_anchors: usize,
        neighbors_per_anchor: usize,
        minimum_long_side_pixels: f64,
        minimum_normalized_area: f64,
        maximum_descriptors: usize,
    ) -> Result<Self, TriangleDescriptorError> {
        if maximum_anchors == 0 || maximum_anchors > MAX_DESCRIPTOR_ANCHORS {
            return Err(TriangleDescriptorError::InvalidMaximumAnchors {
                maximum: MAX_DESCRIPTOR_ANCHORS,
                actual: maximum_anchors,
            });
        }
        if !(2..=MAX_DESCRIPTOR_NEIGHBORS).contains(&neighbors_per_anchor) {
            return Err(TriangleDescriptorError::InvalidNeighborCount {
                minimum: 2,
                maximum: MAX_DESCRIPTOR_NEIGHBORS,
                actual: neighbors_per_anchor,
            });
        }
        if !minimum_long_side_pixels.is_finite() || minimum_long_side_pixels <= 0.0 {
            return Err(TriangleDescriptorError::InvalidMinimumLongSide);
        }
        if !minimum_normalized_area.is_finite()
            || minimum_normalized_area <= 0.0
            || minimum_normalized_area >= 1.0
        {
            return Err(TriangleDescriptorError::InvalidMinimumNormalizedArea);
        }
        if maximum_descriptors == 0 || maximum_descriptors > MAX_TRIANGLE_DESCRIPTORS {
            return Err(TriangleDescriptorError::InvalidMaximumDescriptors {
                maximum: MAX_TRIANGLE_DESCRIPTORS,
                actual: maximum_descriptors,
            });
        }
        let pairs = neighbors_per_anchor
            .checked_mul(neighbors_per_anchor - 1)
            .and_then(|value| value.checked_div(2))
            .ok_or(TriangleDescriptorError::WorkBoundOverflow)?;
        let attempts = maximum_anchors
            .checked_mul(pairs)
            .ok_or(TriangleDescriptorError::WorkBoundOverflow)?;
        if attempts > MAX_TRIANGLE_ATTEMPTS {
            return Err(TriangleDescriptorError::WorkBoundExceeded {
                maximum: MAX_TRIANGLE_ATTEMPTS,
                requested: attempts,
            });
        }
        Ok(Self {
            maximum_anchors,
            neighbors_per_anchor,
            minimum_long_side_pixels,
            minimum_normalized_area,
            maximum_descriptors,
        })
    }

    /// Maximum number of leading catalog features used as anchors.
    #[must_use]
    pub const fn maximum_anchors(self) -> usize {
        self.maximum_anchors
    }

    /// Nearest neighbors considered around each anchor.
    #[must_use]
    pub const fn neighbors_per_anchor(self) -> usize {
        self.neighbors_per_anchor
    }

    /// Inclusive minimum longest triangle side in pixels.
    #[must_use]
    pub const fn minimum_long_side_pixels(self) -> f64 {
        self.minimum_long_side_pixels
    }

    /// Inclusive minimum `abs(cross) / longest_side²`.
    #[must_use]
    pub const fn minimum_normalized_area(self) -> f64 {
        self.minimum_normalized_area
    }

    /// Maximum unique descriptors returned.
    #[must_use]
    pub const fn maximum_descriptors(self) -> usize {
        self.maximum_descriptors
    }
}

/// Orientation in image coordinates, where positive `y` points downward.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TriangleOrientation {
    /// Positive canonical cross product in the downward-positive image frame.
    Clockwise,
    /// Negative canonical cross product in the downward-positive image frame.
    CounterClockwise,
}

/// Scale- and rotation-invariant geometry for three ranked features.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TriangleDescriptor {
    identity_ranks: [usize; 3],
    canonical_ranks: [usize; 3],
    short_to_long: f64,
    middle_to_long: f64,
    normalized_area: f64,
    long_side_pixels: f64,
    orientation: TriangleOrientation,
}

impl TriangleDescriptor {
    /// Ascending feature ranks used for duplicate identity.
    #[must_use]
    pub const fn identity_ranks(self) -> [usize; 3] {
        self.identity_ranks
    }

    /// `[apex, short-side endpoint, middle-side endpoint]` feature ranks.
    #[must_use]
    pub const fn canonical_ranks(self) -> [usize; 3] {
        self.canonical_ranks
    }

    /// Shortest side divided by the longest side.
    #[must_use]
    pub const fn short_to_long(self) -> f64 {
        self.short_to_long
    }

    /// Middle side divided by the longest side.
    #[must_use]
    pub const fn middle_to_long(self) -> f64 {
        self.middle_to_long
    }

    /// Absolute canonical cross product divided by longest-side squared.
    #[must_use]
    pub const fn normalized_area(self) -> f64 {
        self.normalized_area
    }

    /// Absolute longest side retained for scale estimation after matching.
    #[must_use]
    pub const fn long_side_pixels(self) -> f64 {
        self.long_side_pixels
    }

    /// Canonical orientation; reflection reverses this value.
    #[must_use]
    pub const fn orientation(self) -> TriangleOrientation {
        self.orientation
    }
}

/// Deterministic evidence for descriptor generation and early termination.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TriangleDescriptorStatistics {
    anchors_visited: usize,
    attempted_triangles: usize,
    rejected_short_side: usize,
    rejected_degenerate: usize,
    duplicate_triangles: usize,
    stopped_at_output_limit: bool,
}

impl TriangleDescriptorStatistics {
    /// Anchors whose neighborhoods were visited before completion or truncation.
    #[must_use]
    pub const fn anchors_visited(self) -> usize {
        self.anchors_visited
    }

    /// Neighbor pairs inspected, including rejected and duplicate triangles.
    #[must_use]
    pub const fn attempted_triangles(self) -> usize {
        self.attempted_triangles
    }

    /// Triangles below the absolute longest-side threshold.
    #[must_use]
    pub const fn rejected_short_side(self) -> usize {
        self.rejected_short_side
    }

    /// Collinear or near-collinear triangles below normalized-area threshold.
    #[must_use]
    pub const fn rejected_degenerate(self) -> usize {
        self.rejected_degenerate
    }

    /// Valid triangles already emitted from another local anchor.
    #[must_use]
    pub const fn duplicate_triangles(self) -> usize {
        self.duplicate_triangles
    }

    /// Whether generation found another valid unique triangle after filling output.
    #[must_use]
    pub const fn stopped_at_output_limit(self) -> bool {
        self.stopped_at_output_limit
    }
}

/// Immutable triangle descriptors for one feature catalog.
#[derive(Clone, Debug, PartialEq)]
pub struct TriangleDescriptorCatalog {
    frame_id: FrameId,
    source_feature_count: usize,
    parameters: TriangleDescriptorParameters,
    statistics: TriangleDescriptorStatistics,
    descriptors: Vec<TriangleDescriptor>,
}

impl TriangleDescriptorCatalog {
    /// Stable frame identity inherited from the feature catalog.
    #[must_use]
    pub const fn frame_id(&self) -> &FrameId {
        &self.frame_id
    }

    /// Versioned descriptor algorithm.
    #[must_use]
    pub const fn algorithm_id(&self) -> &'static str {
        TRIANGLE_DESCRIPTOR_ALGORITHM_ID
    }

    /// Number of ranked features available at descriptor construction.
    #[must_use]
    pub const fn source_feature_count(&self) -> usize {
        self.source_feature_count
    }

    /// Exact neighborhood and degeneracy controls.
    #[must_use]
    pub const fn parameters(&self) -> TriangleDescriptorParameters {
        self.parameters
    }

    /// Complete generation evidence up to an explicit output-limit stop.
    #[must_use]
    pub const fn statistics(&self) -> TriangleDescriptorStatistics {
        self.statistics
    }

    /// Unique descriptors in deterministic generation order.
    #[must_use]
    pub fn descriptors(&self) -> &[TriangleDescriptor] {
        &self.descriptors
    }
}

#[derive(Clone, Copy, Debug)]
struct Neighbor {
    feature_index: usize,
    distance: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TriangleRejection {
    ShortSide,
    Degenerate,
}

/// Builds bounded local triangle descriptors from a ranked feature catalog.
pub fn build_triangle_descriptors(
    catalog: &FeatureCatalog,
    parameters: TriangleDescriptorParameters,
) -> Result<TriangleDescriptorCatalog, TriangleDescriptorError> {
    let parameters = TriangleDescriptorParameters::new(
        parameters.maximum_anchors,
        parameters.neighbors_per_anchor,
        parameters.minimum_long_side_pixels,
        parameters.minimum_normalized_area,
        parameters.maximum_descriptors,
    )?;
    let features = catalog.features();
    if features.len() < 3 {
        return Err(TriangleDescriptorError::InsufficientFeatures {
            available: features.len(),
            required: 3,
        });
    }
    validate_feature_ranks(features)?;

    let effective_anchors = parameters.maximum_anchors.min(features.len());
    let effective_neighbors = parameters
        .neighbors_per_anchor
        .min(features.len().saturating_sub(1));
    let pairs = effective_neighbors
        .checked_mul(effective_neighbors.saturating_sub(1))
        .and_then(|value| value.checked_div(2))
        .ok_or(TriangleDescriptorError::WorkBoundOverflow)?;
    let potential_attempts = effective_anchors
        .checked_mul(pairs)
        .ok_or(TriangleDescriptorError::WorkBoundOverflow)?;
    let capacity = parameters.maximum_descriptors.min(potential_attempts);

    let mut descriptors = Vec::new();
    descriptors
        .try_reserve_exact(capacity)
        .map_err(|_| TriangleDescriptorError::AllocationFailed)?;
    let mut identities = HashSet::new();
    identities
        .try_reserve(capacity)
        .map_err(|_| TriangleDescriptorError::AllocationFailed)?;
    let mut neighbors = Vec::new();
    neighbors
        .try_reserve_exact(features.len().saturating_sub(1))
        .map_err(|_| TriangleDescriptorError::AllocationFailed)?;
    let mut statistics = TriangleDescriptorStatistics::default();

    'anchors: for anchor_index in 0..effective_anchors {
        statistics.anchors_visited = checked_increment(statistics.anchors_visited)?;
        neighbors.clear();
        let anchor = features[anchor_index];
        for (feature_index, feature) in features.iter().copied().enumerate() {
            if feature_index == anchor_index {
                continue;
            }
            neighbors.push(Neighbor {
                feature_index,
                distance: point_distance(anchor.point(), feature.point())?,
            });
        }
        neighbors.sort_by(|left, right| {
            left.distance.total_cmp(&right.distance).then_with(|| {
                features[left.feature_index]
                    .rank()
                    .cmp(&features[right.feature_index].rank())
            })
        });
        let local = &neighbors[..effective_neighbors];
        for left in 0..local.len() - 1 {
            for right in left + 1..local.len() {
                statistics.attempted_triangles = checked_increment(statistics.attempted_triangles)?;
                let triangle = [
                    anchor,
                    features[local[left].feature_index],
                    features[local[right].feature_index],
                ];
                let descriptor = match describe_triangle(triangle, parameters) {
                    Ok(descriptor) => descriptor,
                    Err(TriangleRejection::ShortSide) => {
                        statistics.rejected_short_side =
                            checked_increment(statistics.rejected_short_side)?;
                        continue;
                    }
                    Err(TriangleRejection::Degenerate) => {
                        statistics.rejected_degenerate =
                            checked_increment(statistics.rejected_degenerate)?;
                        continue;
                    }
                };
                if identities.contains(&descriptor.identity_ranks) {
                    statistics.duplicate_triangles =
                        checked_increment(statistics.duplicate_triangles)?;
                    continue;
                }
                if descriptors.len() == parameters.maximum_descriptors {
                    statistics.stopped_at_output_limit = true;
                    break 'anchors;
                }
                identities.insert(descriptor.identity_ranks);
                descriptors.push(descriptor);
            }
        }
    }

    Ok(TriangleDescriptorCatalog {
        frame_id: catalog.frame_id().clone(),
        source_feature_count: features.len(),
        parameters,
        statistics,
        descriptors,
    })
}

fn validate_feature_ranks(features: &[RegistrationFeature]) -> Result<(), TriangleDescriptorError> {
    if features
        .iter()
        .enumerate()
        .any(|(expected, feature)| feature.rank() != expected)
    {
        return Err(TriangleDescriptorError::NonCanonicalFeatureRanks);
    }
    Ok(())
}

fn describe_triangle(
    features: [RegistrationFeature; 3],
    parameters: TriangleDescriptorParameters,
) -> Result<TriangleDescriptor, TriangleRejection> {
    describe_ranked_points(
        [features[0].rank(), features[1].rank(), features[2].rank()],
        [
            features[0].point(),
            features[1].point(),
            features[2].point(),
        ],
        parameters.minimum_long_side_pixels,
        parameters.minimum_normalized_area,
    )
}

fn describe_ranked_points(
    ranks: [usize; 3],
    points: [ImagePoint; 3],
    minimum_long_side: f64,
    minimum_normalized_area: f64,
) -> Result<TriangleDescriptor, TriangleRejection> {
    let mut edges = [
        (
            point_distance_unchecked(points[0], points[1]),
            0_usize,
            1_usize,
        ),
        (
            point_distance_unchecked(points[0], points[2]),
            0_usize,
            2_usize,
        ),
        (
            point_distance_unchecked(points[1], points[2]),
            1_usize,
            2_usize,
        ),
    ];
    edges.sort_by(|left, right| {
        left.0
            .total_cmp(&right.0)
            .then_with(|| edge_rank_key(ranks, *left).cmp(&edge_rank_key(ranks, *right)))
    });
    let long = edges[2].0;
    if !long.is_finite() || long < minimum_long_side {
        return Err(TriangleRejection::ShortSide);
    }
    let long_start = edges[2].1;
    let long_end = edges[2].2;
    let apex = 3_usize - long_start - long_end;
    let start_distance = point_distance_unchecked(points[apex], points[long_start]);
    let end_distance = point_distance_unchecked(points[apex], points[long_end]);
    let (short_endpoint, middle_endpoint) = match start_distance.total_cmp(&end_distance) {
        std::cmp::Ordering::Less => (long_start, long_end),
        std::cmp::Ordering::Greater => (long_end, long_start),
        std::cmp::Ordering::Equal if ranks[long_start] <= ranks[long_end] => (long_start, long_end),
        std::cmp::Ordering::Equal => (long_end, long_start),
    };
    let apex_point = points[apex];
    let short_point = points[short_endpoint];
    let middle_point = points[middle_endpoint];
    let first_x = short_point.x() - apex_point.x();
    let first_y = short_point.y() - apex_point.y();
    let second_x = middle_point.x() - apex_point.x();
    let second_y = middle_point.y() - apex_point.y();
    let cross = first_x.mul_add(second_y, -(first_y * second_x));
    let normalized_area = cross.abs() / (long * long);
    if !normalized_area.is_finite() || normalized_area < minimum_normalized_area {
        return Err(TriangleRejection::Degenerate);
    }
    let short_to_long = edges[0].0 / long;
    let middle_to_long = edges[1].0 / long;
    if !short_to_long.is_finite() || !middle_to_long.is_finite() {
        return Err(TriangleRejection::Degenerate);
    }
    let mut identity_ranks = ranks;
    identity_ranks.sort_unstable();
    Ok(TriangleDescriptor {
        identity_ranks,
        canonical_ranks: [ranks[apex], ranks[short_endpoint], ranks[middle_endpoint]],
        short_to_long,
        middle_to_long,
        normalized_area,
        long_side_pixels: long,
        orientation: if cross > 0.0 {
            TriangleOrientation::Clockwise
        } else {
            TriangleOrientation::CounterClockwise
        },
    })
}

fn edge_rank_key(ranks: [usize; 3], edge: (f64, usize, usize)) -> [usize; 2] {
    let mut key = [ranks[edge.1], ranks[edge.2]];
    key.sort_unstable();
    key
}

fn point_distance(left: ImagePoint, right: ImagePoint) -> Result<f64, TriangleDescriptorError> {
    let distance = point_distance_unchecked(left, right);
    if !distance.is_finite() {
        return Err(TriangleDescriptorError::NumericalOverflow);
    }
    Ok(distance)
}

fn point_distance_unchecked(left: ImagePoint, right: ImagePoint) -> f64 {
    (left.x() - right.x()).hypot(left.y() - right.y())
}

fn checked_increment(value: usize) -> Result<usize, TriangleDescriptorError> {
    value
        .checked_add(1)
        .ok_or(TriangleDescriptorError::CountOverflow)
}

/// Invalid descriptor controls, source catalog, allocation, or arithmetic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TriangleDescriptorError {
    /// Anchor count was zero or above the hard bound.
    InvalidMaximumAnchors {
        /// Hard maximum.
        maximum: usize,
        /// Requested anchors.
        actual: usize,
    },
    /// Neighbor count was outside the supported interval.
    InvalidNeighborCount {
        /// Required minimum.
        minimum: usize,
        /// Hard maximum.
        maximum: usize,
        /// Requested neighbors.
        actual: usize,
    },
    /// Absolute triangle scale threshold was non-finite or non-positive.
    InvalidMinimumLongSide,
    /// Normalized area threshold was non-finite or outside `(0, 1)`.
    InvalidMinimumNormalizedArea,
    /// Descriptor output bound was zero or above the hard maximum.
    InvalidMaximumDescriptors {
        /// Hard maximum.
        maximum: usize,
        /// Requested descriptors.
        actual: usize,
    },
    /// Parameter work arithmetic overflowed.
    WorkBoundOverflow,
    /// Requested worst-case local pair work exceeded the hard bound.
    WorkBoundExceeded {
        /// Maximum attempted local triangles.
        maximum: usize,
        /// Requested worst case.
        requested: usize,
    },
    /// At least three selected features are required.
    InsufficientFeatures {
        /// Available selected features.
        available: usize,
        /// Required features.
        required: usize,
    },
    /// Input features did not carry contiguous zero-based ranks.
    NonCanonicalFeatureRanks,
    /// Scratch or output allocation failed.
    AllocationFailed,
    /// Distance or triangle geometry left the finite numerical domain.
    NumericalOverflow,
    /// Evidence arithmetic overflowed.
    CountOverflow,
}

impl Display for TriangleDescriptorError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidMaximumAnchors { maximum, actual } => write!(
                formatter,
                "triangle descriptors accept 1..={maximum} anchors, received {actual}"
            ),
            Self::InvalidNeighborCount {
                minimum,
                maximum,
                actual,
            } => write!(
                formatter,
                "triangle descriptors accept {minimum}..={maximum} neighbors, received {actual}"
            ),
            Self::InvalidMinimumLongSide => {
                formatter.write_str("minimum triangle long side is invalid")
            }
            Self::InvalidMinimumNormalizedArea => {
                formatter.write_str("minimum normalized triangle area is invalid")
            }
            Self::InvalidMaximumDescriptors { maximum, actual } => write!(
                formatter,
                "triangle catalog accepts 1..={maximum} descriptors, received {actual}"
            ),
            Self::WorkBoundOverflow => formatter.write_str("triangle work bound overflowed"),
            Self::WorkBoundExceeded { maximum, requested } => write!(
                formatter,
                "triangle work bound is {maximum} attempts, requested {requested}"
            ),
            Self::InsufficientFeatures {
                available,
                required,
            } => write!(
                formatter,
                "triangle descriptors require {required} features, received {available}"
            ),
            Self::NonCanonicalFeatureRanks => {
                formatter.write_str("triangle source feature ranks are not canonical")
            }
            Self::AllocationFailed => formatter.write_str("triangle descriptor allocation failed"),
            Self::NumericalOverflow => {
                formatter.write_str("triangle descriptor geometry overflowed")
            }
            Self::CountOverflow => formatter.write_str("triangle evidence count overflowed"),
        }
    }
}

impl Error for TriangleDescriptorError {}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use crate::features::feature_catalog_for_tests;

    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn StdError>>;

    fn frame_id(digit: char) -> TestResult<FrameId> {
        Ok(FrameId::new(digit.to_string().repeat(64))?)
    }

    fn parameters(maximum_descriptors: usize) -> TestResult<TriangleDescriptorParameters> {
        Ok(TriangleDescriptorParameters::new(
            16,
            3,
            1.0,
            0.01,
            maximum_descriptors,
        )?)
    }

    fn point(x: f64, y: f64) -> TestResult<ImagePoint> {
        Ok(ImagePoint::new(x, y)?)
    }

    #[test]
    fn ratios_survive_translation_rotation_and_scale_while_mirror_flips_orientation() -> TestResult
    {
        let ranks = [0, 1, 2];
        let original = [point(0.0, 0.0)?, point(3.0, 0.0)?, point(0.0, 4.0)?];
        let transformed = [point(10.0, 20.0)?, point(10.0, 26.0)?, point(2.0, 20.0)?];
        let mirrored = [point(0.0, 0.0)?, point(-3.0, 0.0)?, point(0.0, 4.0)?];
        let first = describe_ranked_points(ranks, original, 1.0, 0.01)
            .map_err(|_| "original triangle rejected")?;
        let second = describe_ranked_points(ranks, transformed, 1.0, 0.01)
            .map_err(|_| "transformed triangle rejected")?;
        let reflected = describe_ranked_points(ranks, mirrored, 1.0, 0.01)
            .map_err(|_| "mirrored triangle rejected")?;

        assert_eq!(first.canonical_ranks(), [0, 1, 2]);
        assert!((first.short_to_long() - 0.6).abs() < 1.0e-15);
        assert!((first.middle_to_long() - 0.8).abs() < 1.0e-15);
        assert!((first.normalized_area() - 0.48).abs() < 1.0e-15);
        assert!((first.short_to_long() - second.short_to_long()).abs() < 1.0e-15);
        assert!((first.middle_to_long() - second.middle_to_long()).abs() < 1.0e-15);
        assert!((first.normalized_area() - second.normalized_area()).abs() < 1.0e-15);
        assert_eq!(first.orientation(), second.orientation());
        assert_ne!(first.orientation(), reflected.orientation());
        assert!((first.short_to_long() - reflected.short_to_long()).abs() < 1.0e-15);
        assert!((first.middle_to_long() - reflected.middle_to_long()).abs() < 1.0e-15);
        Ok(())
    }

    #[test]
    fn local_generation_deduplicates_triangle_identities() -> TestResult {
        let catalog = feature_catalog_for_tests(
            frame_id('a')?,
            &[(0.0, 0.0), (10.0, 0.0), (0.0, 10.0), (10.0, 10.0)],
        )?;
        let descriptors = build_triangle_descriptors(&catalog, parameters(100)?)?;

        assert_eq!(descriptors.algorithm_id(), TRIANGLE_DESCRIPTOR_ALGORITHM_ID);
        assert_eq!(descriptors.frame_id(), catalog.frame_id());
        assert_eq!(descriptors.source_feature_count(), 4);
        assert_eq!(descriptors.descriptors().len(), 4);
        assert_eq!(descriptors.statistics().anchors_visited(), 4);
        assert_eq!(descriptors.statistics().attempted_triangles(), 12);
        assert_eq!(descriptors.statistics().duplicate_triangles(), 8);
        assert!(!descriptors.statistics().stopped_at_output_limit());
        let mut identities: Vec<[usize; 3]> = descriptors
            .descriptors()
            .iter()
            .map(|descriptor| descriptor.identity_ranks())
            .collect();
        identities.sort_unstable();
        identities.dedup();
        assert_eq!(identities.len(), descriptors.descriptors().len());
        Ok(())
    }

    #[test]
    fn output_limit_stops_before_unbounded_generation() -> TestResult {
        let catalog = feature_catalog_for_tests(
            frame_id('b')?,
            &[(0.0, 0.0), (10.0, 0.0), (0.0, 10.0), (10.0, 10.0)],
        )?;
        let descriptors = build_triangle_descriptors(&catalog, parameters(1)?)?;

        assert_eq!(descriptors.descriptors().len(), 1);
        assert!(descriptors.statistics().stopped_at_output_limit());
        assert_eq!(descriptors.statistics().anchors_visited(), 1);
        assert_eq!(descriptors.statistics().attempted_triangles(), 2);
        Ok(())
    }

    #[test]
    fn rejects_short_degenerate_and_insufficient_geometry() -> TestResult {
        let short = describe_ranked_points(
            [0, 1, 2],
            [point(0.0, 0.0)?, point(1.0, 0.0)?, point(0.0, 1.0)?],
            2.0,
            0.01,
        );
        assert_eq!(short, Err(TriangleRejection::ShortSide));
        let collinear = describe_ranked_points(
            [0, 1, 2],
            [point(0.0, 0.0)?, point(5.0, 0.0)?, point(10.0, 0.0)?],
            1.0,
            0.01,
        );
        assert_eq!(collinear, Err(TriangleRejection::Degenerate));

        let catalog = feature_catalog_for_tests(frame_id('c')?, &[(0.0, 0.0), (1.0, 1.0)])?;
        assert_eq!(
            build_triangle_descriptors(&catalog, parameters(10)?),
            Err(TriangleDescriptorError::InsufficientFeatures {
                available: 2,
                required: 3,
            })
        );
        Ok(())
    }

    #[test]
    fn validates_all_descriptor_controls() {
        assert!(matches!(
            TriangleDescriptorParameters::new(0, 3, 1.0, 0.01, 10),
            Err(TriangleDescriptorError::InvalidMaximumAnchors { .. })
        ));
        assert!(matches!(
            TriangleDescriptorParameters::new(1, 1, 1.0, 0.01, 10),
            Err(TriangleDescriptorError::InvalidNeighborCount { .. })
        ));
        assert_eq!(
            TriangleDescriptorParameters::new(1, 2, 0.0, 0.01, 10),
            Err(TriangleDescriptorError::InvalidMinimumLongSide)
        );
        assert_eq!(
            TriangleDescriptorParameters::new(1, 2, 1.0, 1.0, 10),
            Err(TriangleDescriptorError::InvalidMinimumNormalizedArea)
        );
        assert!(matches!(
            TriangleDescriptorParameters::new(1, 2, 1.0, 0.01, 0),
            Err(TriangleDescriptorError::InvalidMaximumDescriptors { .. })
        ));
    }
}
