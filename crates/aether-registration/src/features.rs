use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_quality::{FrameQuality, STAR_MEASUREMENT_ALGORITHM_ID, StarMeasurement};
use aether_review::FrameId;

use crate::{CoordinateError, ImagePoint};

/// Versioned policy that converts quality measurements into matching features.
pub const FEATURE_CATALOG_ALGORITHM_ID: &str = "quality-filtered-stars-v1";

/// Hard output bound for one frame's registration feature catalog.
pub const MAX_REGISTRATION_FEATURES: usize = 65_536;

/// Defensive bound on upstream stellar measurements inspected for one frame.
pub const MAX_REGISTRATION_MEASUREMENTS: usize = 1_000_000;

/// Validated controls for selecting reliable registration features.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FeatureSelectionParameters {
    minimum_background_snr: f64,
    maximum_eccentricity: f64,
    border_margin_pixels: f64,
    maximum_features: usize,
}

impl FeatureSelectionParameters {
    /// Creates the deterministic feature filter used after stellar measurement.
    ///
    /// Saturated measurements are always excluded. Remaining measurements must
    /// meet the inclusive SNR and eccentricity limits, remain at least the
    /// requested distance from every image edge, and fit the output bound.
    pub fn new(
        minimum_background_snr: f64,
        maximum_eccentricity: f64,
        border_margin_pixels: f64,
        maximum_features: usize,
    ) -> Result<Self, FeatureCatalogError> {
        if !minimum_background_snr.is_finite() || minimum_background_snr <= 0.0 {
            return Err(FeatureCatalogError::InvalidMinimumSnr);
        }
        if !maximum_eccentricity.is_finite() || !(0.0..1.0).contains(&maximum_eccentricity) {
            return Err(FeatureCatalogError::InvalidMaximumEccentricity);
        }
        if !border_margin_pixels.is_finite() || border_margin_pixels < 0.0 {
            return Err(FeatureCatalogError::InvalidBorderMargin);
        }
        if maximum_features == 0 || maximum_features > MAX_REGISTRATION_FEATURES {
            return Err(FeatureCatalogError::InvalidMaximumFeatures {
                maximum: MAX_REGISTRATION_FEATURES,
                actual: maximum_features,
            });
        }
        Ok(Self {
            minimum_background_snr,
            maximum_eccentricity,
            border_margin_pixels: canonical_zero(border_margin_pixels),
            maximum_features,
        })
    }

    /// Inclusive minimum background SNR proxy from the quality measurement.
    #[must_use]
    pub const fn minimum_background_snr(self) -> f64 {
        self.minimum_background_snr
    }

    /// Inclusive maximum stellar eccentricity.
    #[must_use]
    pub const fn maximum_eccentricity(self) -> f64 {
        self.maximum_eccentricity
    }

    /// Minimum centroid distance from the outer pixel-center rectangle.
    #[must_use]
    pub const fn border_margin_pixels(self) -> f64 {
        self.border_margin_pixels
    }

    /// Maximum number of ranked output features.
    #[must_use]
    pub const fn maximum_features(self) -> usize {
        self.maximum_features
    }
}

/// One ranked, unsaturated stellar feature eligible for matching.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RegistrationFeature {
    rank: usize,
    point: ImagePoint,
    peak: f64,
    flux_above_background: f64,
    background_snr: f64,
    fwhm_major_pixels: f64,
    fwhm_minor_pixels: f64,
    eccentricity: f64,
    measurement_pixels: usize,
}

impl RegistrationFeature {
    /// Zero-based rank after deterministic reliability ordering.
    #[must_use]
    pub const fn rank(self) -> usize {
        self.rank
    }

    /// Sub-pixel centroid in the registration coordinate convention.
    #[must_use]
    pub const fn point(self) -> ImagePoint {
        self.point
    }

    /// Highest physical source value at the local maximum.
    #[must_use]
    pub const fn peak(self) -> f64 {
        self.peak
    }

    /// Positive background-subtracted aperture flux.
    #[must_use]
    pub const fn flux_above_background(self) -> f64 {
        self.flux_above_background
    }

    /// Background SNR proxy used as the primary ranking key.
    #[must_use]
    pub const fn background_snr(self) -> f64 {
        self.background_snr
    }

    /// Threshold-corrected major-axis FWHM in source pixels.
    #[must_use]
    pub const fn fwhm_major_pixels(self) -> f64 {
        self.fwhm_major_pixels
    }

    /// Threshold-corrected minor-axis FWHM in source pixels.
    #[must_use]
    pub const fn fwhm_minor_pixels(self) -> f64 {
        self.fwhm_minor_pixels
    }

    /// Elliptical eccentricity used by the catalog filter.
    #[must_use]
    pub const fn eccentricity(self) -> f64 {
        self.eccentricity
    }

    /// Above-floor samples supporting the centroid and shape estimate.
    #[must_use]
    pub const fn measurement_pixels(self) -> usize {
        self.measurement_pixels
    }
}

/// Complete accounting of measurements not retained in the final catalog.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FeatureExclusions {
    saturated: usize,
    below_minimum_snr: usize,
    above_maximum_eccentricity: usize,
    inside_border_margin: usize,
    beyond_catalog_limit: usize,
}

impl FeatureExclusions {
    /// Measurements excluded because the quality aperture touched saturation.
    #[must_use]
    pub const fn saturated(self) -> usize {
        self.saturated
    }

    /// Measurements below the configured background SNR.
    #[must_use]
    pub const fn below_minimum_snr(self) -> usize {
        self.below_minimum_snr
    }

    /// Measurements above the configured eccentricity limit.
    #[must_use]
    pub const fn above_maximum_eccentricity(self) -> usize {
        self.above_maximum_eccentricity
    }

    /// Measurements too close to an image edge.
    #[must_use]
    pub const fn inside_border_margin(self) -> usize {
        self.inside_border_margin
    }

    /// Otherwise eligible measurements dropped by the output feature limit.
    #[must_use]
    pub const fn beyond_catalog_limit(self) -> usize {
        self.beyond_catalog_limit
    }

    /// Total excluded measurements, with every source assigned once.
    pub fn total(self) -> Result<usize, FeatureCatalogError> {
        self.saturated
            .checked_add(self.below_minimum_snr)
            .and_then(|value| value.checked_add(self.above_maximum_eccentricity))
            .and_then(|value| value.checked_add(self.inside_border_margin))
            .and_then(|value| value.checked_add(self.beyond_catalog_limit))
            .ok_or(FeatureCatalogError::CountOverflow)
    }
}

/// Immutable, deterministically ranked feature catalog for one reviewed frame.
#[derive(Clone, Debug, PartialEq)]
pub struct FeatureCatalog {
    frame_id: FrameId,
    width: usize,
    height: usize,
    parameters: FeatureSelectionParameters,
    source_measurements: usize,
    exclusions: FeatureExclusions,
    features: Vec<RegistrationFeature>,
}

impl FeatureCatalog {
    /// Stable identity of the measured scientific frame.
    #[must_use]
    pub const fn frame_id(&self) -> &FrameId {
        &self.frame_id
    }

    /// Source width in pixels.
    #[must_use]
    pub const fn width(&self) -> usize {
        self.width
    }

    /// Source height in pixels.
    #[must_use]
    pub const fn height(&self) -> usize {
        self.height
    }

    /// Exact filtering and output-bound controls used to build this catalog.
    #[must_use]
    pub const fn parameters(&self) -> FeatureSelectionParameters {
        self.parameters
    }

    /// Versioned upstream stellar-measurement algorithm.
    #[must_use]
    pub const fn source_algorithm_id(&self) -> &'static str {
        STAR_MEASUREMENT_ALGORITHM_ID
    }

    /// Versioned catalog filtering and ordering policy.
    #[must_use]
    pub const fn algorithm_id(&self) -> &'static str {
        FEATURE_CATALOG_ALGORITHM_ID
    }

    /// Number of stellar measurements inspected before catalog filtering.
    #[must_use]
    pub const fn source_measurements(&self) -> usize {
        self.source_measurements
    }

    /// Mutually exclusive exclusion evidence.
    #[must_use]
    pub const fn exclusions(&self) -> FeatureExclusions {
        self.exclusions
    }

    /// Features ordered by reliability and carrying matching rank.
    #[must_use]
    pub fn features(&self) -> &[RegistrationFeature] {
        &self.features
    }
}

/// Builds a bounded matching catalog from the canonical quality measurements.
///
/// Measurements are filtered in the documented evidence order: saturation,
/// SNR, eccentricity, then border margin. Eligible stars are ordered by
/// descending SNR, flux, and peak; lower eccentricity, `y`, and `x` provide
/// deterministic tie breakers. The output limit is applied only after sorting.
pub fn build_feature_catalog(
    frame_id: FrameId,
    width: usize,
    height: usize,
    quality: &FrameQuality,
    parameters: FeatureSelectionParameters,
) -> Result<FeatureCatalog, FeatureCatalogError> {
    let parameters = FeatureSelectionParameters::new(
        parameters.minimum_background_snr,
        parameters.maximum_eccentricity,
        parameters.border_margin_pixels,
        parameters.maximum_features,
    )?;
    let area = width
        .checked_mul(height)
        .ok_or(FeatureCatalogError::DimensionOverflow)?;
    if width == 0 || height == 0 || area != quality.background().total_samples() {
        return Err(FeatureCatalogError::DimensionMismatch {
            width,
            height,
            measured_samples: quality.background().total_samples(),
        });
    }
    let maximum_x = (width - 1) as f64 - parameters.border_margin_pixels;
    let maximum_y = (height - 1) as f64 - parameters.border_margin_pixels;
    if parameters.border_margin_pixels > maximum_x || parameters.border_margin_pixels > maximum_y {
        return Err(FeatureCatalogError::BorderMarginExcludesImage);
    }
    let measurements = quality.stars();
    if measurements.len() > MAX_REGISTRATION_MEASUREMENTS {
        return Err(FeatureCatalogError::TooManySourceMeasurements {
            maximum: MAX_REGISTRATION_MEASUREMENTS,
            actual: measurements.len(),
        });
    }

    let mut exclusions = FeatureExclusions::default();
    let mut features = Vec::new();
    features
        .try_reserve_exact(measurements.len())
        .map_err(|_| FeatureCatalogError::AllocationFailed)?;
    for measurement in measurements {
        if measurement.saturated() {
            exclusions.saturated = checked_increment(exclusions.saturated)?;
            continue;
        }
        if measurement.background_snr() < parameters.minimum_background_snr {
            exclusions.below_minimum_snr = checked_increment(exclusions.below_minimum_snr)?;
            continue;
        }
        if measurement.eccentricity() > parameters.maximum_eccentricity {
            exclusions.above_maximum_eccentricity =
                checked_increment(exclusions.above_maximum_eccentricity)?;
            continue;
        }
        if measurement.centroid_x() < parameters.border_margin_pixels
            || measurement.centroid_y() < parameters.border_margin_pixels
            || measurement.centroid_x() > maximum_x
            || measurement.centroid_y() > maximum_y
        {
            exclusions.inside_border_margin = checked_increment(exclusions.inside_border_margin)?;
            continue;
        }
        features.push(feature_from_measurement(measurement)?);
    }
    features.sort_by(compare_features);
    if features.len() > parameters.maximum_features {
        exclusions.beyond_catalog_limit = features.len() - parameters.maximum_features;
        features.truncate(parameters.maximum_features);
    }
    for (rank, feature) in features.iter_mut().enumerate() {
        feature.rank = rank;
    }
    let accounted = features
        .len()
        .checked_add(exclusions.total()?)
        .ok_or(FeatureCatalogError::CountOverflow)?;
    if accounted != measurements.len() {
        return Err(FeatureCatalogError::CountMismatch);
    }

    Ok(FeatureCatalog {
        frame_id,
        width,
        height,
        parameters,
        source_measurements: measurements.len(),
        exclusions,
        features,
    })
}

fn feature_from_measurement(
    measurement: &StarMeasurement,
) -> Result<RegistrationFeature, FeatureCatalogError> {
    Ok(RegistrationFeature {
        rank: 0,
        point: ImagePoint::new(measurement.centroid_x(), measurement.centroid_y())
            .map_err(FeatureCatalogError::Coordinate)?,
        peak: measurement.peak(),
        flux_above_background: measurement.flux_above_background(),
        background_snr: measurement.background_snr(),
        fwhm_major_pixels: measurement.fwhm_major_pixels(),
        fwhm_minor_pixels: measurement.fwhm_minor_pixels(),
        eccentricity: measurement.eccentricity(),
        measurement_pixels: measurement.measurement_pixels(),
    })
}

fn compare_features(left: &RegistrationFeature, right: &RegistrationFeature) -> std::cmp::Ordering {
    right
        .background_snr
        .total_cmp(&left.background_snr)
        .then_with(|| {
            right
                .flux_above_background
                .total_cmp(&left.flux_above_background)
        })
        .then_with(|| right.peak.total_cmp(&left.peak))
        .then_with(|| left.eccentricity.total_cmp(&right.eccentricity))
        .then_with(|| left.point.y().total_cmp(&right.point.y()))
        .then_with(|| left.point.x().total_cmp(&right.point.x()))
}

fn checked_increment(value: usize) -> Result<usize, FeatureCatalogError> {
    value
        .checked_add(1)
        .ok_or(FeatureCatalogError::CountOverflow)
}

/// Invalid feature-selection request or catalog construction failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FeatureCatalogError {
    /// The minimum SNR must be finite and positive.
    InvalidMinimumSnr,
    /// The eccentricity limit must be finite and in `[0, 1)`.
    InvalidMaximumEccentricity,
    /// The border margin must be finite and nonnegative.
    InvalidBorderMargin,
    /// The requested output bound was zero or exceeded the hard maximum.
    InvalidMaximumFeatures {
        /// Hard maximum.
        maximum: usize,
        /// Requested value.
        actual: usize,
    },
    /// Image area arithmetic overflowed.
    DimensionOverflow,
    /// Supplied image axes do not describe the measured quality plane.
    DimensionMismatch {
        /// Supplied width.
        width: usize,
        /// Supplied height.
        height: usize,
        /// Samples recorded by the quality estimator.
        measured_samples: usize,
    },
    /// The requested border margin leaves no valid pixel-center rectangle.
    BorderMarginExcludesImage,
    /// Upstream measurements exceeded the defensive input bound.
    TooManySourceMeasurements {
        /// Hard maximum.
        maximum: usize,
        /// Supplied measurements.
        actual: usize,
    },
    /// Catalog storage could not be reserved.
    AllocationFailed,
    /// A quality centroid violated the finite coordinate contract.
    Coordinate(CoordinateError),
    /// Evidence arithmetic overflowed.
    CountOverflow,
    /// Internal evidence did not account for every source measurement.
    CountMismatch,
}

impl Display for FeatureCatalogError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidMinimumSnr => formatter.write_str("minimum registration SNR is invalid"),
            Self::InvalidMaximumEccentricity => {
                formatter.write_str("maximum registration eccentricity is invalid")
            }
            Self::InvalidBorderMargin => {
                formatter.write_str("registration border margin is invalid")
            }
            Self::InvalidMaximumFeatures { maximum, actual } => write!(
                formatter,
                "registration catalog accepts 1..={maximum} features, received {actual}"
            ),
            Self::DimensionOverflow => formatter.write_str("registration image area overflowed"),
            Self::DimensionMismatch {
                width,
                height,
                measured_samples,
            } => write!(
                formatter,
                "registration dimensions {width}x{height} do not match {measured_samples} measured samples"
            ),
            Self::BorderMarginExcludesImage => {
                formatter.write_str("registration border margin excludes the complete image")
            }
            Self::TooManySourceMeasurements { maximum, actual } => write!(
                formatter,
                "registration catalog accepts at most {maximum} measurements, received {actual}"
            ),
            Self::AllocationFailed => formatter.write_str("registration feature allocation failed"),
            Self::Coordinate(error) => write!(formatter, "invalid registration centroid: {error}"),
            Self::CountOverflow => formatter.write_str("registration evidence count overflowed"),
            Self::CountMismatch => formatter.write_str("registration evidence is inconsistent"),
        }
    }
}

impl Error for FeatureCatalogError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Coordinate(error) => Some(error),
            _ => None,
        }
    }
}

const fn canonical_zero(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use aether_core::{Dimensions, ScientificImage};
    use aether_quality::{BackgroundParameters, StarMeasurementParameters, measure_frame_quality};

    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn StdError>>;

    #[derive(Clone, Copy)]
    struct SyntheticStar {
        x: f64,
        y: f64,
        amplitude: f64,
        sigma_x: f64,
        sigma_y: f64,
    }

    fn frame_id() -> TestResult<FrameId> {
        Ok(FrameId::new("f".repeat(64))?)
    }

    fn quality(
        width: usize,
        height: usize,
        stars: &[SyntheticStar],
        saturation: Option<f64>,
    ) -> TestResult<FrameQuality> {
        let area = width.checked_mul(height).ok_or("synthetic area overflow")?;
        let mut pixels = Vec::new();
        pixels.try_reserve_exact(area)?;
        for y in 0..height {
            for x in 0..width {
                let noise = ((x + 3 * y) % 5) as f64 - 2.0;
                let mut value = 1_000.0 + noise;
                for star in stars {
                    let dx = x as f64 - star.x;
                    let dy = y as f64 - star.y;
                    value += star.amplitude
                        * (-(dx * dx / (2.0 * star.sigma_x * star.sigma_x)
                            + dy * dy / (2.0 * star.sigma_y * star.sigma_y)))
                            .exp();
                }
                pixels.push(value);
            }
        }
        let image = ScientificImage::from_pixels(Dimensions::new(width, height, 1)?, pixels)?;
        let parameters = StarMeasurementParameters::new(
            BackgroundParameters::new(3.0, 8, 100)?,
            6.0,
            2.0,
            6,
            5,
            9,
            1_000,
            saturation,
        )?;
        Ok(measure_frame_quality(&image, 0, parameters)?)
    }

    #[test]
    fn ranks_eligible_features_and_accounts_for_output_limit() -> TestResult {
        let stars = [
            SyntheticStar {
                x: 20.0,
                y: 20.0,
                amplitude: 300.0,
                sigma_x: 2.0,
                sigma_y: 2.0,
            },
            SyntheticStar {
                x: 40.0,
                y: 40.0,
                amplitude: 700.0,
                sigma_x: 2.0,
                sigma_y: 2.0,
            },
            SyntheticStar {
                x: 62.0,
                y: 58.0,
                amplitude: 500.0,
                sigma_x: 2.0,
                sigma_y: 2.0,
            },
        ];
        let quality = quality(81, 81, &stars, None)?;
        let parameters = FeatureSelectionParameters::new(1.0, 0.8, 0.0, 2)?;
        let catalog = build_feature_catalog(frame_id()?, 81, 81, &quality, parameters)?;

        assert_eq!(catalog.algorithm_id(), FEATURE_CATALOG_ALGORITHM_ID);
        assert_eq!(catalog.source_algorithm_id(), STAR_MEASUREMENT_ALGORITHM_ID);
        assert_eq!(catalog.parameters(), parameters);
        assert_eq!(catalog.source_measurements(), 3);
        assert_eq!(catalog.features().len(), 2);
        assert_eq!(catalog.features()[0].rank(), 0);
        assert_eq!(catalog.features()[1].rank(), 1);
        assert!(catalog.features()[0].background_snr() > catalog.features()[1].background_snr());
        assert!((catalog.features()[0].point().x() - 40.0).abs() < 0.1);
        assert_eq!(catalog.exclusions().beyond_catalog_limit(), 1);
        assert_eq!(catalog.exclusions().total()?, 1);
        Ok(())
    }

    #[test]
    fn excludes_saturated_elongated_and_border_sources_once() -> TestResult {
        let stars = [
            SyntheticStar {
                x: 20.0,
                y: 20.0,
                amplitude: 900.0,
                sigma_x: 2.0,
                sigma_y: 2.0,
            },
            SyntheticStar {
                x: 42.0,
                y: 42.0,
                amplitude: 500.0,
                sigma_x: 1.2,
                sigma_y: 3.6,
            },
            SyntheticStar {
                x: 70.0,
                y: 65.0,
                amplitude: 450.0,
                sigma_x: 2.0,
                sigma_y: 2.0,
            },
        ];
        let quality = quality(91, 91, &stars, Some(1_800.0))?;
        let parameters = FeatureSelectionParameters::new(1.0, 0.75, 22.0, 10)?;
        let catalog = build_feature_catalog(frame_id()?, 91, 91, &quality, parameters)?;

        assert_eq!(catalog.source_measurements(), 3);
        assert!(catalog.features().is_empty());
        assert_eq!(catalog.exclusions().saturated(), 1);
        assert_eq!(catalog.exclusions().above_maximum_eccentricity(), 1);
        assert_eq!(catalog.exclusions().inside_border_margin(), 1);
        assert_eq!(catalog.exclusions().total()?, 3);
        Ok(())
    }

    #[test]
    fn applies_the_inclusive_minimum_snr_after_saturation() -> TestResult {
        let measured = quality(
            41,
            41,
            &[SyntheticStar {
                x: 20.0,
                y: 20.0,
                amplitude: 500.0,
                sigma_x: 2.0,
                sigma_y: 2.0,
            }],
            None,
        )?;
        let measured_snr = measured.stars()[0].background_snr();
        let parameters = FeatureSelectionParameters::new(measured_snr + 1.0, 0.9, 0.0, 10)?;
        let catalog = build_feature_catalog(frame_id()?, 41, 41, &measured, parameters)?;

        assert!(catalog.features().is_empty());
        assert_eq!(catalog.exclusions().below_minimum_snr(), 1);
        assert_eq!(catalog.exclusions().total()?, 1);
        Ok(())
    }

    #[test]
    fn validates_parameters_dimensions_and_border_domain() -> TestResult {
        assert_eq!(
            FeatureSelectionParameters::new(0.0, 0.8, 0.0, 10),
            Err(FeatureCatalogError::InvalidMinimumSnr)
        );
        assert_eq!(
            FeatureSelectionParameters::new(1.0, 1.0, 0.0, 10),
            Err(FeatureCatalogError::InvalidMaximumEccentricity)
        );
        assert_eq!(
            FeatureSelectionParameters::new(1.0, 0.8, -1.0, 10),
            Err(FeatureCatalogError::InvalidBorderMargin)
        );
        assert!(matches!(
            FeatureSelectionParameters::new(1.0, 0.8, 0.0, MAX_REGISTRATION_FEATURES + 1),
            Err(FeatureCatalogError::InvalidMaximumFeatures { .. })
        ));

        let measured = quality(
            31,
            31,
            &[SyntheticStar {
                x: 15.0,
                y: 15.0,
                amplitude: 500.0,
                sigma_x: 2.0,
                sigma_y: 2.0,
            }],
            None,
        )?;
        let normal = FeatureSelectionParameters::new(1.0, 0.8, 0.0, 10)?;
        assert!(matches!(
            build_feature_catalog(frame_id()?, 30, 31, &measured, normal),
            Err(FeatureCatalogError::DimensionMismatch { .. })
        ));
        let excessive_margin = FeatureSelectionParameters::new(1.0, 0.8, 16.0, 10)?;
        assert_eq!(
            build_feature_catalog(frame_id()?, 31, 31, &measured, excessive_margin),
            Err(FeatureCatalogError::BorderMarginExcludesImage)
        );
        Ok(())
    }
}
