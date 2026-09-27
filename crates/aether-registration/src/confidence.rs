use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_review::FrameId;

use crate::{FeatureCatalog, SimilarityConsensus};

/// Versioned fail-closed acceptance policy for one similarity consensus.
pub const REGISTRATION_CONFIDENCE_ALGORITHM_ID: &str = "similarity-confidence-gate-v1";

/// Validated support, ambiguity, residual, and spatial-coverage requirements.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RegistrationConfidenceParameters {
    minimum_inlier_hypotheses: usize,
    minimum_inlier_ratio: f64,
    minimum_feature_pairs: usize,
    minimum_winner_support_margin: usize,
    maximum_rms_residual_pixels: f64,
    maximum_residual_pixels: f64,
    minimum_source_axis_span_fraction: f64,
    minimum_reference_axis_span_fraction: f64,
    allow_reflection: bool,
    allow_truncated_evidence: bool,
}

impl RegistrationConfidenceParameters {
    /// Creates an explicit scientific acceptance gate.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        minimum_inlier_hypotheses: usize,
        minimum_inlier_ratio: f64,
        minimum_feature_pairs: usize,
        minimum_winner_support_margin: usize,
        maximum_rms_residual_pixels: f64,
        maximum_residual_pixels: f64,
        minimum_source_axis_span_fraction: f64,
        minimum_reference_axis_span_fraction: f64,
        allow_reflection: bool,
        allow_truncated_evidence: bool,
    ) -> Result<Self, RegistrationConfidenceError> {
        if minimum_inlier_hypotheses < 2 {
            return Err(RegistrationConfidenceError::InvalidMinimumHypotheses);
        }
        if !valid_fraction(minimum_inlier_ratio) {
            return Err(RegistrationConfidenceError::InvalidInlierRatio);
        }
        if minimum_feature_pairs < 3 {
            return Err(RegistrationConfidenceError::InvalidMinimumFeaturePairs);
        }
        if minimum_winner_support_margin == 0 {
            return Err(RegistrationConfidenceError::InvalidSupportMargin);
        }
        if !maximum_rms_residual_pixels.is_finite() || maximum_rms_residual_pixels <= 0.0 {
            return Err(RegistrationConfidenceError::InvalidRmsResidual);
        }
        if !maximum_residual_pixels.is_finite()
            || maximum_residual_pixels < maximum_rms_residual_pixels
        {
            return Err(RegistrationConfidenceError::InvalidMaximumResidual);
        }
        if !valid_fraction(minimum_source_axis_span_fraction) {
            return Err(RegistrationConfidenceError::InvalidSourceSpan);
        }
        if !valid_fraction(minimum_reference_axis_span_fraction) {
            return Err(RegistrationConfidenceError::InvalidReferenceSpan);
        }
        Ok(Self {
            minimum_inlier_hypotheses,
            minimum_inlier_ratio,
            minimum_feature_pairs,
            minimum_winner_support_margin,
            maximum_rms_residual_pixels,
            maximum_residual_pixels,
            minimum_source_axis_span_fraction,
            minimum_reference_axis_span_fraction,
            allow_reflection,
            allow_truncated_evidence,
        })
    }

    /// Minimum refined inlier triangles.
    #[must_use]
    pub const fn minimum_inlier_hypotheses(self) -> usize {
        self.minimum_inlier_hypotheses
    }

    /// Minimum refined inliers divided by hypotheses of the selected orientation.
    #[must_use]
    pub const fn minimum_inlier_ratio(self) -> f64 {
        self.minimum_inlier_ratio
    }

    /// Minimum distinct one-to-one star correspondences.
    #[must_use]
    pub const fn minimum_feature_pairs(self) -> usize {
        self.minimum_feature_pairs
    }

    /// Minimum triangle-support lead over the best distinct competitor.
    #[must_use]
    pub const fn minimum_winner_support_margin(self) -> usize {
        self.minimum_winner_support_margin
    }

    /// Inclusive RMS residual ceiling in reference pixels.
    #[must_use]
    pub const fn maximum_rms_residual_pixels(self) -> f64 {
        self.maximum_rms_residual_pixels
    }

    /// Inclusive maximum residual ceiling in reference pixels.
    #[must_use]
    pub const fn maximum_residual_pixels(self) -> f64 {
        self.maximum_residual_pixels
    }

    /// Minimum horizontal and vertical source-span fraction.
    #[must_use]
    pub const fn minimum_source_axis_span_fraction(self) -> f64 {
        self.minimum_source_axis_span_fraction
    }

    /// Minimum horizontal and vertical reference-span fraction.
    #[must_use]
    pub const fn minimum_reference_axis_span_fraction(self) -> f64 {
        self.minimum_reference_axis_span_fraction
    }

    /// Whether an orientation-reversing result may pass.
    #[must_use]
    pub const fn allow_reflection(self) -> bool {
        self.allow_reflection
    }

    /// Whether descriptor or model-search truncation may pass.
    #[must_use]
    pub const fn allow_truncated_evidence(self) -> bool {
        self.allow_truncated_evidence
    }
}

/// One deterministic reason why automatic registration must not proceed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistrationConfidenceRejection {
    /// Refined triangle support is below the configured minimum.
    InsufficientInlierHypotheses,
    /// Too few selected-orientation hypotheses support the winner.
    InsufficientInlierRatio,
    /// Too few distinct one-to-one feature pairs support the winner.
    InsufficientFeaturePairs,
    /// A geometrically distinct model has too much competing support.
    InsufficientWinnerSupportMargin,
    /// Strict RMS residual exceeds the configured ceiling.
    ExcessiveRmsResidual,
    /// Strict worst residual exceeds the configured ceiling.
    ExcessiveMaximumResidual,
    /// Inlier stars occupy too little source width.
    InsufficientSourceHorizontalSpan,
    /// Inlier stars occupy too little source height.
    InsufficientSourceVerticalSpan,
    /// Inlier stars occupy too little reference width.
    InsufficientReferenceHorizontalSpan,
    /// Inlier stars occupy too little reference height.
    InsufficientReferenceVerticalSpan,
    /// The selected transform reverses orientation but policy forbids it.
    ReflectionForbidden,
    /// Descriptor generation or model search was truncated.
    TruncatedEvidence,
}

/// Complete acceptance decision and the metrics that produced it.
#[derive(Clone, Debug, PartialEq)]
pub struct RegistrationConfidenceReport {
    source_frame_id: FrameId,
    reference_frame_id: FrameId,
    parameters: RegistrationConfidenceParameters,
    inlier_ratio: f64,
    winner_support_margin: Option<usize>,
    source_horizontal_span_fraction: f64,
    source_vertical_span_fraction: f64,
    reference_horizontal_span_fraction: f64,
    reference_vertical_span_fraction: f64,
    rejections: Vec<RegistrationConfidenceRejection>,
}

impl RegistrationConfidenceReport {
    /// Versioned confidence policy.
    #[must_use]
    pub const fn algorithm_id(&self) -> &'static str {
        REGISTRATION_CONFIDENCE_ALGORITHM_ID
    }

    /// Source frame assessed by this decision.
    #[must_use]
    pub const fn source_frame_id(&self) -> &FrameId {
        &self.source_frame_id
    }

    /// Reference frame assessed by this decision.
    #[must_use]
    pub const fn reference_frame_id(&self) -> &FrameId {
        &self.reference_frame_id
    }

    /// Exact acceptance controls.
    #[must_use]
    pub const fn parameters(&self) -> RegistrationConfidenceParameters {
        self.parameters
    }

    /// Refined triangle support divided by eligible selected-orientation evidence.
    #[must_use]
    pub const fn inlier_ratio(&self) -> f64 {
        self.inlier_ratio
    }

    /// Winner support minus best distinct-competitor support, when one exists.
    #[must_use]
    pub const fn winner_support_margin(&self) -> Option<usize> {
        self.winner_support_margin
    }

    /// Fraction of source width spanned by distinct inlier stars.
    #[must_use]
    pub const fn source_horizontal_span_fraction(&self) -> f64 {
        self.source_horizontal_span_fraction
    }

    /// Fraction of source height spanned by distinct inlier stars.
    #[must_use]
    pub const fn source_vertical_span_fraction(&self) -> f64 {
        self.source_vertical_span_fraction
    }

    /// Fraction of reference width spanned by distinct inlier stars.
    #[must_use]
    pub const fn reference_horizontal_span_fraction(&self) -> f64 {
        self.reference_horizontal_span_fraction
    }

    /// Fraction of reference height spanned by distinct inlier stars.
    #[must_use]
    pub const fn reference_vertical_span_fraction(&self) -> f64 {
        self.reference_vertical_span_fraction
    }

    /// Ordered complete list of failed checks.
    #[must_use]
    pub fn rejections(&self) -> &[RegistrationConfidenceRejection] {
        &self.rejections
    }

    /// Whether every configured confidence requirement passed.
    #[must_use]
    pub fn accepted(&self) -> bool {
        self.rejections.is_empty()
    }
}

/// Evaluates an inspectable registration result without hiding failed criteria.
pub fn assess_registration_confidence(
    source_features: &FeatureCatalog,
    reference_features: &FeatureCatalog,
    consensus: &SimilarityConsensus,
    parameters: RegistrationConfidenceParameters,
) -> Result<RegistrationConfidenceReport, RegistrationConfidenceError> {
    let parameters = RegistrationConfidenceParameters::new(
        parameters.minimum_inlier_hypotheses,
        parameters.minimum_inlier_ratio,
        parameters.minimum_feature_pairs,
        parameters.minimum_winner_support_margin,
        parameters.maximum_rms_residual_pixels,
        parameters.maximum_residual_pixels,
        parameters.minimum_source_axis_span_fraction,
        parameters.minimum_reference_axis_span_fraction,
        parameters.allow_reflection,
        parameters.allow_truncated_evidence,
    )?;
    if source_features.frame_id() != consensus.source_frame_id() {
        return Err(RegistrationConfidenceError::SourceFrameMismatch);
    }
    if reference_features.frame_id() != consensus.reference_frame_id() {
        return Err(RegistrationConfidenceError::ReferenceFrameMismatch);
    }

    let statistics = consensus.statistics();
    let eligible = statistics.selected_orientation_hypotheses();
    if eligible == 0 {
        return Err(RegistrationConfidenceError::MissingOrientationEvidence);
    }
    let inlier_ratio = statistics.refined_inlier_hypotheses() as f64 / eligible as f64;
    if !inlier_ratio.is_finite() {
        return Err(RegistrationConfidenceError::NumericalOverflow);
    }
    let winner_support_margin = consensus.competing_similarity().map(|competitor| {
        statistics
            .refined_inlier_hypotheses()
            .saturating_sub(competitor.inlier_hypotheses())
    });
    let (source_horizontal_span_fraction, source_vertical_span_fraction) = feature_span(
        source_features,
        consensus
            .inlier_feature_pairs()
            .iter()
            .map(|pair| pair.source_rank()),
    )?;
    let (reference_horizontal_span_fraction, reference_vertical_span_fraction) = feature_span(
        reference_features,
        consensus
            .inlier_feature_pairs()
            .iter()
            .map(|pair| pair.reference_rank()),
    )?;

    let mut rejections = Vec::new();
    rejections
        .try_reserve_exact(12)
        .map_err(|_| RegistrationConfidenceError::AllocationFailed)?;
    if statistics.refined_inlier_hypotheses() < parameters.minimum_inlier_hypotheses {
        rejections.push(RegistrationConfidenceRejection::InsufficientInlierHypotheses);
    }
    if inlier_ratio < parameters.minimum_inlier_ratio {
        rejections.push(RegistrationConfidenceRejection::InsufficientInlierRatio);
    }
    if consensus.inlier_feature_pairs().len() < parameters.minimum_feature_pairs {
        rejections.push(RegistrationConfidenceRejection::InsufficientFeaturePairs);
    }
    if winner_support_margin.is_some_and(|margin| margin < parameters.minimum_winner_support_margin)
    {
        rejections.push(RegistrationConfidenceRejection::InsufficientWinnerSupportMargin);
    }
    if consensus.residual_statistics().root_mean_square_pixels()
        > parameters.maximum_rms_residual_pixels
    {
        rejections.push(RegistrationConfidenceRejection::ExcessiveRmsResidual);
    }
    if consensus.residual_statistics().maximum_pixels() > parameters.maximum_residual_pixels {
        rejections.push(RegistrationConfidenceRejection::ExcessiveMaximumResidual);
    }
    if source_horizontal_span_fraction < parameters.minimum_source_axis_span_fraction {
        rejections.push(RegistrationConfidenceRejection::InsufficientSourceHorizontalSpan);
    }
    if source_vertical_span_fraction < parameters.minimum_source_axis_span_fraction {
        rejections.push(RegistrationConfidenceRejection::InsufficientSourceVerticalSpan);
    }
    if reference_horizontal_span_fraction < parameters.minimum_reference_axis_span_fraction {
        rejections.push(RegistrationConfidenceRejection::InsufficientReferenceHorizontalSpan);
    }
    if reference_vertical_span_fraction < parameters.minimum_reference_axis_span_fraction {
        rejections.push(RegistrationConfidenceRejection::InsufficientReferenceVerticalSpan);
    }
    if consensus.reflected() && !parameters.allow_reflection {
        rejections.push(RegistrationConfidenceRejection::ReflectionForbidden);
    }
    let truncated = consensus.source_descriptor_catalog_truncated()
        || consensus.reference_descriptor_catalog_truncated()
        || statistics.models_discarded_by_limit() > 0;
    if truncated && !parameters.allow_truncated_evidence {
        rejections.push(RegistrationConfidenceRejection::TruncatedEvidence);
    }

    Ok(RegistrationConfidenceReport {
        source_frame_id: source_features.frame_id().clone(),
        reference_frame_id: reference_features.frame_id().clone(),
        parameters,
        inlier_ratio,
        winner_support_margin,
        source_horizontal_span_fraction,
        source_vertical_span_fraction,
        reference_horizontal_span_fraction,
        reference_vertical_span_fraction,
        rejections,
    })
}

fn feature_span(
    catalog: &FeatureCatalog,
    ranks: impl Iterator<Item = usize>,
) -> Result<(f64, f64), RegistrationConfidenceError> {
    let mut minimum_x = f64::INFINITY;
    let mut maximum_x = f64::NEG_INFINITY;
    let mut minimum_y = f64::INFINITY;
    let mut maximum_y = f64::NEG_INFINITY;
    let mut count = 0_usize;
    for rank in ranks {
        let feature = catalog
            .features()
            .get(rank)
            .filter(|feature| feature.rank() == rank)
            .ok_or(RegistrationConfidenceError::MissingFeatureRank { rank })?;
        minimum_x = minimum_x.min(feature.point().x());
        maximum_x = maximum_x.max(feature.point().x());
        minimum_y = minimum_y.min(feature.point().y());
        maximum_y = maximum_y.max(feature.point().y());
        count = count
            .checked_add(1)
            .ok_or(RegistrationConfidenceError::CountOverflow)?;
    }
    if count == 0 {
        return Err(RegistrationConfidenceError::NoFeaturePairs);
    }
    let width = catalog.width().saturating_sub(1) as f64;
    let height = catalog.height().saturating_sub(1) as f64;
    let horizontal = if width > 0.0 {
        (maximum_x - minimum_x) / width
    } else {
        0.0
    };
    let vertical = if height > 0.0 {
        (maximum_y - minimum_y) / height
    } else {
        0.0
    };
    if !horizontal.is_finite() || !vertical.is_finite() {
        return Err(RegistrationConfidenceError::NumericalOverflow);
    }
    Ok((horizontal.max(0.0), vertical.max(0.0)))
}

const fn valid_fraction(value: f64) -> bool {
    value.is_finite() && value > 0.0 && value <= 1.0
}

/// Invalid confidence controls or incoherent scientific evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegistrationConfidenceError {
    /// At least two inlier triangles must be requested.
    InvalidMinimumHypotheses,
    /// Inlier ratio must be finite and in `(0, 1]`.
    InvalidInlierRatio,
    /// At least three distinct feature pairs must be requested.
    InvalidMinimumFeaturePairs,
    /// Winner support margin must be nonzero.
    InvalidSupportMargin,
    /// RMS ceiling must be finite and positive.
    InvalidRmsResidual,
    /// Maximum ceiling must be finite and no lower than the RMS ceiling.
    InvalidMaximumResidual,
    /// Source span fraction must be finite and in `(0, 1]`.
    InvalidSourceSpan,
    /// Reference span fraction must be finite and in `(0, 1]`.
    InvalidReferenceSpan,
    /// Source feature and consensus identities differ.
    SourceFrameMismatch,
    /// Reference feature and consensus identities differ.
    ReferenceFrameMismatch,
    /// Consensus statistics contain no selected-orientation evidence.
    MissingOrientationEvidence,
    /// An inlier feature rank is missing from the supplied catalog.
    MissingFeatureRank {
        /// Missing zero-based rank.
        rank: usize,
    },
    /// Consensus unexpectedly contains no distinct feature pair.
    NoFeaturePairs,
    /// Rejection storage allocation failed.
    AllocationFailed,
    /// Metric arithmetic left the finite domain.
    NumericalOverflow,
    /// Evidence accounting overflowed.
    CountOverflow,
}

impl Display for RegistrationConfidenceError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidMinimumHypotheses => {
                formatter.write_str("confidence requires at least two inlier hypotheses")
            }
            Self::InvalidInlierRatio => formatter.write_str("confidence inlier ratio is invalid"),
            Self::InvalidMinimumFeaturePairs => {
                formatter.write_str("confidence requires at least three feature pairs")
            }
            Self::InvalidSupportMargin => {
                formatter.write_str("confidence support margin is invalid")
            }
            Self::InvalidRmsResidual => {
                formatter.write_str("confidence RMS residual ceiling is invalid")
            }
            Self::InvalidMaximumResidual => {
                formatter.write_str("confidence maximum residual ceiling is invalid")
            }
            Self::InvalidSourceSpan => formatter.write_str("confidence source span is invalid"),
            Self::InvalidReferenceSpan => {
                formatter.write_str("confidence reference span is invalid")
            }
            Self::SourceFrameMismatch => {
                formatter.write_str("source feature and consensus frame identities differ")
            }
            Self::ReferenceFrameMismatch => {
                formatter.write_str("reference feature and consensus frame identities differ")
            }
            Self::MissingOrientationEvidence => {
                formatter.write_str("consensus has no selected-orientation evidence")
            }
            Self::MissingFeatureRank { rank } => {
                write!(formatter, "confidence feature rank {rank} is missing")
            }
            Self::NoFeaturePairs => {
                formatter.write_str("consensus has no confidence feature pairs")
            }
            Self::AllocationFailed => formatter.write_str("confidence rejection allocation failed"),
            Self::NumericalOverflow => formatter.write_str("confidence metric overflowed"),
            Self::CountOverflow => formatter.write_str("confidence evidence overflowed"),
        }
    }
}

impl Error for RegistrationConfidenceError {}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use aether_review::FrameId;

    use crate::features::feature_catalog_for_tests;
    use crate::{
        DescriptorMatchParameters, ReflectionPolicy, SimilarityConsensusParameters,
        TriangleDescriptorParameters, build_triangle_descriptors, estimate_similarity_consensus,
        match_triangle_descriptors,
    };

    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn StdError>>;

    fn frame_id(digit: char) -> TestResult<FrameId> {
        Ok(FrameId::new(digit.to_string().repeat(64))?)
    }

    fn feature_catalog(digit: char, points: &[(f64, f64)]) -> TestResult<FeatureCatalog> {
        Ok(feature_catalog_for_tests(frame_id(digit)?, points)?)
    }

    fn consensus(
        source: &FeatureCatalog,
        reference: &FeatureCatalog,
        reflection_policy: ReflectionPolicy,
        maximum_models: usize,
    ) -> TestResult<SimilarityConsensus> {
        let descriptors = TriangleDescriptorParameters::new(
            source.features().len(),
            source.features().len().saturating_sub(1).clamp(2, 8),
            0.1,
            1.0e-9,
            1_000,
        )?;
        let source_descriptors = build_triangle_descriptors(source, descriptors)?;
        let reference_descriptors = build_triangle_descriptors(reference, descriptors)?;
        let matches = match_triangle_descriptors(
            &source_descriptors,
            &reference_descriptors,
            DescriptorMatchParameters::new(
                1.0e-10,
                1.0e-10,
                0.25,
                4.0,
                reflection_policy,
                32,
                1_000,
                100_000,
            )?,
        )?;
        Ok(estimate_similarity_consensus(
            source,
            reference,
            &matches,
            SimilarityConsensusParameters::new(1.0e-7, 2, 3, 0.25, maximum_models, 1_000_000)?,
        )?)
    }

    fn confidence_parameters() -> TestResult<RegistrationConfidenceParameters> {
        Ok(RegistrationConfidenceParameters::new(
            2, 0.5, 4, 1, 1.0e-8, 1.0e-7, 0.1, 0.1, false, false,
        )?)
    }

    #[test]
    fn accepts_well_supported_precise_and_spatially_distributed_geometry() -> TestResult {
        let source_points = [
            (2.0, 3.0),
            (35.0, 5.0),
            (4.0, 47.0),
            (59.0, 33.0),
            (77.0, 17.0),
            (63.0, 74.0),
        ];
        let reference_points = source_points.map(|(x, y)| (10.0 + 0.8 * x, 15.0 + 0.8 * y));
        let source = feature_catalog('a', &source_points)?;
        let reference = feature_catalog('b', &reference_points)?;
        let consensus = consensus(&source, &reference, ReflectionPolicy::Forbid, 1_000)?;

        let report = assess_registration_confidence(
            &source,
            &reference,
            &consensus,
            confidence_parameters()?,
        )?;

        assert_eq!(report.algorithm_id(), REGISTRATION_CONFIDENCE_ALGORITHM_ID);
        assert!(report.accepted());
        assert!(report.rejections().is_empty());
        assert!(report.inlier_ratio() >= 0.5);
        assert!(report.source_horizontal_span_fraction() > 0.7);
        assert!(report.source_vertical_span_fraction() > 0.7);
        Ok(())
    }

    #[test]
    fn equal_support_symmetric_transform_is_rejected_as_ambiguous() -> TestResult {
        let square = [(5.0, 5.0), (75.0, 5.0), (5.0, 75.0), (75.0, 75.0)];
        let translated = square.map(|(x, y)| (x + 3.0, y + 4.0));
        let source = feature_catalog('c', &square)?;
        let reference = feature_catalog('d', &translated)?;
        let consensus = consensus(&source, &reference, ReflectionPolicy::Allow, 100)?;

        let report = assess_registration_confidence(
            &source,
            &reference,
            &consensus,
            confidence_parameters()?,
        )?;

        assert!(!report.accepted());
        assert_eq!(report.winner_support_margin(), Some(0));
        assert!(
            report
                .rejections()
                .contains(&RegistrationConfidenceRejection::InsufficientWinnerSupportMargin)
        );
        Ok(())
    }

    #[test]
    fn precise_but_localized_geometry_fails_both_spatial_axes() -> TestResult {
        let source_points = [(1.0, 1.0), (3.0, 1.5), (1.5, 4.0), (4.5, 3.5), (5.0, 2.0)];
        let reference_points = source_points.map(|(x, y)| (x + 20.0, y + 20.0));
        let source = feature_catalog('e', &source_points)?;
        let reference = feature_catalog('f', &reference_points)?;
        let consensus = consensus(&source, &reference, ReflectionPolicy::Forbid, 100)?;

        let report = assess_registration_confidence(
            &source,
            &reference,
            &consensus,
            confidence_parameters()?,
        )?;

        assert!(!report.accepted());
        assert!(
            report
                .rejections()
                .contains(&RegistrationConfidenceRejection::InsufficientSourceHorizontalSpan)
        );
        assert!(
            report
                .rejections()
                .contains(&RegistrationConfidenceRejection::InsufficientSourceVerticalSpan)
        );
        Ok(())
    }

    #[test]
    fn model_search_truncation_is_visible_and_rejected_by_default() -> TestResult {
        let points = [
            (2.0, 3.0),
            (25.0, 5.0),
            (4.0, 37.0),
            (49.0, 23.0),
            (67.0, 17.0),
        ];
        let shifted = points.map(|(x, y)| (x + 4.0, y + 6.0));
        let source = feature_catalog('1', &points)?;
        let reference = feature_catalog('2', &shifted)?;
        let consensus = consensus(&source, &reference, ReflectionPolicy::Forbid, 1)?;
        assert!(consensus.statistics().models_discarded_by_limit() > 0);

        let report = assess_registration_confidence(
            &source,
            &reference,
            &consensus,
            confidence_parameters()?,
        )?;

        assert!(!report.accepted());
        assert!(
            report
                .rejections()
                .contains(&RegistrationConfidenceRejection::TruncatedEvidence)
        );
        Ok(())
    }

    #[test]
    fn validates_controls() {
        assert_eq!(
            RegistrationConfidenceParameters::new(1, 0.5, 3, 1, 0.1, 0.2, 0.1, 0.1, false, false,),
            Err(RegistrationConfidenceError::InvalidMinimumHypotheses)
        );
        assert_eq!(
            RegistrationConfidenceParameters::new(2, 0.0, 3, 1, 0.1, 0.2, 0.1, 0.1, false, false,),
            Err(RegistrationConfidenceError::InvalidInlierRatio)
        );
        assert_eq!(
            RegistrationConfidenceParameters::new(2, 0.5, 3, 1, 0.2, 0.1, 0.1, 0.1, false, false,),
            Err(RegistrationConfidenceError::InvalidMaximumResidual)
        );
    }
}
