use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_core::CompensatedSum;

use crate::consensus::fit_similarity;
use crate::{
    AffineTransform, CoordinateError, ProjectiveFitError, ProjectiveTransform, RegistrationMatch,
    SimilarityConsensusError, fit_projective,
};

/// Versioned deterministic held-out comparison of similarity and projective fits.
pub const PROJECTIVE_CROSS_VALIDATION_ALGORITHM_ID: &str =
    "spatial-round-robin-kfold-similarity-projective-v1";

/// Maximum number of deterministic validation folds.
pub const MAX_PROJECTIVE_VALIDATION_FOLDS: usize = 16;

/// Out-of-sample prediction evidence aggregated over deterministic folds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectiveCrossValidationEvidence {
    match_count: usize,
    fold_count: usize,
    projective_better_folds: usize,
    similarity_root_mean_square_pixels: f64,
    similarity_maximum_residual_pixels: f64,
    projective_root_mean_square_pixels: f64,
    projective_maximum_residual_pixels: f64,
    root_mean_square_improvement_pixels: f64,
    relative_root_mean_square_improvement: Option<f64>,
    minimum_projective_rank_separation_ratio: f64,
}

impl ProjectiveCrossValidationEvidence {
    /// Versioned fold construction and aggregation policy.
    #[must_use]
    pub const fn algorithm_id(self) -> &'static str {
        PROJECTIVE_CROSS_VALIDATION_ALGORITHM_ID
    }

    /// Exact number of correspondences predicted once out of sample.
    #[must_use]
    pub const fn match_count(self) -> usize {
        self.match_count
    }

    /// Number of nonempty deterministic folds.
    #[must_use]
    pub const fn fold_count(self) -> usize {
        self.fold_count
    }

    /// Folds whose projective validation RMS was strictly lower.
    #[must_use]
    pub const fn projective_better_folds(self) -> usize {
        self.projective_better_folds
    }

    /// Similarity RMS aggregated across all held-out predictions.
    #[must_use]
    pub const fn similarity_root_mean_square_pixels(self) -> f64 {
        self.similarity_root_mean_square_pixels
    }

    /// Largest held-out similarity residual.
    #[must_use]
    pub const fn similarity_maximum_residual_pixels(self) -> f64 {
        self.similarity_maximum_residual_pixels
    }

    /// Projective RMS aggregated across all held-out predictions.
    #[must_use]
    pub const fn projective_root_mean_square_pixels(self) -> f64 {
        self.projective_root_mean_square_pixels
    }

    /// Largest held-out projective residual.
    #[must_use]
    pub const fn projective_maximum_residual_pixels(self) -> f64 {
        self.projective_maximum_residual_pixels
    }

    /// Similarity held-out RMS minus projective held-out RMS.
    #[must_use]
    pub const fn root_mean_square_improvement_pixels(self) -> f64 {
        self.root_mean_square_improvement_pixels
    }

    /// Held-out RMS improvement divided by similarity RMS when nonzero.
    #[must_use]
    pub const fn relative_root_mean_square_improvement(self) -> Option<f64> {
        self.relative_root_mean_square_improvement
    }

    /// Weakest normalized-DLT rank separation observed on training folds.
    #[must_use]
    pub const fn minimum_projective_rank_separation_ratio(self) -> f64 {
        self.minimum_projective_rank_separation_ratio
    }
}

/// Compares both model families on points excluded from each fitted fold.
///
/// Correspondences are sorted by source `y`, then `x`, then original index and
/// assigned round-robin to folds. Every match is predicted exactly once, while
/// both models are refitted on the identical complementary support. The caller
/// supplies the reflection class already established by robust consensus.
pub fn cross_validate_similarity_with_projective(
    matches: &[RegistrationMatch],
    reflected: bool,
    fold_count: usize,
) -> Result<ProjectiveCrossValidationEvidence, ProjectiveCrossValidationError> {
    if !(2..=MAX_PROJECTIVE_VALIDATION_FOLDS).contains(&fold_count) {
        return Err(ProjectiveCrossValidationError::InvalidFoldCount {
            minimum: 2,
            maximum: MAX_PROJECTIVE_VALIDATION_FOLDS,
            actual: fold_count,
        });
    }
    if matches.len() < fold_count || matches.len() < 5 {
        return Err(ProjectiveCrossValidationError::InsufficientMatches {
            minimum: fold_count.max(5),
            actual: matches.len(),
        });
    }

    let mut spatial_order = Vec::new();
    spatial_order
        .try_reserve_exact(matches.len())
        .map_err(|_| ProjectiveCrossValidationError::AllocationFailed)?;
    spatial_order.extend(0..matches.len());
    spatial_order.sort_by(|&left, &right| {
        matches[left]
            .source()
            .y()
            .total_cmp(&matches[right].source().y())
            .then_with(|| {
                matches[left]
                    .source()
                    .x()
                    .total_cmp(&matches[right].source().x())
            })
            .then_with(|| left.cmp(&right))
    });

    let mut fold_by_match = Vec::new();
    fold_by_match
        .try_reserve_exact(matches.len())
        .map_err(|_| ProjectiveCrossValidationError::AllocationFailed)?;
    fold_by_match.resize(matches.len(), 0_usize);
    for (spatial_rank, &match_index) in spatial_order.iter().enumerate() {
        fold_by_match[match_index] = spatial_rank % fold_count;
    }

    let mut similarity_squared = CompensatedSum::new();
    let mut projective_squared = CompensatedSum::new();
    let mut similarity_maximum = 0.0_f64;
    let mut projective_maximum = 0.0_f64;
    let mut projective_better_folds = 0_usize;
    let mut minimum_rank_separation = f64::INFINITY;

    for fold in 0..fold_count {
        let validation_count = fold_by_match.iter().filter(|&&value| value == fold).count();
        let training_count = matches.len() - validation_count;
        if training_count < 4 {
            return Err(
                ProjectiveCrossValidationError::InsufficientTrainingMatches {
                    fold,
                    minimum: 4,
                    actual: training_count,
                },
            );
        }
        let mut training = Vec::new();
        training
            .try_reserve_exact(training_count)
            .map_err(|_| ProjectiveCrossValidationError::AllocationFailed)?;
        for (index, &correspondence) in matches.iter().enumerate() {
            if fold_by_match[index] != fold {
                training.push(correspondence);
            }
        }

        let similarity = fit_similarity(&training, reflected)
            .map_err(ProjectiveCrossValidationError::SimilarityFit)?;
        let projective =
            fit_projective(&training).map_err(ProjectiveCrossValidationError::ProjectiveFit)?;
        minimum_rank_separation = minimum_rank_separation.min(projective.rank_separation_ratio());
        let mut fold_similarity_squared = CompensatedSum::new();
        let mut fold_projective_squared = CompensatedSum::new();
        for (index, &correspondence) in matches.iter().enumerate() {
            if fold_by_match[index] != fold {
                continue;
            }
            let similarity_residual = residual(similarity, correspondence)?;
            let projective_residual = projective_residual(projective.transform(), correspondence)?;
            let similarity_square = similarity_residual * similarity_residual;
            let projective_square = projective_residual * projective_residual;
            fold_similarity_squared.add(similarity_square);
            fold_projective_squared.add(projective_square);
            similarity_squared.add(similarity_square);
            projective_squared.add(projective_square);
            similarity_maximum = similarity_maximum.max(similarity_residual);
            projective_maximum = projective_maximum.max(projective_residual);
        }
        if fold_projective_squared.total() < fold_similarity_squared.total() {
            projective_better_folds += 1;
        }
    }

    let divisor = matches.len() as f64;
    let similarity_rms = (similarity_squared.total() / divisor).sqrt();
    let projective_rms = (projective_squared.total() / divisor).sqrt();
    let improvement = similarity_rms - projective_rms;
    let relative_improvement = if similarity_rms == 0.0 {
        None
    } else {
        Some(improvement / similarity_rms)
    };
    if [
        similarity_rms,
        projective_rms,
        improvement,
        similarity_maximum,
        projective_maximum,
        minimum_rank_separation,
    ]
    .iter()
    .any(|value| !value.is_finite())
        || relative_improvement.is_some_and(|value| !value.is_finite())
    {
        return Err(ProjectiveCrossValidationError::NumericalOverflow);
    }

    Ok(ProjectiveCrossValidationEvidence {
        match_count: matches.len(),
        fold_count,
        projective_better_folds,
        similarity_root_mean_square_pixels: canonical_zero(similarity_rms),
        similarity_maximum_residual_pixels: canonical_zero(similarity_maximum),
        projective_root_mean_square_pixels: canonical_zero(projective_rms),
        projective_maximum_residual_pixels: canonical_zero(projective_maximum),
        root_mean_square_improvement_pixels: canonical_zero(improvement),
        relative_root_mean_square_improvement: relative_improvement.map(canonical_zero),
        minimum_projective_rank_separation_ratio: minimum_rank_separation,
    })
}

fn residual(
    transform: AffineTransform,
    correspondence: RegistrationMatch,
) -> Result<f64, ProjectiveCrossValidationError> {
    let mapped = transform
        .apply(correspondence.source())
        .map_err(ProjectiveCrossValidationError::Coordinate)?;
    finite_residual(mapped.x(), mapped.y(), correspondence)
}

fn projective_residual(
    transform: ProjectiveTransform,
    correspondence: RegistrationMatch,
) -> Result<f64, ProjectiveCrossValidationError> {
    let mapped = transform
        .apply(correspondence.source())
        .map_err(ProjectiveCrossValidationError::Coordinate)?;
    finite_residual(mapped.x(), mapped.y(), correspondence)
}

fn finite_residual(
    mapped_x: f64,
    mapped_y: f64,
    correspondence: RegistrationMatch,
) -> Result<f64, ProjectiveCrossValidationError> {
    let value = (mapped_x - correspondence.reference().x())
        .hypot(mapped_y - correspondence.reference().y());
    if !value.is_finite() || !(value * value).is_finite() {
        return Err(ProjectiveCrossValidationError::NumericalOverflow);
    }
    Ok(value)
}

const fn canonical_zero(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}

/// Invalid folds, degenerate training geometry, or finite-domain failure.
#[derive(Debug)]
pub enum ProjectiveCrossValidationError {
    /// Fold count must remain within the documented work bound.
    InvalidFoldCount {
        /// Inclusive minimum.
        minimum: usize,
        /// Inclusive maximum.
        maximum: usize,
        /// Supplied value.
        actual: usize,
    },
    /// The complete evidence set cannot populate every fold and training set.
    InsufficientMatches {
        /// Minimum correspondence count for this request.
        minimum: usize,
        /// Supplied count.
        actual: usize,
    },
    /// One complementary training set was too small for a homography.
    InsufficientTrainingMatches {
        /// Zero-based validation fold.
        fold: usize,
        /// Minimum training count.
        minimum: usize,
        /// Actual training count.
        actual: usize,
    },
    /// Similarity fitting failed on a training fold.
    SimilarityFit(SimilarityConsensusError),
    /// Projective fitting failed on a training fold.
    ProjectiveFit(ProjectiveFitError),
    /// Applying a fitted transform failed.
    Coordinate(CoordinateError),
    /// Scratch allocation failed.
    AllocationFailed,
    /// A derived metric left the finite binary64 domain.
    NumericalOverflow,
}

impl Display for ProjectiveCrossValidationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidFoldCount {
                minimum,
                maximum,
                actual,
            } => write!(
                formatter,
                "projective validation fold count must be in {minimum}..={maximum}, received {actual}"
            ),
            Self::InsufficientMatches { minimum, actual } => write!(
                formatter,
                "projective validation requires at least {minimum} matches, received {actual}"
            ),
            Self::InsufficientTrainingMatches {
                fold,
                minimum,
                actual,
            } => write!(
                formatter,
                "projective validation fold {fold} requires {minimum} training matches, found {actual}"
            ),
            Self::SimilarityFit(error) => {
                write!(formatter, "cannot fit validation similarity: {error}")
            }
            Self::ProjectiveFit(error) => {
                write!(formatter, "cannot fit validation projective model: {error}")
            }
            Self::Coordinate(error) => {
                write!(formatter, "cannot predict validation point: {error}")
            }
            Self::AllocationFailed => {
                formatter.write_str("projective validation allocation failed")
            }
            Self::NumericalOverflow => {
                formatter.write_str("projective validation exceeded the finite numerical domain")
            }
        }
    }
}

impl Error for ProjectiveCrossValidationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::SimilarityFit(error) => Some(error),
            Self::ProjectiveFit(error) => Some(error),
            Self::Coordinate(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ImagePoint;

    type TestResult = Result<(), Box<dyn Error>>;

    fn matches_for(
        transform: ProjectiveTransform,
        noise: f64,
    ) -> Result<Vec<RegistrationMatch>, CoordinateError> {
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
            let expected = transform.apply(source)?;
            let signed_noise = if index % 2 == 0 { noise } else { -noise };
            let reference = ImagePoint::new(
                expected.x() + signed_noise,
                expected.y() - signed_noise * 0.5,
            )?;
            matches.push(RegistrationMatch::new(source, reference));
        }
        Ok(matches)
    }

    #[test]
    fn exact_similarity_generalizes_without_projective_advantage() -> TestResult {
        let similarity = AffineTransform::new(0.998, -0.02, 0.02, 0.998, 4.0, -3.0)?;
        let matches = matches_for(ProjectiveTransform::from_affine(similarity)?, 0.0)?;
        let evidence = cross_validate_similarity_with_projective(&matches, false, 5)?;

        assert_eq!(
            evidence.algorithm_id(),
            PROJECTIVE_CROSS_VALIDATION_ALGORITHM_ID
        );
        assert_eq!(evidence.match_count(), matches.len());
        assert_eq!(evidence.fold_count(), 5);
        assert!(evidence.similarity_root_mean_square_pixels() < 1.0e-12);
        assert!(evidence.projective_root_mean_square_pixels() < 1.0e-10);
        assert!(evidence.similarity_maximum_residual_pixels() < 1.0e-12);
        assert!(evidence.minimum_projective_rank_separation_ratio() > 0.0);
        Ok(())
    }

    #[test]
    fn genuine_projective_geometry_improves_every_held_out_fold() -> TestResult {
        let projective = ProjectiveTransform::new([
            [1.0, 0.01, 3.0],
            [-0.005, 0.99, -2.0],
            [4.0e-5, -3.0e-5, 1.0],
        ])?;
        let matches = matches_for(projective, 0.01)?;
        let evidence = cross_validate_similarity_with_projective(&matches, false, 5)?;

        assert_eq!(evidence.projective_better_folds(), 5);
        assert!(evidence.similarity_root_mean_square_pixels() > 10.0);
        assert!(evidence.projective_root_mean_square_pixels() < 0.1);
        assert!(evidence.projective_maximum_residual_pixels() < 0.2);
        assert!(evidence.root_mean_square_improvement_pixels() > 10.0);
        assert!(
            evidence
                .relative_root_mean_square_improvement()
                .is_some_and(|value| value > 0.99)
        );
        Ok(())
    }

    #[test]
    fn rejects_invalid_fold_shapes_before_fitting() -> TestResult {
        let matches = matches_for(ProjectiveTransform::IDENTITY, 0.0)?;
        assert!(matches!(
            cross_validate_similarity_with_projective(&matches, false, 1),
            Err(ProjectiveCrossValidationError::InvalidFoldCount { .. })
        ));
        assert!(matches!(
            cross_validate_similarity_with_projective(&matches[..4], false, 5),
            Err(ProjectiveCrossValidationError::InsufficientMatches { .. })
        ));
        assert!(matches!(
            cross_validate_similarity_with_projective(&matches[..5], false, 2),
            Err(ProjectiveCrossValidationError::InsufficientTrainingMatches { .. })
        ));
        Ok(())
    }
}
