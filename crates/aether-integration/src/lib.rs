//! Deterministic strict-reference image integration.
//!
//! The strict unweighted and weighted means are transparent CPU oracles.
//! Separately versioned percentile, sigma, and Winsorized-sigma means add
//! deterministic low/high rejection with exact per-pixel evidence. Linear-fit
//! clipping models the ordered sample distribution without assuming equal
//! exposure scale or background offset. Generalized ESD adds a conservative
//! two-sided normal-population test with Student-t critical values.

use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_core::{CompensatedSum, CoreError, Dimensions, PixelFlags, ScientificImage};
use statrs::distribution::{ContinuousCDF, StudentsT};

mod attribution;
mod large_scale;

pub use attribution::{
    RejectionAttribution, RejectionAttributionError, SampleDisposition, SourceDispositionCounts,
};
pub use large_scale::{
    LARGE_SCALE_REJECTION_ALGORITHM_ID, LargeScaleExpansionSummary, LargeScaleRejectionError,
    LargeScaleRejectionParameterError, LargeScaleRejectionParameters, LargeScaleRejectionTail,
    LargeScaleTailParameters, MAXIMUM_LARGE_SCALE_GROWTH, MAXIMUM_LARGE_SCALE_LAYERS,
    MINIMUM_LARGE_SCALE_LAYERS, classify_large_scale_seeds, expand_large_scale_rejections,
    grow_large_scale_seeds,
};

/// Plane-major low/high count map emitted from percentile support evidence.
pub const PERCENTILE_REJECTION_MAP_ALGORITHM_ID: &str = "percentile-rejection-map-v1";

/// Stable identifier for the strict, frame-weighted arithmetic mean contract.
pub const WEIGHTED_MEAN_ALGORITHM_ID: &str = "weighted-mean-v1";

/// Stable identifier for exact finite-sample median integration.
pub const MEDIAN_ALGORITHM_ID: &str = "median-f64-v1";

/// Stable identifier for deterministic iterative sigma-clipped integration.
pub const SIGMA_CLIPPED_MEAN_ALGORITHM_ID: &str = "sigma-clipped-mean-f64-v1";

/// Stable identifier for sigma clipping with Winsorized population statistics.
pub const WINSORIZED_SIGMA_CLIPPED_MEAN_ALGORITHM_ID: &str = "winsorized-sigma-clipped-mean-v1";

/// Stable identifier for ordered-sample linear-fit rejection.
pub const LINEAR_FIT_CLIPPED_MEAN_ALGORITHM_ID: &str = "linear-fit-clipped-mean-f64-v1";

/// Stable identifier for two-sided generalized ESD rejection.
pub const GENERALIZED_ESD_CLIPPED_MEAN_ALGORITHM_ID: &str = "generalized-esd-mean-f64-v1";

/// Smallest usable population admitted by the generalized ESD approximation.
pub const GENERALIZED_ESD_MINIMUM_SAMPLE_COUNT: u32 = 15;

/// Stable identifier for the first transparent PSF quality-weight expression.
pub const BALANCED_PSF_WEIGHT_ALGORITHM_ID: &str = "balanced-psf-weight-v1";

/// A finite, strictly positive frame weight.
///
/// Keeping validation in this type makes it impossible to start integration
/// with a zero, negative, NaN, or infinite weight. Absolute scale has no
/// scientific meaning: multiplying every frame weight by the same positive
/// factor leaves the integrated image unchanged.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameWeight(f64);

impl FrameWeight {
    /// Validates one dimensionless frame weight.
    ///
    /// # Errors
    ///
    /// Returns [`FrameWeightError`] unless `value` is finite and greater than
    /// zero.
    pub fn new(value: f64) -> Result<Self, FrameWeightError> {
        if !value.is_finite() || value <= 0.0 {
            return Err(FrameWeightError);
        }
        Ok(Self(value))
    }

    /// Returns the validated dimensionless value.
    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }
}

/// A scalar cannot represent a valid frame weight.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameWeightError;

impl Display for FrameWeightError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("frame weight must be finite and strictly positive")
    }
}

impl Error for FrameWeightError {}

/// Validated frame metrics consumed by the balanced PSF weight expression.
///
/// Signal-to-noise and FWHM must describe the same estimator and image scale
/// across the complete stack. Eccentricity follows the conventional
/// `sqrt(1 - minor_variance / major_variance)` definition.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QualityWeightMetrics {
    signal_to_noise: f64,
    fwhm_pixels: f64,
    eccentricity: f64,
}

impl QualityWeightMetrics {
    /// Validates one comparable set of PSF quality metrics.
    ///
    /// # Errors
    ///
    /// Returns [`QualityWeightError`] unless signal-to-noise and FWHM are
    /// finite and positive and eccentricity is finite in `[0, 1)`.
    pub fn new(
        signal_to_noise: f64,
        fwhm_pixels: f64,
        eccentricity: f64,
    ) -> Result<Self, QualityWeightError> {
        if !signal_to_noise.is_finite() || signal_to_noise <= 0.0 {
            return Err(QualityWeightError::InvalidSignalToNoise);
        }
        if !fwhm_pixels.is_finite() || fwhm_pixels <= 0.0 {
            return Err(QualityWeightError::InvalidFwhm);
        }
        if !eccentricity.is_finite() || !(0.0..1.0).contains(&eccentricity) {
            return Err(QualityWeightError::InvalidEccentricity);
        }
        Ok(Self {
            signal_to_noise,
            fwhm_pixels,
            eccentricity: canonical_zero(eccentricity),
        })
    }

    /// Signal-to-noise statistic from a versioned stellar estimator.
    #[must_use]
    pub const fn signal_to_noise(self) -> f64 {
        self.signal_to_noise
    }

    /// Representative major-axis FWHM in source pixels.
    #[must_use]
    pub const fn fwhm_pixels(self) -> f64 {
        self.fwhm_pixels
    }

    /// Representative stellar eccentricity in `[0, 1)`.
    #[must_use]
    pub const fn eccentricity(self) -> f64 {
        self.eccentricity
    }
}

/// Invalid input to the versioned quality-weight expression.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QualityWeightError {
    /// Signal-to-noise was zero, negative, NaN, or infinite.
    InvalidSignalToNoise,
    /// FWHM was zero, negative, NaN, or infinite.
    InvalidFwhm,
    /// Eccentricity was outside the finite half-open interval `[0, 1)`.
    InvalidEccentricity,
}

impl Display for QualityWeightError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidSignalToNoise => {
                "quality-weight signal-to-noise must be finite and positive"
            }
            Self::InvalidFwhm => "quality-weight FWHM must be finite and positive",
            Self::InvalidEccentricity => "quality-weight eccentricity must be finite and in [0, 1)",
        })
    }
}

impl Error for QualityWeightError {}

/// Calculates the versioned balanced PSF weight relative to a reference frame.
///
/// The dimensionless expression is
///
/// `w = (SNR/SNR_ref)^2 * (FWHM_ref/FWHM)^2 * (1-e^2)/(1-e_ref^2)`.
///
/// It rewards stellar signal, penalizes broad PSFs, and uses the squared
/// minor-to-major axis ratio as an explicit shape penalty. Evaluation occurs
/// in the logarithmic domain, then clamps only at the representable positive
/// `f64` boundaries. A frame identical to the reference has exactly unit
/// weight. The expression and its identifier are provenance data; changing
/// either requires a new algorithm identifier.
#[must_use]
pub fn balanced_psf_weight(
    metrics: QualityWeightMetrics,
    reference: QualityWeightMetrics,
) -> FrameWeight {
    if metrics == reference {
        return FrameWeight(1.0);
    }
    let signal_term = 2.0 * stable_ln_ratio(metrics.signal_to_noise, reference.signal_to_noise);
    let resolution_term = 2.0 * stable_ln_ratio(reference.fwhm_pixels, metrics.fwhm_pixels);
    let frame_roundness = 1.0 - metrics.eccentricity * metrics.eccentricity;
    let reference_roundness = 1.0 - reference.eccentricity * reference.eccentricity;
    let shape_term = stable_ln_ratio(frame_roundness, reference_roundness);
    let logarithmic_weight = signal_term + resolution_term + shape_term;
    let bounded = logarithmic_weight.clamp(f64::MIN_POSITIVE.ln(), f64::MAX.ln());
    // Both exponential bounds are finite and strictly positive. Constructing
    // directly avoids a redundant branch while preserving FrameWeight's type
    // invariant.
    FrameWeight(bounded.exp())
}

fn stable_ln_ratio(numerator: f64, denominator: f64) -> f64 {
    let ratio = numerator / denominator;
    if ratio.is_finite() && ratio > 0.0 {
        ratio.ln()
    } else {
        numerator.ln() - denominator.ln()
    }
}

/// Per-pixel accounting for one mean integration.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PixelSupport {
    accepted: u32,
    masked: u32,
    non_finite: u32,
}

/// Per-pixel accounting for deterministic rank-based rejection.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ClippedPixelSupport {
    accepted: u32,
    masked: u32,
    non_finite: u32,
    low_rejected: u32,
    high_rejected: u32,
}

impl ClippedPixelSupport {
    /// Finite samples retained in the final mean.
    #[must_use]
    pub const fn accepted(self) -> u32 {
        self.accepted
    }

    /// Samples excluded by a non-clear quality mask.
    #[must_use]
    pub const fn masked(self) -> u32 {
        self.masked
    }

    /// Unmasked NaN or infinite samples.
    #[must_use]
    pub const fn non_finite(self) -> u32 {
        self.non_finite
    }

    /// Finite samples rejected from the low end of sorted rank order.
    #[must_use]
    pub const fn low_rejected(self) -> u32 {
        self.low_rejected
    }

    /// Finite samples rejected from the high end of sorted rank order.
    #[must_use]
    pub const fn high_rejected(self) -> u32 {
        self.high_rejected
    }

    /// Total input samples represented by this record.
    #[must_use]
    pub const fn total(self) -> u32 {
        self.accepted + self.masked + self.non_finite + self.low_rejected + self.high_rejected
    }
}

/// Validated controls for deterministic percentile rejection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PercentileClipParameters {
    low_fraction: f64,
    high_fraction: f64,
    minimum_retained: u32,
}

impl PercentileClipParameters {
    /// Validates exclusive low/high fractions and the retained-sample floor.
    pub fn new(
        low_fraction: f64,
        high_fraction: f64,
        minimum_retained: u32,
    ) -> Result<Self, IntegrationError> {
        if !low_fraction.is_finite()
            || !high_fraction.is_finite()
            || !(0.0..1.0).contains(&low_fraction)
            || !(0.0..1.0).contains(&high_fraction)
            || low_fraction + high_fraction >= 1.0
            || minimum_retained == 0
        {
            return Err(IntegrationError::InvalidPercentileParameters);
        }
        Ok(Self {
            low_fraction,
            high_fraction,
            minimum_retained,
        })
    }

    /// Fraction rejected from the low sorted tail.
    #[must_use]
    pub const fn low_fraction(self) -> f64 {
        self.low_fraction
    }

    /// Fraction rejected from the high sorted tail.
    #[must_use]
    pub const fn high_fraction(self) -> f64 {
        self.high_fraction
    }

    /// Minimum finite samples required after clipping.
    #[must_use]
    pub const fn minimum_retained(self) -> u32 {
        self.minimum_retained
    }
}

/// Validated controls for deterministic iterative sigma rejection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SigmaClipParameters {
    low_sigma: f64,
    high_sigma: f64,
    maximum_iterations: u32,
    minimum_retained: u32,
}

impl SigmaClipParameters {
    /// Validates asymmetric sigma limits and iteration/support bounds.
    pub fn new(
        low_sigma: f64,
        high_sigma: f64,
        maximum_iterations: u32,
        minimum_retained: u32,
    ) -> Result<Self, IntegrationError> {
        if !low_sigma.is_finite()
            || low_sigma <= 0.0
            || !high_sigma.is_finite()
            || high_sigma <= 0.0
            || maximum_iterations == 0
            || minimum_retained == 0
        {
            return Err(IntegrationError::InvalidSigmaParameters);
        }
        Ok(Self {
            low_sigma,
            high_sigma,
            maximum_iterations,
            minimum_retained,
        })
    }

    /// Lower-tail rejection distance in population standard deviations.
    #[must_use]
    pub const fn low_sigma(self) -> f64 {
        self.low_sigma
    }

    /// Upper-tail rejection distance in population standard deviations.
    #[must_use]
    pub const fn high_sigma(self) -> f64 {
        self.high_sigma
    }

    /// Hard bound on deterministic clipping passes.
    #[must_use]
    pub const fn maximum_iterations(self) -> u32 {
        self.maximum_iterations
    }

    /// Minimum finite support that a clipping pass may retain.
    #[must_use]
    pub const fn minimum_retained(self) -> u32 {
        self.minimum_retained
    }
}

/// Validated controls for deterministic ordered-sample linear-fit rejection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinearFitClipParameters {
    low_sigma: f64,
    high_sigma: f64,
    minimum_retained: u32,
}

impl LinearFitClipParameters {
    /// Validates asymmetric residual limits and the retained-sample floor.
    pub fn new(
        low_sigma: f64,
        high_sigma: f64,
        minimum_retained: u32,
    ) -> Result<Self, IntegrationError> {
        if !low_sigma.is_finite()
            || low_sigma <= 0.0
            || !high_sigma.is_finite()
            || high_sigma <= 0.0
            || minimum_retained < 3
        {
            return Err(IntegrationError::InvalidLinearFitParameters);
        }
        Ok(Self {
            low_sigma,
            high_sigma,
            minimum_retained,
        })
    }

    /// Lower-tail rejection distance in residual population deviations.
    #[must_use]
    pub const fn low_sigma(self) -> f64 {
        self.low_sigma
    }

    /// Upper-tail rejection distance in residual population deviations.
    #[must_use]
    pub const fn high_sigma(self) -> f64 {
        self.high_sigma
    }

    /// Minimum finite support retained after one fit decision.
    #[must_use]
    pub const fn minimum_retained(self) -> u32 {
        self.minimum_retained
    }
}

/// Validated controls for deterministic two-sided generalized ESD rejection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeneralizedEsdParameters {
    maximum_outlier_fraction: f64,
    significance: f64,
    minimum_retained: u32,
}

impl GeneralizedEsdParameters {
    /// Validates the suspected-outlier bound, significance, and support floor.
    pub fn new(
        maximum_outlier_fraction: f64,
        significance: f64,
        minimum_retained: u32,
    ) -> Result<Self, IntegrationError> {
        if !maximum_outlier_fraction.is_finite()
            || maximum_outlier_fraction <= 0.0
            || maximum_outlier_fraction > 0.5
            || !significance.is_finite()
            || significance <= 0.0
            || significance >= 1.0
            || minimum_retained < 3
        {
            return Err(IntegrationError::InvalidGeneralizedEsdParameters);
        }
        Ok(Self {
            maximum_outlier_fraction,
            significance,
            minimum_retained,
        })
    }

    /// Upper bound on suspected outliers as a fraction of usable samples.
    #[must_use]
    pub const fn maximum_outlier_fraction(self) -> f64 {
        self.maximum_outlier_fraction
    }

    /// Family-wise two-sided significance level.
    #[must_use]
    pub const fn significance(self) -> f64 {
        self.significance
    }

    /// Minimum finite support retained after rejection.
    #[must_use]
    pub const fn minimum_retained(self) -> u32 {
        self.minimum_retained
    }
}

impl PixelSupport {
    /// Finite, clear samples included in the mean.
    #[must_use]
    pub const fn accepted(self) -> u32 {
        self.accepted
    }

    /// Samples excluded because at least one quality bit was set.
    #[must_use]
    pub const fn masked(self) -> u32 {
        self.masked
    }

    /// Unmasked samples excluded because they were NaN or infinite.
    #[must_use]
    pub const fn non_finite(self) -> u32 {
        self.non_finite
    }

    /// Total input samples represented by this accounting record.
    #[must_use]
    pub const fn total(self) -> u32 {
        self.accepted + self.masked + self.non_finite
    }
}

/// Integrated image and exact per-pixel contribution accounting.
#[derive(Clone, Debug, PartialEq)]
pub struct MeanIntegration {
    image: ScientificImage,
    support: Vec<PixelSupport>,
}

/// Weighted image and exact per-pixel contribution accounting.
#[derive(Clone, Debug, PartialEq)]
pub struct WeightedMeanIntegration {
    image: ScientificImage,
    support: Vec<PixelSupport>,
}

/// Median image and exact per-pixel contribution accounting.
#[derive(Clone, Debug, PartialEq)]
pub struct MedianIntegration {
    image: ScientificImage,
    support: Vec<PixelSupport>,
}

impl MedianIntegration {
    /// Exact finite-sample median image.
    #[must_use]
    pub const fn image(&self) -> &ScientificImage {
        &self.image
    }

    /// Support records in the same planar order as the image samples.
    #[must_use]
    pub fn support(&self) -> &[PixelSupport] {
        &self.support
    }

    /// Consumes the result and returns its image and support map.
    #[must_use]
    pub fn into_parts(self) -> (ScientificImage, Vec<PixelSupport>) {
        (self.image, self.support)
    }
}

impl WeightedMeanIntegration {
    /// Strict frame-weighted mean image.
    #[must_use]
    pub const fn image(&self) -> &ScientificImage {
        &self.image
    }

    /// Support records in the same planar order as the image samples.
    #[must_use]
    pub fn support(&self) -> &[PixelSupport] {
        &self.support
    }

    /// Consumes the result and returns its image and support map.
    #[must_use]
    pub fn into_parts(self) -> (ScientificImage, Vec<PixelSupport>) {
        (self.image, self.support)
    }
}

/// Percentile-clipped image and exact per-pixel rejection accounting.
#[derive(Clone, Debug, PartialEq)]
pub struct PercentileClippedIntegration {
    image: ScientificImage,
    support: Vec<ClippedPixelSupport>,
}

/// Sigma-clipped image and exact per-pixel rejection accounting.
#[derive(Clone, Debug, PartialEq)]
pub struct SigmaClippedIntegration {
    image: ScientificImage,
    support: Vec<ClippedPixelSupport>,
}

/// Linear-fit-clipped image and exact per-pixel rejection accounting.
#[derive(Clone, Debug, PartialEq)]
pub struct LinearFitClippedIntegration {
    image: ScientificImage,
    support: Vec<ClippedPixelSupport>,
}

/// Generalized-ESD-clipped image and exact per-pixel rejection accounting.
#[derive(Clone, Debug, PartialEq)]
pub struct GeneralizedEsdIntegration {
    image: ScientificImage,
    support: Vec<ClippedPixelSupport>,
    attribution: RejectionAttribution,
    large_scale_summary: Option<LargeScaleExpansionSummary>,
}

impl GeneralizedEsdIntegration {
    /// Integrated image after two-sided generalized ESD rejection.
    #[must_use]
    pub const fn image(&self) -> &ScientificImage {
        &self.image
    }

    /// Exact accepted and rejected counts in planar sample order.
    #[must_use]
    pub fn support(&self) -> &[ClippedPixelSupport] {
        &self.support
    }

    /// Exact per-source disposition at every planar sample.
    #[must_use]
    pub const fn attribution(&self) -> &RejectionAttribution {
        &self.attribution
    }

    /// Spatial expansion accounting when large-scale rejection was applied.
    #[must_use]
    pub const fn large_scale_summary(&self) -> Option<LargeScaleExpansionSummary> {
        self.large_scale_summary
    }

    /// Applies source-owned spatial expansion and rebuilds science from the
    /// original accepted samples.
    pub fn apply_large_scale_rejection(
        mut self,
        inputs: &[&ScientificImage],
        parameters: LargeScaleRejectionParameters,
    ) -> Result<Self, IntegrationError> {
        let summary = expand_large_scale_rejections(&mut self.attribution, parameters)
            .map_err(IntegrationError::LargeScale)?;
        rebuild_generalized_esd_output(&mut self, inputs)?;
        self.large_scale_summary = Some(summary);
        Ok(self)
    }

    /// Consumes the result into its scientific image and evidence map.
    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        ScientificImage,
        Vec<ClippedPixelSupport>,
        RejectionAttribution,
    ) {
        (self.image, self.support, self.attribution)
    }
}

impl LinearFitClippedIntegration {
    /// Integrated image after ordered-sample linear-fit rejection.
    #[must_use]
    pub const fn image(&self) -> &ScientificImage {
        &self.image
    }

    /// Exact accepted and rejected counts in planar sample order.
    #[must_use]
    pub fn support(&self) -> &[ClippedPixelSupport] {
        &self.support
    }

    /// Consumes the result into its scientific image and evidence map.
    #[must_use]
    pub fn into_parts(self) -> (ScientificImage, Vec<ClippedPixelSupport>) {
        (self.image, self.support)
    }
}

impl SigmaClippedIntegration {
    /// Integrated image after iterative population-sigma rejection.
    #[must_use]
    pub const fn image(&self) -> &ScientificImage {
        &self.image
    }

    /// Exact accepted and rejected counts in planar sample order.
    #[must_use]
    pub fn support(&self) -> &[ClippedPixelSupport] {
        &self.support
    }

    /// Consumes the result into its scientific image and evidence map.
    #[must_use]
    pub fn into_parts(self) -> (ScientificImage, Vec<ClippedPixelSupport>) {
        (self.image, self.support)
    }
}

impl PercentileClippedIntegration {
    /// Integrated image after rank rejection.
    #[must_use]
    pub const fn image(&self) -> &ScientificImage {
        &self.image
    }

    /// Exact accepted and rejected counts in planar sample order.
    #[must_use]
    pub fn support(&self) -> &[ClippedPixelSupport] {
        &self.support
    }

    /// Consumes the result into its scientific image and evidence map.
    #[must_use]
    pub fn into_parts(self) -> (ScientificImage, Vec<ClippedPixelSupport>) {
        (self.image, self.support)
    }
}

/// Exact low/high rejection-count images in original planar order.
#[derive(Clone, Debug, PartialEq)]
pub struct PercentileRejectionMaps {
    low: ScientificImage,
    high: ScientificImage,
}

impl PercentileRejectionMaps {
    /// Counts rejected from the low sorted tail.
    #[must_use]
    pub const fn low(&self) -> &ScientificImage {
        &self.low
    }

    /// Counts rejected from the high sorted tail.
    #[must_use]
    pub const fn high(&self) -> &ScientificImage {
        &self.high
    }

    /// Consumes both exact count maps.
    #[must_use]
    pub fn into_parts(self) -> (ScientificImage, ScientificImage) {
        (self.low, self.high)
    }
}

/// Materializes exact low/high rejection counts as two scientific images.
///
/// Each result keeps the source dimensions and planar order. Every `u32` count
/// is exactly representable as binary64 and every output mask is clear. Masked
/// and non-finite exclusions remain in the support records and are deliberately
/// not mislabeled as statistical rejects.
///
/// # Errors
///
/// Returns an error when the support length differs from the source sample
/// count.
pub fn materialize_percentile_rejection_map(
    source_dimensions: Dimensions,
    support: &[ClippedPixelSupport],
) -> Result<PercentileRejectionMaps, IntegrationError> {
    if support.len() != source_dimensions.pixel_count() {
        return Err(IntegrationError::SupportLengthMismatch {
            expected: source_dimensions.pixel_count(),
            actual: support.len(),
        });
    }
    let mut low = Vec::new();
    low.try_reserve_exact(source_dimensions.pixel_count())
        .map_err(|_| IntegrationError::SupportAllocationFailed {
            elements: source_dimensions.pixel_count(),
        })?;
    low.extend(support.iter().map(|entry| f64::from(entry.low_rejected)));
    let mut high = Vec::new();
    high.try_reserve_exact(source_dimensions.pixel_count())
        .map_err(|_| IntegrationError::SupportAllocationFailed {
            elements: source_dimensions.pixel_count(),
        })?;
    high.extend(support.iter().map(|entry| f64::from(entry.high_rejected)));
    Ok(PercentileRejectionMaps {
        low: ScientificImage::from_pixels(source_dimensions, low)
            .map_err(IntegrationError::Core)?,
        high: ScientificImage::from_pixels(source_dimensions, high)
            .map_err(IntegrationError::Core)?,
    })
}

/// Inclusive origin and exclusive extent for strict spatial integration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IntegrationRegion {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

impl IntegrationRegion {
    /// Builds a non-empty region whose coordinate additions cannot overflow.
    ///
    /// Bounds against an image are checked by [`integrate_mean_region`].
    pub fn new(x: usize, y: usize, width: usize, height: usize) -> Result<Self, IntegrationError> {
        if width == 0
            || height == 0
            || x.checked_add(width).is_none()
            || y.checked_add(height).is_none()
        {
            return Err(IntegrationError::InvalidRegion {
                x,
                y,
                width,
                height,
            });
        }
        Ok(Self {
            x,
            y,
            width,
            height,
        })
    }

    /// Horizontal origin in source pixels.
    #[must_use]
    pub const fn x(self) -> usize {
        self.x
    }

    /// Vertical origin in source pixels.
    #[must_use]
    pub const fn y(self) -> usize {
        self.y
    }

    /// Output width in pixels.
    #[must_use]
    pub const fn width(self) -> usize {
        self.width
    }

    /// Output height in pixels.
    #[must_use]
    pub const fn height(self) -> usize {
        self.height
    }
}

impl MeanIntegration {
    /// Strict mean image.
    #[must_use]
    pub const fn image(&self) -> &ScientificImage {
        &self.image
    }

    /// Support records in the same planar order as the image samples.
    #[must_use]
    pub fn support(&self) -> &[PixelSupport] {
        &self.support
    }

    /// Consumes the result and returns its image and support map.
    #[must_use]
    pub fn into_parts(self) -> (ScientificImage, Vec<PixelSupport>) {
        (self.image, self.support)
    }
}

/// Failure to construct a strict mean integration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IntegrationError {
    /// At least one input image is required to establish dimensions.
    NoInputImages,
    /// The number of inputs cannot be represented in a support record.
    TooManyInputImages {
        /// Received image count.
        count: usize,
        /// Maximum supported count.
        maximum: u32,
    },
    /// Percentile fractions or the retained-sample floor are invalid.
    InvalidPercentileParameters,
    /// Sigma limits, iteration bound, or retained-sample floor are invalid.
    InvalidSigmaParameters,
    /// Linear-fit residual limits or retained-sample floor are invalid.
    InvalidLinearFitParameters,
    /// Generalized ESD fraction, significance, or support floor is invalid.
    InvalidGeneralizedEsdParameters,
    /// Per-source rejection evidence could not be represented.
    Attribution(RejectionAttributionError),
    /// Source-owned spatial rejection could not be represented or expanded.
    LargeScale(LargeScaleRejectionError),
    /// Rebuild inputs did not match the attribution source count.
    AttributionSourceCountMismatch {
        /// Source identities represented by the attribution cube.
        expected: usize,
        /// Input images supplied for the rebuild.
        actual: usize,
    },
    /// A rebuild input no longer matched its recorded sample disposition.
    AttributionInputMismatch {
        /// Zero-based input image index.
        input_index: usize,
        /// Zero-based planar sample index.
        sample_index: usize,
        /// Recorded sample disposition.
        disposition: SampleDisposition,
    },
    /// An input does not match the first image's dimensions.
    DimensionMismatch {
        /// Zero-based input position.
        input_index: usize,
        /// Required dimensions.
        expected: Dimensions,
        /// Received dimensions.
        actual: Dimensions,
    },
    /// A region is empty or its exclusive bounds overflow `usize`.
    InvalidRegion {
        /// Horizontal origin.
        x: usize,
        /// Vertical origin.
        y: usize,
        /// Requested width.
        width: usize,
        /// Requested height.
        height: usize,
    },
    /// The requested region is not wholly contained in every input plane.
    RegionOutsideInput {
        /// Requested region.
        region: IntegrationRegion,
        /// Common input dimensions.
        dimensions: Dimensions,
    },
    /// A supposedly valid image violated its internal sample/mask length invariant.
    InternalImageInvariant {
        /// Zero-based input position.
        input_index: usize,
    },
    /// Per-pixel support categories did not account for every input.
    InternalAccountingInvariant {
        /// Expected number of categorized inputs.
        expected: u32,
        /// Observed category total.
        actual: u32,
    },
    /// Support-map allocation failed.
    SupportAllocationFailed {
        /// Number of per-pixel records requested.
        elements: usize,
    },
    /// Reusable finite-sample sorting storage could not be reserved.
    SampleAllocationFailed {
        /// Maximum samples retained for one output pixel.
        elements: usize,
    },
    /// A support slice does not describe every source sample exactly once.
    SupportLengthMismatch {
        /// Required support record count.
        expected: usize,
        /// Supplied support record count.
        actual: usize,
    },
    /// Output image allocation or construction failed.
    Core(CoreError),
}

impl Display for IntegrationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoInputImages => formatter.write_str("mean integration requires an input image"),
            Self::TooManyInputImages { count, maximum } => write!(
                formatter,
                "mean integration received {count} images; support map maximum is {maximum}"
            ),
            Self::InvalidPercentileParameters => formatter.write_str(
                "percentile fractions must be finite, nonnegative, total below one, with a positive retained minimum",
            ),
            Self::InvalidSigmaParameters => formatter.write_str(
                "sigma limits must be finite and positive, with positive iteration and retained-sample bounds",
            ),
            Self::InvalidLinearFitParameters => formatter.write_str(
                "linear-fit limits must be finite and positive, with at least three retained samples",
            ),
            Self::InvalidGeneralizedEsdParameters => formatter.write_str(
                "generalized ESD requires a finite outlier fraction in (0, 0.5], significance in (0, 1), and at least three retained samples",
            ),
            Self::Attribution(error) => Display::fmt(error, formatter),
            Self::LargeScale(error) => Display::fmt(error, formatter),
            Self::AttributionSourceCountMismatch { expected, actual } => write!(
                formatter,
                "rejection attribution represents {expected} sources; received {actual} rebuild inputs"
            ),
            Self::AttributionInputMismatch {
                input_index,
                sample_index,
                disposition,
            } => write!(
                formatter,
                "rebuild input {input_index} sample {sample_index} does not match recorded {disposition:?} disposition"
            ),
            Self::DimensionMismatch {
                input_index,
                expected,
                actual,
            } => write!(
                formatter,
                "input {input_index} dimensions {}x{}x{} do not match {}x{}x{}",
                actual.width(),
                actual.height(),
                actual.planes(),
                expected.width(),
                expected.height(),
                expected.planes()
            ),
            Self::InvalidRegion {
                x,
                y,
                width,
                height,
            } => write!(
                formatter,
                "integration region {x},{y} + {width}x{height} is empty or overflows"
            ),
            Self::RegionOutsideInput { region, dimensions } => write!(
                formatter,
                "integration region {},{} + {}x{} exceeds {}x{} input planes",
                region.x(),
                region.y(),
                region.width(),
                region.height(),
                dimensions.width(),
                dimensions.height()
            ),
            Self::InternalImageInvariant { input_index } => write!(
                formatter,
                "input {input_index} violates the image sample/mask length invariant"
            ),
            Self::InternalAccountingInvariant { expected, actual } => write!(
                formatter,
                "pixel support accounts for {actual} inputs; expected {expected}"
            ),
            Self::SupportAllocationFailed { elements } => write!(
                formatter,
                "cannot reserve memory for {elements} pixel support records"
            ),
            Self::SampleAllocationFailed { elements } => write!(
                formatter,
                "cannot reserve memory for {elements} finite samples"
            ),
            Self::SupportLengthMismatch { expected, actual } => write!(
                formatter,
                "rejection-map support has {actual} records; expected {expected}"
            ),
            Self::Core(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for IntegrationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Core(error) => Some(error),
            Self::Attribution(error) => Some(error),
            Self::LargeScale(error) => Some(error),
            Self::NoInputImages
            | Self::TooManyInputImages { .. }
            | Self::InvalidPercentileParameters
            | Self::InvalidSigmaParameters
            | Self::InvalidLinearFitParameters
            | Self::InvalidGeneralizedEsdParameters
            | Self::AttributionSourceCountMismatch { .. }
            | Self::AttributionInputMismatch { .. }
            | Self::DimensionMismatch { .. }
            | Self::InvalidRegion { .. }
            | Self::RegionOutsideInput { .. }
            | Self::InternalImageInvariant { .. }
            | Self::InternalAccountingInvariant { .. }
            | Self::SupportAllocationFailed { .. }
            | Self::SampleAllocationFailed { .. }
            | Self::SupportLengthMismatch { .. } => None,
        }
    }
}

/// Integrates equal-sized images with a strict unweighted arithmetic mean.
///
/// For each pixel, the first pass classifies every input and determines the
/// largest absolute usable value. The second pass accumulates each normalized
/// value divided by the accepted count with Neumaier compensation. Scaling
/// before summation prevents equal large values from overflowing the mean.
///
/// A sample participates only when its mask is clear and its value is finite.
/// When at least one sample participates, the output mask is clear and support
/// records every exclusion. With no usable contribution, the output is NaN and
/// marked `MISSING`; all input mask bits are retained and `INVALID` is added if
/// an unmasked non-finite value was observed.
///
/// Input order is part of the strict execution contract and must follow stable
/// manifest order. Spatial tiling does not change a pixel's reduction order.
///
/// # Errors
///
/// Returns an error for empty input, excessive input count, mismatched
/// dimensions, or fallible output allocation.
pub fn integrate_mean(inputs: &[&ScientificImage]) -> Result<MeanIntegration, IntegrationError> {
    let Some(first) = inputs.first().copied() else {
        return Err(IntegrationError::NoInputImages);
    };
    let dimensions = first.dimensions();
    let region = IntegrationRegion {
        x: 0,
        y: 0,
        width: dimensions.width(),
        height: dimensions.height(),
    };
    integrate_mean_impl(inputs, region)
}

/// Integrates one exact spatial region from equal-sized images.
///
/// The output dimensions are the requested width and height with the original
/// plane count. Samples are read directly from the source images in stable
/// planar order; no full-frame cropped copies are allocated. Classification,
/// compensated arithmetic, mask propagation, and support accounting are
/// identical to [`integrate_mean`].
///
/// # Errors
///
/// Returns an error when the input set is empty, dimensions differ, or the
/// region is not wholly contained in the common input plane.
pub fn integrate_mean_region(
    inputs: &[&ScientificImage],
    region: IntegrationRegion,
) -> Result<MeanIntegration, IntegrationError> {
    integrate_mean_impl(inputs, region)
}

/// Integrates equal-sized images with an exact finite-sample median.
///
/// Clear finite samples are selected with IEEE total ordering. Odd populations
/// return the middle sample exactly. Even populations use an overflow-safe
/// midpoint: same-sign values use a bounded difference, while opposite-sign
/// values are halved before addition. Masked and non-finite samples retain the
/// same conservative support and output-mask semantics as [`integrate_mean`].
/// One reusable allocation holds at most one scalar per input frame.
///
/// # Errors
///
/// Returns a typed error for empty or mismatched input, excessive input count,
/// invariant failure, or bounded allocation failure.
pub fn integrate_median(
    inputs: &[&ScientificImage],
) -> Result<MedianIntegration, IntegrationError> {
    let Some(first) = inputs.first().copied() else {
        return Err(IntegrationError::NoInputImages);
    };
    let input_count =
        u32::try_from(inputs.len()).map_err(|_| IntegrationError::TooManyInputImages {
            count: inputs.len(),
            maximum: u32::MAX,
        })?;
    let dimensions = first.dimensions();
    for (input_index, input) in inputs.iter().enumerate().skip(1) {
        let actual = input.dimensions();
        if actual != dimensions {
            return Err(IntegrationError::DimensionMismatch {
                input_index,
                expected: dimensions,
                actual,
            });
        }
    }

    let mut output =
        ScientificImage::filled(dimensions, f64::NAN).map_err(IntegrationError::Core)?;
    let mut support = Vec::new();
    support
        .try_reserve_exact(dimensions.pixel_count())
        .map_err(|_| IntegrationError::SupportAllocationFailed {
            elements: dimensions.pixel_count(),
        })?;
    support.resize(dimensions.pixel_count(), PixelSupport::default());
    let mut finite = Vec::new();
    finite.try_reserve_exact(inputs.len()).map_err(|_| {
        IntegrationError::SampleAllocationFailed {
            elements: inputs.len(),
        }
    })?;
    let (output_pixels, output_mask) = output.pixels_and_mask_mut();
    for (pixel_index, ((output, output_flags), output_support)) in output_pixels
        .iter_mut()
        .zip(output_mask.as_mut_slice())
        .zip(&mut support)
        .enumerate()
    {
        finite.clear();
        let mut combined_rejected_flags = PixelFlags::CLEAR;
        for (input_index, input) in inputs.iter().enumerate() {
            let (value, flags) = sample_at(input, input_index, pixel_index)?;
            if !flags.is_clear() {
                output_support.masked += 1;
                combined_rejected_flags |= flags;
            } else if !value.is_finite() {
                output_support.non_finite += 1;
            } else {
                output_support.accepted += 1;
                finite.push(value);
            }
        }

        if output_support.total() != input_count {
            return Err(IntegrationError::InternalAccountingInvariant {
                expected: input_count,
                actual: output_support.total(),
            });
        }
        if finite.is_empty() {
            *output = f64::NAN;
            let mut flags = combined_rejected_flags | PixelFlags::MISSING;
            if output_support.non_finite > 0 {
                flags |= PixelFlags::INVALID;
            }
            *output_flags = flags;
            continue;
        }

        *output = exact_median(&mut finite);
        *output_flags = PixelFlags::CLEAR;
    }

    Ok(MedianIntegration {
        image: output,
        support,
    })
}

/// Integrates equal-sized images with explicit, validated frame weights.
///
/// A frame's weight participates only where that frame contributes a clear,
/// finite sample. Masked and non-finite samples are removed from both the
/// numerator and denominator, so missing support cannot dim the result. For
/// every output pixel, values and weights are independently normalized by
/// their largest magnitudes before stable, compensated accumulation. This
/// avoids overflow for finite extremes and makes a common rescaling of all
/// weights numerically invariant.
///
/// Input order is part of the algorithm contract. Callers must supply stable
/// manifest order when bitwise repeatability is required.
///
/// # Errors
///
/// Returns an error for empty input, excessive input count, mismatched
/// dimensions, invariant failure, or fallible output allocation. Invalid
/// scalar weights cannot enter this function because [`FrameWeight`] validates
/// them at construction.
pub fn integrate_weighted_mean(
    inputs: &[(&ScientificImage, FrameWeight)],
) -> Result<WeightedMeanIntegration, IntegrationError> {
    let Some((first, _)) = inputs.first().copied() else {
        return Err(IntegrationError::NoInputImages);
    };
    let input_count =
        u32::try_from(inputs.len()).map_err(|_| IntegrationError::TooManyInputImages {
            count: inputs.len(),
            maximum: u32::MAX,
        })?;
    let dimensions = first.dimensions();
    for (input_index, (input, _)) in inputs.iter().enumerate().skip(1) {
        let actual = input.dimensions();
        if actual != dimensions {
            return Err(IntegrationError::DimensionMismatch {
                input_index,
                expected: dimensions,
                actual,
            });
        }
    }

    let mut output =
        ScientificImage::filled(dimensions, f64::NAN).map_err(IntegrationError::Core)?;
    let mut support = Vec::new();
    support
        .try_reserve_exact(dimensions.pixel_count())
        .map_err(|_| IntegrationError::SupportAllocationFailed {
            elements: dimensions.pixel_count(),
        })?;
    support.resize(dimensions.pixel_count(), PixelSupport::default());

    let (output_pixels, output_mask) = output.pixels_and_mask_mut();
    for (pixel_index, ((output, output_flags), output_support)) in output_pixels
        .iter_mut()
        .zip(output_mask.as_mut_slice())
        .zip(&mut support)
        .enumerate()
    {
        let mut value_scale = 0.0_f64;
        let mut weight_scale = 0.0_f64;
        let mut minimum = f64::INFINITY;
        let mut maximum = f64::NEG_INFINITY;
        let mut combined_rejected_flags = PixelFlags::CLEAR;

        for (input_index, (input, weight)) in inputs.iter().enumerate() {
            let (value, flags) = sample_at(input, input_index, pixel_index)?;
            if !flags.is_clear() {
                output_support.masked += 1;
                combined_rejected_flags |= flags;
            } else if !value.is_finite() {
                output_support.non_finite += 1;
            } else {
                output_support.accepted += 1;
                value_scale = value_scale.max(value.abs());
                weight_scale = weight_scale.max(weight.get());
                minimum = minimum.min(value);
                maximum = maximum.max(value);
            }
        }

        if output_support.total() != input_count {
            return Err(IntegrationError::InternalAccountingInvariant {
                expected: input_count,
                actual: output_support.total(),
            });
        }
        if output_support.accepted == 0 {
            *output = f64::NAN;
            let mut flags = combined_rejected_flags | PixelFlags::MISSING;
            if output_support.non_finite > 0 {
                flags |= PixelFlags::INVALID;
            }
            *output_flags = flags;
            continue;
        }
        if value_scale == 0.0 {
            *output = 0.0;
            *output_flags = PixelFlags::CLEAR;
            continue;
        }

        // Every accepted weight is finite and positive. Normalization bounds
        // each term to (0, 1], while u32-bounded support keeps the sum finite.
        let mut normalized_weight_sum = CompensatedSum::new();
        for (input_index, (input, weight)) in inputs.iter().enumerate() {
            let (value, flags) = sample_at(input, input_index, pixel_index)?;
            if flags.is_clear() && value.is_finite() {
                normalized_weight_sum.add(weight.get() / weight_scale);
            }
        }
        let normalized_weight_sum = normalized_weight_sum.total();

        let mut normalized_mean = CompensatedSum::new();
        for (input_index, (input, weight)) in inputs.iter().enumerate() {
            let (value, flags) = sample_at(input, input_index, pixel_index)?;
            if flags.is_clear() && value.is_finite() {
                let normalized_weight = (weight.get() / weight_scale) / normalized_weight_sum;
                normalized_mean.add((value / value_scale) * normalized_weight);
            }
        }
        let normalized_mean = normalized_mean
            .total()
            .max(minimum / value_scale)
            .min(maximum / value_scale);
        *output = canonical_zero(normalized_mean * value_scale);
        *output_flags = PixelFlags::CLEAR;
    }

    Ok(WeightedMeanIntegration {
        image: output,
        support,
    })
}

/// Integrates equal-sized images after deterministic percentile-tail rejection.
///
/// Clear finite samples are sorted with IEEE total ordering at each pixel.
/// `floor(n * low_fraction)` and `floor(n * high_fraction)` samples are removed
/// from the respective tails only when at least `minimum_retained` samples
/// remain. Masked and non-finite samples are excluded before rank calculation.
/// The retained mean uses the same normalized Neumaier summation as
/// [`integrate_mean`]. Exact low/high counts are retained for future rejection
/// map publication.
///
/// # Errors
///
/// Returns a typed error for empty or mismatched input, excessive input count,
/// invalid controls, invariant failure, or bounded allocation failure.
pub fn integrate_percentile_clipped_mean(
    inputs: &[&ScientificImage],
    parameters: PercentileClipParameters,
) -> Result<PercentileClippedIntegration, IntegrationError> {
    let Some(first) = inputs.first().copied() else {
        return Err(IntegrationError::NoInputImages);
    };
    let input_count =
        u32::try_from(inputs.len()).map_err(|_| IntegrationError::TooManyInputImages {
            count: inputs.len(),
            maximum: u32::MAX,
        })?;
    let dimensions = first.dimensions();
    for (input_index, input) in inputs.iter().enumerate().skip(1) {
        let actual = input.dimensions();
        if actual != dimensions {
            return Err(IntegrationError::DimensionMismatch {
                input_index,
                expected: dimensions,
                actual,
            });
        }
    }
    let mut output =
        ScientificImage::filled(dimensions, f64::NAN).map_err(IntegrationError::Core)?;
    let mut support = Vec::new();
    support
        .try_reserve_exact(dimensions.pixel_count())
        .map_err(|_| IntegrationError::SupportAllocationFailed {
            elements: dimensions.pixel_count(),
        })?;
    support.resize(dimensions.pixel_count(), ClippedPixelSupport::default());
    let mut finite = Vec::new();
    finite.try_reserve_exact(inputs.len()).map_err(|_| {
        IntegrationError::SampleAllocationFailed {
            elements: inputs.len(),
        }
    })?;

    let (output_pixels, output_mask) = output.pixels_and_mask_mut();
    for (pixel_index, ((output, output_flags), output_support)) in output_pixels
        .iter_mut()
        .zip(output_mask.as_mut_slice())
        .zip(&mut support)
        .enumerate()
    {
        finite.clear();
        let mut combined_rejected_flags = PixelFlags::CLEAR;
        for (input_index, input) in inputs.iter().enumerate() {
            let (value, flags) = sample_at(input, input_index, pixel_index)?;
            if !flags.is_clear() {
                output_support.masked += 1;
                combined_rejected_flags |= flags;
            } else if !value.is_finite() {
                output_support.non_finite += 1;
            } else {
                finite.push(value);
            }
        }
        if finite.is_empty() {
            *output = f64::NAN;
            let mut flags = combined_rejected_flags | PixelFlags::MISSING;
            if output_support.non_finite > 0 {
                flags |= PixelFlags::INVALID;
            }
            *output_flags = flags;
        } else {
            finite.sort_by(f64::total_cmp);
            let mut low = fraction_count(finite.len(), parameters.low_fraction);
            let mut high = fraction_count(finite.len(), parameters.high_fraction);
            let retained = finite.len().saturating_sub(low).saturating_sub(high);
            if retained < usize::try_from(parameters.minimum_retained).unwrap_or(usize::MAX) {
                low = 0;
                high = 0;
            }
            let retained = &finite[low..finite.len() - high];
            output_support.low_rejected =
                u32::try_from(low).map_err(|_| IntegrationError::TooManyInputImages {
                    count: inputs.len(),
                    maximum: u32::MAX,
                })?;
            output_support.high_rejected =
                u32::try_from(high).map_err(|_| IntegrationError::TooManyInputImages {
                    count: inputs.len(),
                    maximum: u32::MAX,
                })?;
            output_support.accepted = u32::try_from(retained.len()).map_err(|_| {
                IntegrationError::TooManyInputImages {
                    count: inputs.len(),
                    maximum: u32::MAX,
                }
            })?;
            *output = stable_mean(retained);
            *output_flags = PixelFlags::CLEAR;
        }
        if output_support.total() != input_count {
            return Err(IntegrationError::InternalAccountingInvariant {
                expected: input_count,
                actual: output_support.total(),
            });
        }
    }

    Ok(PercentileClippedIntegration {
        image: output,
        support,
    })
}

/// Integrates equal-sized images after deterministic iterative sigma rejection.
///
/// At each pixel, clear finite samples are sorted once with IEEE total order.
/// Each pass computes a normalized compensated mean and population standard
/// deviation over the retained interval, then rejects values strictly outside
/// the asymmetric sigma limits. Because thresholding a sorted population keeps
/// a contiguous interval, no per-sample side allocation is required. A pass is
/// discarded in full if it would violate `minimum_retained`; convergence or the
/// explicit iteration bound stops the process. The final mean and exact low/high
/// counts use the same strict arithmetic and evidence semantics as percentile
/// clipping.
///
/// # Errors
///
/// Returns a typed error for empty or mismatched input, excessive input count,
/// invalid controls, invariant failure, or bounded allocation failure.
pub fn integrate_sigma_clipped_mean(
    inputs: &[&ScientificImage],
    parameters: SigmaClipParameters,
) -> Result<SigmaClippedIntegration, IntegrationError> {
    integrate_iterative_sigma_clipped_mean(inputs, parameters, false)
}

/// Integrates equal-sized images with iterative Winsorized sigma rejection.
///
/// Rejection limits are applied to the original sorted samples. After the
/// first pass, population statistics are recomputed over the original sample
/// count with rejected tails replaced by their nearest retained boundary.
/// This bounds each outlier's influence without letting successive passes
/// shrink the distribution as aggressively as ordinary sigma clipping. The
/// final science value remains the strict mean of original retained samples;
/// substituted values are used only to estimate the next rejection limits.
///
/// # Errors
///
/// Returns the same bounded validation, allocation, shape, and accounting
/// errors as [`integrate_sigma_clipped_mean`].
pub fn integrate_winsorized_sigma_clipped_mean(
    inputs: &[&ScientificImage],
    parameters: SigmaClipParameters,
) -> Result<SigmaClippedIntegration, IntegrationError> {
    integrate_iterative_sigma_clipped_mean(inputs, parameters, true)
}

/// Integrates equal-sized images after one ordered-sample linear-fit decision.
///
/// At each output pixel, usable finite samples are sorted once. Their values
/// are normalized by the largest magnitude and fitted against symmetric rank
/// coordinates in `(-1, 1)`. Population deviation is measured from the fitted
/// line, then samples strictly outside the asymmetric residual limits are
/// rejected. If that decision would cross `minimum_retained`, it is discarded
/// in full. The final value is the strict mean of original retained samples;
/// no fitted or normalized value enters the science output.
///
/// This oracle intentionally performs one fit. Changing it to iterative
/// refitting would be a different scientific algorithm and requires a new
/// identifier.
pub fn integrate_linear_fit_clipped_mean(
    inputs: &[&ScientificImage],
    parameters: LinearFitClipParameters,
) -> Result<LinearFitClippedIntegration, IntegrationError> {
    let Some(first) = inputs.first().copied() else {
        return Err(IntegrationError::NoInputImages);
    };
    let input_count =
        u32::try_from(inputs.len()).map_err(|_| IntegrationError::TooManyInputImages {
            count: inputs.len(),
            maximum: u32::MAX,
        })?;
    let dimensions = first.dimensions();
    for (input_index, input) in inputs.iter().enumerate().skip(1) {
        let actual = input.dimensions();
        if actual != dimensions {
            return Err(IntegrationError::DimensionMismatch {
                input_index,
                expected: dimensions,
                actual,
            });
        }
    }

    let mut output =
        ScientificImage::filled(dimensions, f64::NAN).map_err(IntegrationError::Core)?;
    let mut support = Vec::new();
    support
        .try_reserve_exact(dimensions.pixel_count())
        .map_err(|_| IntegrationError::SupportAllocationFailed {
            elements: dimensions.pixel_count(),
        })?;
    support.resize(dimensions.pixel_count(), ClippedPixelSupport::default());
    let mut finite = Vec::new();
    finite.try_reserve_exact(inputs.len()).map_err(|_| {
        IntegrationError::SampleAllocationFailed {
            elements: inputs.len(),
        }
    })?;

    let minimum_retained = usize::try_from(parameters.minimum_retained).unwrap_or(usize::MAX);
    let (output_pixels, output_mask) = output.pixels_and_mask_mut();
    for (pixel_index, ((output, output_flags), output_support)) in output_pixels
        .iter_mut()
        .zip(output_mask.as_mut_slice())
        .zip(&mut support)
        .enumerate()
    {
        finite.clear();
        let mut combined_rejected_flags = PixelFlags::CLEAR;
        for (input_index, input) in inputs.iter().enumerate() {
            let (value, flags) = sample_at(input, input_index, pixel_index)?;
            if !flags.is_clear() {
                output_support.masked += 1;
                combined_rejected_flags |= flags;
            } else if !value.is_finite() {
                output_support.non_finite += 1;
            } else {
                finite.push(value);
            }
        }

        if finite.is_empty() {
            *output = f64::NAN;
            let mut flags = combined_rejected_flags | PixelFlags::MISSING;
            if output_support.non_finite > 0 {
                flags |= PixelFlags::INVALID;
            }
            *output_flags = flags;
        } else {
            finite.sort_by(f64::total_cmp);
            let fit = ordered_sample_linear_fit(&finite);
            let mut low_rejected = 0_usize;
            let mut high_rejected = 0_usize;
            if let Some(fit) = fit {
                for (rank, value) in finite.iter().copied().enumerate() {
                    match classify_linear_fit_residual(value, rank, finite.len(), fit, parameters) {
                        LinearFitDecision::Retain => {}
                        LinearFitDecision::RejectLow => low_rejected += 1,
                        LinearFitDecision::RejectHigh => high_rejected += 1,
                    }
                }
            }
            if finite
                .len()
                .saturating_sub(low_rejected)
                .saturating_sub(high_rejected)
                < minimum_retained
            {
                low_rejected = 0;
                high_rejected = 0;
            } else if let Some(fit) = fit {
                let sample_count = finite.len();
                let mut rank = 0_usize;
                finite.retain(|value| {
                    let decision =
                        classify_linear_fit_residual(*value, rank, sample_count, fit, parameters);
                    rank += 1;
                    decision == LinearFitDecision::Retain
                });
            }

            output_support.low_rejected =
                u32::try_from(low_rejected).map_err(|_| IntegrationError::TooManyInputImages {
                    count: inputs.len(),
                    maximum: u32::MAX,
                })?;
            output_support.high_rejected =
                u32::try_from(high_rejected).map_err(|_| IntegrationError::TooManyInputImages {
                    count: inputs.len(),
                    maximum: u32::MAX,
                })?;
            output_support.accepted =
                u32::try_from(finite.len()).map_err(|_| IntegrationError::TooManyInputImages {
                    count: inputs.len(),
                    maximum: u32::MAX,
                })?;
            *output = stable_mean(&finite);
            *output_flags = PixelFlags::CLEAR;
        }

        if output_support.total() != input_count {
            return Err(IntegrationError::InternalAccountingInvariant {
                expected: input_count,
                actual: output_support.total(),
            });
        }
    }

    Ok(LinearFitClippedIntegration {
        image: output,
        support,
    })
}

/// Integrates equal-sized images after a two-sided generalized ESD test.
///
/// The Rosner procedure removes the largest studentized deviation for each
/// candidate count up to the configured fractional bound, calculates the
/// matching Student-t critical value, then rejects through the last significant
/// candidate. Populations below 15 usable samples are retained unchanged
/// because the published critical-value approximation is not admitted there.
/// Original samples, never fitted or normalized substitutes, form the final
/// compensated mean.
pub fn integrate_generalized_esd_mean(
    inputs: &[&ScientificImage],
    parameters: GeneralizedEsdParameters,
) -> Result<GeneralizedEsdIntegration, IntegrationError> {
    let Some(first) = inputs.first().copied() else {
        return Err(IntegrationError::NoInputImages);
    };
    let input_count =
        u32::try_from(inputs.len()).map_err(|_| IntegrationError::TooManyInputImages {
            count: inputs.len(),
            maximum: u32::MAX,
        })?;
    let dimensions = first.dimensions();
    for (input_index, input) in inputs.iter().enumerate().skip(1) {
        let actual = input.dimensions();
        if actual != dimensions {
            return Err(IntegrationError::DimensionMismatch {
                input_index,
                expected: dimensions,
                actual,
            });
        }
    }

    let mut output =
        ScientificImage::filled(dimensions, f64::NAN).map_err(IntegrationError::Core)?;
    let mut attribution = RejectionAttribution::new(dimensions, inputs.len())
        .map_err(IntegrationError::Attribution)?;
    let mut support = Vec::new();
    support
        .try_reserve_exact(dimensions.pixel_count())
        .map_err(|_| IntegrationError::SupportAllocationFailed {
            elements: dimensions.pixel_count(),
        })?;
    support.resize(dimensions.pixel_count(), ClippedPixelSupport::default());
    let mut finite = Vec::new();
    finite.try_reserve_exact(inputs.len()).map_err(|_| {
        IntegrationError::SampleAllocationFailed {
            elements: inputs.len(),
        }
    })?;
    let mut decisions = Vec::new();
    decisions.try_reserve_exact(inputs.len()).map_err(|_| {
        IntegrationError::SampleAllocationFailed {
            elements: inputs.len(),
        }
    })?;
    let mut esd_working = Vec::new();
    esd_working.try_reserve_exact(inputs.len()).map_err(|_| {
        IntegrationError::SampleAllocationFailed {
            elements: inputs.len(),
        }
    })?;
    let mut esd_candidates = Vec::new();
    esd_candidates
        .try_reserve_exact(inputs.len())
        .map_err(|_| IntegrationError::SampleAllocationFailed {
            elements: inputs.len(),
        })?;

    let (output_pixels, output_mask) = output.pixels_and_mask_mut();
    for (pixel_index, ((output, output_flags), output_support)) in output_pixels
        .iter_mut()
        .zip(output_mask.as_mut_slice())
        .zip(&mut support)
        .enumerate()
    {
        finite.clear();
        let mut combined_rejected_flags = PixelFlags::CLEAR;
        for (input_index, input) in inputs.iter().enumerate() {
            let (value, flags) = sample_at(input, input_index, pixel_index)?;
            if !flags.is_clear() {
                output_support.masked += 1;
                combined_rejected_flags |= flags;
            } else if !value.is_finite() {
                output_support.non_finite += 1;
            } else {
                finite.push((input_index, value));
            }
            if !flags.is_clear() {
                attribution
                    .set_disposition(pixel_index, input_index, SampleDisposition::Masked)
                    .map_err(IntegrationError::Attribution)?;
            } else if !value.is_finite() {
                attribution
                    .set_disposition(pixel_index, input_index, SampleDisposition::NonFinite)
                    .map_err(IntegrationError::Attribution)?;
            }
        }

        if finite.is_empty() {
            *output = f64::NAN;
            let mut flags = combined_rejected_flags | PixelFlags::MISSING;
            if output_support.non_finite > 0 {
                flags |= PixelFlags::INVALID;
            }
            *output_flags = flags;
        } else {
            finite.sort_by(|left, right| left.1.total_cmp(&right.1));
            generalized_esd_decisions(
                &finite,
                parameters,
                &mut decisions,
                &mut esd_working,
                &mut esd_candidates,
            );
            for (rank, (source_index, _)) in finite.iter().copied().enumerate() {
                let disposition = match decisions.get(rank).copied().unwrap_or(EsdDecision::Retain)
                {
                    EsdDecision::Retain => SampleDisposition::Accepted,
                    EsdDecision::RejectLow => SampleDisposition::RejectedLow,
                    EsdDecision::RejectHigh => SampleDisposition::RejectedHigh,
                };
                attribution
                    .set_disposition(pixel_index, source_index, disposition)
                    .map_err(IntegrationError::Attribution)?;
            }
            let mut rank = 0_usize;
            finite.retain(|_| {
                let decision = decisions.get(rank).copied().unwrap_or(EsdDecision::Retain);
                rank += 1;
                match decision {
                    EsdDecision::Retain => true,
                    EsdDecision::RejectLow => {
                        output_support.low_rejected += 1;
                        false
                    }
                    EsdDecision::RejectHigh => {
                        output_support.high_rejected += 1;
                        false
                    }
                }
            });
            output_support.accepted =
                u32::try_from(finite.len()).map_err(|_| IntegrationError::TooManyInputImages {
                    count: inputs.len(),
                    maximum: u32::MAX,
                })?;
            *output = stable_mean_indexed(&finite);
            *output_flags = PixelFlags::CLEAR;
        }

        if output_support.total() != input_count {
            return Err(IntegrationError::InternalAccountingInvariant {
                expected: input_count,
                actual: output_support.total(),
            });
        }
    }

    Ok(GeneralizedEsdIntegration {
        image: output,
        support,
        attribution,
        large_scale_summary: None,
    })
}

fn rebuild_generalized_esd_output(
    integration: &mut GeneralizedEsdIntegration,
    inputs: &[&ScientificImage],
) -> Result<(), IntegrationError> {
    let dimensions = integration.attribution.dimensions();
    if inputs.len() != integration.attribution.source_count() {
        return Err(IntegrationError::AttributionSourceCountMismatch {
            expected: integration.attribution.source_count(),
            actual: inputs.len(),
        });
    }
    for (input_index, input) in inputs.iter().enumerate() {
        if input.dimensions() != dimensions {
            return Err(IntegrationError::DimensionMismatch {
                input_index,
                expected: dimensions,
                actual: input.dimensions(),
            });
        }
    }
    let input_count =
        u32::try_from(inputs.len()).map_err(|_| IntegrationError::TooManyInputImages {
            count: inputs.len(),
            maximum: u32::MAX,
        })?;
    let mut retained = Vec::new();
    retained.try_reserve_exact(inputs.len()).map_err(|_| {
        IntegrationError::SampleAllocationFailed {
            elements: inputs.len(),
        }
    })?;
    let (output_pixels, output_mask) = integration.image.pixels_and_mask_mut();
    for (sample_index, ((output, output_flags), support)) in output_pixels
        .iter_mut()
        .zip(output_mask.as_mut_slice())
        .zip(&mut integration.support)
        .enumerate()
    {
        *support = ClippedPixelSupport::default();
        retained.clear();
        let mut combined_masked_flags = PixelFlags::CLEAR;
        for (input_index, input) in inputs.iter().enumerate() {
            let (value, flags) = sample_at(input, input_index, sample_index)?;
            let disposition = integration
                .attribution
                .disposition(sample_index, input_index)
                .map_err(IntegrationError::Attribution)?;
            let matches = match disposition {
                SampleDisposition::Accepted
                | SampleDisposition::RejectedLow
                | SampleDisposition::RejectedHigh => flags.is_clear() && value.is_finite(),
                SampleDisposition::Masked => !flags.is_clear(),
                SampleDisposition::NonFinite => flags.is_clear() && !value.is_finite(),
            };
            if !matches {
                return Err(IntegrationError::AttributionInputMismatch {
                    input_index,
                    sample_index,
                    disposition,
                });
            }
            match disposition {
                SampleDisposition::Accepted => retained.push(value),
                SampleDisposition::Masked => {
                    support.masked += 1;
                    combined_masked_flags |= flags;
                }
                SampleDisposition::NonFinite => support.non_finite += 1,
                SampleDisposition::RejectedLow => support.low_rejected += 1,
                SampleDisposition::RejectedHigh => support.high_rejected += 1,
            }
        }
        support.accepted =
            u32::try_from(retained.len()).map_err(|_| IntegrationError::TooManyInputImages {
                count: inputs.len(),
                maximum: u32::MAX,
            })?;
        if retained.is_empty() {
            *output = f64::NAN;
            *output_flags = combined_masked_flags | PixelFlags::MISSING;
            if support.non_finite > 0 {
                *output_flags |= PixelFlags::INVALID;
            }
        } else {
            retained.sort_by(f64::total_cmp);
            *output = stable_mean(&retained);
            *output_flags = PixelFlags::CLEAR;
        }
        if support.total() != input_count {
            return Err(IntegrationError::InternalAccountingInvariant {
                expected: input_count,
                actual: support.total(),
            });
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EsdDecision {
    Retain,
    RejectLow,
    RejectHigh,
}

#[derive(Clone, Copy, Debug)]
struct EsdCandidate {
    original_rank: usize,
    decision: EsdDecision,
    statistic: f64,
    critical_value: f64,
}

fn generalized_esd_decisions(
    sorted_values: &[(usize, f64)],
    parameters: GeneralizedEsdParameters,
    decisions: &mut Vec<EsdDecision>,
    working: &mut Vec<(usize, f64)>,
    candidates: &mut Vec<EsdCandidate>,
) {
    decisions.clear();
    decisions.resize(sorted_values.len(), EsdDecision::Retain);
    if sorted_values.len() < GENERALIZED_ESD_MINIMUM_SAMPLE_COUNT as usize {
        return;
    }
    let fraction_bound =
        (sorted_values.len() as f64 * parameters.maximum_outlier_fraction).floor() as usize;
    let support_bound = sorted_values
        .len()
        .saturating_sub(parameters.minimum_retained as usize);
    let maximum_outliers = fraction_bound.min(support_bound);
    if maximum_outliers == 0 {
        return;
    }

    working.clear();
    working.extend(
        sorted_values
            .iter()
            .enumerate()
            .map(|(rank, (_, value))| (rank, *value)),
    );
    candidates.clear();
    for _ in 0..maximum_outliers {
        let Some((candidate_index, statistic, decision)) = esd_candidate(working) else {
            break;
        };
        let sample_count = working.len();
        let degrees_of_freedom = (sample_count - 2) as f64;
        let probability = 1.0 - parameters.significance / (2.0 * sample_count as f64);
        let Ok(distribution) = StudentsT::new(0.0, 1.0, degrees_of_freedom) else {
            break;
        };
        let Ok(student_quantile) = distribution.try_inverse_cdf(probability) else {
            break;
        };
        let numerator = (sample_count - 1) as f64 * student_quantile;
        let denominator = ((degrees_of_freedom + student_quantile * student_quantile)
            * sample_count as f64)
            .sqrt();
        let critical_value = numerator / denominator;
        if !critical_value.is_finite() {
            break;
        }
        let (original_rank, _) = working.remove(candidate_index);
        candidates.push(EsdCandidate {
            original_rank,
            decision,
            statistic,
            critical_value,
        });
    }

    let rejected_count = candidates
        .iter()
        .rposition(|candidate| candidate.statistic > candidate.critical_value)
        .map_or(0, |index| index + 1);
    for candidate in candidates.iter().take(rejected_count) {
        decisions[candidate.original_rank] = candidate.decision;
    }
}

fn esd_candidate(samples: &[(usize, f64)]) -> Option<(usize, f64, EsdDecision)> {
    if samples.len() < 3 {
        return None;
    }
    let scale = samples
        .iter()
        .fold(0.0_f64, |maximum, (_, value)| maximum.max(value.abs()));
    if scale == 0.0 {
        return None;
    }
    let divisor = samples.len() as f64;
    let mut mean = CompensatedSum::new();
    for (_, value) in samples {
        mean.add((*value / scale) / divisor);
    }
    let mean = mean.total();
    let mut squared_residuals = CompensatedSum::new();
    for (_, value) in samples {
        let residual = *value / scale - mean;
        squared_residuals.add(residual * residual);
    }
    let sample_deviation = (squared_residuals.total().max(0.0) / (samples.len() - 1) as f64).sqrt();
    if !sample_deviation.is_finite() || sample_deviation <= 32.0 * f64::EPSILON {
        return None;
    }
    let mut candidate_index = 0_usize;
    let mut signed_residual = samples[0].1 / scale - mean;
    for (index, (_, value)) in samples.iter().enumerate().skip(1) {
        let residual = *value / scale - mean;
        if residual.abs() > signed_residual.abs() {
            candidate_index = index;
            signed_residual = residual;
        }
    }
    let decision = if signed_residual.is_sign_negative() {
        EsdDecision::RejectLow
    } else {
        EsdDecision::RejectHigh
    };
    Some((
        candidate_index,
        signed_residual.abs() / sample_deviation,
        decision,
    ))
}

#[derive(Clone, Copy, Debug)]
struct OrderedLinearFit {
    scale: f64,
    intercept: f64,
    slope: f64,
    residual_sigma: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LinearFitDecision {
    Retain,
    RejectLow,
    RejectHigh,
}

fn ordered_sample_linear_fit(values: &[f64]) -> Option<OrderedLinearFit> {
    if values.len() < 3 {
        return None;
    }
    let scale = values
        .iter()
        .fold(0.0_f64, |maximum, value| maximum.max(value.abs()));
    if scale == 0.0 {
        return None;
    }
    let divisor = values.len() as f64;
    let mut mean = CompensatedSum::new();
    let mut covariance = CompensatedSum::new();
    let mut rank_variance = CompensatedSum::new();
    for (rank, value) in values.iter().copied().enumerate() {
        let x = symmetric_rank(rank, values.len());
        let y = value / scale;
        mean.add(y / divisor);
        covariance.add((x * y) / divisor);
        rank_variance.add((x * x) / divisor);
    }
    let rank_variance = rank_variance.total();
    if !rank_variance.is_finite() || rank_variance <= 0.0 {
        return None;
    }
    let intercept = mean.total();
    let slope = covariance.total() / rank_variance;
    let mut squared_residuals = CompensatedSum::new();
    for (rank, value) in values.iter().copied().enumerate() {
        let fitted = intercept + slope * symmetric_rank(rank, values.len());
        let residual = value / scale - fitted;
        squared_residuals.add((residual * residual) / divisor);
    }
    let measured_sigma = squared_residuals.total().max(0.0).sqrt();
    let residual_sigma = if measured_sigma <= 32.0 * f64::EPSILON {
        0.0
    } else {
        measured_sigma
    };
    if !intercept.is_finite() || !slope.is_finite() || !residual_sigma.is_finite() {
        return None;
    }
    Some(OrderedLinearFit {
        scale,
        intercept,
        slope,
        residual_sigma,
    })
}

fn classify_linear_fit_residual(
    value: f64,
    rank: usize,
    sample_count: usize,
    fit: OrderedLinearFit,
    parameters: LinearFitClipParameters,
) -> LinearFitDecision {
    if fit.residual_sigma == 0.0 {
        return LinearFitDecision::Retain;
    }
    let fitted = fit.intercept + fit.slope * symmetric_rank(rank, sample_count);
    let residual = value / fit.scale - fitted;
    if residual < -parameters.low_sigma * fit.residual_sigma {
        LinearFitDecision::RejectLow
    } else if residual > parameters.high_sigma * fit.residual_sigma {
        LinearFitDecision::RejectHigh
    } else {
        LinearFitDecision::Retain
    }
}

fn symmetric_rank(rank: usize, sample_count: usize) -> f64 {
    let numerator = 2.0 * rank as f64 + 1.0 - sample_count as f64;
    numerator / sample_count as f64
}

fn integrate_iterative_sigma_clipped_mean(
    inputs: &[&ScientificImage],
    parameters: SigmaClipParameters,
    winsorized: bool,
) -> Result<SigmaClippedIntegration, IntegrationError> {
    let Some(first) = inputs.first().copied() else {
        return Err(IntegrationError::NoInputImages);
    };
    let input_count =
        u32::try_from(inputs.len()).map_err(|_| IntegrationError::TooManyInputImages {
            count: inputs.len(),
            maximum: u32::MAX,
        })?;
    let dimensions = first.dimensions();
    for (input_index, input) in inputs.iter().enumerate().skip(1) {
        let actual = input.dimensions();
        if actual != dimensions {
            return Err(IntegrationError::DimensionMismatch {
                input_index,
                expected: dimensions,
                actual,
            });
        }
    }

    let mut output =
        ScientificImage::filled(dimensions, f64::NAN).map_err(IntegrationError::Core)?;
    let mut support = Vec::new();
    support
        .try_reserve_exact(dimensions.pixel_count())
        .map_err(|_| IntegrationError::SupportAllocationFailed {
            elements: dimensions.pixel_count(),
        })?;
    support.resize(dimensions.pixel_count(), ClippedPixelSupport::default());
    let mut finite = Vec::new();
    finite.try_reserve_exact(inputs.len()).map_err(|_| {
        IntegrationError::SampleAllocationFailed {
            elements: inputs.len(),
        }
    })?;

    let minimum_retained = usize::try_from(parameters.minimum_retained).unwrap_or(usize::MAX);
    let (output_pixels, output_mask) = output.pixels_and_mask_mut();
    for (pixel_index, ((output, output_flags), output_support)) in output_pixels
        .iter_mut()
        .zip(output_mask.as_mut_slice())
        .zip(&mut support)
        .enumerate()
    {
        finite.clear();
        let mut combined_rejected_flags = PixelFlags::CLEAR;
        for (input_index, input) in inputs.iter().enumerate() {
            let (value, flags) = sample_at(input, input_index, pixel_index)?;
            if !flags.is_clear() {
                output_support.masked += 1;
                combined_rejected_flags |= flags;
            } else if !value.is_finite() {
                output_support.non_finite += 1;
            } else {
                finite.push(value);
            }
        }

        if finite.is_empty() {
            *output = f64::NAN;
            let mut flags = combined_rejected_flags | PixelFlags::MISSING;
            if output_support.non_finite > 0 {
                flags |= PixelFlags::INVALID;
            }
            *output_flags = flags;
        } else {
            finite.sort_by(f64::total_cmp);
            let mut low = 0;
            let mut high = finite.len();
            for _ in 0..parameters.maximum_iterations {
                let retained = &finite[low..high];
                if retained.len() <= minimum_retained {
                    break;
                }
                let (scale, mean, sigma) = if winsorized {
                    normalized_winsorized_population_statistics(&finite, low, high)
                } else {
                    normalized_population_statistics(retained)
                };
                if sigma == 0.0 {
                    break;
                }
                let lower = mean - parameters.low_sigma * sigma;
                let upper = mean + parameters.high_sigma * sigma;
                let next_low = low + retained.partition_point(|value| *value / scale < lower);
                let next_high = low + retained.partition_point(|value| *value / scale <= upper);
                if next_high.saturating_sub(next_low) < minimum_retained {
                    break;
                }
                if next_low == low && next_high == high {
                    break;
                }
                low = next_low;
                high = next_high;
            }

            let retained = &finite[low..high];
            output_support.low_rejected =
                u32::try_from(low).map_err(|_| IntegrationError::TooManyInputImages {
                    count: inputs.len(),
                    maximum: u32::MAX,
                })?;
            output_support.high_rejected = u32::try_from(finite.len() - high).map_err(|_| {
                IntegrationError::TooManyInputImages {
                    count: inputs.len(),
                    maximum: u32::MAX,
                }
            })?;
            output_support.accepted = u32::try_from(retained.len()).map_err(|_| {
                IntegrationError::TooManyInputImages {
                    count: inputs.len(),
                    maximum: u32::MAX,
                }
            })?;
            *output = stable_mean(retained);
            *output_flags = PixelFlags::CLEAR;
        }

        if output_support.total() != input_count {
            return Err(IntegrationError::InternalAccountingInvariant {
                expected: input_count,
                actual: output_support.total(),
            });
        }
    }

    Ok(SigmaClippedIntegration {
        image: output,
        support,
    })
}

fn fraction_count(count: usize, fraction: f64) -> usize {
    ((count as f64) * fraction).floor() as usize
}

fn stable_mean(values: &[f64]) -> f64 {
    let scale = values
        .iter()
        .fold(0.0_f64, |scale, value| scale.max(value.abs()));
    if scale == 0.0 {
        return 0.0;
    }
    let minimum = values[0];
    let maximum = values[values.len() - 1];
    let divisor = values.len() as f64;
    let mut normalized = CompensatedSum::new();
    for value in values {
        normalized.add((value / scale) / divisor);
    }
    canonical_zero(normalized.total().max(minimum / scale).min(maximum / scale) * scale)
}

fn stable_mean_indexed(values: &[(usize, f64)]) -> f64 {
    let scale = values
        .iter()
        .fold(0.0_f64, |scale, (_, value)| scale.max(value.abs()));
    if scale == 0.0 {
        return 0.0;
    }
    let minimum = values[0].1;
    let maximum = values[values.len() - 1].1;
    let divisor = values.len() as f64;
    let mut normalized = CompensatedSum::new();
    for (_, value) in values {
        normalized.add((value / scale) / divisor);
    }
    canonical_zero(normalized.total().max(minimum / scale).min(maximum / scale) * scale)
}

fn normalized_population_statistics(values: &[f64]) -> (f64, f64, f64) {
    let scale = values
        .iter()
        .fold(0.0_f64, |scale, value| scale.max(value.abs()));
    if scale == 0.0 {
        return (1.0, 0.0, 0.0);
    }
    let divisor = values.len() as f64;
    let mut sum = CompensatedSum::new();
    for value in values {
        sum.add((*value / scale) / divisor);
    }
    let mean = sum
        .total()
        .max(values[0] / scale)
        .min(values[values.len() - 1] / scale);
    let mut squared_deviations = CompensatedSum::new();
    for value in values {
        let deviation = *value / scale - mean;
        squared_deviations.add((deviation * deviation) / divisor);
    }
    (scale, mean, squared_deviations.total().max(0.0).sqrt())
}

fn normalized_winsorized_population_statistics(
    sorted_values: &[f64],
    low: usize,
    high: usize,
) -> (f64, f64, f64) {
    let lower = sorted_values[low];
    let upper = sorted_values[high - 1];
    let scale = lower.abs().max(upper.abs());
    if scale == 0.0 {
        return (1.0, 0.0, 0.0);
    }
    let divisor = sorted_values.len() as f64;
    let mut sum = CompensatedSum::new();
    for value in sorted_values {
        sum.add((value.clamp(lower, upper) / scale) / divisor);
    }
    let mean = sum.total().max(lower / scale).min(upper / scale);
    let mut squared_deviations = CompensatedSum::new();
    for value in sorted_values {
        let deviation = value.clamp(lower, upper) / scale - mean;
        squared_deviations.add((deviation * deviation) / divisor);
    }
    (scale, mean, squared_deviations.total().max(0.0).sqrt())
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

fn integrate_mean_impl(
    inputs: &[&ScientificImage],
    region: IntegrationRegion,
) -> Result<MeanIntegration, IntegrationError> {
    let Some(first) = inputs.first().copied() else {
        return Err(IntegrationError::NoInputImages);
    };
    let input_count =
        u32::try_from(inputs.len()).map_err(|_| IntegrationError::TooManyInputImages {
            count: inputs.len(),
            maximum: u32::MAX,
        })?;
    let dimensions = first.dimensions();
    for (input_index, input) in inputs.iter().enumerate().skip(1) {
        let actual = input.dimensions();
        if actual != dimensions {
            return Err(IntegrationError::DimensionMismatch {
                input_index,
                expected: dimensions,
                actual,
            });
        }
    }

    let region_right =
        region
            .x
            .checked_add(region.width)
            .ok_or(IntegrationError::InvalidRegion {
                x: region.x,
                y: region.y,
                width: region.width,
                height: region.height,
            })?;
    let region_bottom =
        region
            .y
            .checked_add(region.height)
            .ok_or(IntegrationError::InvalidRegion {
                x: region.x,
                y: region.y,
                width: region.width,
                height: region.height,
            })?;
    if region.width == 0
        || region.height == 0
        || region_right > dimensions.width()
        || region_bottom > dimensions.height()
    {
        return Err(IntegrationError::RegionOutsideInput { region, dimensions });
    }

    let output_dimensions = Dimensions::new(region.width, region.height, dimensions.planes())
        .map_err(IntegrationError::Core)?;

    let mut output =
        ScientificImage::filled(output_dimensions, f64::NAN).map_err(IntegrationError::Core)?;
    let mut support = Vec::new();
    support
        .try_reserve_exact(output_dimensions.pixel_count())
        .map_err(|_| IntegrationError::SupportAllocationFailed {
            elements: output_dimensions.pixel_count(),
        })?;
    support.resize(output_dimensions.pixel_count(), PixelSupport::default());

    let (output_pixels, output_mask) = output.pixels_and_mask_mut();
    for (output_index, ((output, output_flags), output_support)) in output_pixels
        .iter_mut()
        .zip(output_mask.as_mut_slice())
        .zip(&mut support)
        .enumerate()
    {
        let pixel_index = source_index_for_region(dimensions, region, output_index);
        let mut scale = 0.0_f64;
        let mut minimum = f64::INFINITY;
        let mut maximum = f64::NEG_INFINITY;
        let mut combined_rejected_flags = PixelFlags::CLEAR;

        for (input_index, input) in inputs.iter().enumerate() {
            let (value, flags) = sample_at(input, input_index, pixel_index)?;
            if !flags.is_clear() {
                output_support.masked += 1;
                combined_rejected_flags |= flags;
            } else if !value.is_finite() {
                output_support.non_finite += 1;
            } else {
                output_support.accepted += 1;
                scale = scale.max(value.abs());
                minimum = minimum.min(value);
                maximum = maximum.max(value);
            }
        }

        // Conversion was validated once above; this protects future changes to
        // accounting branches from silently dropping an input category.
        if output_support.total() != input_count {
            return Err(IntegrationError::InternalAccountingInvariant {
                expected: input_count,
                actual: output_support.total(),
            });
        }

        if output_support.accepted == 0 {
            *output = f64::NAN;
            let mut flags = combined_rejected_flags | PixelFlags::MISSING;
            if output_support.non_finite > 0 {
                flags |= PixelFlags::INVALID;
            }
            *output_flags = flags;
            continue;
        }

        *output = if scale == 0.0 {
            0.0
        } else {
            let divisor = f64::from(output_support.accepted);
            let mut normalized_mean = CompensatedSum::new();
            for (input_index, input) in inputs.iter().enumerate() {
                let (value, flags) = sample_at(input, input_index, pixel_index)?;
                if flags.is_clear() && value.is_finite() {
                    normalized_mean.add((value / scale) / divisor);
                }
            }
            let normalized_mean = normalized_mean
                .total()
                .max(minimum / scale)
                .min(maximum / scale);
            canonical_zero(normalized_mean * scale)
        };
        *output_flags = PixelFlags::CLEAR;
    }

    Ok(MeanIntegration {
        image: output,
        support,
    })
}

fn source_index_for_region(
    source: Dimensions,
    region: IntegrationRegion,
    output_index: usize,
) -> usize {
    let output_plane_samples = region.width * region.height;
    let plane = output_index / output_plane_samples;
    let within_plane = output_index % output_plane_samples;
    let row = within_plane / region.width;
    let column = within_plane % region.width;
    plane * source.width() * source.height() + (region.y + row) * source.width() + region.x + column
}

fn sample_at(
    input: &ScientificImage,
    input_index: usize,
    pixel_index: usize,
) -> Result<(f64, PixelFlags), IntegrationError> {
    let value = input
        .pixels()
        .get(pixel_index)
        .copied()
        .ok_or(IntegrationError::InternalImageInvariant { input_index })?;
    let flags = input
        .mask()
        .as_slice()
        .get(pixel_index)
        .copied()
        .ok_or(IntegrationError::InternalImageInvariant { input_index })?;
    Ok((value, flags))
}

fn canonical_zero(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn StdError>>;

    fn image(values: Vec<f64>) -> TestResult<ScientificImage> {
        let dimensions = Dimensions::new(values.len(), 1, 1)?;
        Ok(ScientificImage::from_pixels(dimensions, values)?)
    }

    #[test]
    fn calculates_exact_means_and_support_in_stable_input_order() -> TestResult {
        let first = image(vec![1.0, 10.0])?;
        let second = image(vec![3.0, 20.0])?;
        let third = image(vec![5.0, 30.0])?;

        let result = integrate_mean(&[&first, &second, &third])?;

        assert_eq!(result.image().pixels(), &[3.0, 20.0]);
        assert_eq!(
            result.support(),
            &[
                PixelSupport {
                    accepted: 3,
                    masked: 0,
                    non_finite: 0,
                },
                PixelSupport {
                    accepted: 3,
                    masked: 0,
                    non_finite: 0,
                },
            ]
        );
        Ok(())
    }

    #[test]
    fn median_selects_odd_middle_and_overflow_safe_even_midpoint() -> TestResult {
        let low = image(vec![1.0, f64::MAX, -f64::MAX])?;
        let middle = image(vec![9.0, f64::MAX, f64::MAX])?;
        let high = image(vec![5.0, 1.0, -1.0])?;

        let odd = integrate_median(&[&low, &middle, &high])?;
        assert_eq!(odd.image().pixels()[0].to_bits(), 5.0_f64.to_bits());
        assert_eq!(odd.image().pixels()[1].to_bits(), f64::MAX.to_bits());
        assert_eq!(odd.image().pixels()[2].to_bits(), (-1.0_f64).to_bits());

        let even = integrate_median(&[&low, &middle])?;
        assert_eq!(even.image().pixels()[0].to_bits(), 5.0_f64.to_bits());
        assert_eq!(even.image().pixels()[1].to_bits(), f64::MAX.to_bits());
        assert_eq!(even.image().pixels()[2].to_bits(), 0.0_f64.to_bits());
        Ok(())
    }

    #[test]
    fn median_excludes_unusable_samples_and_retains_support_evidence() -> TestResult {
        let usable = image(vec![7.0, f64::NAN])?;
        let mut masked = image(vec![100.0, 50.0])?;
        masked.mask_mut().as_mut_slice()[0] = PixelFlags::SATURATED;
        masked.mask_mut().as_mut_slice()[1] = PixelFlags::HOT;
        let non_finite = image(vec![f64::INFINITY, f64::NEG_INFINITY])?;

        let result = integrate_median(&[&usable, &masked, &non_finite])?;
        assert_eq!(result.image().pixels()[0].to_bits(), 7.0_f64.to_bits());
        assert!(result.image().mask().as_slice()[0].is_clear());
        assert_eq!(result.support()[0].accepted(), 1);
        assert_eq!(result.support()[0].masked(), 1);
        assert_eq!(result.support()[0].non_finite(), 1);

        assert!(result.image().pixels()[1].is_nan());
        let flags = result.image().mask().as_slice()[1];
        assert!(flags.contains(PixelFlags::HOT));
        assert!(flags.contains(PixelFlags::MISSING));
        assert!(flags.contains(PixelFlags::INVALID));
        assert_eq!(result.support()[1].total(), 3);
        Ok(())
    }

    #[test]
    fn median_rejects_empty_and_mismatched_inputs() -> TestResult {
        assert!(matches!(
            integrate_median(&[]),
            Err(IntegrationError::NoInputImages)
        ));
        let first = image(vec![1.0])?;
        let second = image(vec![1.0, 2.0])?;
        assert!(matches!(
            integrate_median(&[&first, &second]),
            Err(IntegrationError::DimensionMismatch { input_index: 1, .. })
        ));
        Ok(())
    }

    #[test]
    fn excludes_masked_and_non_finite_values_without_invalidating_valid_mean() -> TestResult {
        let first = image(vec![2.0])?;
        let mut masked = image(vec![100.0])?;
        masked.mask_mut().as_mut_slice()[0] = PixelFlags::SATURATED;
        let non_finite = image(vec![f64::NAN])?;

        let result = integrate_mean(&[&first, &masked, &non_finite])?;

        assert_eq!(result.image().pixels()[0].to_bits(), 2.0_f64.to_bits());
        assert!(result.image().mask().as_slice()[0].is_clear());
        assert_eq!(
            result.support()[0],
            PixelSupport {
                accepted: 1,
                masked: 1,
                non_finite: 1,
            }
        );
        Ok(())
    }

    #[test]
    fn marks_pixels_without_support_and_retains_rejection_reasons() -> TestResult {
        let mut saturated = image(vec![10.0])?;
        saturated.mask_mut().as_mut_slice()[0] = PixelFlags::SATURATED;
        let mut unknown = image(vec![20.0])?;
        unknown.mask_mut().as_mut_slice()[0] = PixelFlags::from_bits_retain(0b1000_0000);
        let non_finite = image(vec![f64::INFINITY])?;

        let result = integrate_mean(&[&saturated, &unknown, &non_finite])?;
        let flags = result.image().mask().as_slice()[0];

        assert!(result.image().pixels()[0].is_nan());
        assert!(flags.contains(PixelFlags::MISSING));
        assert!(flags.contains(PixelFlags::INVALID));
        assert!(flags.contains(PixelFlags::SATURATED));
        assert_eq!(flags.bits() & 0b1000_0000, 0b1000_0000);
        assert_eq!(result.support()[0].total(), 3);
        Ok(())
    }

    #[test]
    fn scaled_mean_avoids_overflow_for_large_equal_inputs() -> TestResult {
        let first = image(vec![f64::MAX])?;
        let second = image(vec![f64::MAX])?;

        let result = integrate_mean(&[&first, &second])?;

        assert_eq!(result.image().pixels()[0].to_bits(), f64::MAX.to_bits());
        assert!(result.image().mask().as_slice()[0].is_clear());
        Ok(())
    }

    #[test]
    fn compensated_normalized_mean_preserves_small_residual() -> TestResult {
        let positive = image(vec![1.0e16])?;
        let residual = image(vec![1.0])?;
        let negative = image(vec![-1.0e16])?;

        let result = integrate_mean(&[&positive, &residual, &negative])?;
        let expected = 1.0_f64 / 3.0;

        assert!((result.image().pixels()[0] - expected).abs() <= f64::EPSILON);
        Ok(())
    }

    #[test]
    fn frame_weights_reject_every_non_positive_or_non_finite_value() {
        for invalid in [0.0, -0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(FrameWeight::new(invalid), Err(FrameWeightError));
        }
        assert_eq!(
            FrameWeight::new(f64::MIN_POSITIVE).map(FrameWeight::get),
            Ok(f64::MIN_POSITIVE)
        );
        assert_eq!(
            FrameWeight::new(f64::MAX).map(FrameWeight::get),
            Ok(f64::MAX)
        );
    }

    #[test]
    fn quality_weight_metrics_fail_closed() {
        assert_eq!(
            QualityWeightMetrics::new(0.0, 2.0, 0.2),
            Err(QualityWeightError::InvalidSignalToNoise)
        );
        assert_eq!(
            QualityWeightMetrics::new(10.0, f64::NAN, 0.2),
            Err(QualityWeightError::InvalidFwhm)
        );
        for eccentricity in [-0.1, 1.0, f64::INFINITY] {
            assert_eq!(
                QualityWeightMetrics::new(10.0, 2.0, eccentricity),
                Err(QualityWeightError::InvalidEccentricity)
            );
        }
        assert_eq!(
            QualityWeightMetrics::new(10.0, 2.0, -0.0)
                .map(QualityWeightMetrics::eccentricity)
                .map(f64::to_bits),
            Ok(0.0_f64.to_bits())
        );
    }

    #[test]
    fn balanced_psf_weight_has_auditable_monotonic_terms() -> TestResult {
        let reference = QualityWeightMetrics::new(10.0, 2.0, 0.0)?;
        assert_eq!(
            balanced_psf_weight(reference, reference).get().to_bits(),
            1.0_f64.to_bits()
        );

        let stronger_signal = QualityWeightMetrics::new(20.0, 2.0, 0.0)?;
        let narrower_psf = QualityWeightMetrics::new(10.0, 1.0, 0.0)?;
        let elongated_psf = QualityWeightMetrics::new(10.0, 2.0, 0.75_f64.sqrt())?;
        assert!((balanced_psf_weight(stronger_signal, reference).get() - 4.0).abs() < 1.0e-14);
        assert!((balanced_psf_weight(narrower_psf, reference).get() - 4.0).abs() < 1.0e-14);
        assert!((balanced_psf_weight(elongated_psf, reference).get() - 0.25).abs() < 1.0e-14);
        Ok(())
    }

    #[test]
    fn balanced_psf_weight_keeps_extreme_valid_metrics_representable() -> TestResult {
        let best = QualityWeightMetrics::new(f64::MAX, f64::MIN_POSITIVE, 0.0)?;
        let worst = QualityWeightMetrics::new(f64::MIN_POSITIVE, f64::MAX, 1.0 - f64::EPSILON)?;

        for weight in [
            balanced_psf_weight(best, worst),
            balanced_psf_weight(worst, best),
        ] {
            assert!(weight.get().is_finite());
            assert!(weight.get() > 0.0);
        }
        Ok(())
    }

    #[test]
    fn weighted_mean_uses_exact_frame_weights_and_support() -> TestResult {
        let first = image(vec![10.0])?;
        let second = image(vec![20.0])?;
        let third = image(vec![40.0])?;
        let inputs = [
            (&first, FrameWeight::new(1.0)?),
            (&second, FrameWeight::new(2.0)?),
            (&third, FrameWeight::new(1.0)?),
        ];

        let result = integrate_weighted_mean(&inputs)?;

        assert_eq!(result.image().pixels(), &[22.5]);
        assert_eq!(
            result.support(),
            &[PixelSupport {
                accepted: 3,
                masked: 0,
                non_finite: 0,
            }]
        );
        Ok(())
    }

    #[test]
    fn weighted_mean_removes_excluded_samples_from_both_sums() -> TestResult {
        let valid = image(vec![7.0])?;
        let mut masked = image(vec![1_000.0])?;
        masked.mask_mut().as_mut_slice()[0] = PixelFlags::SATURATED;
        let non_finite = image(vec![f64::NAN])?;
        let inputs = [
            (&valid, FrameWeight::new(1.0)?),
            (&masked, FrameWeight::new(f64::MAX)?),
            (&non_finite, FrameWeight::new(f64::MAX)?),
        ];

        let result = integrate_weighted_mean(&inputs)?;

        assert_eq!(result.image().pixels(), &[7.0]);
        assert_eq!(result.support()[0].accepted(), 1);
        assert_eq!(result.support()[0].masked(), 1);
        assert_eq!(result.support()[0].non_finite(), 1);
        Ok(())
    }

    #[test]
    fn weighted_mean_is_invariant_to_common_weight_scale() -> TestResult {
        let first = image(vec![10.0])?;
        let second = image(vec![30.0])?;
        let ordinary = integrate_weighted_mean(&[
            (&first, FrameWeight::new(1.0)?),
            (&second, FrameWeight::new(2.0)?),
        ])?;
        let extreme = integrate_weighted_mean(&[
            (&first, FrameWeight::new(f64::MAX / 2.0)?),
            (&second, FrameWeight::new(f64::MAX)?),
        ])?;

        assert_eq!(
            ordinary.image().pixels()[0].to_bits(),
            extreme.image().pixels()[0].to_bits()
        );
        Ok(())
    }

    #[test]
    fn weighted_mean_handles_extreme_values_and_compensated_cancellation() -> TestResult {
        let maximum_a = image(vec![f64::MAX, 1.0e16])?;
        let maximum_b = image(vec![f64::MAX, 1.0])?;
        let maximum_c = image(vec![f64::MAX, -1.0e16])?;
        let unit = FrameWeight::new(1.0)?;

        let result =
            integrate_weighted_mean(&[(&maximum_a, unit), (&maximum_b, unit), (&maximum_c, unit)])?;

        assert_eq!(result.image().pixels()[0].to_bits(), f64::MAX.to_bits());
        assert!((result.image().pixels()[1] - 1.0 / 3.0).abs() <= f64::EPSILON);
        Ok(())
    }

    #[test]
    fn weighted_mean_preserves_missing_pixel_evidence_and_dimension_errors() -> TestResult {
        let mut masked = image(vec![3.0])?;
        masked.mask_mut().as_mut_slice()[0] = PixelFlags::HOT;
        let invalid = image(vec![f64::INFINITY])?;
        let unit = FrameWeight::new(1.0)?;
        let missing = integrate_weighted_mean(&[(&masked, unit), (&invalid, unit)])?;

        assert!(missing.image().pixels()[0].is_nan());
        let flags = missing.image().mask().as_slice()[0];
        assert!(flags.contains(PixelFlags::HOT));
        assert!(flags.contains(PixelFlags::MISSING));
        assert!(flags.contains(PixelFlags::INVALID));

        let other = image(vec![1.0, 2.0])?;
        assert!(matches!(
            integrate_weighted_mean(&[(&masked, unit), (&other, unit)]),
            Err(IntegrationError::DimensionMismatch { input_index: 1, .. })
        ));
        assert_eq!(
            integrate_weighted_mean(&[]),
            Err(IntegrationError::NoInputImages)
        );
        Ok(())
    }

    #[test]
    fn percentile_clipping_rejects_exact_sorted_tails_and_records_maps() -> TestResult {
        let inputs = [0.0, 10.0, 11.0, 12.0, 100.0]
            .into_iter()
            .map(|value| image(vec![value]))
            .collect::<Result<Vec<_>, _>>()?;
        let references = inputs.iter().collect::<Vec<_>>();
        let parameters = PercentileClipParameters::new(0.2, 0.2, 3)?;

        let result = integrate_percentile_clipped_mean(&references, parameters)?;

        assert_eq!(result.image().pixels(), &[11.0]);
        assert_eq!(
            result.support(),
            &[ClippedPixelSupport {
                accepted: 3,
                masked: 0,
                non_finite: 0,
                low_rejected: 1,
                high_rejected: 1,
            }]
        );
        Ok(())
    }

    #[test]
    fn percentile_clipping_keeps_mask_and_non_finite_evidence_separate() -> TestResult {
        let valid = image(vec![2.0])?;
        let mut masked = image(vec![100.0])?;
        masked.mask_mut().as_mut_slice()[0] = PixelFlags::SATURATED;
        let invalid = image(vec![f64::NAN])?;
        let parameters = PercentileClipParameters::new(0.25, 0.25, 1)?;

        let result = integrate_percentile_clipped_mean(&[&valid, &masked, &invalid], parameters)?;

        assert_eq!(result.image().pixels(), &[2.0]);
        assert_eq!(result.support()[0].accepted(), 1);
        assert_eq!(result.support()[0].masked(), 1);
        assert_eq!(result.support()[0].non_finite(), 1);
        assert_eq!(result.support()[0].low_rejected(), 0);
        assert_eq!(result.support()[0].high_rejected(), 0);
        assert_eq!(result.support()[0].total(), 3);
        Ok(())
    }

    #[test]
    fn percentile_clipping_respects_the_minimum_and_large_value_precision() -> TestResult {
        let first = image(vec![f64::MAX])?;
        let second = image(vec![f64::MAX])?;
        let parameters = PercentileClipParameters::new(0.49, 0.49, 2)?;

        let result = integrate_percentile_clipped_mean(&[&first, &second], parameters)?;

        assert_eq!(result.image().pixels()[0].to_bits(), f64::MAX.to_bits());
        assert_eq!(result.support()[0].accepted(), 2);
        assert_eq!(result.support()[0].low_rejected(), 0);
        assert_eq!(result.support()[0].high_rejected(), 0);
        Ok(())
    }

    #[test]
    fn percentile_parameters_fail_closed() {
        for (low, high, retained) in [
            (-0.1, 0.0, 1),
            (0.0, f64::NAN, 1),
            (0.5, 0.5, 1),
            (1.0, 0.0, 1),
            (0.0, 0.0, 0),
        ] {
            assert_eq!(
                PercentileClipParameters::new(low, high, retained),
                Err(IntegrationError::InvalidPercentileParameters)
            );
        }
    }

    #[test]
    fn sigma_clipping_iterates_asymmetric_sorted_tails_with_exact_evidence() -> TestResult {
        let inputs = [-100.0, 9.0, 10.0, 10.0, 11.0, 100.0]
            .into_iter()
            .map(|value| image(vec![value]))
            .collect::<Result<Vec<_>, _>>()?;
        let references = inputs.iter().collect::<Vec<_>>();
        let parameters = SigmaClipParameters::new(1.4, 1.4, 8, 2)?;

        let result = integrate_sigma_clipped_mean(&references, parameters)?;

        assert_eq!(result.image().pixels(), &[10.0]);
        assert_eq!(
            result.support(),
            &[ClippedPixelSupport {
                accepted: 2,
                masked: 0,
                non_finite: 0,
                low_rejected: 2,
                high_rejected: 2,
            }]
        );
        Ok(())
    }

    #[test]
    fn winsorized_sigma_limits_tail_influence_without_over_clipping() -> TestResult {
        let inputs = [-100.0, 9.0, 10.0, 10.0, 11.0, 100.0]
            .into_iter()
            .map(|value| image(vec![value]))
            .collect::<Result<Vec<_>, _>>()?;
        let references = inputs.iter().collect::<Vec<_>>();
        let parameters = SigmaClipParameters::new(1.4, 1.4, 8, 2)?;

        let ordinary = integrate_sigma_clipped_mean(&references, parameters)?;
        let winsorized = integrate_winsorized_sigma_clipped_mean(&references, parameters)?;

        assert_eq!(ordinary.support()[0].accepted(), 2);
        assert_eq!(winsorized.image().pixels(), &[10.0]);
        assert_eq!(
            winsorized.support(),
            &[ClippedPixelSupport {
                accepted: 4,
                masked: 0,
                non_finite: 0,
                low_rejected: 1,
                high_rejected: 1,
            }]
        );
        Ok(())
    }

    #[test]
    fn winsorized_sigma_preserves_asymmetric_evidence_and_support_floor() -> TestResult {
        let inputs = [-100.0, 9.0, 10.0, 10.0, 11.0, 12.0, 1000.0]
            .into_iter()
            .map(|value| image(vec![value]))
            .collect::<Result<Vec<_>, _>>()?;
        let references = inputs.iter().collect::<Vec<_>>();

        let asymmetric = integrate_winsorized_sigma_clipped_mean(
            &references,
            SigmaClipParameters::new(10.0, 1.5, 8, 2)?,
        )?;
        assert_eq!(asymmetric.support()[0].low_rejected(), 0);
        assert_eq!(asymmetric.support()[0].high_rejected(), 1);
        assert_eq!(asymmetric.support()[0].total(), 7);

        let floored = integrate_winsorized_sigma_clipped_mean(
            &references,
            SigmaClipParameters::new(1.5, 1.5, 8, 7)?,
        )?;
        assert_eq!(floored.support()[0].accepted(), 7);
        assert_eq!(floored.support()[0].low_rejected(), 0);
        assert_eq!(floored.support()[0].high_rejected(), 0);
        Ok(())
    }

    #[test]
    fn sigma_clipping_discards_a_pass_that_would_cross_the_support_floor() -> TestResult {
        let inputs = [-100.0, 9.0, 10.0, 10.0, 11.0, 100.0]
            .into_iter()
            .map(|value| image(vec![value]))
            .collect::<Result<Vec<_>, _>>()?;
        let references = inputs.iter().collect::<Vec<_>>();
        let parameters = SigmaClipParameters::new(1.4, 1.4, 8, 5)?;

        let result = integrate_sigma_clipped_mean(&references, parameters)?;

        assert!((result.image().pixels()[0] - (40.0 / 6.0)).abs() <= f64::EPSILON);
        assert_eq!(result.support()[0].accepted(), 6);
        assert_eq!(result.support()[0].low_rejected(), 0);
        assert_eq!(result.support()[0].high_rejected(), 0);
        Ok(())
    }

    #[test]
    fn sigma_clipping_handles_extremes_masks_and_non_finite_samples() -> TestResult {
        let negative = image(vec![-f64::MAX, f64::NAN])?;
        let first_maximum = image(vec![f64::MAX, f64::INFINITY])?;
        let second_maximum = image(vec![f64::MAX, 7.0])?;
        let mut third_maximum = image(vec![f64::MAX, 100.0])?;
        third_maximum.mask_mut().as_mut_slice()[1] = PixelFlags::HOT;
        let parameters = SigmaClipParameters::new(1.0, 10.0, 4, 2)?;

        let result = integrate_sigma_clipped_mean(
            &[&negative, &first_maximum, &second_maximum, &third_maximum],
            parameters,
        )?;
        let winsorized = integrate_winsorized_sigma_clipped_mean(
            &[&negative, &first_maximum, &second_maximum, &third_maximum],
            parameters,
        )?;

        assert_eq!(result.image().pixels()[0].to_bits(), f64::MAX.to_bits());
        assert_eq!(result.support()[0].low_rejected(), 1);
        assert_eq!(result.support()[0].accepted(), 3);
        assert_eq!(result.image().pixels()[1].to_bits(), 7.0_f64.to_bits());
        assert_eq!(result.support()[1].accepted(), 1);
        assert_eq!(result.support()[1].masked(), 1);
        assert_eq!(result.support()[1].non_finite(), 2);
        assert_eq!(result.support()[1].total(), 4);
        assert_eq!(winsorized.image().pixels(), result.image().pixels());
        assert_eq!(winsorized.support(), result.support());
        Ok(())
    }

    #[test]
    fn sigma_parameters_and_input_shape_fail_closed() -> TestResult {
        for (low, high, iterations, retained) in [
            (0.0, 3.0, 1, 1),
            (-1.0, 3.0, 1, 1),
            (3.0, f64::NAN, 1, 1),
            (3.0, 3.0, 0, 1),
            (3.0, 3.0, 1, 0),
        ] {
            assert_eq!(
                SigmaClipParameters::new(low, high, iterations, retained),
                Err(IntegrationError::InvalidSigmaParameters)
            );
        }

        let parameters = SigmaClipParameters::new(3.0, 3.0, 1, 1)?;
        assert_eq!(
            integrate_sigma_clipped_mean(&[], parameters),
            Err(IntegrationError::NoInputImages)
        );
        let first = image(vec![1.0])?;
        let second = image(vec![1.0, 2.0])?;
        assert!(matches!(
            integrate_sigma_clipped_mean(&[&first, &second], parameters),
            Err(IntegrationError::DimensionMismatch { input_index: 1, .. })
        ));
        Ok(())
    }

    #[test]
    fn linear_fit_parameters_are_explicit_and_bounded() -> TestResult {
        for (low, high, retained) in [
            (0.0, 3.5, 3),
            (-1.0, 3.5, 3),
            (5.0, f64::NAN, 3),
            (5.0, 3.5, 0),
            (5.0, 3.5, 2),
        ] {
            assert_eq!(
                LinearFitClipParameters::new(low, high, retained),
                Err(IntegrationError::InvalidLinearFitParameters)
            );
        }
        let parameters = LinearFitClipParameters::new(5.0, 3.5, 3)?;
        assert_eq!(parameters.low_sigma().to_bits(), 5.0_f64.to_bits());
        assert_eq!(parameters.high_sigma().to_bits(), 3.5_f64.to_bits());
        assert_eq!(parameters.minimum_retained(), 3);
        Ok(())
    }

    #[test]
    fn generalized_esd_parameters_are_explicit_and_bounded() -> TestResult {
        for (fraction, significance, retained) in [
            (0.0, 0.05, 3),
            (0.51, 0.05, 3),
            (f64::NAN, 0.05, 3),
            (0.30, 0.0, 3),
            (0.30, 1.0, 3),
            (0.30, f64::INFINITY, 3),
            (0.30, 0.05, 2),
        ] {
            assert_eq!(
                GeneralizedEsdParameters::new(fraction, significance, retained),
                Err(IntegrationError::InvalidGeneralizedEsdParameters)
            );
        }
        let parameters = GeneralizedEsdParameters::new(0.30, 0.05, 3)?;
        assert_eq!(
            parameters.maximum_outlier_fraction().to_bits(),
            0.30_f64.to_bits()
        );
        assert_eq!(parameters.significance().to_bits(), 0.05_f64.to_bits());
        assert_eq!(parameters.minimum_retained(), 3);
        assert_eq!(GENERALIZED_ESD_MINIMUM_SAMPLE_COUNT, 15);
        Ok(())
    }

    #[test]
    fn generalized_esd_matches_the_published_nist_example() -> TestResult {
        let values = [
            -0.25, 0.68, 0.94, 1.15, 1.20, 1.26, 1.26, 1.34, 1.38, 1.43, 1.49, 1.49, 1.55, 1.56,
            1.58, 1.65, 1.69, 1.70, 1.76, 1.77, 1.81, 1.91, 1.94, 1.96, 1.99, 2.06, 2.09, 2.10,
            2.14, 2.15, 2.23, 2.24, 2.26, 2.35, 2.37, 2.40, 2.47, 2.54, 2.62, 2.64, 2.90, 2.92,
            2.92, 2.93, 3.21, 3.26, 3.30, 3.59, 3.68, 4.30, 4.64, 5.34, 5.42, 6.01,
        ];
        let inputs = values
            .into_iter()
            .map(|value| image(vec![value]))
            .collect::<Result<Vec<_>, _>>()?;
        let references = inputs.iter().collect::<Vec<_>>();

        let result = integrate_generalized_esd_mean(
            &references,
            GeneralizedEsdParameters::new(0.20, 0.05, 3)?,
        )?;

        assert_eq!(result.support()[0].accepted(), 51);
        assert_eq!(result.support()[0].low_rejected(), 0);
        assert_eq!(result.support()[0].high_rejected(), 3);
        assert_eq!(result.support()[0].total(), 54);
        assert!(result.image().pixels()[0].is_finite());
        Ok(())
    }

    #[test]
    fn generalized_esd_is_permutation_invariant_and_fails_closed() -> TestResult {
        let mut ordered = (0_u32..15).map(f64::from).collect::<Vec<_>>();
        ordered.push(1_000.0);
        let mut permuted = ordered.clone();
        permuted.reverse();
        let integrate = |values: &[f64], retained| -> TestResult<GeneralizedEsdIntegration> {
            let inputs = values
                .iter()
                .copied()
                .map(|value| image(vec![value]))
                .collect::<Result<Vec<_>, _>>()?;
            let references = inputs.iter().collect::<Vec<_>>();
            Ok(integrate_generalized_esd_mean(
                &references,
                GeneralizedEsdParameters::new(0.30, 0.05, retained)?,
            )?)
        };

        let first = integrate(&ordered, 3)?;
        let second = integrate(&permuted, 3)?;
        assert_eq!(first.image().pixels(), second.image().pixels());
        assert_eq!(first.support(), second.support());
        assert_eq!(first.image().pixels()[0].to_bits(), 7.0_f64.to_bits());
        assert_eq!(first.support()[0].accepted(), 15);
        assert_eq!(first.support()[0].high_rejected(), 1);

        let below_admitted_population = integrate(&ordered[..14], 3)?;
        assert_eq!(below_admitted_population.support()[0].accepted(), 14);
        assert_eq!(below_admitted_population.support()[0].high_rejected(), 0);

        let support_limited = integrate(&ordered, 16)?;
        assert_eq!(support_limited.support()[0].accepted(), 16);
        assert_eq!(support_limited.support()[0].high_rejected(), 0);
        Ok(())
    }

    #[test]
    fn generalized_esd_keeps_mask_and_nonfinite_evidence_separate() -> TestResult {
        let mut inputs = (0_u32..15)
            .map(|value| image(vec![f64::from(value)]))
            .collect::<Result<Vec<_>, _>>()?;
        inputs.push(image(vec![1_000.0])?);
        let mut masked = image(vec![-1_000.0])?;
        masked.mask_mut().as_mut_slice()[0] = PixelFlags::HOT;
        inputs.push(masked);
        inputs.push(image(vec![f64::NAN])?);
        let references = inputs.iter().collect::<Vec<_>>();

        let result = integrate_generalized_esd_mean(
            &references,
            GeneralizedEsdParameters::new(0.30, 0.05, 3)?,
        )?;

        assert_eq!(result.support()[0].accepted(), 15);
        assert_eq!(result.support()[0].high_rejected(), 1);
        assert_eq!(result.support()[0].low_rejected(), 0);
        assert_eq!(result.support()[0].masked(), 1);
        assert_eq!(result.support()[0].non_finite(), 1);
        assert_eq!(result.support()[0].total(), 18);
        for source_index in 0..15 {
            assert_eq!(
                result.attribution().disposition(0, source_index)?,
                SampleDisposition::Accepted
            );
        }
        assert_eq!(
            result.attribution().disposition(0, 15)?,
            SampleDisposition::RejectedHigh
        );
        assert_eq!(
            result.attribution().disposition(0, 16)?,
            SampleDisposition::Masked
        );
        assert_eq!(
            result.attribution().disposition(0, 17)?,
            SampleDisposition::NonFinite
        );
        Ok(())
    }

    #[test]
    fn large_scale_rejection_rebuilds_science_from_original_sources() -> TestResult {
        let dimensions = Dimensions::new(7, 5, 1)?;
        let mut inputs = Vec::new();
        for source_index in 0..15 {
            let mut pixels = vec![source_index as f64; dimensions.pixel_count()];
            if source_index == 14 {
                for x in 1..=5 {
                    pixels[2 * 7 + x] = 100.0;
                }
            }
            inputs.push(ScientificImage::from_pixels(dimensions, pixels)?);
        }
        let references = inputs.iter().collect::<Vec<_>>();
        let esd = GeneralizedEsdParameters::new(0.30, 0.05, 3)?;
        let initial = integrate_generalized_esd_mean(&references, esd)?;
        assert_eq!(initial.image().pixels()[7 + 3].to_bits(), 7.0_f64.to_bits());
        assert_eq!(initial.support()[2 * 7 + 3].high_rejected(), 1);
        let high = LargeScaleTailParameters::new(1, 1)?;
        let spatial = LargeScaleRejectionParameters::new(None, Some(high), 3)?;

        let expanded = initial.apply_large_scale_rejection(&references, spatial)?;

        assert_eq!(
            expanded.image().pixels()[7 + 3].to_bits(),
            6.5_f64.to_bits()
        );
        assert_eq!(expanded.image().pixels()[3].to_bits(), 7.0_f64.to_bits());
        assert_eq!(expanded.support()[7 + 3].accepted(), 14);
        assert_eq!(expanded.support()[7 + 3].high_rejected(), 1);
        assert_eq!(
            expanded.attribution().disposition(7 + 3, 14)?,
            SampleDisposition::RejectedHigh
        );
        let summary = expanded
            .large_scale_summary()
            .ok_or("missing large-scale summary")?;
        assert_eq!(summary.promoted_high(), 10);
        assert_eq!(summary.promoted_low(), 0);
        assert_eq!(summary.conflicts_retained(), 0);
        assert_eq!(summary.support_floor_retained(), 0);
        Ok(())
    }

    #[test]
    fn linear_fit_rejects_asymmetric_tail_outliers_with_exact_evidence() -> TestResult {
        let mut values = (0_u32..20).map(f64::from).collect::<Vec<_>>();
        values.push(1_000.0);
        let inputs = values
            .into_iter()
            .map(|value| image(vec![value]))
            .collect::<Result<Vec<_>, _>>()?;
        let references = inputs.iter().collect::<Vec<_>>();

        let result = integrate_linear_fit_clipped_mean(
            &references,
            LinearFitClipParameters::new(5.0, 3.5, 3)?,
        )?;

        assert_eq!(result.image().pixels()[0].to_bits(), 9.5_f64.to_bits());
        assert_eq!(result.support()[0].accepted(), 20);
        assert_eq!(result.support()[0].low_rejected(), 0);
        assert_eq!(result.support()[0].high_rejected(), 1);
        assert_eq!(result.support()[0].total(), 21);
        Ok(())
    }

    #[test]
    fn linear_fit_is_permutation_invariant_and_preserves_a_perfect_ramp() -> TestResult {
        let ordered = (0_u32..17).map(f64::from).collect::<Vec<_>>();
        let permuted = [8, 0, 16, 4, 12, 2, 14, 6, 10, 1, 15, 3, 13, 5, 11, 7, 9]
            .into_iter()
            .map(f64::from)
            .collect::<Vec<_>>();
        let integrate = |values: &[f64]| -> TestResult<LinearFitClippedIntegration> {
            let inputs = values
                .iter()
                .copied()
                .map(|value| image(vec![value]))
                .collect::<Result<Vec<_>, _>>()?;
            let references = inputs.iter().collect::<Vec<_>>();
            Ok(integrate_linear_fit_clipped_mean(
                &references,
                LinearFitClipParameters::new(2.0, 2.0, 3)?,
            )?)
        };

        let first = integrate(&ordered)?;
        let second = integrate(&permuted)?;
        assert_eq!(first.image().pixels(), second.image().pixels());
        assert_eq!(first.support(), second.support());
        assert_eq!(first.support()[0].accepted(), 17);
        assert_eq!(first.support()[0].low_rejected(), 0);
        assert_eq!(first.support()[0].high_rejected(), 0);
        Ok(())
    }

    #[test]
    fn linear_fit_support_floor_masks_and_nonfinite_values_fail_closed() -> TestResult {
        let mut images = (0_u32..5)
            .map(|value| image(vec![f64::from(value)]))
            .collect::<Result<Vec<_>, _>>()?;
        images.push(image(vec![1_000.0])?);
        let mut masked = image(vec![-1_000.0])?;
        masked.mask_mut().as_mut_slice()[0] = PixelFlags::SATURATED;
        images.push(masked);
        images.push(image(vec![f64::NAN])?);
        let references = images.iter().collect::<Vec<_>>();

        let result = integrate_linear_fit_clipped_mean(
            &references,
            LinearFitClipParameters::new(1.0, 1.0, 6)?,
        )?;

        assert_eq!(result.support()[0].accepted(), 6);
        assert_eq!(result.support()[0].low_rejected(), 0);
        assert_eq!(result.support()[0].high_rejected(), 0);
        assert_eq!(result.support()[0].masked(), 1);
        assert_eq!(result.support()[0].non_finite(), 1);
        assert_eq!(result.support()[0].total(), 8);
        Ok(())
    }

    #[test]
    fn materializes_exact_low_then_high_rejection_planes() -> TestResult {
        let dimensions = Dimensions::new(2, 1, 2)?;
        let support = [
            ClippedPixelSupport {
                low_rejected: 1,
                high_rejected: 5,
                ..ClippedPixelSupport::default()
            },
            ClippedPixelSupport {
                low_rejected: 2,
                high_rejected: 6,
                ..ClippedPixelSupport::default()
            },
            ClippedPixelSupport {
                low_rejected: 3,
                high_rejected: 7,
                ..ClippedPixelSupport::default()
            },
            ClippedPixelSupport {
                low_rejected: 4,
                high_rejected: 8,
                ..ClippedPixelSupport::default()
            },
        ];

        let maps = materialize_percentile_rejection_map(dimensions, &support)?;

        assert_eq!(maps.low().dimensions(), dimensions);
        assert_eq!(maps.high().dimensions(), dimensions);
        assert_eq!(maps.low().pixels(), &[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(maps.high().pixels(), &[5.0, 6.0, 7.0, 8.0]);
        assert!(
            maps.low()
                .mask()
                .as_slice()
                .iter()
                .chain(maps.high().mask().as_slice())
                .all(|flags| flags.is_clear())
        );
        Ok(())
    }

    #[test]
    fn rejection_map_requires_one_support_record_per_source_sample() -> TestResult {
        let dimensions = Dimensions::new(2, 1, 1)?;
        assert_eq!(
            materialize_percentile_rejection_map(dimensions, &[ClippedPixelSupport::default()]),
            Err(IntegrationError::SupportLengthMismatch {
                expected: 2,
                actual: 1,
            })
        );
        Ok(())
    }

    #[test]
    fn rejects_empty_and_mismatched_inputs() -> TestResult {
        assert_eq!(integrate_mean(&[]), Err(IntegrationError::NoInputImages));

        let first = image(vec![1.0, 2.0])?;
        let second = image(vec![1.0])?;
        assert!(matches!(
            integrate_mean(&[&first, &second]),
            Err(IntegrationError::DimensionMismatch { input_index: 1, .. })
        ));
        Ok(())
    }

    #[test]
    fn preserves_planar_order_and_can_return_owned_parts() -> TestResult {
        let dimensions = Dimensions::new(2, 1, 2)?;
        let first = ScientificImage::from_pixels(dimensions, vec![1.0, 2.0, 3.0, 4.0])?;
        let second = ScientificImage::from_pixels(dimensions, vec![3.0, 4.0, 5.0, 6.0])?;

        let result = integrate_mean(&[&first, &second])?;
        let (image, support) = result.into_parts();

        assert_eq!(image.pixels(), &[2.0, 3.0, 4.0, 5.0]);
        assert!(support.iter().all(|entry| entry.accepted() == 2));
        Ok(())
    }

    #[test]
    fn integrates_a_crop_directly_in_planar_order() -> TestResult {
        let dimensions = Dimensions::new(4, 3, 2)?;
        let first = ScientificImage::from_pixels(dimensions, (0..24).map(f64::from).collect())?;
        let second = ScientificImage::from_pixels(
            dimensions,
            (0..24).map(|value| f64::from(value) + 10.0).collect(),
        )?;
        let region = IntegrationRegion::new(1, 1, 2, 2)?;

        let result = integrate_mean_region(&[&first, &second], region)?;

        assert_eq!(result.image().dimensions(), Dimensions::new(2, 2, 2)?);
        for (actual, expected) in result
            .image()
            .pixels()
            .iter()
            .zip([10.0, 11.0, 14.0, 15.0, 22.0, 23.0, 26.0, 27.0])
        {
            assert!((actual - expected).abs() <= expected * f64::EPSILON);
        }
        assert!(
            result
                .support()
                .iter()
                .all(|support| support.accepted() == 2)
        );
        Ok(())
    }

    #[test]
    fn cropped_support_retains_mask_and_non_finite_accounting() -> TestResult {
        let dimensions = Dimensions::new(3, 2, 1)?;
        let first = ScientificImage::from_pixels(dimensions, vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0])?;
        let mut second =
            ScientificImage::from_pixels(dimensions, vec![10.0, 20.0, 30.0, 40.0, f64::NAN, 60.0])?;
        second.mask_mut().as_mut_slice()[3] = PixelFlags::SATURATED;

        let result =
            integrate_mean_region(&[&first, &second], IntegrationRegion::new(0, 1, 2, 1)?)?;

        assert_eq!(result.image().pixels(), &[4.0, 5.0]);
        assert_eq!(
            result.support(),
            &[
                PixelSupport {
                    accepted: 1,
                    masked: 1,
                    non_finite: 0,
                },
                PixelSupport {
                    accepted: 1,
                    masked: 0,
                    non_finite: 1,
                },
            ]
        );
        Ok(())
    }

    #[test]
    fn validates_region_construction_and_source_bounds() -> TestResult {
        assert!(matches!(
            IntegrationRegion::new(0, 0, 0, 1),
            Err(IntegrationError::InvalidRegion { .. })
        ));
        assert!(matches!(
            IntegrationRegion::new(usize::MAX, 0, 1, 1),
            Err(IntegrationError::InvalidRegion { .. })
        ));
        let source = ScientificImage::from_pixels(Dimensions::new(3, 2, 1)?, vec![0.0; 6])?;
        assert!(matches!(
            integrate_mean_region(&[&source], IntegrationRegion::new(2, 1, 2, 1)?,),
            Err(IntegrationError::RegionOutsideInput { .. })
        ));
        Ok(())
    }

    #[test]
    fn full_region_is_bit_identical_to_the_existing_entry_point() -> TestResult {
        let dimensions = Dimensions::new(3, 2, 2)?;
        let first = ScientificImage::from_pixels(
            dimensions,
            vec![
                1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
            ],
        )?;
        let second = ScientificImage::from_pixels(
            dimensions,
            vec![
                2.0, 4.0, 6.0, 8.0, 10.0, 12.0, 14.0, 16.0, 18.0, 20.0, 22.0, 24.0,
            ],
        )?;

        let existing = integrate_mean(&[&first, &second])?;
        let regional =
            integrate_mean_region(&[&first, &second], IntegrationRegion::new(0, 0, 3, 2)?)?;

        assert_eq!(existing, regional);
        Ok(())
    }
}
