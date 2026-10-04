use crate::{AffineTransform, CoordinateError, ImagePoint};

/// Canonical finite homography from source pixels to reference pixels.
///
/// The matrix acts on homogeneous column vectors. Matrices that differ only by
/// a nonzero global scale describe the same map; construction removes that
/// ambiguity by dividing by the first coefficient with maximum absolute value.
/// Pixel centers follow [`ImagePoint`]: the top-left center is `(0, 0)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectiveTransform {
    coefficients: [[f64; 3]; 3],
}

impl ProjectiveTransform {
    /// Identity mapping.
    pub const IDENTITY: Self = Self {
        coefficients: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    };

    /// Validates and canonicalizes one row-major 3 × 3 homography.
    pub fn new(mut coefficients: [[f64; 3]; 3]) -> Result<Self, CoordinateError> {
        if coefficients
            .iter()
            .flatten()
            .any(|value| !value.is_finite())
        {
            return Err(CoordinateError::NonFiniteTransform);
        }

        let mut pivot = 0.0_f64;
        for value in coefficients.iter().flatten().copied() {
            if value.abs() > pivot.abs() {
                pivot = value;
            }
        }
        if pivot == 0.0 {
            return Err(CoordinateError::SingularTransform);
        }
        for value in coefficients.iter_mut().flatten() {
            *value = canonical_zero(*value / pivot);
        }

        let determinant = determinant(coefficients);
        if !determinant.is_finite() || determinant == 0.0 {
            return Err(CoordinateError::SingularTransform);
        }
        Ok(Self { coefficients })
    }

    /// Lifts an affine source-to-reference map without changing its action.
    pub fn from_affine(transform: AffineTransform) -> Result<Self, CoordinateError> {
        let [m00, m01, m10, m11, tx, ty] = transform.coefficients();
        Self::new([[m00, m01, tx], [m10, m11, ty], [0.0, 0.0, 1.0]])
    }

    /// Returns the canonical row-major matrix.
    #[must_use]
    pub const fn coefficients(self) -> [[f64; 3]; 3] {
        self.coefficients
    }

    /// Returns whether the map has no perspective division terms.
    #[must_use]
    pub fn is_affine(self) -> bool {
        self.coefficients[2][0] == 0.0 && self.coefficients[2][1] == 0.0
    }

    /// Maps one finite source point into reference coordinates.
    pub fn apply(self, point: ImagePoint) -> Result<ImagePoint, CoordinateError> {
        let matrix = self.coefficients;
        let denominator =
            matrix[2][0].mul_add(point.x(), matrix[2][1].mul_add(point.y(), matrix[2][2]));
        if !denominator.is_finite() || denominator == 0.0 {
            return Err(CoordinateError::TransformOverflow);
        }
        let x = matrix[0][0].mul_add(point.x(), matrix[0][1].mul_add(point.y(), matrix[0][2]))
            / denominator;
        let y = matrix[1][0].mul_add(point.x(), matrix[1][1].mul_add(point.y(), matrix[1][2]))
            / denominator;
        ImagePoint::new(x, y).map_err(|_| CoordinateError::TransformOverflow)
    }

    /// Returns the inverse map in canonical scale.
    pub fn inverse(self) -> Result<Self, CoordinateError> {
        let m = self.coefficients;
        // The adjugate represents the inverse up to the nonzero determinant.
        // Omitting that common division avoids overflow before canonicalization.
        Self::new([
            [
                minor(m[1][1], m[1][2], m[2][1], m[2][2]),
                -minor(m[0][1], m[0][2], m[2][1], m[2][2]),
                minor(m[0][1], m[0][2], m[1][1], m[1][2]),
            ],
            [
                -minor(m[1][0], m[1][2], m[2][0], m[2][2]),
                minor(m[0][0], m[0][2], m[2][0], m[2][2]),
                -minor(m[0][0], m[0][2], m[1][0], m[1][2]),
            ],
            [
                minor(m[1][0], m[1][1], m[2][0], m[2][1]),
                -minor(m[0][0], m[0][1], m[2][0], m[2][1]),
                minor(m[0][0], m[0][1], m[1][0], m[1][1]),
            ],
        ])
        .map_err(|_| CoordinateError::TransformOverflow)
    }

    /// Composes maps in execution order: `self.then(next)` applies `self` first.
    pub fn then(self, next: Self) -> Result<Self, CoordinateError> {
        let mut product = [[0.0_f64; 3]; 3];
        for (row, product_row) in product.iter_mut().enumerate() {
            for (column, value) in product_row.iter_mut().enumerate() {
                *value = next.coefficients[row][0].mul_add(
                    self.coefficients[0][column],
                    next.coefficients[row][1].mul_add(
                        self.coefficients[1][column],
                        next.coefficients[row][2] * self.coefficients[2][column],
                    ),
                );
            }
        }
        Self::new(product).map_err(|_| CoordinateError::TransformOverflow)
    }
}

fn minor(a: f64, b: f64, c: f64, d: f64) -> f64 {
    a.mul_add(d, -(b * c))
}

fn determinant(matrix: [[f64; 3]; 3]) -> f64 {
    matrix[0][0].mul_add(
        minor(matrix[1][1], matrix[1][2], matrix[2][1], matrix[2][2]),
        matrix[0][1].mul_add(
            -minor(matrix[1][0], matrix[1][2], matrix[2][0], matrix[2][2]),
            matrix[0][2] * minor(matrix[1][0], matrix[1][1], matrix[2][0], matrix[2][1]),
        ),
    )
}

const fn canonical_zero(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;

    type TestResult = Result<(), Box<dyn Error>>;

    #[test]
    fn canonicalizes_global_scale_and_signed_zero() -> TestResult {
        let base = [[2.0, -0.0, 6.0], [0.0, 2.0, -4.0], [2e-4, -4e-4, 2.0]];
        let scaled = base.map(|row| row.map(|value| value * -1e150));
        let first = ProjectiveTransform::new(base)?;
        let second = ProjectiveTransform::new(scaled)?;

        for (left, right) in first
            .coefficients()
            .iter()
            .flatten()
            .zip(second.coefficients().iter().flatten())
        {
            assert!((left - right).abs() < 1e-15);
        }
        assert_eq!(first.coefficients()[0][1].to_bits(), 0.0_f64.to_bits());
        assert!(!first.is_affine());
        Ok(())
    }

    #[test]
    fn affine_lift_has_identical_action() -> TestResult {
        let affine = AffineTransform::new(0.9, -0.2, 0.2, 0.9, 12.0, -7.0)?;
        let projective = ProjectiveTransform::from_affine(affine)?;
        let point = ImagePoint::new(123.25, 45.5)?;
        let affine_point = affine.apply(point)?;
        let projective_point = projective.apply(point)?;

        assert!(projective.is_affine());
        assert!((projective_point.x() - affine_point.x()).abs() < 1e-13);
        assert!((projective_point.y() - affine_point.y()).abs() < 1e-13);
        Ok(())
    }

    #[test]
    fn inverse_and_composition_round_trip_projective_points() -> TestResult {
        let transform =
            ProjectiveTransform::new([[1.1, 0.02, 7.0], [-0.03, 0.9, -4.0], [2e-5, -3e-5, 1.0]])?;
        let point = ImagePoint::new(4000.25, 2700.75)?;
        let mapped = transform.apply(point)?;
        let restored = transform.inverse()?.apply(mapped)?;
        let identity = transform.then(transform.inverse()?)?;

        assert!((restored.x() - point.x()).abs() < 1e-11);
        assert!((restored.y() - point.y()).abs() < 1e-11);
        assert!((identity.apply(point)?.x() - point.x()).abs() < 1e-11);
        assert!((identity.apply(point)?.y() - point.y()).abs() < 1e-11);
        Ok(())
    }

    #[test]
    fn rejects_invalid_matrices_and_points_on_the_projective_horizon() -> TestResult {
        assert_eq!(
            ProjectiveTransform::new([[0.0; 3]; 3]),
            Err(CoordinateError::SingularTransform)
        );
        assert_eq!(
            ProjectiveTransform::new([[1.0, 0.0, f64::NAN], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0],]),
            Err(CoordinateError::NonFiniteTransform)
        );
        let horizon =
            ProjectiveTransform::new([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 0.0, -2.0]])?;
        assert_eq!(
            horizon.apply(ImagePoint::new(2.0, 0.0)?),
            Err(CoordinateError::TransformOverflow)
        );
        Ok(())
    }
}
