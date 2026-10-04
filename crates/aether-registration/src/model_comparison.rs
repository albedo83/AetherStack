use std::error::Error;
use std::fmt::{Display, Formatter};

use crate::{
    AffineTransform, CoordinateError, ProjectiveFit, ProjectiveFitError, RegistrationMatch,
    ResidualError, evaluate_residuals, fit_projective,
};

/// Versioned same-support similarity/projective comparison policy.
pub const PROJECTIVE_ADEQUACY_ALGORITHM_ID: &str = "similarity-projective-adequacy-v1";

/// Inspectable same-support comparison between similarity and projective fits.
///
/// This is evidence, not an automatic model-selection decision. Both models are
/// evaluated on the exact correspondence slice supplied by the caller.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectiveAdequacyEvidence {
    match_count: usize,
    similarity_root_mean_square_pixels: f64,
    similarity_maximum_residual_pixels: f64,
    projective_fit: ProjectiveFit,
    root_mean_square_improvement_pixels: f64,
    relative_root_mean_square_improvement: Option<f64>,
    maximum_model_separation_pixels: f64,
}

impl ProjectiveAdequacyEvidence {
    /// Versioned comparison policy used to produce this evidence.
    #[must_use]
    pub const fn algorithm_id(self) -> &'static str {
        PROJECTIVE_ADEQUACY_ALGORITHM_ID
    }

    /// Exact common support evaluated by both models.
    #[must_use]
    pub const fn match_count(self) -> usize {
        self.match_count
    }

    /// Similarity RMS on the common support.
    #[must_use]
    pub const fn similarity_root_mean_square_pixels(self) -> f64 {
        self.similarity_root_mean_square_pixels
    }

    /// Similarity worst residual on the common support.
    #[must_use]
    pub const fn similarity_maximum_residual_pixels(self) -> f64 {
        self.similarity_maximum_residual_pixels
    }

    /// Complete normalized-DLT result on the common support.
    #[must_use]
    pub const fn projective_fit(self) -> ProjectiveFit {
        self.projective_fit
    }

    /// Similarity RMS minus projective RMS in reference pixels.
    #[must_use]
    pub const fn root_mean_square_improvement_pixels(self) -> f64 {
        self.root_mean_square_improvement_pixels
    }

    /// RMS improvement divided by similarity RMS, absent for exact similarity.
    #[must_use]
    pub const fn relative_root_mean_square_improvement(self) -> Option<f64> {
        self.relative_root_mean_square_improvement
    }

    /// Largest distance between model predictions at corners and center.
    #[must_use]
    pub const fn maximum_model_separation_pixels(self) -> f64 {
        self.maximum_model_separation_pixels
    }
}

/// Fits and compares a projective model on one already robust correspondence set.
pub fn compare_similarity_with_projective(
    similarity: AffineTransform,
    matches: &[RegistrationMatch],
    source_width: usize,
    source_height: usize,
) -> Result<ProjectiveAdequacyEvidence, ProjectiveAdequacyError> {
    if source_width == 0 || source_height == 0 {
        return Err(ProjectiveAdequacyError::InvalidSourceDimensions);
    }
    let similarity_residuals =
        evaluate_residuals(similarity, matches).map_err(ProjectiveAdequacyError::Similarity)?;
    let projective_fit = fit_projective(matches).map_err(ProjectiveAdequacyError::Projective)?;
    let similarity_rms = similarity_residuals.root_mean_square_pixels();
    let projective_rms = projective_fit.root_mean_square_pixels();
    let improvement = similarity_rms - projective_rms;
    let relative_improvement = if similarity_rms == 0.0 {
        None
    } else {
        let value = improvement / similarity_rms;
        if !value.is_finite() {
            return Err(ProjectiveAdequacyError::NumericalOverflow);
        }
        Some(value)
    };

    let maximum_x = (source_width - 1) as f64;
    let maximum_y = (source_height - 1) as f64;
    let control_points = [
        (0.0, 0.0),
        (maximum_x, 0.0),
        (0.0, maximum_y),
        (maximum_x, maximum_y),
        (maximum_x * 0.5, maximum_y * 0.5),
    ];
    let mut maximum_separation = 0.0_f64;
    for (x, y) in control_points {
        let point = crate::ImagePoint::new(x, y).map_err(ProjectiveAdequacyError::Coordinate)?;
        let similarity_point = similarity
            .apply(point)
            .map_err(ProjectiveAdequacyError::Coordinate)?;
        let projective_point = projective_fit
            .transform()
            .apply(point)
            .map_err(ProjectiveAdequacyError::Coordinate)?;
        let separation = (similarity_point.x() - projective_point.x())
            .hypot(similarity_point.y() - projective_point.y());
        if !separation.is_finite() {
            return Err(ProjectiveAdequacyError::NumericalOverflow);
        }
        maximum_separation = maximum_separation.max(separation);
    }

    Ok(ProjectiveAdequacyEvidence {
        match_count: matches.len(),
        similarity_root_mean_square_pixels: similarity_rms,
        similarity_maximum_residual_pixels: similarity_residuals.maximum_pixels(),
        projective_fit,
        root_mean_square_improvement_pixels: improvement,
        relative_root_mean_square_improvement: relative_improvement,
        maximum_model_separation_pixels: maximum_separation,
    })
}

/// Failure to construct comparable model-adequacy evidence.
#[derive(Debug)]
pub enum ProjectiveAdequacyError {
    /// Width and height must both be positive.
    InvalidSourceDimensions,
    /// Similarity residual evaluation failed.
    Similarity(ResidualError),
    /// Projective fitting failed.
    Projective(ProjectiveFitError),
    /// A control-point projection failed.
    Coordinate(CoordinateError),
    /// A derived comparison metric left the finite domain.
    NumericalOverflow,
}

impl Display for ProjectiveAdequacyError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSourceDimensions => {
                formatter.write_str("projective comparison requires positive source dimensions")
            }
            Self::Similarity(error) => write!(formatter, "cannot evaluate similarity: {error}"),
            Self::Projective(error) => write!(formatter, "cannot fit projective model: {error}"),
            Self::Coordinate(error) => write!(formatter, "cannot compare model geometry: {error}"),
            Self::NumericalOverflow => {
                formatter.write_str("projective comparison exceeded the finite numerical domain")
            }
        }
    }
}

impl Error for ProjectiveAdequacyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Similarity(error) => Some(error),
            Self::Projective(error) => Some(error),
            Self::Coordinate(error) => Some(error),
            Self::InvalidSourceDimensions | Self::NumericalOverflow => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ImagePoint, ProjectiveTransform};

    type TestResult = Result<(), Box<dyn Error>>;

    fn matches_for(
        transform: ProjectiveTransform,
    ) -> Result<Vec<RegistrationMatch>, CoordinateError> {
        let mut matches = Vec::new();
        for y in [0.0, 500.0, 1000.0] {
            for x in [0.0, 750.0, 1500.0] {
                let source = ImagePoint::new(x, y)?;
                matches.push(RegistrationMatch::new(source, transform.apply(source)?));
            }
        }
        Ok(matches)
    }

    #[test]
    fn exact_similarity_reports_no_projective_need() -> TestResult {
        let similarity = AffineTransform::new(0.999, -0.01, 0.01, 0.999, 3.0, -2.0)?;
        let mut matches = Vec::new();
        for y in [0.0, 500.0, 1000.0] {
            for x in [0.0, 750.0, 1500.0] {
                let source = ImagePoint::new(x, y)?;
                matches.push(RegistrationMatch::new(source, similarity.apply(source)?));
            }
        }
        let evidence = compare_similarity_with_projective(similarity, &matches, 1501, 1001)?;

        assert_eq!(evidence.match_count(), matches.len());
        assert!(evidence.similarity_root_mean_square_pixels() < 1e-12);
        assert!(evidence.projective_fit().root_mean_square_pixels() < 1e-10);
        assert!(evidence.maximum_model_separation_pixels() < 1e-10);
        assert_eq!(evidence.relative_root_mean_square_improvement(), None);
        Ok(())
    }

    #[test]
    fn projective_field_reports_residual_gain_and_edge_separation() -> TestResult {
        let similarity = AffineTransform::IDENTITY;
        let projective =
            ProjectiveTransform::new([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [2e-5, -1e-5, 1.0]])?;
        let matches = matches_for(projective)?;
        let evidence = compare_similarity_with_projective(similarity, &matches, 1501, 1001)?;

        assert!(evidence.similarity_root_mean_square_pixels() > 10.0);
        assert!(evidence.similarity_maximum_residual_pixels() > 20.0);
        assert!(evidence.projective_fit().root_mean_square_pixels() < 1e-10);
        assert!(evidence.root_mean_square_improvement_pixels() > 10.0);
        assert!(
            evidence
                .relative_root_mean_square_improvement()
                .is_some_and(|value| value > 0.99)
        );
        assert!(evidence.maximum_model_separation_pixels() > 20.0);
        Ok(())
    }

    #[test]
    fn rejects_invalid_dimensions_and_insufficient_support() -> TestResult {
        let point = ImagePoint::new(0.0, 0.0)?;
        let matches = [RegistrationMatch::new(point, point); 3];
        assert!(matches!(
            compare_similarity_with_projective(AffineTransform::IDENTITY, &matches, 0, 10),
            Err(ProjectiveAdequacyError::InvalidSourceDimensions)
        ));
        assert!(matches!(
            compare_similarity_with_projective(AffineTransform::IDENTITY, &matches, 10, 10),
            Err(ProjectiveAdequacyError::Projective(
                ProjectiveFitError::InsufficientMatches { .. }
            ))
        ));
        Ok(())
    }
}
