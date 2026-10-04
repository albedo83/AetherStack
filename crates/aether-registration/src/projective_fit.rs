use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_core::CompensatedSum;
use nalgebra::{DMatrix, linalg::SVD};

use crate::{CoordinateError, ImagePoint, ProjectiveTransform, RegistrationMatch};

/// Hard bound on correspondences admitted to one projective least-squares fit.
pub const MAX_PROJECTIVE_FIT_MATCHES: usize = 16_384;

const MIN_PROJECTIVE_FIT_MATCHES: usize = 4;
const DESIGN_COLUMNS: usize = 9;
const SVD_MAX_ITERATIONS: usize = 10_000;
const MINIMUM_RANK_RATIO: f64 = 1e-10;

/// Deterministic evidence returned with a normalized projective fit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectiveFit {
    transform: ProjectiveTransform,
    match_count: usize,
    root_mean_square_pixels: f64,
    maximum_residual_pixels: f64,
    rank_separation_ratio: f64,
}

impl ProjectiveFit {
    /// Fitted source-to-reference homography.
    #[must_use]
    pub const fn transform(self) -> ProjectiveTransform {
        self.transform
    }

    /// Number of source/reference pairs used without clipping.
    #[must_use]
    pub const fn match_count(self) -> usize {
        self.match_count
    }

    /// Root mean square reprojection error in reference pixels.
    #[must_use]
    pub const fn root_mean_square_pixels(self) -> f64 {
        self.root_mean_square_pixels
    }

    /// Largest reprojection error in reference pixels.
    #[must_use]
    pub const fn maximum_residual_pixels(self) -> f64 {
        self.maximum_residual_pixels
    }

    /// Smallest retained design singular value divided by the largest.
    ///
    /// Values near zero indicate rank loss. For overdetermined systems the
    /// fitted null singular value is excluded from this diagnostic.
    #[must_use]
    pub const fn rank_separation_ratio(self) -> f64 {
        self.rank_separation_ratio
    }
}

/// Fits a source-to-reference homography with Hartley-normalized DLT and SVD.
///
/// This function performs no robust clipping. Callers must supply a bounded set
/// of already selected correspondences and retain responsibility for consensus.
pub fn fit_projective(matches: &[RegistrationMatch]) -> Result<ProjectiveFit, ProjectiveFitError> {
    if matches.len() < MIN_PROJECTIVE_FIT_MATCHES {
        return Err(ProjectiveFitError::InsufficientMatches {
            minimum: MIN_PROJECTIVE_FIT_MATCHES,
            actual: matches.len(),
        });
    }
    if matches.len() > MAX_PROJECTIVE_FIT_MATCHES {
        return Err(ProjectiveFitError::TooManyMatches {
            maximum: MAX_PROJECTIVE_FIT_MATCHES,
            actual: matches.len(),
        });
    }

    let source_normalization = Normalization::from_matches(matches, true)?;
    let reference_normalization = Normalization::from_matches(matches, false)?;
    let constraint_rows = matches
        .len()
        .checked_mul(2)
        .ok_or(ProjectiveFitError::AllocationFailed)?;
    // A four-pair DLT has eight constraint rows and a one-dimensional null
    // space. One explicit zero row requests the complete 9 × 9 right-singular
    // basis from nalgebra without changing the system.
    let rows = constraint_rows.max(DESIGN_COLUMNS);
    let element_count = rows
        .checked_mul(DESIGN_COLUMNS)
        .ok_or(ProjectiveFitError::AllocationFailed)?;
    let mut elements = Vec::new();
    elements
        .try_reserve_exact(element_count)
        .map_err(|_| ProjectiveFitError::AllocationFailed)?;
    for correspondence in matches {
        let (x, y) = source_normalization.apply(correspondence.source());
        let (u, v) = reference_normalization.apply(correspondence.reference());
        elements.extend_from_slice(&[-x, -y, -1.0, 0.0, 0.0, 0.0, u * x, u * y, u]);
        elements.extend_from_slice(&[0.0, 0.0, 0.0, -x, -y, -1.0, v * x, v * y, v]);
    }
    elements.resize(element_count, 0.0);
    let design = DMatrix::from_row_iterator(rows, DESIGN_COLUMNS, elements);
    let decomposition = SVD::try_new(design, false, true, f64::EPSILON, SVD_MAX_ITERATIONS)
        .ok_or(ProjectiveFitError::DecompositionDidNotConverge)?;
    let v_transpose = decomposition
        .v_t
        .ok_or(ProjectiveFitError::DecompositionDidNotConverge)?;
    let singular_values = decomposition.singular_values.as_slice();
    let largest = singular_values.iter().copied().fold(0.0_f64, f64::max);
    if !largest.is_finite() || largest == 0.0 {
        return Err(ProjectiveFitError::DegenerateGeometry);
    }

    let mut ordered = singular_values.to_vec();
    ordered.sort_by(f64::total_cmp);
    let null_value = ordered[0];
    let null_index = singular_values
        .iter()
        .position(|value| value.to_bits() == null_value.to_bits())
        .ok_or(ProjectiveFitError::DecompositionDidNotConverge)?;
    let retained_smallest = ordered[1];
    let rank_separation_ratio = retained_smallest / largest;
    if !rank_separation_ratio.is_finite() || rank_separation_ratio <= MINIMUM_RANK_RATIO {
        return Err(ProjectiveFitError::DegenerateGeometry);
    }
    if v_transpose.nrows() <= null_index || v_transpose.ncols() != DESIGN_COLUMNS {
        return Err(ProjectiveFitError::DecompositionDidNotConverge);
    }
    let normalized = [
        [
            v_transpose[(null_index, 0)],
            v_transpose[(null_index, 1)],
            v_transpose[(null_index, 2)],
        ],
        [
            v_transpose[(null_index, 3)],
            v_transpose[(null_index, 4)],
            v_transpose[(null_index, 5)],
        ],
        [
            v_transpose[(null_index, 6)],
            v_transpose[(null_index, 7)],
            v_transpose[(null_index, 8)],
        ],
    ];
    let denormalized = multiply_3x3(
        reference_normalization.inverse_matrix(),
        multiply_3x3(normalized, source_normalization.matrix()),
    );
    let transform =
        ProjectiveTransform::new(denormalized).map_err(ProjectiveFitError::Coordinate)?;
    let (root_mean_square_pixels, maximum_residual_pixels) = residual_evidence(transform, matches)?;
    Ok(ProjectiveFit {
        transform,
        match_count: matches.len(),
        root_mean_square_pixels,
        maximum_residual_pixels,
        rank_separation_ratio,
    })
}

#[derive(Clone, Copy)]
struct Normalization {
    center_x: f64,
    center_y: f64,
    scale: f64,
}

impl Normalization {
    fn from_matches(
        matches: &[RegistrationMatch],
        source: bool,
    ) -> Result<Self, ProjectiveFitError> {
        let count = matches.len() as f64;
        let mut x_sum = CompensatedSum::new();
        let mut y_sum = CompensatedSum::new();
        for correspondence in matches {
            let point = if source {
                correspondence.source()
            } else {
                correspondence.reference()
            };
            x_sum.add(point.x());
            y_sum.add(point.y());
        }
        let center_x = x_sum.total() / count;
        let center_y = y_sum.total() / count;
        let mut distance_sum = CompensatedSum::new();
        for correspondence in matches {
            let point = if source {
                correspondence.source()
            } else {
                correspondence.reference()
            };
            distance_sum.add((point.x() - center_x).hypot(point.y() - center_y));
        }
        let mean_distance = distance_sum.total() / count;
        let scale = 2.0_f64.sqrt() / mean_distance;
        if !center_x.is_finite()
            || !center_y.is_finite()
            || !mean_distance.is_finite()
            || mean_distance <= 0.0
            || !scale.is_finite()
        {
            return Err(ProjectiveFitError::DegenerateGeometry);
        }
        Ok(Self {
            center_x,
            center_y,
            scale,
        })
    }

    fn apply(self, point: ImagePoint) -> (f64, f64) {
        (
            (point.x() - self.center_x) * self.scale,
            (point.y() - self.center_y) * self.scale,
        )
    }

    fn matrix(self) -> [[f64; 3]; 3] {
        [
            [self.scale, 0.0, -self.scale * self.center_x],
            [0.0, self.scale, -self.scale * self.center_y],
            [0.0, 0.0, 1.0],
        ]
    }

    fn inverse_matrix(self) -> [[f64; 3]; 3] {
        let inverse_scale = self.scale.recip();
        [
            [inverse_scale, 0.0, self.center_x],
            [0.0, inverse_scale, self.center_y],
            [0.0, 0.0, 1.0],
        ]
    }
}

fn multiply_3x3(left: [[f64; 3]; 3], right: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut product = [[0.0_f64; 3]; 3];
    for (row, product_row) in product.iter_mut().enumerate() {
        for (column, value) in product_row.iter_mut().enumerate() {
            *value = left[row][0].mul_add(
                right[0][column],
                left[row][1].mul_add(right[1][column], left[row][2] * right[2][column]),
            );
        }
    }
    product
}

fn residual_evidence(
    transform: ProjectiveTransform,
    matches: &[RegistrationMatch],
) -> Result<(f64, f64), ProjectiveFitError> {
    let mut squared_sum = CompensatedSum::new();
    let mut maximum = 0.0_f64;
    for correspondence in matches {
        let mapped = transform
            .apply(correspondence.source())
            .map_err(ProjectiveFitError::Coordinate)?;
        let residual = (mapped.x() - correspondence.reference().x())
            .hypot(mapped.y() - correspondence.reference().y());
        let squared = residual * residual;
        if !residual.is_finite() || !squared.is_finite() {
            return Err(ProjectiveFitError::NumericalOverflow);
        }
        squared_sum.add(squared);
        maximum = maximum.max(residual);
    }
    let rms = (squared_sum.total() / matches.len() as f64).sqrt();
    if !rms.is_finite() {
        return Err(ProjectiveFitError::NumericalOverflow);
    }
    Ok((rms, maximum))
}

/// Projective least-squares fitting failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectiveFitError {
    /// At least four point pairs are required.
    InsufficientMatches {
        /// Required number of correspondences.
        minimum: usize,
        /// Supplied number of correspondences.
        actual: usize,
    },
    /// The explicit fitting bound was exceeded.
    TooManyMatches {
        /// Configured hard bound.
        maximum: usize,
        /// Supplied number of correspondences.
        actual: usize,
    },
    /// Design-matrix storage could not be reserved.
    AllocationFailed,
    /// Normalized DLT did not retain eight independent constraints.
    DegenerateGeometry,
    /// The bounded SVD iteration budget was exhausted.
    DecompositionDidNotConverge,
    /// Constructing or applying the resulting homography failed.
    Coordinate(CoordinateError),
    /// Residual evaluation exceeded the finite numerical domain.
    NumericalOverflow,
}

impl Display for ProjectiveFitError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InsufficientMatches { minimum, actual } => write!(
                formatter,
                "projective fitting requires at least {minimum} matches, received {actual}"
            ),
            Self::TooManyMatches { maximum, actual } => write!(
                formatter,
                "projective fitting accepts at most {maximum} matches, received {actual}"
            ),
            Self::AllocationFailed => {
                formatter.write_str("projective design-matrix allocation failed")
            }
            Self::DegenerateGeometry => {
                formatter.write_str("projective correspondences are geometrically degenerate")
            }
            Self::DecompositionDidNotConverge => {
                formatter.write_str("projective SVD did not converge within its iteration bound")
            }
            Self::Coordinate(error) => write!(formatter, "projective transform failed: {error}"),
            Self::NumericalOverflow => {
                formatter.write_str("projective fitting exceeded the finite numerical domain")
            }
        }
    }
}

impl Error for ProjectiveFitError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Coordinate(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn Error>>;

    fn exact_matches(
        transform: ProjectiveTransform,
    ) -> Result<Vec<RegistrationMatch>, CoordinateError> {
        let mut matches = Vec::new();
        for y in [0.0, 900.0, 1800.0] {
            for x in [0.0, 1500.0, 3000.0] {
                let source = ImagePoint::new(x, y)?;
                matches.push(RegistrationMatch::new(source, transform.apply(source)?));
            }
        }
        Ok(matches)
    }

    #[test]
    fn recovers_exact_projective_ground_truth() -> TestResult {
        let expected = ProjectiveTransform::new([
            [1.0002, -0.0003, 12.5],
            [0.0004, 0.9998, -7.25],
            [2e-8, -3e-8, 1.0],
        ])?;
        let matches = exact_matches(expected)?;
        let fit = fit_projective(&matches)?;

        assert_eq!(fit.match_count(), matches.len());
        assert!(fit.rank_separation_ratio() > MINIMUM_RANK_RATIO);
        assert!(fit.root_mean_square_pixels() < 1e-10);
        assert!(fit.maximum_residual_pixels() < 1e-10);
        for correspondence in matches {
            let actual = fit.transform().apply(correspondence.source())?;
            assert!((actual.x() - correspondence.reference().x()).abs() < 1e-10);
            assert!((actual.y() - correspondence.reference().y()).abs() < 1e-10);
        }
        Ok(())
    }

    #[test]
    fn four_well_spread_pairs_define_an_exact_homography() -> TestResult {
        let expected = ProjectiveTransform::new([
            [0.98, 0.01, 20.0],
            [-0.02, 1.03, -10.0],
            [3e-6, -2e-6, 1.0],
        ])?;
        let sources = [
            ImagePoint::new(0.0, 0.0)?,
            ImagePoint::new(4000.0, 0.0)?,
            ImagePoint::new(0.0, 3000.0)?,
            ImagePoint::new(4000.0, 3000.0)?,
        ];
        let matches = sources
            .into_iter()
            .map(|source| Ok(RegistrationMatch::new(source, expected.apply(source)?)))
            .collect::<Result<Vec<_>, CoordinateError>>()?;
        let fit = fit_projective(&matches)?;
        assert!(fit.maximum_residual_pixels() < 1e-10);
        Ok(())
    }

    #[test]
    fn remains_stable_for_large_offset_coordinates() -> TestResult {
        let expected = ProjectiveTransform::new([
            [1.0, 0.002, 5000.0],
            [-0.001, 0.999, -8000.0],
            [2e-10, -4e-10, 1.0],
        ])?;
        let mut matches = Vec::new();
        for y in [1e9, 1e9 + 1000.0, 1e9 + 2000.0] {
            for x in [-2e9, -2e9 + 1500.0, -2e9 + 3000.0] {
                let source = ImagePoint::new(x, y)?;
                matches.push(RegistrationMatch::new(source, expected.apply(source)?));
            }
        }
        let fit = fit_projective(&matches)?;
        assert!(fit.root_mean_square_pixels() < 1e-5);
        Ok(())
    }

    #[test]
    fn least_squares_fit_reports_bounded_deterministic_noise() -> TestResult {
        let expected = ProjectiveTransform::new([
            [1.002, -0.001, 8.0],
            [0.0015, 0.998, -5.0],
            [8e-7, -6e-7, 1.0],
        ])?;
        let mut matches = Vec::new();
        for row in 0..5 {
            for column in 0..5 {
                let source = ImagePoint::new(f64::from(column) * 1000.0, f64::from(row) * 700.0)?;
                let exact = expected.apply(source)?;
                let ordinal = row * 5 + column;
                let noise_x = (ordinal % 5) as f64 * 0.004 - 0.008;
                let noise_y = (ordinal * 3 % 7) as f64 * 0.003 - 0.009;
                let reference = ImagePoint::new(exact.x() + noise_x, exact.y() + noise_y)?;
                matches.push(RegistrationMatch::new(source, reference));
            }
        }
        let fit = fit_projective(&matches)?;
        assert!(fit.root_mean_square_pixels() < 0.015);
        assert!(fit.maximum_residual_pixels() < 0.025);
        Ok(())
    }

    #[test]
    fn rejects_insufficient_excessive_and_collinear_correspondences() -> TestResult {
        let point = ImagePoint::new(0.0, 0.0)?;
        let one = RegistrationMatch::new(point, point);
        assert!(matches!(
            fit_projective(&[one; 3]),
            Err(ProjectiveFitError::InsufficientMatches { .. })
        ));
        let too_many = vec![one; MAX_PROJECTIVE_FIT_MATCHES + 1];
        assert!(matches!(
            fit_projective(&too_many),
            Err(ProjectiveFitError::TooManyMatches { .. })
        ));
        let collinear = (0..8)
            .map(|index| {
                let coordinate = f64::from(index);
                let source = ImagePoint::new(coordinate, 2.0 * coordinate)?;
                let reference = ImagePoint::new(3.0 * coordinate, 6.0 * coordinate)?;
                Ok(RegistrationMatch::new(source, reference))
            })
            .collect::<Result<Vec<_>, CoordinateError>>()?;
        assert_eq!(
            fit_projective(&collinear),
            Err(ProjectiveFitError::DegenerateGeometry)
        );
        Ok(())
    }
}
