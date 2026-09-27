use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_review::FrameId;

use crate::{TriangleDescriptor, TriangleDescriptorCatalog};

/// Versioned tolerant descriptor-index and hypothesis policy.
pub const DESCRIPTOR_MATCH_ALGORITHM_ID: &str = "triangle-grid-hypotheses-v1";

/// Hard bound on retained candidates for one source descriptor.
pub const MAX_MATCH_CANDIDATES_PER_DESCRIPTOR: usize = 256;

/// Hard bound on retained hypotheses for one frame pair.
pub const MAX_MATCH_HYPOTHESES: usize = 1_000_000;

/// Hard bound on exact source/reference descriptor comparisons.
pub const MAX_DESCRIPTOR_COMPARISONS: usize = 50_000_000;

/// Whether descriptor hypotheses may reverse canonical orientation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReflectionPolicy {
    /// Accept only orientation-preserving hypotheses.
    Forbid,
    /// Accept both orientation-preserving and reflected hypotheses.
    Allow,
    /// Accept only reflected hypotheses.
    Require,
}

/// Validated tolerances and work/output bounds for descriptor matching.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DescriptorMatchParameters {
    maximum_side_ratio_error: f64,
    maximum_normalized_area_error: f64,
    minimum_scale: f64,
    maximum_scale: f64,
    reflection_policy: ReflectionPolicy,
    maximum_candidates_per_descriptor: usize,
    maximum_hypotheses: usize,
    maximum_comparisons: usize,
}

impl DescriptorMatchParameters {
    /// Creates explicit descriptor error budgets and resource limits.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        maximum_side_ratio_error: f64,
        maximum_normalized_area_error: f64,
        minimum_scale: f64,
        maximum_scale: f64,
        reflection_policy: ReflectionPolicy,
        maximum_candidates_per_descriptor: usize,
        maximum_hypotheses: usize,
        maximum_comparisons: usize,
    ) -> Result<Self, DescriptorMatchError> {
        if !valid_invariant_tolerance(maximum_side_ratio_error) {
            return Err(DescriptorMatchError::InvalidSideRatioTolerance);
        }
        if !valid_invariant_tolerance(maximum_normalized_area_error) {
            return Err(DescriptorMatchError::InvalidAreaTolerance);
        }
        if !minimum_scale.is_finite()
            || minimum_scale <= 0.0
            || !maximum_scale.is_finite()
            || maximum_scale < minimum_scale
        {
            return Err(DescriptorMatchError::InvalidScaleRange);
        }
        if maximum_candidates_per_descriptor == 0
            || maximum_candidates_per_descriptor > MAX_MATCH_CANDIDATES_PER_DESCRIPTOR
        {
            return Err(DescriptorMatchError::InvalidCandidateLimit {
                maximum: MAX_MATCH_CANDIDATES_PER_DESCRIPTOR,
                actual: maximum_candidates_per_descriptor,
            });
        }
        if maximum_hypotheses == 0 || maximum_hypotheses > MAX_MATCH_HYPOTHESES {
            return Err(DescriptorMatchError::InvalidHypothesisLimit {
                maximum: MAX_MATCH_HYPOTHESES,
                actual: maximum_hypotheses,
            });
        }
        if maximum_comparisons == 0 || maximum_comparisons > MAX_DESCRIPTOR_COMPARISONS {
            return Err(DescriptorMatchError::InvalidComparisonLimit {
                maximum: MAX_DESCRIPTOR_COMPARISONS,
                actual: maximum_comparisons,
            });
        }
        Ok(Self {
            maximum_side_ratio_error,
            maximum_normalized_area_error,
            minimum_scale,
            maximum_scale,
            reflection_policy,
            maximum_candidates_per_descriptor,
            maximum_hypotheses,
            maximum_comparisons,
        })
    }

    /// Inclusive absolute error for both side ratios.
    #[must_use]
    pub const fn maximum_side_ratio_error(self) -> f64 {
        self.maximum_side_ratio_error
    }

    /// Inclusive absolute error for normalized area.
    #[must_use]
    pub const fn maximum_normalized_area_error(self) -> f64 {
        self.maximum_normalized_area_error
    }

    /// Inclusive minimum reference/source scale ratio.
    #[must_use]
    pub const fn minimum_scale(self) -> f64 {
        self.minimum_scale
    }

    /// Inclusive maximum reference/source scale ratio.
    #[must_use]
    pub const fn maximum_scale(self) -> f64 {
        self.maximum_scale
    }

    /// Orientation policy for mirror hypotheses.
    #[must_use]
    pub const fn reflection_policy(self) -> ReflectionPolicy {
        self.reflection_policy
    }

    /// Maximum best candidates retained from one source descriptor.
    #[must_use]
    pub const fn maximum_candidates_per_descriptor(self) -> usize {
        self.maximum_candidates_per_descriptor
    }

    /// Maximum best hypotheses retained globally.
    #[must_use]
    pub const fn maximum_hypotheses(self) -> usize {
        self.maximum_hypotheses
    }

    /// Maximum exact descriptor comparisons before a typed failure.
    #[must_use]
    pub const fn maximum_comparisons(self) -> usize {
        self.maximum_comparisons
    }
}

/// One canonical source/reference feature-rank correspondence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FeaturePair {
    source_rank: usize,
    reference_rank: usize,
}

impl FeaturePair {
    /// Source feature rank.
    #[must_use]
    pub const fn source_rank(self) -> usize {
        self.source_rank
    }

    /// Reference feature rank.
    #[must_use]
    pub const fn reference_rank(self) -> usize {
        self.reference_rank
    }
}

/// One ambiguity-preserving triangle correspondence hypothesis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DescriptorMatchHypothesis {
    source_descriptor_index: usize,
    reference_descriptor_index: usize,
    feature_pairs: [FeaturePair; 3],
    side_ratio_error: f64,
    normalized_area_error: f64,
    normalized_distance: f64,
    scale: f64,
    reflected: bool,
}

impl DescriptorMatchHypothesis {
    /// Source descriptor position in its deterministic catalog.
    #[must_use]
    pub const fn source_descriptor_index(self) -> usize {
        self.source_descriptor_index
    }

    /// Reference descriptor position in its deterministic catalog.
    #[must_use]
    pub const fn reference_descriptor_index(self) -> usize {
        self.reference_descriptor_index
    }

    /// Canonical apex and endpoint feature mappings.
    #[must_use]
    pub const fn feature_pairs(self) -> [FeaturePair; 3] {
        self.feature_pairs
    }

    /// Maximum absolute error across the two side ratios.
    #[must_use]
    pub const fn side_ratio_error(self) -> f64 {
        self.side_ratio_error
    }

    /// Absolute normalized-area error.
    #[must_use]
    pub const fn normalized_area_error(self) -> f64 {
        self.normalized_area_error
    }

    /// Euclidean distance after normalizing each invariant by its tolerance.
    #[must_use]
    pub const fn normalized_distance(self) -> f64 {
        self.normalized_distance
    }

    /// Reference longest side divided by source longest side.
    #[must_use]
    pub const fn scale(self) -> f64 {
        self.scale
    }

    /// Whether canonical orientation reverses between source and reference.
    #[must_use]
    pub const fn reflected(self) -> bool {
        self.reflected
    }
}

/// Work, ambiguity, and truncation evidence for one descriptor match.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DescriptorMatchStatistics {
    comparisons: usize,
    geometric_candidates: usize,
    source_descriptors_without_candidates: usize,
    ambiguous_source_descriptors: usize,
    discarded_by_per_descriptor_limit: usize,
    discarded_by_global_limit: usize,
    reflected_hypotheses: usize,
}

impl DescriptorMatchStatistics {
    /// Exact descriptor comparisons performed after indexed lookup.
    #[must_use]
    pub const fn comparisons(self) -> usize {
        self.comparisons
    }

    /// Candidates satisfying all geometry, scale, and orientation filters.
    #[must_use]
    pub const fn geometric_candidates(self) -> usize {
        self.geometric_candidates
    }

    /// Source descriptors with no valid candidate.
    #[must_use]
    pub const fn source_descriptors_without_candidates(self) -> usize {
        self.source_descriptors_without_candidates
    }

    /// Source descriptors having more than one valid candidate before limits.
    #[must_use]
    pub const fn ambiguous_source_descriptors(self) -> usize {
        self.ambiguous_source_descriptors
    }

    /// Valid candidates not retained in their source descriptor's best set.
    #[must_use]
    pub const fn discarded_by_per_descriptor_limit(self) -> usize {
        self.discarded_by_per_descriptor_limit
    }

    /// Per-descriptor finalists not retained in the global best set.
    #[must_use]
    pub const fn discarded_by_global_limit(self) -> usize {
        self.discarded_by_global_limit
    }

    /// Retained hypotheses whose canonical orientation is reversed.
    #[must_use]
    pub const fn reflected_hypotheses(self) -> usize {
        self.reflected_hypotheses
    }
}

/// Deterministically ordered descriptor hypotheses between two distinct frames.
#[derive(Clone, Debug, PartialEq)]
pub struct DescriptorMatchCatalog {
    source_frame_id: FrameId,
    reference_frame_id: FrameId,
    parameters: DescriptorMatchParameters,
    statistics: DescriptorMatchStatistics,
    source_descriptor_catalog_truncated: bool,
    reference_descriptor_catalog_truncated: bool,
    hypotheses: Vec<DescriptorMatchHypothesis>,
}

impl DescriptorMatchCatalog {
    /// Versioned matching policy.
    #[must_use]
    pub const fn algorithm_id(&self) -> &'static str {
        DESCRIPTOR_MATCH_ALGORITHM_ID
    }

    /// Source frame transformed later into the reference coordinate system.
    #[must_use]
    pub const fn source_frame_id(&self) -> &FrameId {
        &self.source_frame_id
    }

    /// Fixed reference frame identity.
    #[must_use]
    pub const fn reference_frame_id(&self) -> &FrameId {
        &self.reference_frame_id
    }

    /// Exact tolerances and bounds used for matching.
    #[must_use]
    pub const fn parameters(&self) -> DescriptorMatchParameters {
        self.parameters
    }

    /// Complete work, ambiguity, and limit evidence.
    #[must_use]
    pub const fn statistics(&self) -> DescriptorMatchStatistics {
        self.statistics
    }

    /// Whether source descriptor generation stopped at its output bound.
    #[must_use]
    pub const fn source_descriptor_catalog_truncated(&self) -> bool {
        self.source_descriptor_catalog_truncated
    }

    /// Whether reference descriptor generation stopped at its output bound.
    #[must_use]
    pub const fn reference_descriptor_catalog_truncated(&self) -> bool {
        self.reference_descriptor_catalog_truncated
    }

    /// Best retained hypotheses ordered by normalized error then stable indices.
    #[must_use]
    pub fn hypotheses(&self) -> &[DescriptorMatchHypothesis] {
        &self.hypotheses
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct DescriptorBin([u64; 3]);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ReferenceIndexEntry {
    bin: DescriptorBin,
    descriptor_index: usize,
}

#[derive(Clone, Copy, Debug)]
struct HeapEntry(DescriptorMatchHypothesis);

impl PartialEq for HeapEntry {
    fn eq(&self, other: &Self) -> bool {
        compare_hypotheses(&self.0, &other.0) == Ordering::Equal
    }
}

impl Eq for HeapEntry {}

impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        compare_hypotheses(&self.0, &other.0)
    }
}

/// Finds bounded tolerant triangle hypotheses without selecting a transform.
pub fn match_triangle_descriptors(
    source: &TriangleDescriptorCatalog,
    reference: &TriangleDescriptorCatalog,
    parameters: DescriptorMatchParameters,
) -> Result<DescriptorMatchCatalog, DescriptorMatchError> {
    let parameters = DescriptorMatchParameters::new(
        parameters.maximum_side_ratio_error,
        parameters.maximum_normalized_area_error,
        parameters.minimum_scale,
        parameters.maximum_scale,
        parameters.reflection_policy,
        parameters.maximum_candidates_per_descriptor,
        parameters.maximum_hypotheses,
        parameters.maximum_comparisons,
    )?;
    if source.frame_id() == reference.frame_id() {
        return Err(DescriptorMatchError::SameFrame);
    }
    if source.descriptors().is_empty() {
        return Err(DescriptorMatchError::NoSourceDescriptors);
    }
    if reference.descriptors().is_empty() {
        return Err(DescriptorMatchError::NoReferenceDescriptors);
    }

    let index = build_reference_index(reference.descriptors(), parameters)?;
    let global_capacity = parameters.maximum_hypotheses.min(
        source
            .descriptors()
            .len()
            .checked_mul(parameters.maximum_candidates_per_descriptor)
            .ok_or(DescriptorMatchError::CountOverflow)?,
    );
    let mut best = BinaryHeap::new();
    best.try_reserve(global_capacity)
        .map_err(|_| DescriptorMatchError::AllocationFailed)?;
    let mut local = Vec::new();
    local
        .try_reserve_exact(parameters.maximum_candidates_per_descriptor)
        .map_err(|_| DescriptorMatchError::AllocationFailed)?;
    let mut statistics = DescriptorMatchStatistics::default();

    for (source_index, source_descriptor) in source.descriptors().iter().copied().enumerate() {
        local.clear();
        let mut source_candidates = 0_usize;
        let source_bin = descriptor_bin(source_descriptor, parameters)?;
        for candidate_bin in neighboring_bins(source_bin)? {
            let start = index.partition_point(|entry| entry.bin < candidate_bin);
            let end = index.partition_point(|entry| entry.bin <= candidate_bin);
            for entry in &index[start..end] {
                statistics.comparisons = checked_increment(statistics.comparisons)?;
                if statistics.comparisons > parameters.maximum_comparisons {
                    return Err(DescriptorMatchError::ComparisonLimitExceeded {
                        maximum: parameters.maximum_comparisons,
                    });
                }
                let reference_descriptor = reference.descriptors()[entry.descriptor_index];
                let Some(hypothesis) = compare_descriptors(
                    source_index,
                    source_descriptor,
                    entry.descriptor_index,
                    reference_descriptor,
                    parameters,
                )?
                else {
                    continue;
                };
                source_candidates = checked_increment(source_candidates)?;
                statistics.geometric_candidates =
                    checked_increment(statistics.geometric_candidates)?;
                retain_local_best(
                    &mut local,
                    hypothesis,
                    parameters.maximum_candidates_per_descriptor,
                );
            }
        }
        if source_candidates == 0 {
            statistics.source_descriptors_without_candidates =
                checked_increment(statistics.source_descriptors_without_candidates)?;
        } else if source_candidates > 1 {
            statistics.ambiguous_source_descriptors =
                checked_increment(statistics.ambiguous_source_descriptors)?;
        }
        statistics.discarded_by_per_descriptor_limit = statistics
            .discarded_by_per_descriptor_limit
            .checked_add(source_candidates.saturating_sub(local.len()))
            .ok_or(DescriptorMatchError::CountOverflow)?;
        local.sort_by(compare_hypotheses);
        for hypothesis in local.drain(..) {
            retain_global_best(
                &mut best,
                hypothesis,
                parameters.maximum_hypotheses,
                &mut statistics,
            )?;
        }
    }

    let heap_entries = best.into_vec();
    let mut hypotheses = Vec::new();
    hypotheses
        .try_reserve_exact(heap_entries.len())
        .map_err(|_| DescriptorMatchError::AllocationFailed)?;
    hypotheses.extend(heap_entries.into_iter().map(|entry| entry.0));
    hypotheses.sort_by(compare_hypotheses);
    statistics.reflected_hypotheses = hypotheses
        .iter()
        .filter(|hypothesis| hypothesis.reflected)
        .count();
    Ok(DescriptorMatchCatalog {
        source_frame_id: source.frame_id().clone(),
        reference_frame_id: reference.frame_id().clone(),
        parameters,
        statistics,
        source_descriptor_catalog_truncated: source.statistics().stopped_at_output_limit(),
        reference_descriptor_catalog_truncated: reference.statistics().stopped_at_output_limit(),
        hypotheses,
    })
}

fn build_reference_index(
    descriptors: &[TriangleDescriptor],
    parameters: DescriptorMatchParameters,
) -> Result<Vec<ReferenceIndexEntry>, DescriptorMatchError> {
    let mut index = Vec::new();
    index
        .try_reserve_exact(descriptors.len())
        .map_err(|_| DescriptorMatchError::AllocationFailed)?;
    for (descriptor_index, descriptor) in descriptors.iter().copied().enumerate() {
        index.push(ReferenceIndexEntry {
            bin: descriptor_bin(descriptor, parameters)?,
            descriptor_index,
        });
    }
    index.sort_unstable();
    Ok(index)
}

fn descriptor_bin(
    descriptor: TriangleDescriptor,
    parameters: DescriptorMatchParameters,
) -> Result<DescriptorBin, DescriptorMatchError> {
    Ok(DescriptorBin([
        invariant_bin(
            descriptor.short_to_long(),
            parameters.maximum_side_ratio_error,
        )?,
        invariant_bin(
            descriptor.middle_to_long(),
            parameters.maximum_side_ratio_error,
        )?,
        invariant_bin(
            descriptor.normalized_area(),
            parameters.maximum_normalized_area_error,
        )?,
    ]))
}

fn invariant_bin(value: f64, width: f64) -> Result<u64, DescriptorMatchError> {
    let quotient = (value / width).floor();
    if !quotient.is_finite() || quotient < 0.0 || quotient >= u64::MAX as f64 {
        return Err(DescriptorMatchError::IndexRangeOverflow);
    }
    Ok(quotient as u64)
}

fn neighboring_bins(center: DescriptorBin) -> Result<Vec<DescriptorBin>, DescriptorMatchError> {
    let first = neighboring_axis(center.0[0]);
    let second = neighboring_axis(center.0[1]);
    let third = neighboring_axis(center.0[2]);
    let mut bins = Vec::new();
    bins.try_reserve_exact(27)
        .map_err(|_| DescriptorMatchError::AllocationFailed)?;
    for a in first.into_iter().flatten() {
        for b in second.into_iter().flatten() {
            for c in third.into_iter().flatten() {
                bins.push(DescriptorBin([a, b, c]));
            }
        }
    }
    Ok(bins)
}

const fn neighboring_axis(center: u64) -> [Option<u64>; 3] {
    [center.checked_sub(1), Some(center), center.checked_add(1)]
}

fn compare_descriptors(
    source_index: usize,
    source: TriangleDescriptor,
    reference_index: usize,
    reference: TriangleDescriptor,
    parameters: DescriptorMatchParameters,
) -> Result<Option<DescriptorMatchHypothesis>, DescriptorMatchError> {
    let short_error = (source.short_to_long() - reference.short_to_long()).abs();
    let middle_error = (source.middle_to_long() - reference.middle_to_long()).abs();
    let area_error = (source.normalized_area() - reference.normalized_area()).abs();
    if short_error > parameters.maximum_side_ratio_error
        || middle_error > parameters.maximum_side_ratio_error
        || area_error > parameters.maximum_normalized_area_error
    {
        return Ok(None);
    }
    let scale = reference.long_side_pixels() / source.long_side_pixels();
    if !scale.is_finite() {
        return Err(DescriptorMatchError::NumericalOverflow);
    }
    if scale < parameters.minimum_scale || scale > parameters.maximum_scale {
        return Ok(None);
    }
    let reflected = source.orientation() != reference.orientation();
    let orientation_allowed = match parameters.reflection_policy {
        ReflectionPolicy::Forbid => !reflected,
        ReflectionPolicy::Allow => true,
        ReflectionPolicy::Require => reflected,
    };
    if !orientation_allowed {
        return Ok(None);
    }
    let short_normalized = short_error / parameters.maximum_side_ratio_error;
    let middle_normalized = middle_error / parameters.maximum_side_ratio_error;
    let area_normalized = area_error / parameters.maximum_normalized_area_error;
    let normalized_distance = short_normalized
        .hypot(middle_normalized)
        .hypot(area_normalized);
    if !normalized_distance.is_finite() {
        return Err(DescriptorMatchError::NumericalOverflow);
    }
    let source_ranks = source.canonical_ranks();
    let reference_ranks = reference.canonical_ranks();
    Ok(Some(DescriptorMatchHypothesis {
        source_descriptor_index: source_index,
        reference_descriptor_index: reference_index,
        feature_pairs: [
            FeaturePair {
                source_rank: source_ranks[0],
                reference_rank: reference_ranks[0],
            },
            FeaturePair {
                source_rank: source_ranks[1],
                reference_rank: reference_ranks[1],
            },
            FeaturePair {
                source_rank: source_ranks[2],
                reference_rank: reference_ranks[2],
            },
        ],
        side_ratio_error: short_error.max(middle_error),
        normalized_area_error: area_error,
        normalized_distance,
        scale,
        reflected,
    }))
}

fn retain_local_best(
    hypotheses: &mut Vec<DescriptorMatchHypothesis>,
    candidate: DescriptorMatchHypothesis,
    limit: usize,
) {
    if hypotheses.len() < limit {
        hypotheses.push(candidate);
        return;
    }
    if let Some((worst_index, worst)) = hypotheses
        .iter()
        .enumerate()
        .max_by(|(_, left), (_, right)| compare_hypotheses(left, right))
        && compare_hypotheses(&candidate, worst).is_lt()
    {
        hypotheses[worst_index] = candidate;
    }
}

fn retain_global_best(
    hypotheses: &mut BinaryHeap<HeapEntry>,
    candidate: DescriptorMatchHypothesis,
    limit: usize,
    statistics: &mut DescriptorMatchStatistics,
) -> Result<(), DescriptorMatchError> {
    if hypotheses.len() < limit {
        hypotheses.push(HeapEntry(candidate));
        return Ok(());
    }
    statistics.discarded_by_global_limit = checked_increment(statistics.discarded_by_global_limit)?;
    if hypotheses
        .peek()
        .is_some_and(|worst| compare_hypotheses(&candidate, &worst.0).is_lt())
    {
        hypotheses.pop();
        hypotheses.push(HeapEntry(candidate));
    }
    Ok(())
}

fn compare_hypotheses(
    left: &DescriptorMatchHypothesis,
    right: &DescriptorMatchHypothesis,
) -> Ordering {
    left.normalized_distance
        .total_cmp(&right.normalized_distance)
        .then_with(|| left.reflected.cmp(&right.reflected))
        .then_with(|| {
            left.source_descriptor_index
                .cmp(&right.source_descriptor_index)
        })
        .then_with(|| {
            left.reference_descriptor_index
                .cmp(&right.reference_descriptor_index)
        })
}

fn checked_increment(value: usize) -> Result<usize, DescriptorMatchError> {
    value
        .checked_add(1)
        .ok_or(DescriptorMatchError::CountOverflow)
}

fn valid_invariant_tolerance(value: f64) -> bool {
    value.is_finite() && (f64::EPSILON..=1.0).contains(&value)
}

/// Invalid controls, catalogs, allocation, or bounded matching work.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DescriptorMatchError {
    /// Side-ratio error must be finite and in `[EPSILON, 1]`.
    InvalidSideRatioTolerance,
    /// Area error must be finite and in `[EPSILON, 1]`.
    InvalidAreaTolerance,
    /// Scale bounds must be finite, positive, and ordered.
    InvalidScaleRange,
    /// Per-descriptor ambiguity bound was invalid.
    InvalidCandidateLimit {
        /// Hard maximum.
        maximum: usize,
        /// Requested value.
        actual: usize,
    },
    /// Global hypothesis bound was invalid.
    InvalidHypothesisLimit {
        /// Hard maximum.
        maximum: usize,
        /// Requested value.
        actual: usize,
    },
    /// Comparison bound was invalid.
    InvalidComparisonLimit {
        /// Hard maximum.
        maximum: usize,
        /// Requested value.
        actual: usize,
    },
    /// Source and reference identities were equal.
    SameFrame,
    /// Source descriptor catalog was empty.
    NoSourceDescriptors,
    /// Reference descriptor catalog was empty.
    NoReferenceDescriptors,
    /// Quantized invariant exceeded the index key domain.
    IndexRangeOverflow,
    /// Scratch or retained output allocation failed.
    AllocationFailed,
    /// Exact comparison work exceeded the configured bound.
    ComparisonLimitExceeded {
        /// Configured maximum.
        maximum: usize,
    },
    /// Geometry arithmetic left the finite domain.
    NumericalOverflow,
    /// Evidence arithmetic overflowed.
    CountOverflow,
}

impl Display for DescriptorMatchError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSideRatioTolerance => {
                formatter.write_str("descriptor side-ratio tolerance is invalid")
            }
            Self::InvalidAreaTolerance => {
                formatter.write_str("descriptor area tolerance is invalid")
            }
            Self::InvalidScaleRange => formatter.write_str("descriptor scale range is invalid"),
            Self::InvalidCandidateLimit { maximum, actual } => write!(
                formatter,
                "descriptor match accepts 1..={maximum} candidates per source, received {actual}"
            ),
            Self::InvalidHypothesisLimit { maximum, actual } => write!(
                formatter,
                "descriptor match accepts 1..={maximum} hypotheses, received {actual}"
            ),
            Self::InvalidComparisonLimit { maximum, actual } => write!(
                formatter,
                "descriptor match accepts 1..={maximum} comparisons, received {actual}"
            ),
            Self::SameFrame => formatter.write_str("source and reference frame are identical"),
            Self::NoSourceDescriptors => formatter.write_str("source descriptor catalog is empty"),
            Self::NoReferenceDescriptors => {
                formatter.write_str("reference descriptor catalog is empty")
            }
            Self::IndexRangeOverflow => formatter.write_str("descriptor index range overflowed"),
            Self::AllocationFailed => formatter.write_str("descriptor match allocation failed"),
            Self::ComparisonLimitExceeded { maximum } => write!(
                formatter,
                "descriptor match exceeded its {maximum}-comparison bound"
            ),
            Self::NumericalOverflow => formatter.write_str("descriptor match geometry overflowed"),
            Self::CountOverflow => formatter.write_str("descriptor match evidence overflowed"),
        }
    }
}

impl Error for DescriptorMatchError {}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use crate::features::feature_catalog_for_tests;
    use crate::{TriangleDescriptorParameters, build_triangle_descriptors};

    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn StdError>>;

    fn frame_id(digit: char) -> TestResult<FrameId> {
        Ok(FrameId::new(digit.to_string().repeat(64))?)
    }

    fn descriptor_catalog(
        digit: char,
        points: &[(f64, f64)],
        maximum_descriptors: usize,
    ) -> TestResult<TriangleDescriptorCatalog> {
        let features = feature_catalog_for_tests(frame_id(digit)?, points)?;
        let parameters = TriangleDescriptorParameters::new(
            points.len().max(1),
            points.len().saturating_sub(1).clamp(2, 8),
            0.1,
            1.0e-9,
            maximum_descriptors,
        )?;
        Ok(build_triangle_descriptors(&features, parameters)?)
    }

    fn match_parameters(
        reflection_policy: ReflectionPolicy,
        maximum_candidates_per_descriptor: usize,
        maximum_hypotheses: usize,
        maximum_comparisons: usize,
    ) -> TestResult<DescriptorMatchParameters> {
        Ok(DescriptorMatchParameters::new(
            1.0e-12,
            1.0e-12,
            0.25,
            4.0,
            reflection_policy,
            maximum_candidates_per_descriptor,
            maximum_hypotheses,
            maximum_comparisons,
        )?)
    }

    #[test]
    fn matches_translation_rotation_and_scale_without_collapsing_ambiguity() -> TestResult {
        let source_points = [
            (0.0, 0.0),
            (10.0, 1.0),
            (3.0, 14.0),
            (17.0, 11.0),
            (25.0, 4.0),
        ];
        let reference_points = source_points.map(|(x, y)| (100.0 - 2.0 * y, 50.0 + 2.0 * x));
        let source = descriptor_catalog('a', &source_points, 100)?;
        let reference = descriptor_catalog('b', &reference_points, 100)?;

        let matches = match_triangle_descriptors(
            &source,
            &reference,
            match_parameters(ReflectionPolicy::Forbid, 8, 100, 10_000)?,
        )?;

        assert_eq!(matches.algorithm_id(), DESCRIPTOR_MATCH_ALGORITHM_ID);
        assert_eq!(matches.source_frame_id(), source.frame_id());
        assert_eq!(matches.reference_frame_id(), reference.frame_id());
        assert!(!matches.hypotheses().is_empty());
        assert!(!matches.source_descriptor_catalog_truncated());
        assert!(!matches.reference_descriptor_catalog_truncated());
        assert!(matches.statistics().comparisons() < 10_000);
        let best = matches.hypotheses()[0];
        assert!(best.normalized_distance() < 0.01);
        assert!((best.scale() - 2.0).abs() < 1.0e-12);
        assert!(!best.reflected());
        assert_eq!(
            best.feature_pairs(),
            best.feature_pairs().map(|pair| FeaturePair {
                source_rank: pair.source_rank(),
                reference_rank: pair.source_rank(),
            })
        );
        Ok(())
    }

    #[test]
    fn reflection_policy_is_explicit_and_testable() -> TestResult {
        let source_points = [(0.0, 0.0), (10.0, 1.0), (3.0, 14.0), (17.0, 11.0)];
        let mirrored_points = source_points.map(|(x, y)| (100.0 - x, 20.0 + y));
        let source = descriptor_catalog('c', &source_points, 100)?;
        let reference = descriptor_catalog('d', &mirrored_points, 100)?;

        let forbidden = match_triangle_descriptors(
            &source,
            &reference,
            match_parameters(ReflectionPolicy::Forbid, 8, 100, 10_000)?,
        )?;
        assert!(forbidden.hypotheses().is_empty());
        assert!(
            forbidden
                .statistics()
                .source_descriptors_without_candidates()
                > 0
        );

        let required = match_triangle_descriptors(
            &source,
            &reference,
            match_parameters(ReflectionPolicy::Require, 8, 100, 10_000)?,
        )?;
        assert!(!required.hypotheses().is_empty());
        assert_eq!(
            required.statistics().reflected_hypotheses(),
            required.hypotheses().len()
        );
        assert!(required.hypotheses().iter().all(|item| item.reflected()));
        Ok(())
    }

    #[test]
    fn ambiguity_and_both_retention_limits_are_reported() -> TestResult {
        let square = [(0.0, 0.0), (10.0, 0.0), (0.0, 10.0), (10.0, 10.0)];
        let shifted = square.map(|(x, y)| (x + 30.0, y + 20.0));
        let source = descriptor_catalog('e', &square, 100)?;
        let reference = descriptor_catalog('f', &shifted, 100)?;

        let matches = match_triangle_descriptors(
            &source,
            &reference,
            match_parameters(ReflectionPolicy::Allow, 1, 1, 10_000)?,
        )?;

        assert_eq!(matches.hypotheses().len(), 1);
        assert!(matches.statistics().geometric_candidates() > 1);
        assert!(matches.statistics().ambiguous_source_descriptors() > 0);
        assert!(matches.statistics().discarded_by_per_descriptor_limit() > 0);
        assert!(matches.statistics().discarded_by_global_limit() > 0);
        Ok(())
    }

    #[test]
    fn comparison_budget_fails_closed() -> TestResult {
        let square = [(0.0, 0.0), (10.0, 0.0), (0.0, 10.0), (10.0, 10.0)];
        let shifted = square.map(|(x, y)| (x + 1.0, y + 1.0));
        let source = descriptor_catalog('1', &square, 100)?;
        let reference = descriptor_catalog('2', &shifted, 100)?;

        assert_eq!(
            match_triangle_descriptors(
                &source,
                &reference,
                match_parameters(ReflectionPolicy::Allow, 1, 1, 1)?,
            ),
            Err(DescriptorMatchError::ComparisonLimitExceeded { maximum: 1 })
        );
        Ok(())
    }

    #[test]
    fn rejects_invalid_controls_same_frame_and_empty_geometry() -> TestResult {
        assert_eq!(
            DescriptorMatchParameters::new(0.0, 0.1, 1.0, 1.0, ReflectionPolicy::Forbid, 1, 1, 1,),
            Err(DescriptorMatchError::InvalidSideRatioTolerance)
        );
        assert_eq!(
            DescriptorMatchParameters::new(0.1, 0.1, 2.0, 1.0, ReflectionPolicy::Forbid, 1, 1, 1,),
            Err(DescriptorMatchError::InvalidScaleRange)
        );

        let triangle = [(0.0, 0.0), (10.0, 0.0), (0.0, 10.0)];
        let catalog = descriptor_catalog('3', &triangle, 10)?;
        assert_eq!(
            match_triangle_descriptors(
                &catalog,
                &catalog,
                match_parameters(ReflectionPolicy::Forbid, 1, 1, 10)?,
            ),
            Err(DescriptorMatchError::SameFrame)
        );

        let collinear = [(0.0, 0.0), (5.0, 0.0), (10.0, 0.0)];
        let empty = descriptor_catalog('4', &collinear, 10)?;
        assert!(empty.descriptors().is_empty());
        assert_eq!(
            match_triangle_descriptors(
                &empty,
                &catalog,
                match_parameters(ReflectionPolicy::Forbid, 1, 1, 10)?,
            ),
            Err(DescriptorMatchError::NoSourceDescriptors)
        );
        Ok(())
    }
}
