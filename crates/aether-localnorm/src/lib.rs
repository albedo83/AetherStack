//! Deterministic robust primitives for local photometric normalization.
//!
//! Deterministic masked grid sampling feeds a strict per-cell oracle. Protected
//! source detection, surface regularization, and FITS orchestration remain
//! separate layers so none of them can silently change the fitted statistic.

use std::error::Error;
use std::fmt::{Display, Formatter};

mod application;
mod protection;
mod sampling;
mod surface;

pub use application::{
    ApplicationError, LOCAL_APPLICATION_ALGORITHM_ID, LocalApplication, LocalApplicationEvidence,
    apply_local_surfaces,
};
pub use protection::{
    PROTECTED_SOURCE_MASK_ALGORITHM_ID, ProtectedSource, ProtectionError, ProtectionMask,
    ProtectionParameters, build_protection_mask, protected_sources_from_stars,
};
pub use sampling::{
    CELL_SAMPLING_ALGORITHM_ID, CellBounds, CellSamplingEvidence, LocalCellFit, LocalCellSamples,
    SamplingError, SamplingGridParameters, SpatialSample, fit_local_grid, sample_local_grid,
};
pub use surface::{
    LOCAL_SURFACE_ALGORITHM_ID, LocalCoefficientSurface, SurfaceError, SurfaceEvaluation,
    SurfaceParameters, build_local_surface,
};

/// Stable identity of the first bounded Theil-Sen local affine fit.
pub const LOCAL_AFFINE_FIT_ALGORITHM_ID: &str = "local-theil-sen-affine-f64-v1";

/// One clear finite source/reference sample pair from matching coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NormalizationSample {
    source: f64,
    reference: f64,
}

impl NormalizationSample {
    /// Validates a pair without changing either scientific value.
    pub fn new(source: f64, reference: f64) -> Result<Self, LocalFitError> {
        if !source.is_finite() || !reference.is_finite() {
            return Err(LocalFitError::NonFiniteSample);
        }
        Ok(Self { source, reference })
    }

    /// Source-image value.
    #[must_use]
    pub const fn source(self) -> f64 {
        self.source
    }

    /// Reference-image value at the same coordinate.
    #[must_use]
    pub const fn reference(self) -> f64 {
        self.reference
    }
}

/// Explicit support and degeneracy limits for one local fit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocalFitParameters {
    minimum_samples: usize,
    maximum_samples: usize,
    maximum_pairwise_slopes: usize,
    minimum_absolute_scale: f64,
}

impl LocalFitParameters {
    /// Builds bounded fitting controls.
    pub fn new(
        minimum_samples: usize,
        maximum_samples: usize,
        maximum_pairwise_slopes: usize,
        minimum_absolute_scale: f64,
    ) -> Result<Self, LocalFitError> {
        if minimum_samples < 2
            || maximum_samples < minimum_samples
            || maximum_pairwise_slopes == 0
            || !minimum_absolute_scale.is_finite()
            || minimum_absolute_scale < 0.0
        {
            return Err(LocalFitError::InvalidParameters);
        }
        Ok(Self {
            minimum_samples,
            maximum_samples,
            maximum_pairwise_slopes,
            minimum_absolute_scale: canonical_zero(minimum_absolute_scale),
        })
    }

    /// Minimum number of coordinate pairs required.
    #[must_use]
    pub const fn minimum_samples(self) -> usize {
        self.minimum_samples
    }

    /// Hard bound on coordinate pairs accepted by one fit.
    #[must_use]
    pub const fn maximum_samples(self) -> usize {
        self.maximum_samples
    }

    /// Hard bound on pairwise slopes allocated and evaluated.
    #[must_use]
    pub const fn maximum_pairwise_slopes(self) -> usize {
        self.maximum_pairwise_slopes
    }

    /// Inclusive lower bound for the fitted scale magnitude.
    #[must_use]
    pub const fn minimum_absolute_scale(self) -> f64 {
        self.minimum_absolute_scale
    }
}

/// Robust local relation `reference = scale * source + offset`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocalAffineFit {
    scale: f64,
    offset: f64,
    sample_count: usize,
    slope_count: usize,
    median_absolute_residual: f64,
}

impl LocalAffineFit {
    /// Multiplicative source-to-reference scale.
    #[must_use]
    pub const fn scale(self) -> f64 {
        self.scale
    }

    /// Additive source-to-reference offset.
    #[must_use]
    pub const fn offset(self) -> f64 {
        self.offset
    }

    /// Coordinate pairs included in the fit.
    #[must_use]
    pub const fn sample_count(self) -> usize {
        self.sample_count
    }

    /// Finite nonvertical pairwise slopes that supported the fit.
    #[must_use]
    pub const fn slope_count(self) -> usize {
        self.slope_count
    }

    /// Median absolute residual in reference-image units.
    #[must_use]
    pub const fn median_absolute_residual(self) -> f64 {
        self.median_absolute_residual
    }

    /// Applies the fitted photometric relation and rejects overflow.
    pub fn apply(self, source: f64) -> Result<f64, LocalFitError> {
        if !source.is_finite() {
            return Err(LocalFitError::NonFiniteSample);
        }
        let normalized = self.scale.mul_add(source, self.offset);
        if !normalized.is_finite() {
            return Err(LocalFitError::NonFiniteModel);
        }
        Ok(canonical_zero(normalized))
    }
}

/// Failure to validate or solve one local affine normalization cell.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalFitError {
    /// At least one control is zero or the bounds are inconsistent.
    InvalidParameters,
    /// A source or reference sample is NaN or infinite.
    NonFiniteSample,
    /// The supplied sample count is outside the explicit bounds.
    SampleCountOutsideBounds,
    /// The requested pairwise work exceeds its configured bound.
    PairwiseWorkLimitExceeded,
    /// Every source coordinate is identical at binary64 precision.
    DegenerateSource,
    /// The fitted multiplicative scale is too close to zero for safe use.
    ScaleBelowMinimum,
    /// Scratch allocation failed.
    AllocationFailed,
    /// The fitted coefficients or a transformed value are not finite.
    NonFiniteModel,
}

impl Display for LocalFitError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidParameters => "local-fit bounds are inconsistent",
            Self::NonFiniteSample => "local-fit samples must be finite",
            Self::SampleCountOutsideBounds => "local-fit sample count is outside its bounds",
            Self::PairwiseWorkLimitExceeded => "local-fit pairwise work exceeds its bound",
            Self::DegenerateSource => "local-fit source samples have no finite variation",
            Self::ScaleBelowMinimum => "local-fit scale is below its configured safety floor",
            Self::AllocationFailed => "local-fit scratch allocation failed",
            Self::NonFiniteModel => "local-fit result is outside the finite binary64 domain",
        })
    }
}

impl Error for LocalFitError {}

/// Fits a bounded Theil-Sen scale and median intercept.
///
/// Both axes are normalized independently before slope construction, avoiding
/// overflow in pair differences. Duplicate source values contribute no slope.
/// The final median intercept and median absolute residual use every input pair.
pub fn fit_local_affine(
    samples: &[NormalizationSample],
    parameters: LocalFitParameters,
) -> Result<LocalAffineFit, LocalFitError> {
    fit_local_affine_iter(samples.iter().copied(), samples.len(), parameters)
}

pub(crate) fn fit_local_affine_iter<I>(
    samples: I,
    sample_count: usize,
    parameters: LocalFitParameters,
) -> Result<LocalAffineFit, LocalFitError>
where
    I: Clone + Iterator<Item = NormalizationSample>,
{
    if sample_count < parameters.minimum_samples || sample_count > parameters.maximum_samples {
        return Err(LocalFitError::SampleCountOutsideBounds);
    }
    let pair_count = sample_count
        .checked_mul(sample_count.saturating_sub(1))
        .and_then(|value| value.checked_div(2))
        .ok_or(LocalFitError::PairwiseWorkLimitExceeded)?;
    if pair_count > parameters.maximum_pairwise_slopes {
        return Err(LocalFitError::PairwiseWorkLimitExceeded);
    }
    let source_scale = samples
        .clone()
        .fold(0.0_f64, |scale, sample| scale.max(sample.source.abs()));
    let reference_scale = samples
        .clone()
        .fold(0.0_f64, |scale, sample| scale.max(sample.reference.abs()));
    if source_scale == 0.0 {
        return Err(LocalFitError::DegenerateSource);
    }
    let reference_scale = if reference_scale == 0.0 {
        1.0
    } else {
        reference_scale
    };

    let mut slopes = Vec::new();
    slopes
        .try_reserve_exact(pair_count)
        .map_err(|_| LocalFitError::AllocationFailed)?;
    for (left_index, left) in samples.clone().enumerate() {
        for right in samples.clone().skip(left_index + 1) {
            let source_difference = right.source / source_scale - left.source / source_scale;
            if source_difference == 0.0 {
                continue;
            }
            let reference_difference =
                right.reference / reference_scale - left.reference / reference_scale;
            let direct_source_difference = right.source - left.source;
            let direct_reference_difference = right.reference - left.reference;
            let slope = if direct_source_difference.is_finite()
                && direct_source_difference != 0.0
                && direct_reference_difference.is_finite()
            {
                direct_reference_difference / direct_source_difference
            } else {
                (reference_difference / source_difference) * (reference_scale / source_scale)
            };
            if slope.is_finite() {
                slopes.push(slope);
            }
        }
    }
    if slopes.is_empty() {
        return Err(LocalFitError::DegenerateSource);
    }
    let scale = exact_median(&mut slopes);
    if !scale.is_finite() {
        return Err(LocalFitError::NonFiniteModel);
    }
    if scale.abs() < parameters.minimum_absolute_scale {
        return Err(LocalFitError::ScaleBelowMinimum);
    }

    let mut intercepts = Vec::new();
    intercepts
        .try_reserve_exact(sample_count)
        .map_err(|_| LocalFitError::AllocationFailed)?;
    for sample in samples.clone() {
        let intercept = (-scale).mul_add(sample.source, sample.reference);
        if !intercept.is_finite() {
            return Err(LocalFitError::NonFiniteModel);
        }
        intercepts.push(intercept);
    }
    let offset = exact_median(&mut intercepts);
    if !offset.is_finite() {
        return Err(LocalFitError::NonFiniteModel);
    }

    let mut residuals = Vec::new();
    residuals
        .try_reserve_exact(sample_count)
        .map_err(|_| LocalFitError::AllocationFailed)?;
    for sample in samples {
        let prediction = scale.mul_add(sample.source, offset);
        let residual = (sample.reference - prediction).abs();
        if !residual.is_finite() {
            return Err(LocalFitError::NonFiniteModel);
        }
        residuals.push(residual);
    }
    let median_absolute_residual = exact_median(&mut residuals);
    Ok(LocalAffineFit {
        scale: canonical_zero(scale),
        offset: canonical_zero(offset),
        sample_count,
        slope_count: slopes.len(),
        median_absolute_residual: canonical_zero(median_absolute_residual),
    })
}

fn exact_median(values: &mut [f64]) -> f64 {
    let length = values.len();
    let middle = length / 2;
    let (lower, upper, _) = values.select_nth_unstable_by(middle, f64::total_cmp);
    if !length.is_multiple_of(2) {
        return canonical_zero(*upper);
    }
    let lower = lower
        .iter()
        .copied()
        .max_by(f64::total_cmp)
        .unwrap_or(*upper);
    let midpoint = if lower.is_sign_negative() == upper.is_sign_negative() {
        lower + (*upper - lower) * 0.5
    } else {
        lower * 0.5 + *upper * 0.5
    };
    canonical_zero(midpoint)
}

fn canonical_zero(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn Error>>;

    fn sample(source: f64, reference: f64) -> Result<NormalizationSample, LocalFitError> {
        NormalizationSample::new(source, reference)
    }

    fn parameters() -> Result<LocalFitParameters, LocalFitError> {
        LocalFitParameters::new(3, 16, 120, 1.0e-12)
    }

    #[test]
    fn recovers_exact_scale_offset_and_zero_residual() -> TestResult {
        let samples = [-4.0, -1.0, 0.0, 2.0, 7.0]
            .into_iter()
            .map(|value| sample(value, 2.5 * value - 3.0))
            .collect::<Result<Vec<_>, _>>()?;
        let fit = fit_local_affine(&samples, parameters()?)?;
        assert_eq!(fit.scale().to_bits(), 2.5_f64.to_bits());
        assert_eq!(fit.offset().to_bits(), (-3.0_f64).to_bits());
        assert_eq!(fit.median_absolute_residual().to_bits(), 0.0_f64.to_bits());
        assert_eq!(fit.apply(8.0)?.to_bits(), 17.0_f64.to_bits());
        assert_eq!(fit.sample_count(), 5);
        assert_eq!(fit.slope_count(), 10);
        Ok(())
    }

    #[test]
    fn one_reference_outlier_does_not_move_the_model() -> TestResult {
        let samples = [
            sample(0.0, 1.0)?,
            sample(1.0, 3.0)?,
            sample(2.0, 5.0)?,
            sample(3.0, 7.0)?,
            sample(4.0, 1000.0)?,
        ];
        let fit = fit_local_affine(&samples, parameters()?)?;
        assert_eq!(fit.scale().to_bits(), 2.0_f64.to_bits());
        assert_eq!(fit.offset().to_bits(), 1.0_f64.to_bits());
        assert_eq!(fit.median_absolute_residual().to_bits(), 0.0_f64.to_bits());
        Ok(())
    }

    #[test]
    fn opposite_sign_extremes_use_the_overflow_safe_slope_path() -> TestResult {
        let magnitude = f64::MAX * 0.75;
        let samples = [
            sample(-magnitude, -magnitude * 0.25)?,
            sample(0.0, 0.0)?,
            sample(magnitude, magnitude * 0.25)?,
        ];
        assert!((samples[2].source() - samples[0].source()).is_infinite());
        let fit = fit_local_affine(&samples, parameters()?)?;
        assert_eq!(fit.scale().to_bits(), 0.25_f64.to_bits());
        assert_eq!(fit.offset().to_bits(), 0.0_f64.to_bits());
        assert_eq!(fit.median_absolute_residual().to_bits(), 0.0_f64.to_bits());
        Ok(())
    }

    #[test]
    fn duplicate_source_values_are_skipped_but_accounted() -> TestResult {
        let samples = [
            sample(1.0, 3.0)?,
            sample(1.0, 3.0)?,
            sample(2.0, 5.0)?,
            sample(3.0, 7.0)?,
        ];
        let fit = fit_local_affine(&samples, parameters()?)?;
        assert_eq!(fit.scale().to_bits(), 2.0_f64.to_bits());
        assert_eq!(fit.offset().to_bits(), 1.0_f64.to_bits());
        assert_eq!(fit.slope_count(), 5);
        Ok(())
    }

    #[test]
    fn validation_and_degeneracy_fail_closed() -> TestResult {
        assert_eq!(
            NormalizationSample::new(f64::NAN, 1.0),
            Err(LocalFitError::NonFiniteSample)
        );
        assert_eq!(
            LocalFitParameters::new(1, 4, 6, 0.0),
            Err(LocalFitError::InvalidParameters)
        );
        assert_eq!(
            LocalFitParameters::new(2, 4, 6, f64::NAN),
            Err(LocalFitError::InvalidParameters)
        );
        let controls = LocalFitParameters::new(2, 3, 3, 0.0)?;
        assert_eq!(
            fit_local_affine(&[sample(1.0, 2.0)?], controls),
            Err(LocalFitError::SampleCountOutsideBounds)
        );
        assert_eq!(
            fit_local_affine(&[sample(1.0, 2.0)?, sample(1.0, 3.0)?], controls),
            Err(LocalFitError::DegenerateSource)
        );
        assert_eq!(
            LocalFitParameters::new(2, 4, 2, 0.0).and_then(|value| fit_local_affine(
                &[sample(0.0, 0.0)?, sample(1.0, 1.0)?, sample(2.0, 2.0)?],
                value
            )
            .map(|_| value)),
            Err(LocalFitError::PairwiseWorkLimitExceeded)
        );
        let flat_reference = [sample(0.0, 1.0)?, sample(1.0, 1.0)?, sample(2.0, 1.0)?];
        assert_eq!(
            fit_local_affine(&flat_reference, LocalFitParameters::new(3, 3, 3, 0.01)?),
            Err(LocalFitError::ScaleBelowMinimum)
        );
        Ok(())
    }
}
