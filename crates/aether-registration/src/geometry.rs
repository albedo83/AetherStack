use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_core::CompensatedSum;

/// One finite continuous image coordinate in source pixel units.
///
/// Pixel centers use integer coordinates: the center of the first pixel is
/// `(0, 0)`. `x` increases to the right and `y` increases downward. Transformed
/// points may be negative or outside the source image.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImagePoint {
    x: f64,
    y: f64,
}

impl ImagePoint {
    /// Creates a finite coordinate and canonicalizes signed zero.
    pub fn new(x: f64, y: f64) -> Result<Self, CoordinateError> {
        if !x.is_finite() || !y.is_finite() {
            return Err(CoordinateError::NonFinitePoint);
        }
        Ok(Self {
            x: canonical_zero(x),
            y: canonical_zero(y),
        })
    }

    /// Horizontal coordinate in pixel units.
    #[must_use]
    pub const fn x(self) -> f64 {
        self.x
    }

    /// Vertical coordinate in pixel units.
    #[must_use]
    pub const fn y(self) -> f64 {
        self.y
    }
}

/// Finite, nonsingular affine map from source coordinates to reference coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AffineTransform {
    coefficients: [f64; 6],
}

impl AffineTransform {
    /// Identity mapping.
    pub const IDENTITY: Self = Self {
        coefficients: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
    };

    /// Creates `x' = m00*x + m01*y + tx`, `y' = m10*x + m11*y + ty`.
    pub fn new(
        m00: f64,
        m01: f64,
        m10: f64,
        m11: f64,
        tx: f64,
        ty: f64,
    ) -> Result<Self, CoordinateError> {
        let mut coefficients = [m00, m01, m10, m11, tx, ty];
        if coefficients.iter().any(|value| !value.is_finite()) {
            return Err(CoordinateError::NonFiniteTransform);
        }
        let determinant = m00.mul_add(m11, -(m01 * m10));
        if !determinant.is_finite() || determinant == 0.0 {
            return Err(CoordinateError::SingularTransform);
        }
        coefficients
            .iter_mut()
            .for_each(|value| *value = canonical_zero(*value));
        Ok(Self { coefficients })
    }

    /// Returns `[m00, m01, m10, m11, tx, ty]` in the documented equation order.
    #[must_use]
    pub const fn coefficients(self) -> [f64; 6] {
        self.coefficients
    }

    /// Maps one point into reference coordinates.
    pub fn apply(self, point: ImagePoint) -> Result<ImagePoint, CoordinateError> {
        let [m00, m01, m10, m11, tx, ty] = self.coefficients;
        let x = m00.mul_add(point.x, m01.mul_add(point.y, tx));
        let y = m10.mul_add(point.x, m11.mul_add(point.y, ty));
        ImagePoint::new(x, y).map_err(|_| CoordinateError::TransformOverflow)
    }

    /// Returns the exact algebraic inverse when it remains finite.
    pub fn inverse(self) -> Result<Self, CoordinateError> {
        let [m00, m01, m10, m11, tx, ty] = self.coefficients;
        let determinant = m00.mul_add(m11, -(m01 * m10));
        if !determinant.is_finite() || determinant == 0.0 {
            return Err(CoordinateError::SingularTransform);
        }
        let inverse_determinant = determinant.recip();
        let i00 = m11 * inverse_determinant;
        let i01 = -m01 * inverse_determinant;
        let i10 = -m10 * inverse_determinant;
        let i11 = m00 * inverse_determinant;
        let itx = -i00.mul_add(tx, i01 * ty);
        let ity = -i10.mul_add(tx, i11 * ty);
        Self::new(i00, i01, i10, i11, itx, ity).map_err(|_| CoordinateError::TransformOverflow)
    }

    /// Composes maps in execution order: `self.then(next)` applies `self` first.
    pub fn then(self, next: Self) -> Result<Self, CoordinateError> {
        let [a00, a01, a10, a11, atx, aty] = self.coefficients;
        let [b00, b01, b10, b11, btx, bty] = next.coefficients;
        Self::new(
            b00.mul_add(a00, b01 * a10),
            b00.mul_add(a01, b01 * a11),
            b10.mul_add(a00, b11 * a10),
            b10.mul_add(a01, b11 * a11),
            b00.mul_add(atx, b01.mul_add(aty, btx)),
            b10.mul_add(atx, b11.mul_add(aty, bty)),
        )
        .map_err(|_| CoordinateError::TransformOverflow)
    }
}

/// Coordinate or affine-map validation failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoordinateError {
    /// A point contained NaN or infinity.
    NonFinitePoint,
    /// A transform coefficient contained NaN or infinity.
    NonFiniteTransform,
    /// The two-dimensional linear part has zero determinant.
    SingularTransform,
    /// Application, inversion, or composition left the finite numerical domain.
    TransformOverflow,
}

impl Display for CoordinateError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::NonFinitePoint => "image point is not finite",
            Self::NonFiniteTransform => "affine transform is not finite",
            Self::SingularTransform => "affine transform is singular",
            Self::TransformOverflow => "affine operation exceeded the finite numerical domain",
        })
    }
}

impl Error for CoordinateError {}

/// One source/reference point correspondence.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RegistrationMatch {
    source: ImagePoint,
    reference: ImagePoint,
}

impl RegistrationMatch {
    /// Binds one measured source point to its reference-frame point.
    #[must_use]
    pub const fn new(source: ImagePoint, reference: ImagePoint) -> Self {
        Self { source, reference }
    }

    /// Measured point in the image being transformed.
    #[must_use]
    pub const fn source(self) -> ImagePoint {
        self.source
    }

    /// Corresponding point in the fixed reference image.
    #[must_use]
    pub const fn reference(self) -> ImagePoint {
        self.reference
    }
}

/// Deterministic Euclidean residual summary in reference pixel units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResidualStatistics {
    count: usize,
    mean_pixels: f64,
    root_mean_square_pixels: f64,
    maximum_pixels: f64,
}

impl ResidualStatistics {
    /// Number of correspondences evaluated.
    #[must_use]
    pub const fn count(self) -> usize {
        self.count
    }

    /// Compensated arithmetic mean residual in pixels.
    #[must_use]
    pub const fn mean_pixels(self) -> f64 {
        self.mean_pixels
    }

    /// Root mean square residual in pixels.
    #[must_use]
    pub const fn root_mean_square_pixels(self) -> f64 {
        self.root_mean_square_pixels
    }

    /// Largest residual in pixels.
    #[must_use]
    pub const fn maximum_pixels(self) -> f64 {
        self.maximum_pixels
    }
}

/// Computes residuals without changing correspondence order or discarding outliers.
pub fn evaluate_residuals(
    transform: AffineTransform,
    matches: &[RegistrationMatch],
) -> Result<ResidualStatistics, ResidualError> {
    if matches.is_empty() {
        return Err(ResidualError::NoMatches);
    }
    let mut sum = CompensatedSum::new();
    let mut squared_sum = CompensatedSum::new();
    let mut maximum = 0.0_f64;
    for correspondence in matches {
        let mapped = transform
            .apply(correspondence.source)
            .map_err(ResidualError::Coordinate)?;
        let dx = mapped.x - correspondence.reference.x;
        let dy = mapped.y - correspondence.reference.y;
        let residual = dx.hypot(dy);
        let squared = residual * residual;
        if !residual.is_finite() || !squared.is_finite() {
            return Err(ResidualError::NumericalOverflow);
        }
        sum.add(residual);
        squared_sum.add(squared);
        maximum = maximum.max(residual);
    }
    let count = matches.len();
    let divisor = count as f64;
    let mean = sum.total() / divisor;
    let rms = (squared_sum.total() / divisor).sqrt();
    if !mean.is_finite() || !rms.is_finite() {
        return Err(ResidualError::NumericalOverflow);
    }
    Ok(ResidualStatistics {
        count,
        mean_pixels: canonical_zero(mean),
        root_mean_square_pixels: canonical_zero(rms),
        maximum_pixels: canonical_zero(maximum),
    })
}

/// Residual calculation failure.
#[derive(Debug)]
pub enum ResidualError {
    /// At least one correspondence is required.
    NoMatches,
    /// Mapping a source point failed.
    Coordinate(CoordinateError),
    /// A distance or reduction left the finite domain.
    NumericalOverflow,
}

impl Display for ResidualError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoMatches => formatter.write_str("registration residuals require matches"),
            Self::Coordinate(error) => write!(formatter, "cannot map registration match: {error}"),
            Self::NumericalOverflow => {
                formatter.write_str("registration residual calculation overflowed")
            }
        }
    }
}

impl Error for ResidualError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Coordinate(error) => Some(error),
            Self::NoMatches | Self::NumericalOverflow => None,
        }
    }
}

const fn canonical_zero(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn Error>>;

    #[test]
    fn composes_and_inverts_under_the_pixel_center_convention() -> TestResult {
        let point = ImagePoint::new(2.0, 3.0)?;
        let first = AffineTransform::new(2.0, 0.0, 0.0, 3.0, 5.0, -4.0)?;
        let second = AffineTransform::new(0.0, -1.0, 1.0, 0.0, 10.0, 20.0)?;
        let composed = first.then(second)?;

        assert_eq!(composed.apply(point)?, second.apply(first.apply(point)?)?);
        let restored = composed.inverse()?.apply(composed.apply(point)?)?;
        assert!((restored.x() - point.x()).abs() < 1.0e-12);
        assert!((restored.y() - point.y()).abs() < 1.0e-12);
        Ok(())
    }

    #[test]
    fn rejects_non_finite_and_singular_geometry() -> TestResult {
        assert_eq!(
            ImagePoint::new(f64::NAN, 0.0),
            Err(CoordinateError::NonFinitePoint)
        );
        assert_eq!(
            AffineTransform::new(1.0, 2.0, 2.0, 4.0, 0.0, 0.0),
            Err(CoordinateError::SingularTransform)
        );
        assert_eq!(
            AffineTransform::new(1.0, 0.0, 0.0, 1.0, f64::INFINITY, 0.0),
            Err(CoordinateError::NonFiniteTransform)
        );
        let overflowing = AffineTransform::new(1.0, 0.0, 0.0, 1.0, f64::MAX, 0.0)?;
        let largest = ImagePoint::new(f64::MAX, 0.0)?;
        assert_eq!(
            overflowing.apply(largest),
            Err(CoordinateError::TransformOverflow)
        );
        Ok(())
    }

    #[test]
    fn canonicalizes_signed_zero_and_exposes_match_direction() -> TestResult {
        let source = ImagePoint::new(-0.0, 1.0)?;
        let reference = ImagePoint::new(2.0, -0.0)?;
        let correspondence = RegistrationMatch::new(source, reference);

        assert_eq!(source.x().to_bits(), 0.0_f64.to_bits());
        assert_eq!(reference.y().to_bits(), 0.0_f64.to_bits());
        assert_eq!(correspondence.source(), source);
        assert_eq!(correspondence.reference(), reference);
        Ok(())
    }

    #[test]
    fn reports_exact_zero_and_known_nonzero_residuals() -> TestResult {
        let translation = AffineTransform::new(1.0, 0.0, 0.0, 1.0, 2.0, -1.0)?;
        let exact = RegistrationMatch::new(ImagePoint::new(3.0, 4.0)?, ImagePoint::new(5.0, 3.0)?);
        let offset = RegistrationMatch::new(ImagePoint::new(0.0, 0.0)?, ImagePoint::new(5.0, 3.0)?);
        let statistics = evaluate_residuals(translation, &[exact, offset])?;

        assert_eq!(statistics.count(), 2);
        assert_eq!(statistics.mean_pixels().to_bits(), 2.5_f64.to_bits());
        assert_eq!(
            statistics.root_mean_square_pixels().to_bits(),
            (12.5_f64).sqrt().to_bits()
        );
        assert_eq!(statistics.maximum_pixels().to_bits(), 5.0_f64.to_bits());
        assert!(matches!(
            evaluate_residuals(translation, &[]),
            Err(ResidualError::NoMatches)
        ));
        Ok(())
    }
}
