use std::cmp::Ordering;
use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_core::CompensatedSum;
use aether_review::FrameId;

use crate::{
    AffineTransform, CoordinateError, DescriptorMatchCatalog, DescriptorMatchHypothesis,
    FeatureCatalog, FeaturePair, ImagePoint, RegistrationMatch, ResidualError, ResidualStatistics,
    evaluate_residuals,
};

/// Versioned deterministic similarity-consensus policy.
pub const SIMILARITY_CONSENSUS_ALGORITHM_ID: &str = "triangle-similarity-consensus-v1";

/// Hard bound on triangle-seeded transform models evaluated per frame pair.
pub const MAX_CONSENSUS_MODELS: usize = 100_000;

/// Hard bound on source-point residual evaluations during consensus search.
pub const MAX_CONSENSUS_RESIDUAL_EVALUATIONS: usize = 100_000_000;

/// Validated inlier requirements and deterministic work limits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SimilarityConsensusParameters {
    maximum_residual_pixels: f64,
    minimum_inlier_hypotheses: usize,
    minimum_inlier_feature_pairs: usize,
    minimum_competing_model_separation_pixels: f64,
    maximum_models: usize,
    maximum_residual_evaluations: usize,
}

impl SimilarityConsensusParameters {
    /// Creates explicit geometric support and resource limits.
    pub fn new(
        maximum_residual_pixels: f64,
        minimum_inlier_hypotheses: usize,
        minimum_inlier_feature_pairs: usize,
        minimum_competing_model_separation_pixels: f64,
        maximum_models: usize,
        maximum_residual_evaluations: usize,
    ) -> Result<Self, SimilarityConsensusError> {
        if !maximum_residual_pixels.is_finite() || maximum_residual_pixels <= 0.0 {
            return Err(SimilarityConsensusError::InvalidResidualThreshold);
        }
        if minimum_inlier_hypotheses < 2 {
            return Err(SimilarityConsensusError::InvalidMinimumHypotheses);
        }
        if minimum_inlier_feature_pairs < 3 {
            return Err(SimilarityConsensusError::InvalidMinimumFeaturePairs);
        }
        if !minimum_competing_model_separation_pixels.is_finite()
            || minimum_competing_model_separation_pixels <= 0.0
        {
            return Err(SimilarityConsensusError::InvalidCompetingModelSeparation);
        }
        if maximum_models == 0 || maximum_models > MAX_CONSENSUS_MODELS {
            return Err(SimilarityConsensusError::InvalidModelLimit {
                maximum: MAX_CONSENSUS_MODELS,
                actual: maximum_models,
            });
        }
        if maximum_residual_evaluations == 0
            || maximum_residual_evaluations > MAX_CONSENSUS_RESIDUAL_EVALUATIONS
        {
            return Err(SimilarityConsensusError::InvalidEvaluationLimit {
                maximum: MAX_CONSENSUS_RESIDUAL_EVALUATIONS,
                actual: maximum_residual_evaluations,
            });
        }
        Ok(Self {
            maximum_residual_pixels,
            minimum_inlier_hypotheses,
            minimum_inlier_feature_pairs,
            minimum_competing_model_separation_pixels,
            maximum_models,
            maximum_residual_evaluations,
        })
    }

    /// Inclusive Euclidean inlier threshold in reference pixels.
    #[must_use]
    pub const fn maximum_residual_pixels(self) -> f64 {
        self.maximum_residual_pixels
    }

    /// Minimum independently matched triangles required after refinement.
    #[must_use]
    pub const fn minimum_inlier_hypotheses(self) -> usize {
        self.minimum_inlier_hypotheses
    }

    /// Minimum distinct star correspondences required after refinement.
    #[must_use]
    pub const fn minimum_inlier_feature_pairs(self) -> usize {
        self.minimum_inlier_feature_pairs
    }

    /// Minimum maximum control-point displacement identifying a distinct model.
    #[must_use]
    pub const fn minimum_competing_model_separation_pixels(self) -> f64 {
        self.minimum_competing_model_separation_pixels
    }

    /// Maximum leading descriptor hypotheses allowed to seed models.
    #[must_use]
    pub const fn maximum_models(self) -> usize {
        self.maximum_models
    }

    /// Maximum individual point residuals evaluated during model search.
    #[must_use]
    pub const fn maximum_residual_evaluations(self) -> usize {
        self.maximum_residual_evaluations
    }
}

/// Bounded-work and support evidence for one consensus result.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SimilarityConsensusStatistics {
    candidate_hypotheses: usize,
    models_evaluated: usize,
    residual_evaluations: usize,
    models_discarded_by_limit: usize,
    seed_inlier_hypotheses: usize,
    refined_inlier_hypotheses: usize,
    outlier_hypotheses: usize,
    distinct_competing_models: usize,
    selected_orientation_hypotheses: usize,
}

impl SimilarityConsensusStatistics {
    /// Descriptor hypotheses available before the model bound.
    #[must_use]
    pub const fn candidate_hypotheses(self) -> usize {
        self.candidate_hypotheses
    }

    /// Triangle-seeded models scored.
    #[must_use]
    pub const fn models_evaluated(self) -> usize {
        self.models_evaluated
    }

    /// Individual transformed-point residuals evaluated.
    #[must_use]
    pub const fn residual_evaluations(self) -> usize {
        self.residual_evaluations
    }

    /// Candidate seeds not evaluated because of the model limit.
    #[must_use]
    pub const fn models_discarded_by_limit(self) -> usize {
        self.models_discarded_by_limit
    }

    /// Triangle support of the winning seed before least-squares refinement.
    #[must_use]
    pub const fn seed_inlier_hypotheses(self) -> usize {
        self.seed_inlier_hypotheses
    }

    /// Triangle support after fitting all distinct inlier feature pairs.
    #[must_use]
    pub const fn refined_inlier_hypotheses(self) -> usize {
        self.refined_inlier_hypotheses
    }

    /// Retained descriptor hypotheses rejected by the refined transform.
    #[must_use]
    pub const fn outlier_hypotheses(self) -> usize {
        self.outlier_hypotheses
    }

    /// Evaluated seed models geometrically distinct from the winner.
    #[must_use]
    pub const fn distinct_competing_models(self) -> usize {
        self.distinct_competing_models
    }

    /// Retained hypotheses eligible to vote for the selected mirror state.
    #[must_use]
    pub const fn selected_orientation_hypotheses(self) -> usize {
        self.selected_orientation_hypotheses
    }
}

/// Best evaluated transform that is geometrically distinct from the winner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompetingSimilarity {
    seed_hypothesis_index: usize,
    reflected: bool,
    inlier_hypotheses: usize,
    root_mean_square_pixels: f64,
    maximum_separation_pixels: f64,
}

impl CompetingSimilarity {
    /// Descriptor hypothesis that seeded this competing model.
    #[must_use]
    pub const fn seed_hypothesis_index(self) -> usize {
        self.seed_hypothesis_index
    }

    /// Whether the competing model reverses orientation.
    #[must_use]
    pub const fn reflected(self) -> bool {
        self.reflected
    }

    /// Triangle support of the competing model.
    #[must_use]
    pub const fn inlier_hypotheses(self) -> usize {
        self.inlier_hypotheses
    }

    /// RMS over all triangle observations supporting the competing seed.
    #[must_use]
    pub const fn root_mean_square_pixels(self) -> f64 {
        self.root_mean_square_pixels
    }

    /// Largest winner/competitor displacement across source control points.
    #[must_use]
    pub const fn maximum_separation_pixels(self) -> f64 {
        self.maximum_separation_pixels
    }
}

/// One inspectable source-to-reference similarity selected by robust consensus.
#[derive(Clone, Debug, PartialEq)]
pub struct SimilarityConsensus {
    source_frame_id: FrameId,
    reference_frame_id: FrameId,
    parameters: SimilarityConsensusParameters,
    transform: AffineTransform,
    reflected: bool,
    scale: f64,
    rotation_radians: f64,
    seed_hypothesis_index: usize,
    inlier_hypothesis_indices: Vec<usize>,
    inlier_feature_pairs: Vec<FeaturePair>,
    residual_statistics: ResidualStatistics,
    competing_similarity: Option<CompetingSimilarity>,
    statistics: SimilarityConsensusStatistics,
    source_descriptor_catalog_truncated: bool,
    reference_descriptor_catalog_truncated: bool,
}

impl SimilarityConsensus {
    /// Versioned fitting and consensus policy.
    #[must_use]
    pub const fn algorithm_id(&self) -> &'static str {
        SIMILARITY_CONSENSUS_ALGORITHM_ID
    }

    /// Source frame mapped by the result.
    #[must_use]
    pub const fn source_frame_id(&self) -> &FrameId {
        &self.source_frame_id
    }

    /// Fixed reference frame.
    #[must_use]
    pub const fn reference_frame_id(&self) -> &FrameId {
        &self.reference_frame_id
    }

    /// Exact threshold and bounds used by consensus.
    #[must_use]
    pub const fn parameters(&self) -> SimilarityConsensusParameters {
        self.parameters
    }

    /// Refined source-to-reference similarity represented as an affine map.
    #[must_use]
    pub const fn transform(&self) -> AffineTransform {
        self.transform
    }

    /// Whether the fitted linear map reverses orientation.
    #[must_use]
    pub const fn reflected(&self) -> bool {
        self.reflected
    }

    /// Uniform reference/source scale of the refined map.
    #[must_use]
    pub const fn scale(&self) -> f64 {
        self.scale
    }

    /// Rotation component in radians under the documented reflection convention.
    #[must_use]
    pub const fn rotation_radians(&self) -> f64 {
        self.rotation_radians
    }

    /// Index of the descriptor hypothesis that seeded the winning model.
    #[must_use]
    pub const fn seed_hypothesis_index(&self) -> usize {
        self.seed_hypothesis_index
    }

    /// Descriptor hypotheses supported by the refined transform.
    #[must_use]
    pub fn inlier_hypothesis_indices(&self) -> &[usize] {
        &self.inlier_hypothesis_indices
    }

    /// Sorted distinct star correspondences supporting the refined transform.
    #[must_use]
    pub fn inlier_feature_pairs(&self) -> &[FeaturePair] {
        &self.inlier_feature_pairs
    }

    /// Resolves the retained rank pairs to their measured source/reference points.
    ///
    /// Catalog identities are revalidated before any rank is dereferenced. This
    /// makes the returned correspondences safe to pass to independent model-fit
    /// diagnostics without duplicating rank-resolution logic in callers.
    pub fn resolve_inlier_matches(
        &self,
        source_features: &FeatureCatalog,
        reference_features: &FeatureCatalog,
    ) -> Result<Vec<RegistrationMatch>, SimilarityConsensusError> {
        if self.source_frame_id() != source_features.frame_id() {
            return Err(SimilarityConsensusError::SourceFrameMismatch);
        }
        if self.reference_frame_id() != reference_features.frame_id() {
            return Err(SimilarityConsensusError::ReferenceFrameMismatch);
        }
        resolve_feature_pairs(
            source_features,
            reference_features,
            self.inlier_feature_pairs(),
        )
    }

    /// Strict residual summary over distinct inlier star correspondences.
    #[must_use]
    pub const fn residual_statistics(&self) -> ResidualStatistics {
        self.residual_statistics
    }

    /// Best evaluated model distinct from the winner, when one exists.
    #[must_use]
    pub const fn competing_similarity(&self) -> Option<CompetingSimilarity> {
        self.competing_similarity
    }

    /// Complete search, refinement, and outlier accounting.
    #[must_use]
    pub const fn statistics(&self) -> SimilarityConsensusStatistics {
        self.statistics
    }

    /// Whether source descriptor generation reached its output limit.
    #[must_use]
    pub const fn source_descriptor_catalog_truncated(&self) -> bool {
        self.source_descriptor_catalog_truncated
    }

    /// Whether reference descriptor generation reached its output limit.
    #[must_use]
    pub const fn reference_descriptor_catalog_truncated(&self) -> bool {
        self.reference_descriptor_catalog_truncated
    }
}

#[derive(Clone, Copy, Debug)]
struct ModelScore {
    transform: AffineTransform,
    reflected: bool,
    seed_hypothesis_index: usize,
    inlier_hypotheses: usize,
    root_mean_square_pixels: f64,
    descriptor_distance: f64,
}

/// Selects and refines a similarity only when multiple triangle hypotheses agree.
pub fn estimate_similarity_consensus(
    source_features: &FeatureCatalog,
    reference_features: &FeatureCatalog,
    matches: &DescriptorMatchCatalog,
    parameters: SimilarityConsensusParameters,
) -> Result<SimilarityConsensus, SimilarityConsensusError> {
    let parameters = SimilarityConsensusParameters::new(
        parameters.maximum_residual_pixels,
        parameters.minimum_inlier_hypotheses,
        parameters.minimum_inlier_feature_pairs,
        parameters.minimum_competing_model_separation_pixels,
        parameters.maximum_models,
        parameters.maximum_residual_evaluations,
    )?;
    validate_catalog_identities(source_features, reference_features, matches)?;
    if matches.hypotheses().is_empty() {
        return Err(SimilarityConsensusError::NoHypotheses);
    }

    let hypotheses = matches.hypotheses();
    let model_count = hypotheses.len().min(parameters.maximum_models);
    let mut statistics = SimilarityConsensusStatistics {
        candidate_hypotheses: hypotheses.len(),
        models_discarded_by_limit: hypotheses.len() - model_count,
        ..SimilarityConsensusStatistics::default()
    };
    let mut scores = Vec::new();
    scores
        .try_reserve_exact(model_count)
        .map_err(|_| SimilarityConsensusError::AllocationFailed)?;

    for (seed_index, seed) in hypotheses.iter().copied().take(model_count).enumerate() {
        let seed_pairs =
            resolved_matches(source_features, reference_features, seed.feature_pairs())?;
        let transform = fit_similarity(&seed_pairs, seed.reflected())?;
        let score = score_model(
            transform,
            seed.reflected(),
            seed_index,
            seed.normalized_distance(),
            source_features,
            reference_features,
            hypotheses,
            parameters,
            &mut statistics,
        )?;
        statistics.models_evaluated = checked_increment(statistics.models_evaluated)?;
        scores.push(score);
    }

    let seed = scores
        .iter()
        .copied()
        .min_by(compare_model_scores)
        .ok_or(SimilarityConsensusError::NoUsableModel)?;
    statistics.selected_orientation_hypotheses = hypotheses
        .iter()
        .filter(|hypothesis| hypothesis.reflected() == seed.reflected)
        .count();
    statistics.seed_inlier_hypotheses = seed.inlier_hypotheses;
    if seed.inlier_hypotheses < parameters.minimum_inlier_hypotheses {
        return Err(SimilarityConsensusError::InsufficientHypothesisConsensus {
            required: parameters.minimum_inlier_hypotheses,
            actual: seed.inlier_hypotheses,
        });
    }

    let (_, seed_pairs) = collect_inliers(
        seed.transform,
        seed.reflected,
        source_features,
        reference_features,
        hypotheses,
        parameters,
        &mut statistics,
    )?;
    if seed_pairs.len() < parameters.minimum_inlier_feature_pairs {
        return Err(SimilarityConsensusError::InsufficientFeatureConsensus {
            required: parameters.minimum_inlier_feature_pairs,
            actual: seed_pairs.len(),
        });
    }
    validate_one_to_one_pairs(source_features, reference_features, &seed_pairs)?;
    let resolved_seed_pairs =
        resolve_feature_pairs(source_features, reference_features, &seed_pairs)?;
    let refined_transform = fit_similarity(&resolved_seed_pairs, seed.reflected)?;
    let (inlier_hypothesis_indices, inlier_feature_pairs) = collect_inliers(
        refined_transform,
        seed.reflected,
        source_features,
        reference_features,
        hypotheses,
        parameters,
        &mut statistics,
    )?;
    statistics.refined_inlier_hypotheses = inlier_hypothesis_indices.len();
    statistics.outlier_hypotheses = hypotheses.len() - inlier_hypothesis_indices.len();
    if inlier_hypothesis_indices.len() < parameters.minimum_inlier_hypotheses {
        return Err(SimilarityConsensusError::InsufficientHypothesisConsensus {
            required: parameters.minimum_inlier_hypotheses,
            actual: inlier_hypothesis_indices.len(),
        });
    }
    if inlier_feature_pairs.len() < parameters.minimum_inlier_feature_pairs {
        return Err(SimilarityConsensusError::InsufficientFeatureConsensus {
            required: parameters.minimum_inlier_feature_pairs,
            actual: inlier_feature_pairs.len(),
        });
    }
    validate_one_to_one_pairs(source_features, reference_features, &inlier_feature_pairs)?;

    let resolved_inlier_pairs =
        resolve_feature_pairs(source_features, reference_features, &inlier_feature_pairs)?;
    let residual_statistics = evaluate_residuals(refined_transform, &resolved_inlier_pairs)
        .map_err(SimilarityConsensusError::Residual)?;
    let [m00, _, m10, _, _, _] = refined_transform.coefficients();
    let scale = m00.hypot(m10);
    let rotation_radians = m10.atan2(m00);
    if !scale.is_finite() || scale <= 0.0 || !rotation_radians.is_finite() {
        return Err(SimilarityConsensusError::NumericalOverflow);
    }

    let mut competing: Option<(ModelScore, f64)> = None;
    for score in scores.iter().copied() {
        if score.seed_hypothesis_index == seed.seed_hypothesis_index {
            continue;
        }
        let separation = maximum_transform_separation(
            refined_transform,
            score.transform,
            source_features.width(),
            source_features.height(),
        )?;
        if separation < parameters.minimum_competing_model_separation_pixels {
            continue;
        }
        statistics.distinct_competing_models =
            checked_increment(statistics.distinct_competing_models)?;
        if competing
            .as_ref()
            .is_none_or(|(current, _)| compare_model_scores(&score, current).is_lt())
        {
            competing = Some((score, separation));
        }
    }
    let competing_similarity =
        competing.map(|(model, maximum_separation_pixels)| CompetingSimilarity {
            seed_hypothesis_index: model.seed_hypothesis_index,
            reflected: model.reflected,
            inlier_hypotheses: model.inlier_hypotheses,
            root_mean_square_pixels: model.root_mean_square_pixels,
            maximum_separation_pixels,
        });

    Ok(SimilarityConsensus {
        source_frame_id: source_features.frame_id().clone(),
        reference_frame_id: reference_features.frame_id().clone(),
        parameters,
        transform: refined_transform,
        reflected: seed.reflected,
        scale,
        rotation_radians,
        seed_hypothesis_index: seed.seed_hypothesis_index,
        inlier_hypothesis_indices,
        inlier_feature_pairs,
        residual_statistics,
        competing_similarity,
        statistics,
        source_descriptor_catalog_truncated: matches.source_descriptor_catalog_truncated(),
        reference_descriptor_catalog_truncated: matches.reference_descriptor_catalog_truncated(),
    })
}

fn maximum_transform_separation(
    first: AffineTransform,
    second: AffineTransform,
    width: usize,
    height: usize,
) -> Result<f64, SimilarityConsensusError> {
    let maximum_x = width.saturating_sub(1) as f64;
    let maximum_y = height.saturating_sub(1) as f64;
    let points = [
        ImagePoint::new(0.0, 0.0).map_err(SimilarityConsensusError::Coordinate)?,
        ImagePoint::new(maximum_x, 0.0).map_err(SimilarityConsensusError::Coordinate)?,
        ImagePoint::new(0.0, maximum_y).map_err(SimilarityConsensusError::Coordinate)?,
        ImagePoint::new(maximum_x, maximum_y).map_err(SimilarityConsensusError::Coordinate)?,
        ImagePoint::new(maximum_x * 0.5, maximum_y * 0.5)
            .map_err(SimilarityConsensusError::Coordinate)?,
    ];
    let mut maximum = 0.0_f64;
    for point in points {
        let first_point = first
            .apply(point)
            .map_err(SimilarityConsensusError::Coordinate)?;
        let second_point = second
            .apply(point)
            .map_err(SimilarityConsensusError::Coordinate)?;
        let separation =
            (first_point.x() - second_point.x()).hypot(first_point.y() - second_point.y());
        if !separation.is_finite() {
            return Err(SimilarityConsensusError::NumericalOverflow);
        }
        maximum = maximum.max(separation);
    }
    Ok(maximum)
}

fn validate_catalog_identities(
    source: &FeatureCatalog,
    reference: &FeatureCatalog,
    matches: &DescriptorMatchCatalog,
) -> Result<(), SimilarityConsensusError> {
    if source.frame_id() != matches.source_frame_id() {
        return Err(SimilarityConsensusError::SourceFrameMismatch);
    }
    if reference.frame_id() != matches.reference_frame_id() {
        return Err(SimilarityConsensusError::ReferenceFrameMismatch);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn score_model(
    transform: AffineTransform,
    reflected: bool,
    seed_hypothesis_index: usize,
    descriptor_distance: f64,
    source_features: &FeatureCatalog,
    reference_features: &FeatureCatalog,
    hypotheses: &[DescriptorMatchHypothesis],
    parameters: SimilarityConsensusParameters,
    statistics: &mut SimilarityConsensusStatistics,
) -> Result<ModelScore, SimilarityConsensusError> {
    let mut squared_sum = CompensatedSum::new();
    let mut inlier_hypotheses = 0_usize;
    let mut inlier_points = 0_usize;
    for hypothesis in hypotheses {
        if hypothesis.reflected() != reflected {
            continue;
        }
        let pairs = resolved_matches(
            source_features,
            reference_features,
            hypothesis.feature_pairs(),
        )?;
        let mut squared = [0.0; 3];
        let mut is_inlier = true;
        for (index, correspondence) in pairs.iter().enumerate() {
            statistics.residual_evaluations = checked_increment(statistics.residual_evaluations)?;
            if statistics.residual_evaluations > parameters.maximum_residual_evaluations {
                return Err(SimilarityConsensusError::EvaluationLimitExceeded {
                    maximum: parameters.maximum_residual_evaluations,
                });
            }
            let residual = residual_pixels(transform, *correspondence)?;
            squared[index] = residual * residual;
            if residual > parameters.maximum_residual_pixels {
                is_inlier = false;
                break;
            }
        }
        if is_inlier {
            inlier_hypotheses = checked_increment(inlier_hypotheses)?;
            inlier_points = inlier_points
                .checked_add(3)
                .ok_or(SimilarityConsensusError::CountOverflow)?;
            for value in squared {
                squared_sum.add(value);
            }
        }
    }
    let root_mean_square_pixels = if inlier_points == 0 {
        f64::INFINITY
    } else {
        (squared_sum.total() / inlier_points as f64).sqrt()
    };
    if root_mean_square_pixels.is_nan() {
        return Err(SimilarityConsensusError::NumericalOverflow);
    }
    Ok(ModelScore {
        transform,
        reflected,
        seed_hypothesis_index,
        inlier_hypotheses,
        root_mean_square_pixels,
        descriptor_distance,
    })
}

fn compare_model_scores(left: &ModelScore, right: &ModelScore) -> Ordering {
    right
        .inlier_hypotheses
        .cmp(&left.inlier_hypotheses)
        .then_with(|| {
            left.root_mean_square_pixels
                .total_cmp(&right.root_mean_square_pixels)
        })
        .then_with(|| {
            left.descriptor_distance
                .total_cmp(&right.descriptor_distance)
        })
        .then_with(|| left.reflected.cmp(&right.reflected))
        .then_with(|| left.seed_hypothesis_index.cmp(&right.seed_hypothesis_index))
}

fn collect_inliers(
    transform: AffineTransform,
    reflected: bool,
    source_features: &FeatureCatalog,
    reference_features: &FeatureCatalog,
    hypotheses: &[DescriptorMatchHypothesis],
    parameters: SimilarityConsensusParameters,
    statistics: &mut SimilarityConsensusStatistics,
) -> Result<(Vec<usize>, Vec<FeaturePair>), SimilarityConsensusError> {
    let mut hypothesis_indices = Vec::new();
    hypothesis_indices
        .try_reserve_exact(hypotheses.len())
        .map_err(|_| SimilarityConsensusError::AllocationFailed)?;
    let pair_capacity = hypotheses
        .len()
        .checked_mul(3)
        .ok_or(SimilarityConsensusError::CountOverflow)?;
    let mut feature_pairs = Vec::new();
    feature_pairs
        .try_reserve_exact(pair_capacity)
        .map_err(|_| SimilarityConsensusError::AllocationFailed)?;
    for (hypothesis_index, hypothesis) in hypotheses.iter().enumerate() {
        if hypothesis.reflected() != reflected {
            continue;
        }
        let pairs = resolved_matches(
            source_features,
            reference_features,
            hypothesis.feature_pairs(),
        )?;
        let mut is_inlier = true;
        for correspondence in pairs {
            statistics.residual_evaluations = checked_increment(statistics.residual_evaluations)?;
            if statistics.residual_evaluations > parameters.maximum_residual_evaluations {
                return Err(SimilarityConsensusError::EvaluationLimitExceeded {
                    maximum: parameters.maximum_residual_evaluations,
                });
            }
            if residual_pixels(transform, correspondence)? > parameters.maximum_residual_pixels {
                is_inlier = false;
                break;
            }
        }
        if is_inlier {
            hypothesis_indices.push(hypothesis_index);
            feature_pairs.extend(hypothesis.feature_pairs());
        }
    }
    let feature_pairs = select_bijective_pairs(
        transform,
        source_features,
        reference_features,
        feature_pairs,
        parameters,
        statistics,
    )?;
    Ok((hypothesis_indices, feature_pairs))
}

#[derive(Clone, Copy, Debug)]
struct SupportedFeaturePair {
    pair: FeaturePair,
    triangle_support: usize,
    residual_pixels: f64,
}

/// Resolves collisions between individually plausible triangle correspondences.
///
/// Dense or partly symmetric star fields can place two different references
/// within the residual radius of one source. Treating that normal ambiguity as
/// a fatal error makes real-field consensus brittle. Candidates are therefore
/// ranked by independent triangle support, then geometric residual and stable
/// feature ranks, before a deterministic one-to-one assignment is extracted.
fn select_bijective_pairs(
    transform: AffineTransform,
    source_features: &FeatureCatalog,
    reference_features: &FeatureCatalog,
    mut pairs: Vec<FeaturePair>,
    parameters: SimilarityConsensusParameters,
    statistics: &mut SimilarityConsensusStatistics,
) -> Result<Vec<FeaturePair>, SimilarityConsensusError> {
    pairs.sort_unstable_by_key(|pair| (pair.source_rank(), pair.reference_rank()));
    let mut candidates = Vec::new();
    candidates
        .try_reserve_exact(pairs.len())
        .map_err(|_| SimilarityConsensusError::AllocationFailed)?;
    let mut start = 0_usize;
    while start < pairs.len() {
        let pair = pairs[start];
        let mut end = start + 1;
        while end < pairs.len() && pairs[end] == pair {
            end = checked_increment(end)?;
        }
        statistics.residual_evaluations = checked_increment(statistics.residual_evaluations)?;
        if statistics.residual_evaluations > parameters.maximum_residual_evaluations {
            return Err(SimilarityConsensusError::EvaluationLimitExceeded {
                maximum: parameters.maximum_residual_evaluations,
            });
        }
        let correspondence = resolve_feature_pair(source_features, reference_features, pair)?;
        candidates.push(SupportedFeaturePair {
            pair,
            triangle_support: end - start,
            residual_pixels: residual_pixels(transform, correspondence)?,
        });
        start = end;
    }
    candidates.sort_by(|left, right| {
        right
            .triangle_support
            .cmp(&left.triangle_support)
            .then_with(|| left.residual_pixels.total_cmp(&right.residual_pixels))
            .then_with(|| left.pair.source_rank().cmp(&right.pair.source_rank()))
            .then_with(|| left.pair.reference_rank().cmp(&right.pair.reference_rank()))
    });

    let mut source_assigned = Vec::new();
    source_assigned
        .try_reserve_exact(source_features.features().len())
        .map_err(|_| SimilarityConsensusError::AllocationFailed)?;
    source_assigned.resize(source_features.features().len(), false);
    let mut reference_assigned = Vec::new();
    reference_assigned
        .try_reserve_exact(reference_features.features().len())
        .map_err(|_| SimilarityConsensusError::AllocationFailed)?;
    reference_assigned.resize(reference_features.features().len(), false);
    let mut selected = Vec::new();
    selected
        .try_reserve_exact(candidates.len())
        .map_err(|_| SimilarityConsensusError::AllocationFailed)?;
    for candidate in candidates {
        let Some(source_slot) = source_assigned.get_mut(candidate.pair.source_rank()) else {
            return Err(SimilarityConsensusError::MissingSourceRank {
                rank: candidate.pair.source_rank(),
            });
        };
        let Some(reference_slot) = reference_assigned.get_mut(candidate.pair.reference_rank())
        else {
            return Err(SimilarityConsensusError::MissingReferenceRank {
                rank: candidate.pair.reference_rank(),
            });
        };
        if *source_slot || *reference_slot {
            continue;
        }
        *source_slot = true;
        *reference_slot = true;
        selected.push(candidate.pair);
    }
    selected.sort_unstable_by_key(|pair| (pair.source_rank(), pair.reference_rank()));
    Ok(selected)
}

fn resolved_matches(
    source_features: &FeatureCatalog,
    reference_features: &FeatureCatalog,
    pairs: [FeaturePair; 3],
) -> Result<[RegistrationMatch; 3], SimilarityConsensusError> {
    Ok([
        resolve_feature_pair(source_features, reference_features, pairs[0])?,
        resolve_feature_pair(source_features, reference_features, pairs[1])?,
        resolve_feature_pair(source_features, reference_features, pairs[2])?,
    ])
}

fn resolve_feature_pairs(
    source_features: &FeatureCatalog,
    reference_features: &FeatureCatalog,
    pairs: &[FeaturePair],
) -> Result<Vec<RegistrationMatch>, SimilarityConsensusError> {
    let mut resolved = Vec::new();
    resolved
        .try_reserve_exact(pairs.len())
        .map_err(|_| SimilarityConsensusError::AllocationFailed)?;
    for &pair in pairs {
        resolved.push(resolve_feature_pair(
            source_features,
            reference_features,
            pair,
        )?);
    }
    Ok(resolved)
}

fn validate_one_to_one_pairs(
    source_features: &FeatureCatalog,
    reference_features: &FeatureCatalog,
    pairs: &[FeaturePair],
) -> Result<(), SimilarityConsensusError> {
    let mut source_assignments = Vec::new();
    source_assignments
        .try_reserve_exact(source_features.features().len())
        .map_err(|_| SimilarityConsensusError::AllocationFailed)?;
    source_assignments.resize(source_features.features().len(), None);
    let mut reference_assignments = Vec::new();
    reference_assignments
        .try_reserve_exact(reference_features.features().len())
        .map_err(|_| SimilarityConsensusError::AllocationFailed)?;
    reference_assignments.resize(reference_features.features().len(), None);

    for &pair in pairs {
        let source_assignment = source_assignments.get_mut(pair.source_rank()).ok_or(
            SimilarityConsensusError::MissingSourceRank {
                rank: pair.source_rank(),
            },
        )?;
        if source_assignment.is_some_and(|rank| rank != pair.reference_rank()) {
            return Err(SimilarityConsensusError::AmbiguousFeatureMapping);
        }
        *source_assignment = Some(pair.reference_rank());

        let reference_assignment = reference_assignments.get_mut(pair.reference_rank()).ok_or(
            SimilarityConsensusError::MissingReferenceRank {
                rank: pair.reference_rank(),
            },
        )?;
        if reference_assignment.is_some_and(|rank| rank != pair.source_rank()) {
            return Err(SimilarityConsensusError::AmbiguousFeatureMapping);
        }
        *reference_assignment = Some(pair.source_rank());
    }
    Ok(())
}

fn resolve_feature_pair(
    source_features: &FeatureCatalog,
    reference_features: &FeatureCatalog,
    pair: FeaturePair,
) -> Result<RegistrationMatch, SimilarityConsensusError> {
    let source = source_features
        .features()
        .get(pair.source_rank())
        .filter(|feature| feature.rank() == pair.source_rank())
        .ok_or(SimilarityConsensusError::MissingSourceRank {
            rank: pair.source_rank(),
        })?;
    let reference = reference_features
        .features()
        .get(pair.reference_rank())
        .filter(|feature| feature.rank() == pair.reference_rank())
        .ok_or(SimilarityConsensusError::MissingReferenceRank {
            rank: pair.reference_rank(),
        })?;
    Ok(RegistrationMatch::new(source.point(), reference.point()))
}

pub(crate) fn fit_similarity(
    matches: &[RegistrationMatch],
    reflected: bool,
) -> Result<AffineTransform, SimilarityConsensusError> {
    if matches.len() < 2 {
        return Err(SimilarityConsensusError::DegenerateModel);
    }
    let count = matches.len() as f64;
    let mut source_x = CompensatedSum::new();
    let mut source_y = CompensatedSum::new();
    let mut reference_x = CompensatedSum::new();
    let mut reference_y = CompensatedSum::new();
    for correspondence in matches {
        source_x.add(correspondence.source().x());
        source_y.add(correspondence.source().y());
        reference_x.add(correspondence.reference().x());
        reference_y.add(correspondence.reference().y());
    }
    let source_center = (source_x.total() / count, source_y.total() / count);
    let reference_center = (reference_x.total() / count, reference_y.total() / count);
    if [
        source_center.0,
        source_center.1,
        reference_center.0,
        reference_center.1,
    ]
    .iter()
    .any(|value| !value.is_finite())
    {
        return Err(SimilarityConsensusError::NumericalOverflow);
    }

    let mut denominator = CompensatedSum::new();
    let mut first = CompensatedSum::new();
    let mut second = CompensatedSum::new();
    for correspondence in matches {
        let x = correspondence.source().x() - source_center.0;
        let y = correspondence.source().y() - source_center.1;
        let u = correspondence.reference().x() - reference_center.0;
        let v = correspondence.reference().y() - reference_center.1;
        denominator.add(x.mul_add(x, y * y));
        if reflected {
            first.add(x.mul_add(u, -(y * v)));
            second.add(y.mul_add(u, x * v));
        } else {
            first.add(x.mul_add(u, y * v));
            second.add(x.mul_add(v, -(y * u)));
        }
    }
    let denominator = denominator.total();
    if !denominator.is_finite() || denominator <= 0.0 {
        return Err(SimilarityConsensusError::DegenerateModel);
    }
    let a = first.total() / denominator;
    let b = second.total() / denominator;
    let scale = a.hypot(b);
    if !a.is_finite() || !b.is_finite() || !scale.is_finite() || scale == 0.0 {
        return Err(SimilarityConsensusError::DegenerateModel);
    }
    let (m00, m01, m10, m11) = if reflected {
        (a, b, b, -a)
    } else {
        (a, -b, b, a)
    };
    let tx = reference_center.0 - m00.mul_add(source_center.0, m01 * source_center.1);
    let ty = reference_center.1 - m10.mul_add(source_center.0, m11 * source_center.1);
    AffineTransform::new(m00, m01, m10, m11, tx, ty).map_err(SimilarityConsensusError::Coordinate)
}

fn residual_pixels(
    transform: AffineTransform,
    correspondence: RegistrationMatch,
) -> Result<f64, SimilarityConsensusError> {
    let mapped = transform
        .apply(correspondence.source())
        .map_err(SimilarityConsensusError::Coordinate)?;
    let residual = (mapped.x() - correspondence.reference().x())
        .hypot(mapped.y() - correspondence.reference().y());
    if !residual.is_finite() {
        return Err(SimilarityConsensusError::NumericalOverflow);
    }
    Ok(residual)
}

fn checked_increment(value: usize) -> Result<usize, SimilarityConsensusError> {
    value
        .checked_add(1)
        .ok_or(SimilarityConsensusError::CountOverflow)
}

/// Invalid controls, incoherent catalogs, bounded-work failure, or weak consensus.
#[derive(Debug)]
pub enum SimilarityConsensusError {
    /// The inlier radius was non-finite or non-positive.
    InvalidResidualThreshold,
    /// At least two agreeing triangle hypotheses are required.
    InvalidMinimumHypotheses,
    /// At least three distinct feature pairs are required.
    InvalidMinimumFeaturePairs,
    /// Distinct transform threshold must be finite and positive.
    InvalidCompetingModelSeparation,
    /// The model limit was outside the supported range.
    InvalidModelLimit {
        /// Hard maximum.
        maximum: usize,
        /// Requested value.
        actual: usize,
    },
    /// The point-evaluation limit was outside the supported range.
    InvalidEvaluationLimit {
        /// Hard maximum.
        maximum: usize,
        /// Requested value.
        actual: usize,
    },
    /// Feature and match source identities differ.
    SourceFrameMismatch,
    /// Feature and match reference identities differ.
    ReferenceFrameMismatch,
    /// No descriptor hypothesis was available.
    NoHypotheses,
    /// No finite nonsingular candidate model could be formed.
    NoUsableModel,
    /// A source rank referenced by matching was not present in the feature catalog.
    MissingSourceRank {
        /// Missing zero-based rank.
        rank: usize,
    },
    /// A reference rank referenced by matching was not present in the feature catalog.
    MissingReferenceRank {
        /// Missing zero-based rank.
        rank: usize,
    },
    /// The best model did not retain enough triangle support.
    InsufficientHypothesisConsensus {
        /// Configured minimum.
        required: usize,
        /// Support retained by the best or refined model.
        actual: usize,
    },
    /// The best model did not retain enough distinct feature correspondences.
    InsufficientFeatureConsensus {
        /// Configured minimum.
        required: usize,
        /// Distinct pairs retained by the best or refined model.
        actual: usize,
    },
    /// Inlier evidence mapped one feature rank to more than one counterpart.
    AmbiguousFeatureMapping,
    /// The configured point-evaluation budget was exceeded.
    EvaluationLimitExceeded {
        /// Configured maximum.
        maximum: usize,
    },
    /// The least-squares source geometry or scale was degenerate.
    DegenerateModel,
    /// Scratch or retained evidence allocation failed.
    AllocationFailed,
    /// Transform construction or application failed.
    Coordinate(CoordinateError),
    /// Strict final residual evaluation failed.
    Residual(ResidualError),
    /// Finite geometry left the representable domain.
    NumericalOverflow,
    /// Evidence arithmetic overflowed.
    CountOverflow,
}

impl Display for SimilarityConsensusError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidResidualThreshold => {
                formatter.write_str("consensus residual threshold is invalid")
            }
            Self::InvalidMinimumHypotheses => {
                formatter.write_str("consensus requires at least two triangle hypotheses")
            }
            Self::InvalidMinimumFeaturePairs => {
                formatter.write_str("consensus requires at least three distinct feature pairs")
            }
            Self::InvalidCompetingModelSeparation => {
                formatter.write_str("competing model separation is invalid")
            }
            Self::InvalidModelLimit { maximum, actual } => write!(
                formatter,
                "consensus accepts 1..={maximum} models, received {actual}"
            ),
            Self::InvalidEvaluationLimit { maximum, actual } => write!(
                formatter,
                "consensus accepts 1..={maximum} residual evaluations, received {actual}"
            ),
            Self::SourceFrameMismatch => {
                formatter.write_str("source feature and match frame identities differ")
            }
            Self::ReferenceFrameMismatch => {
                formatter.write_str("reference feature and match frame identities differ")
            }
            Self::NoHypotheses => {
                formatter.write_str("descriptor match catalog contains no hypotheses")
            }
            Self::NoUsableModel => {
                formatter.write_str("no usable similarity model could be formed")
            }
            Self::MissingSourceRank { rank } => {
                write!(formatter, "source feature rank {rank} is missing")
            }
            Self::MissingReferenceRank { rank } => {
                write!(formatter, "reference feature rank {rank} is missing")
            }
            Self::InsufficientHypothesisConsensus { required, actual } => write!(
                formatter,
                "similarity consensus requires {required} inlier hypotheses, found {actual}"
            ),
            Self::InsufficientFeatureConsensus { required, actual } => write!(
                formatter,
                "similarity consensus requires {required} distinct feature pairs, found {actual}"
            ),
            Self::AmbiguousFeatureMapping => {
                formatter.write_str("similarity consensus contains non-bijective feature mappings")
            }
            Self::EvaluationLimitExceeded { maximum } => write!(
                formatter,
                "similarity consensus exceeded its {maximum}-residual-evaluation bound"
            ),
            Self::DegenerateModel => formatter.write_str("similarity model geometry is degenerate"),
            Self::AllocationFailed => formatter.write_str("similarity consensus allocation failed"),
            Self::Coordinate(error) => write!(formatter, "similarity coordinate failure: {error}"),
            Self::Residual(error) => write!(formatter, "similarity residual failure: {error}"),
            Self::NumericalOverflow => {
                formatter.write_str("similarity consensus geometry overflowed")
            }
            Self::CountOverflow => formatter.write_str("similarity consensus evidence overflowed"),
        }
    }
}

impl Error for SimilarityConsensusError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Coordinate(error) => Some(error),
            Self::Residual(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use aether_review::FrameId;

    use crate::features::feature_catalog_for_tests;
    use crate::{
        DescriptorMatchParameters, ReflectionPolicy, TriangleDescriptorParameters,
        build_triangle_descriptors, match_triangle_descriptors,
    };

    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn StdError>>;

    fn frame_id(digit: char) -> TestResult<FrameId> {
        Ok(FrameId::new(digit.to_string().repeat(64))?)
    }

    fn feature_catalog(digit: char, points: &[(f64, f64)]) -> TestResult<FeatureCatalog> {
        Ok(feature_catalog_for_tests(frame_id(digit)?, points)?)
    }

    fn descriptor_matches(
        source: &FeatureCatalog,
        reference: &FeatureCatalog,
        reflection_policy: ReflectionPolicy,
    ) -> TestResult<DescriptorMatchCatalog> {
        descriptor_matches_with_tolerance(source, reference, reflection_policy, 1.0e-10, 1.0e-10)
    }

    fn descriptor_matches_with_tolerance(
        source: &FeatureCatalog,
        reference: &FeatureCatalog,
        reflection_policy: ReflectionPolicy,
        side_ratio_tolerance: f64,
        area_tolerance: f64,
    ) -> TestResult<DescriptorMatchCatalog> {
        let descriptor_parameters = TriangleDescriptorParameters::new(
            source.features().len(),
            source.features().len().saturating_sub(1).clamp(2, 8),
            0.1,
            1.0e-9,
            1_000,
        )?;
        let source_descriptors = build_triangle_descriptors(source, descriptor_parameters)?;
        let reference_descriptors = build_triangle_descriptors(reference, descriptor_parameters)?;
        let match_parameters = DescriptorMatchParameters::new(
            side_ratio_tolerance,
            area_tolerance,
            0.25,
            4.0,
            reflection_policy,
            32,
            1_000,
            100_000,
        )?;
        Ok(match_triangle_descriptors(
            &source_descriptors,
            &reference_descriptors,
            match_parameters,
        )?)
    }

    fn parameters(
        minimum_inlier_hypotheses: usize,
        maximum_models: usize,
        maximum_evaluations: usize,
    ) -> TestResult<SimilarityConsensusParameters> {
        Ok(SimilarityConsensusParameters::new(
            1.0e-7,
            minimum_inlier_hypotheses,
            3,
            0.25,
            maximum_models,
            maximum_evaluations,
        )?)
    }

    #[test]
    fn recovers_exact_rotation_scale_and_translation_from_multiple_triangles() -> TestResult {
        let source_points = [
            (2.0, 3.0),
            (11.0, 5.0),
            (4.0, 17.0),
            (19.0, 13.0),
            (27.0, 7.0),
            (23.0, 24.0),
        ];
        let reference_points = source_points.map(|(x, y)| (30.0 - 2.0 * y, 10.0 + 2.0 * x));
        let source = feature_catalog('a', &source_points)?;
        let reference = feature_catalog('b', &reference_points)?;
        let descriptor_matches = descriptor_matches(&source, &reference, ReflectionPolicy::Forbid)?;

        let consensus = estimate_similarity_consensus(
            &source,
            &reference,
            &descriptor_matches,
            parameters(2, 1_000, 1_000_000)?,
        )?;

        assert_eq!(consensus.algorithm_id(), SIMILARITY_CONSENSUS_ALGORITHM_ID);
        assert_eq!(consensus.source_frame_id(), source.frame_id());
        assert_eq!(consensus.reference_frame_id(), reference.frame_id());
        assert!(!consensus.reflected());
        assert!((consensus.scale() - 2.0).abs() < 1.0e-12);
        assert!((consensus.rotation_radians() - std::f64::consts::FRAC_PI_2).abs() < 1.0e-12);
        let coefficients = consensus.transform().coefficients();
        let expected = [0.0, -2.0, 2.0, 0.0, 30.0, 10.0];
        assert!(
            coefficients
                .iter()
                .zip(expected)
                .all(|(actual, expected)| (actual - expected).abs() < 1.0e-11)
        );
        assert!(consensus.inlier_hypothesis_indices().len() >= 2);
        assert_eq!(consensus.inlier_feature_pairs().len(), source_points.len());
        let resolved = consensus.resolve_inlier_matches(&source, &reference)?;
        assert_eq!(resolved.len(), source_points.len());
        for correspondence in &resolved {
            let expected = consensus.transform().apply(correspondence.source())?;
            assert!(
                (expected.x() - correspondence.reference().x()).abs() < 1.0e-11
                    && (expected.y() - correspondence.reference().y()).abs() < 1.0e-11
            );
        }
        assert!(matches!(
            consensus.resolve_inlier_matches(&reference, &reference),
            Err(SimilarityConsensusError::SourceFrameMismatch)
        ));
        assert!(matches!(
            consensus.resolve_inlier_matches(&source, &source),
            Err(SimilarityConsensusError::ReferenceFrameMismatch)
        ));
        assert!(consensus.residual_statistics().maximum_pixels() < 1.0e-11);
        assert_eq!(consensus.statistics().outlier_hypotheses(), 0);
        Ok(())
    }

    #[test]
    fn recovers_noisy_subpixel_ground_truth_without_exact_descriptor_equality() -> TestResult {
        let source_points = [
            (4.0, 8.0),
            (18.0, 3.0),
            (9.0, 31.0),
            (33.0, 24.0),
            (48.0, 11.0),
            (57.0, 39.0),
            (22.0, 52.0),
            (69.0, 58.0),
        ];
        let noise = [
            (0.04, -0.03),
            (-0.06, 0.02),
            (0.03, 0.07),
            (-0.02, -0.05),
            (0.08, 0.01),
            (-0.04, 0.06),
            (0.01, -0.08),
            (-0.05, 0.03),
        ];
        let angle = 0.173_f64;
        let scale = 0.97_f64;
        let a = scale * angle.cos();
        let b = scale * angle.sin();
        let mut reference_points = [(0.0, 0.0); 8];
        for (index, &(x, y)) in source_points.iter().enumerate() {
            reference_points[index] = (
                a.mul_add(x, (-b).mul_add(y, 7.25)) + noise[index].0,
                b.mul_add(x, a.mul_add(y, -3.4)) + noise[index].1,
            );
        }
        let source = feature_catalog('0', &source_points)?;
        let reference = feature_catalog('a', &reference_points)?;
        let descriptor_matches = descriptor_matches_with_tolerance(
            &source,
            &reference,
            ReflectionPolicy::Forbid,
            0.02,
            0.02,
        )?;

        let consensus = estimate_similarity_consensus(
            &source,
            &reference,
            &descriptor_matches,
            SimilarityConsensusParameters::new(0.3, 2, 4, 0.25, 1_000, 1_000_000)?,
        )?;

        let [m00, m01, m10, m11, tx, ty] = consensus.transform().coefficients();
        assert!((m00 - a).abs() < 0.002);
        assert!((m01 + b).abs() < 0.002);
        assert!((m10 - b).abs() < 0.002);
        assert!((m11 - a).abs() < 0.002);
        assert!((tx - 7.25).abs() < 0.08);
        assert!((ty + 3.4).abs() < 0.08);
        assert!((consensus.scale() - scale).abs() < 0.002);
        assert!((consensus.rotation_radians() - angle).abs() < 0.002);
        assert!(consensus.residual_statistics().maximum_pixels() < 0.12);
        assert_eq!(consensus.inlier_feature_pairs().len(), source_points.len());
        Ok(())
    }

    #[test]
    fn reflection_is_fitted_only_from_reflected_hypotheses() -> TestResult {
        let source_points = [
            (2.0, 3.0),
            (11.0, 5.0),
            (4.0, 17.0),
            (19.0, 13.0),
            (27.0, 7.0),
        ];
        let reference_points = source_points.map(|(x, y)| (80.0 - 1.5 * x, 10.0 + 1.5 * y));
        let source = feature_catalog('c', &source_points)?;
        let reference = feature_catalog('d', &reference_points)?;
        let descriptor_matches =
            descriptor_matches(&source, &reference, ReflectionPolicy::Require)?;

        let consensus = estimate_similarity_consensus(
            &source,
            &reference,
            &descriptor_matches,
            parameters(2, 1_000, 1_000_000)?,
        )?;

        assert!(consensus.reflected());
        assert!((consensus.scale() - 1.5).abs() < 1.0e-12);
        assert!(consensus.residual_statistics().maximum_pixels() < 1.0e-11);
        assert_eq!(consensus.inlier_feature_pairs().len(), source_points.len());
        Ok(())
    }

    #[test]
    fn model_limit_is_visible_without_preventing_consensus_scoring() -> TestResult {
        let source_points = [
            (1.0, 2.0),
            (9.0, 3.0),
            (3.0, 12.0),
            (15.0, 10.0),
            (20.0, 4.0),
        ];
        let reference_points = source_points.map(|(x, y)| (x + 7.0, y - 3.0));
        let source = feature_catalog('e', &source_points)?;
        let reference = feature_catalog('f', &reference_points)?;
        let descriptor_matches = descriptor_matches(&source, &reference, ReflectionPolicy::Forbid)?;
        assert!(descriptor_matches.hypotheses().len() > 1);

        let consensus = estimate_similarity_consensus(
            &source,
            &reference,
            &descriptor_matches,
            parameters(2, 1, 100_000)?,
        )?;

        assert_eq!(consensus.statistics().models_evaluated(), 1);
        assert_eq!(
            consensus.statistics().models_discarded_by_limit(),
            descriptor_matches.hypotheses().len() - 1
        );
        assert!(consensus.statistics().refined_inlier_hypotheses() >= 2);
        Ok(())
    }

    #[test]
    fn rejects_competing_symmetric_hypotheses_as_outliers() -> TestResult {
        let square = [(0.0, 0.0), (10.0, 0.0), (0.0, 10.0), (10.0, 10.0)];
        let translated = square.map(|(x, y)| (x + 12.0, y + 7.0));
        let source = feature_catalog('8', &square)?;
        let reference = feature_catalog('9', &translated)?;
        let descriptor_matches = descriptor_matches(&source, &reference, ReflectionPolicy::Allow)?;
        assert!(descriptor_matches.hypotheses().len() > 4);

        let consensus = estimate_similarity_consensus(
            &source,
            &reference,
            &descriptor_matches,
            parameters(2, 100, 100_000)?,
        )?;

        assert_eq!(consensus.inlier_feature_pairs().len(), 4);
        assert!(consensus.statistics().refined_inlier_hypotheses() >= 2);
        assert!(consensus.statistics().outlier_hypotheses() > 0);
        let competitor = consensus
            .competing_similarity()
            .ok_or("symmetric field did not retain a competing transform")?;
        assert_eq!(
            competitor.inlier_hypotheses(),
            consensus.statistics().seed_inlier_hypotheses()
        );
        assert!(competitor.maximum_separation_pixels() > 10.0);
        assert!(consensus.statistics().distinct_competing_models() > 0);
        assert!(consensus.residual_statistics().maximum_pixels() < 1.0e-11);
        Ok(())
    }

    #[test]
    fn resolves_colliding_triangle_votes_into_stable_bijective_pairs() -> TestResult {
        let points = [(0.0, 0.0), (10.0, 0.0), (0.0, 10.0)];
        let source = feature_catalog('a', &points)?;
        let reference = feature_catalog('b', &points)?;
        let dominant = FeaturePair::new(0, 0);
        let collision = FeaturePair::new(0, 1);
        let second = FeaturePair::new(1, 1);
        let third = FeaturePair::new(2, 2);
        let candidates = vec![
            dominant, dominant, dominant, collision, second, second, third, third,
        ];
        let parameters = parameters(2, 10, 100)?;
        let mut statistics = SimilarityConsensusStatistics::default();

        let selected = select_bijective_pairs(
            AffineTransform::IDENTITY,
            &source,
            &reference,
            candidates,
            parameters,
            &mut statistics,
        )?;

        assert_eq!(selected, vec![dominant, second, third]);
        validate_one_to_one_pairs(&source, &reference, &selected)?;
        assert_eq!(statistics.residual_evaluations(), 4);
        Ok(())
    }

    #[test]
    fn refuses_one_triangle_and_exhausted_evaluation_budget() -> TestResult {
        let triangle = [(0.0, 0.0), (10.0, 1.0), (2.0, 12.0)];
        let shifted = triangle.map(|(x, y)| (x + 5.0, y + 6.0));
        let source = feature_catalog('1', &triangle)?;
        let reference = feature_catalog('2', &shifted)?;
        let one_match = descriptor_matches(&source, &reference, ReflectionPolicy::Forbid)?;
        assert_eq!(one_match.hypotheses().len(), 1);
        assert!(matches!(
            estimate_similarity_consensus(&source, &reference, &one_match, parameters(2, 10, 100)?,),
            Err(SimilarityConsensusError::InsufficientHypothesisConsensus {
                required: 2,
                actual: 1
            })
        ));

        let square = [(0.0, 0.0), (10.0, 0.0), (0.0, 10.0), (10.0, 10.0)];
        let translated = square.map(|(x, y)| (x + 2.0, y + 3.0));
        let square_source = feature_catalog('3', &square)?;
        let square_reference = feature_catalog('4', &translated)?;
        let many_matches =
            descriptor_matches(&square_source, &square_reference, ReflectionPolicy::Allow)?;
        assert!(matches!(
            estimate_similarity_consensus(
                &square_source,
                &square_reference,
                &many_matches,
                parameters(2, 10, 1)?,
            ),
            Err(SimilarityConsensusError::EvaluationLimitExceeded { maximum: 1 })
        ));
        Ok(())
    }

    #[test]
    fn validates_controls_and_frame_identity() -> TestResult {
        assert!(matches!(
            SimilarityConsensusParameters::new(0.0, 2, 3, 0.25, 1, 1),
            Err(SimilarityConsensusError::InvalidResidualThreshold)
        ));
        assert!(matches!(
            SimilarityConsensusParameters::new(1.0, 1, 3, 0.25, 1, 1),
            Err(SimilarityConsensusError::InvalidMinimumHypotheses)
        ));

        let points = [(0.0, 0.0), (10.0, 1.0), (2.0, 12.0), (14.0, 9.0)];
        let shifted = points.map(|(x, y)| (x + 1.0, y + 1.0));
        let source = feature_catalog('5', &points)?;
        let reference = feature_catalog('6', &shifted)?;
        let wrong_source = feature_catalog('7', &points)?;
        let matches = descriptor_matches(&source, &reference, ReflectionPolicy::Forbid)?;
        assert!(matches!(
            estimate_similarity_consensus(
                &wrong_source,
                &reference,
                &matches,
                parameters(2, 10, 1_000)?,
            ),
            Err(SimilarityConsensusError::SourceFrameMismatch)
        ));
        Ok(())
    }
}
