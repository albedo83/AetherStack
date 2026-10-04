use std::error::Error;
use std::fmt::{Display, Formatter};

use crate::{ProjectiveAdequacyEvidence, ProjectiveCrossValidationEvidence, ProjectiveTransform};

/// Versioned conservative projective recommendation policy.
pub const PROJECTIVE_SELECTION_ALGORITHM_ID: &str = "projective-selection-conservative-v1";

/// Explicit thresholds used to decide whether a homography is warranted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectiveSelectionPolicy {
    minimum_matches: usize,
    minimum_validation_folds: usize,
    minimum_projective_better_folds: usize,
    minimum_rms_improvement_pixels: f64,
    minimum_relative_rms_improvement: f64,
    minimum_rank_separation_ratio: f64,
    maximum_projective_rms_pixels: f64,
    minimum_model_separation_pixels: f64,
}

impl ProjectiveSelectionPolicy {
    /// Constructs an auditable policy after validating every threshold.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        minimum_matches: usize,
        minimum_validation_folds: usize,
        minimum_projective_better_folds: usize,
        minimum_rms_improvement_pixels: f64,
        minimum_relative_rms_improvement: f64,
        minimum_rank_separation_ratio: f64,
        maximum_projective_rms_pixels: f64,
        minimum_model_separation_pixels: f64,
    ) -> Result<Self, ProjectiveSelectionError> {
        if minimum_matches < 5 {
            return Err(ProjectiveSelectionError::InvalidMinimumMatches);
        }
        if minimum_validation_folds < 2
            || minimum_projective_better_folds == 0
            || minimum_projective_better_folds > minimum_validation_folds
        {
            return Err(ProjectiveSelectionError::InvalidFoldThresholds);
        }
        for (name, value) in [
            ("minimum RMS improvement", minimum_rms_improvement_pixels),
            (
                "minimum relative RMS improvement",
                minimum_relative_rms_improvement,
            ),
            (
                "minimum rank separation ratio",
                minimum_rank_separation_ratio,
            ),
            ("maximum projective RMS", maximum_projective_rms_pixels),
            ("minimum model separation", minimum_model_separation_pixels),
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(ProjectiveSelectionError::InvalidThreshold { name });
            }
        }
        if minimum_relative_rms_improvement > 1.0
            || minimum_rank_separation_ratio > 1.0
            || maximum_projective_rms_pixels == 0.0
        {
            return Err(ProjectiveSelectionError::InvalidThresholdRange);
        }
        Ok(Self {
            minimum_matches,
            minimum_validation_folds,
            minimum_projective_better_folds,
            minimum_rms_improvement_pixels,
            minimum_relative_rms_improvement,
            minimum_rank_separation_ratio,
            maximum_projective_rms_pixels,
            minimum_model_separation_pixels,
        })
    }

    /// High-confidence defaults expressed in detection-plane pixels.
    pub const fn conservative() -> Self {
        Self {
            minimum_matches: 20,
            minimum_validation_folds: 5,
            minimum_projective_better_folds: 5,
            minimum_rms_improvement_pixels: 0.05,
            minimum_relative_rms_improvement: 0.10,
            minimum_rank_separation_ratio: 0.01,
            maximum_projective_rms_pixels: 1.0,
            minimum_model_separation_pixels: 0.25,
        }
    }

    /// Minimum common-support correspondence count.
    #[must_use]
    pub const fn minimum_matches(self) -> usize {
        self.minimum_matches
    }

    /// Minimum number of independent spatial folds.
    #[must_use]
    pub const fn minimum_validation_folds(self) -> usize {
        self.minimum_validation_folds
    }

    /// Minimum folds in which projective squared error must be lower.
    #[must_use]
    pub const fn minimum_projective_better_folds(self) -> usize {
        self.minimum_projective_better_folds
    }

    /// Minimum held-out RMS gain in detection-plane pixels.
    #[must_use]
    pub const fn minimum_rms_improvement_pixels(self) -> f64 {
        self.minimum_rms_improvement_pixels
    }

    /// Minimum held-out RMS gain relative to similarity RMS.
    #[must_use]
    pub const fn minimum_relative_rms_improvement(self) -> f64 {
        self.minimum_relative_rms_improvement
    }

    /// Minimum weakest-fold normalized-DLT rank separation.
    #[must_use]
    pub const fn minimum_rank_separation_ratio(self) -> f64 {
        self.minimum_rank_separation_ratio
    }

    /// Maximum acceptable held-out projective RMS.
    #[must_use]
    pub const fn maximum_projective_rms_pixels(self) -> f64 {
        self.maximum_projective_rms_pixels
    }

    /// Minimum full-field difference between similarity and projective models.
    #[must_use]
    pub const fn minimum_model_separation_pixels(self) -> f64 {
        self.minimum_model_separation_pixels
    }
}

impl Default for ProjectiveSelectionPolicy {
    fn default() -> Self {
        Self::conservative()
    }
}

/// Per-criterion evidence behind one non-mutating model recommendation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectiveSelectionDecision {
    projective_transform: ProjectiveTransform,
    support_sufficient: bool,
    validation_folds_sufficient: bool,
    fold_wins_sufficient: bool,
    absolute_gain_sufficient: bool,
    relative_gain_sufficient: bool,
    rank_separation_sufficient: bool,
    projective_rms_acceptable: bool,
    worst_residual_not_increased: bool,
    model_separation_sufficient: bool,
}

impl ProjectiveSelectionDecision {
    /// Versioned decision policy.
    #[must_use]
    pub const fn algorithm_id(self) -> &'static str {
        PROJECTIVE_SELECTION_ALGORITHM_ID
    }

    /// True only when every conservative gate passes.
    #[must_use]
    pub const fn recommends_projective(self) -> bool {
        self.support_sufficient
            && self.validation_folds_sufficient
            && self.fold_wins_sufficient
            && self.absolute_gain_sufficient
            && self.relative_gain_sufficient
            && self.rank_separation_sufficient
            && self.projective_rms_acceptable
            && self.worst_residual_not_increased
            && self.model_separation_sufficient
    }

    /// Recommended transform, absent whenever any gate fails.
    #[must_use]
    pub const fn recommended_transform(self) -> Option<ProjectiveTransform> {
        if self.recommends_projective() {
            Some(self.projective_transform)
        } else {
            None
        }
    }

    /// Whether the common correspondence support reaches the policy minimum.
    #[must_use]
    pub const fn support_sufficient(self) -> bool {
        self.support_sufficient
    }

    /// Whether enough spatial validation folds were evaluated.
    #[must_use]
    pub const fn validation_folds_sufficient(self) -> bool {
        self.validation_folds_sufficient
    }

    /// Whether enough validation folds favored the projective model.
    #[must_use]
    pub const fn fold_wins_sufficient(self) -> bool {
        self.fold_wins_sufficient
    }

    /// Whether held-out absolute RMS gain reaches the policy minimum.
    #[must_use]
    pub const fn absolute_gain_sufficient(self) -> bool {
        self.absolute_gain_sufficient
    }

    /// Whether held-out relative RMS gain reaches the policy minimum.
    #[must_use]
    pub const fn relative_gain_sufficient(self) -> bool {
        self.relative_gain_sufficient
    }

    /// Whether every projective training fit retained enough rank separation.
    #[must_use]
    pub const fn rank_separation_sufficient(self) -> bool {
        self.rank_separation_sufficient
    }

    /// Whether held-out projective RMS remains below the absolute ceiling.
    #[must_use]
    pub const fn projective_rms_acceptable(self) -> bool {
        self.projective_rms_acceptable
    }

    /// Whether the projective model avoids a worse held-out extreme residual.
    #[must_use]
    pub const fn worst_residual_not_increased(self) -> bool {
        self.worst_residual_not_increased
    }

    /// Whether both models differ enough over the field to warrant complexity.
    #[must_use]
    pub const fn model_separation_sufficient(self) -> bool {
        self.model_separation_sufficient
    }
}

/// Evaluates a recommendation without changing a registration plan.
pub fn evaluate_projective_selection(
    adequacy: ProjectiveAdequacyEvidence,
    validation: ProjectiveCrossValidationEvidence,
    policy: ProjectiveSelectionPolicy,
) -> Result<ProjectiveSelectionDecision, ProjectiveSelectionError> {
    if adequacy.match_count() != validation.match_count() {
        return Err(ProjectiveSelectionError::MismatchedEvidenceSupport {
            adequacy: adequacy.match_count(),
            validation: validation.match_count(),
        });
    }
    let relative_gain_sufficient = validation
        .relative_root_mean_square_improvement()
        .is_some_and(|value| value >= policy.minimum_relative_rms_improvement);
    Ok(ProjectiveSelectionDecision {
        projective_transform: adequacy.projective_fit().transform(),
        support_sufficient: adequacy.match_count() >= policy.minimum_matches,
        validation_folds_sufficient: validation.fold_count() >= policy.minimum_validation_folds,
        fold_wins_sufficient: validation.projective_better_folds()
            >= policy.minimum_projective_better_folds,
        absolute_gain_sufficient: validation.root_mean_square_improvement_pixels()
            >= policy.minimum_rms_improvement_pixels,
        relative_gain_sufficient,
        rank_separation_sufficient: validation.minimum_projective_rank_separation_ratio()
            >= policy.minimum_rank_separation_ratio,
        projective_rms_acceptable: validation.projective_root_mean_square_pixels()
            <= policy.maximum_projective_rms_pixels,
        worst_residual_not_increased: validation.projective_maximum_residual_pixels()
            <= validation.similarity_maximum_residual_pixels(),
        model_separation_sufficient: adequacy.maximum_model_separation_pixels()
            >= policy.minimum_model_separation_pixels,
    })
}

/// Invalid policy configuration or incomparable evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectiveSelectionError {
    /// At least five pairs are required by the validation primitive.
    InvalidMinimumMatches,
    /// Fold thresholds must be positive, coherent, and at least two.
    InvalidFoldThresholds,
    /// A named floating threshold was negative or non-finite.
    InvalidThreshold {
        /// Human-readable threshold name.
        name: &'static str,
    },
    /// A unit interval or positive ceiling was outside its domain.
    InvalidThresholdRange,
    /// Fitting and held-out evidence must describe identical support.
    MismatchedEvidenceSupport {
        /// Full-support comparison count.
        adequacy: usize,
        /// Held-out validation count.
        validation: usize,
    },
}

impl Display for ProjectiveSelectionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidMinimumMatches => {
                formatter.write_str("projective selection requires at least five matches")
            }
            Self::InvalidFoldThresholds => formatter.write_str(
                "projective selection fold thresholds must be coherent and at least two",
            ),
            Self::InvalidThreshold { name } => {
                write!(
                    formatter,
                    "projective selection {name} must be finite and non-negative"
                )
            }
            Self::InvalidThresholdRange => formatter.write_str(
                "projective selection ratios must be at most one and RMS ceiling positive",
            ),
            Self::MismatchedEvidenceSupport {
                adequacy,
                validation,
            } => write!(
                formatter,
                "projective evidence support differs: adequacy {adequacy}, validation {validation}"
            ),
        }
    }
}

impl Error for ProjectiveSelectionError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AffineTransform, ImagePoint, RegistrationMatch, compare_similarity_with_projective,
        cross_validate_similarity_with_projective,
    };

    type TestResult<T = ()> = Result<T, Box<dyn Error>>;

    fn matches_for(transform: ProjectiveTransform) -> TestResult<Vec<RegistrationMatch>> {
        let mut matches = Vec::new();
        for (index, (x, y)) in [
            (0.0, 0.0),
            (300.0, 20.0),
            (620.0, 5.0),
            (950.0, 40.0),
            (1200.0, 10.0),
            (80.0, 280.0),
            (350.0, 330.0),
            (670.0, 260.0),
            (900.0, 350.0),
            (1250.0, 300.0),
            (20.0, 620.0),
            (310.0, 580.0),
            (640.0, 650.0),
            (970.0, 590.0),
            (1210.0, 640.0),
            (60.0, 900.0),
            (380.0, 940.0),
            (690.0, 880.0),
            (930.0, 960.0),
            (1280.0, 910.0),
        ]
        .into_iter()
        .enumerate()
        {
            let source = ImagePoint::new(x, y)?;
            let mapped = transform.apply(source)?;
            let noise = if index % 2 == 0 { 0.01 } else { -0.01 };
            let reference = ImagePoint::new(mapped.x() + noise, mapped.y() - noise * 0.5)?;
            matches.push(RegistrationMatch::new(source, reference));
        }
        Ok(matches)
    }

    #[test]
    fn conservative_policy_recommends_clear_projective_geometry() -> TestResult {
        let transform = ProjectiveTransform::new([
            [1.0, 0.01, 3.0],
            [-0.005, 0.99, -2.0],
            [4.0e-5, -3.0e-5, 1.0],
        ])?;
        let matches = matches_for(transform)?;
        let adequacy =
            compare_similarity_with_projective(AffineTransform::IDENTITY, &matches, 1281, 961)?;
        let validation = cross_validate_similarity_with_projective(&matches, false, 5)?;

        let decision = evaluate_projective_selection(
            adequacy,
            validation,
            ProjectiveSelectionPolicy::conservative(),
        )?;

        assert_eq!(decision.algorithm_id(), PROJECTIVE_SELECTION_ALGORITHM_ID);
        assert!(decision.recommends_projective());
        assert_eq!(
            decision.recommended_transform(),
            Some(adequacy.projective_fit().transform())
        );
        assert!(decision.support_sufficient());
        assert!(decision.validation_folds_sufficient());
        assert!(decision.fold_wins_sufficient());
        assert!(decision.absolute_gain_sufficient());
        assert!(decision.relative_gain_sufficient());
        assert!(decision.rank_separation_sufficient());
        assert!(decision.projective_rms_acceptable());
        assert!(decision.worst_residual_not_increased());
        assert!(decision.model_separation_sufficient());
        Ok(())
    }

    #[test]
    fn conservative_policy_rejects_unneeded_projective_complexity() -> TestResult {
        let similarity = AffineTransform::new(0.998, -0.02, 0.02, 0.998, 4.0, -3.0)?;
        let matches = matches_for(ProjectiveTransform::from_affine(similarity)?)?;
        let adequacy = compare_similarity_with_projective(similarity, &matches, 1281, 961)?;
        let validation = cross_validate_similarity_with_projective(&matches, false, 5)?;

        let decision = evaluate_projective_selection(
            adequacy,
            validation,
            ProjectiveSelectionPolicy::conservative(),
        )?;

        assert!(!decision.recommends_projective());
        assert_eq!(decision.recommended_transform(), None);
        assert!(!decision.absolute_gain_sufficient());
        assert!(!decision.relative_gain_sufficient());
        assert!(!decision.model_separation_sufficient());
        Ok(())
    }

    #[test]
    fn validates_policy_domains() {
        assert!(matches!(
            ProjectiveSelectionPolicy::new(4, 5, 5, 0.05, 0.1, 0.01, 1.0, 0.25),
            Err(ProjectiveSelectionError::InvalidMinimumMatches)
        ));
        assert!(matches!(
            ProjectiveSelectionPolicy::new(20, 5, 6, 0.05, 0.1, 0.01, 1.0, 0.25),
            Err(ProjectiveSelectionError::InvalidFoldThresholds)
        ));
        assert!(matches!(
            ProjectiveSelectionPolicy::new(20, 5, 5, f64::NAN, 0.1, 0.01, 1.0, 0.25),
            Err(ProjectiveSelectionError::InvalidThreshold { .. })
        ));
        assert!(matches!(
            ProjectiveSelectionPolicy::new(20, 5, 5, 0.05, 1.1, 0.01, 1.0, 0.25),
            Err(ProjectiveSelectionError::InvalidThresholdRange)
        ));
    }

    #[test]
    fn rejects_evidence_built_from_different_support() -> TestResult {
        let transform = ProjectiveTransform::new([
            [1.0, 0.01, 3.0],
            [-0.005, 0.99, -2.0],
            [4.0e-5, -3.0e-5, 1.0],
        ])?;
        let matches = matches_for(transform)?;
        let adequacy =
            compare_similarity_with_projective(AffineTransform::IDENTITY, &matches, 1281, 961)?;
        let validation = cross_validate_similarity_with_projective(&matches[..19], false, 5)?;

        assert!(matches!(
            evaluate_projective_selection(
                adequacy,
                validation,
                ProjectiveSelectionPolicy::conservative(),
            ),
            Err(ProjectiveSelectionError::MismatchedEvidenceSupport {
                adequacy: 20,
                validation: 19,
            })
        ));
        Ok(())
    }
}
