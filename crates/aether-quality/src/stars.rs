use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_core::{CompensatedSum, ScientificImage};
use serde::Serialize;

use crate::background::exact_median;
use crate::{BackgroundError, BackgroundEstimate, BackgroundParameters, estimate_plane_background};

const GAUSSIAN_FWHM_FACTOR: f64 = 2.354_820_045_030_949_3;

/// Validated controls for deterministic stellar detection and shape measurement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StarMeasurementParameters {
    background: BackgroundParameters,
    detection_sigma: f64,
    measurement_floor_sigma: f64,
    measurement_radius: usize,
    minimum_separation: usize,
    minimum_measurement_pixels: usize,
    maximum_candidates: usize,
    saturation_level: Option<f64>,
}

impl StarMeasurementParameters {
    /// Creates explicit source-measurement controls.
    ///
    /// Detection uses local maxima above `detection_sigma`. Shape moments use
    /// pixels above the lower `measurement_floor_sigma` within a circular
    /// aperture. The floor must remain below the detection threshold.
    ///
    /// # Errors
    ///
    /// Returns a typed error for a non-finite threshold, incoherent floor,
    /// undersized aperture/support, zero candidate bound, or non-finite
    /// saturation level.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        background: BackgroundParameters,
        detection_sigma: f64,
        measurement_floor_sigma: f64,
        measurement_radius: usize,
        minimum_separation: usize,
        minimum_measurement_pixels: usize,
        maximum_candidates: usize,
        saturation_level: Option<f64>,
    ) -> Result<Self, FrameQualityError> {
        if !detection_sigma.is_finite() || detection_sigma <= 0.0 {
            return Err(FrameQualityError::InvalidDetectionSigma { detection_sigma });
        }
        if !measurement_floor_sigma.is_finite()
            || measurement_floor_sigma <= 0.0
            || measurement_floor_sigma >= detection_sigma
        {
            return Err(FrameQualityError::InvalidMeasurementFloorSigma {
                measurement_floor_sigma,
                detection_sigma,
            });
        }
        if measurement_radius < 2 {
            return Err(FrameQualityError::InvalidMeasurementRadius { measurement_radius });
        }
        if minimum_separation == 0 {
            return Err(FrameQualityError::ZeroMinimumSeparation);
        }
        if minimum_measurement_pixels < 3 {
            return Err(FrameQualityError::InvalidMinimumMeasurementPixels {
                minimum_measurement_pixels,
            });
        }
        if maximum_candidates == 0 {
            return Err(FrameQualityError::ZeroMaximumCandidates);
        }
        if saturation_level.is_some_and(|value| !value.is_finite()) {
            return Err(FrameQualityError::InvalidSaturationLevel);
        }
        Ok(Self {
            background,
            detection_sigma,
            measurement_floor_sigma,
            measurement_radius,
            minimum_separation,
            minimum_measurement_pixels,
            maximum_candidates,
            saturation_level,
        })
    }

    /// Background estimator used before source detection.
    #[must_use]
    pub const fn background(self) -> BackgroundParameters {
        self.background
    }

    /// Local-maximum threshold above background, in robust noise sigmas.
    #[must_use]
    pub const fn detection_sigma(self) -> f64 {
        self.detection_sigma
    }

    /// Shape-measurement floor above background, in robust noise sigmas.
    #[must_use]
    pub const fn measurement_floor_sigma(self) -> f64 {
        self.measurement_floor_sigma
    }

    /// Circular shape aperture radius in pixels.
    #[must_use]
    pub const fn measurement_radius(self) -> usize {
        self.measurement_radius
    }

    /// Minimum distance between retained local maxima, in pixels.
    #[must_use]
    pub const fn minimum_separation(self) -> usize {
        self.minimum_separation
    }

    /// Minimum above-floor pixels required for a shape measurement.
    #[must_use]
    pub const fn minimum_measurement_pixels(self) -> usize {
        self.minimum_measurement_pixels
    }

    /// Hard bound on local maxima retained before suppression.
    #[must_use]
    pub const fn maximum_candidates(self) -> usize {
        self.maximum_candidates
    }

    /// Physical value at or above which a source is marked saturated.
    #[must_use]
    pub const fn saturation_level(self) -> Option<f64> {
        self.saturation_level
    }
}

/// One deterministic stellar centroid and second-moment shape measurement.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct StarMeasurement {
    centroid_x: f64,
    centroid_y: f64,
    peak: f64,
    flux_above_background: f64,
    background_snr: f64,
    fwhm_major_pixels: f64,
    fwhm_minor_pixels: f64,
    eccentricity: f64,
    measurement_pixels: usize,
    saturated: bool,
}

impl StarMeasurement {
    /// Sub-pixel flux centroid in zero-based image coordinates.
    #[must_use]
    pub const fn centroid_x(self) -> f64 {
        self.centroid_x
    }

    /// Sub-pixel flux centroid in zero-based image coordinates.
    #[must_use]
    pub const fn centroid_y(self) -> f64 {
        self.centroid_y
    }

    /// Highest physical pixel value at the detected local maximum.
    #[must_use]
    pub const fn peak(self) -> f64 {
        self.peak
    }

    /// Sum of positive background-subtracted aperture weights above the floor.
    #[must_use]
    pub const fn flux_above_background(self) -> f64 {
        self.flux_above_background
    }

    /// Flux divided by background sigma times square root of pixel support.
    #[must_use]
    pub const fn background_snr(self) -> f64 {
        self.background_snr
    }

    /// Threshold-corrected major-axis Gaussian FWHM in pixels.
    #[must_use]
    pub const fn fwhm_major_pixels(self) -> f64 {
        self.fwhm_major_pixels
    }

    /// Threshold-corrected minor-axis Gaussian FWHM in pixels.
    #[must_use]
    pub const fn fwhm_minor_pixels(self) -> f64 {
        self.fwhm_minor_pixels
    }

    /// Elliptical eccentricity `sqrt(1 - minor_variance / major_variance)`.
    #[must_use]
    pub const fn eccentricity(self) -> f64 {
        self.eccentricity
    }

    /// Above-floor pixels contributing to centroid and shape.
    #[must_use]
    pub const fn measurement_pixels(self) -> usize {
        self.measurement_pixels
    }

    /// Whether any contributing aperture pixel reached the saturation level.
    #[must_use]
    pub const fn saturated(self) -> bool {
        self.saturated
    }
}

/// Aggregate measurements used by frame review and selection.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FrameQuality {
    background: BackgroundEstimate,
    plane: usize,
    detection_threshold: f64,
    raw_candidates: usize,
    suppressed_candidates: usize,
    rejected_measurements: usize,
    saturated_stars: usize,
    median_fwhm_major_pixels: Option<f64>,
    median_eccentricity: Option<f64>,
    stars: Vec<StarMeasurement>,
}

impl FrameQuality {
    /// Robust background estimate used by every measurement.
    #[must_use]
    pub const fn background(&self) -> BackgroundEstimate {
        self.background
    }

    /// Zero-based planar channel measured.
    #[must_use]
    pub const fn plane(&self) -> usize {
        self.plane
    }

    /// Physical local-maximum detection threshold.
    #[must_use]
    pub const fn detection_threshold(&self) -> f64 {
        self.detection_threshold
    }

    /// Local maxima found before minimum-separation suppression.
    #[must_use]
    pub const fn raw_candidates(&self) -> usize {
        self.raw_candidates
    }

    /// Lower peaks removed near a brighter retained candidate.
    #[must_use]
    pub const fn suppressed_candidates(&self) -> usize {
        self.suppressed_candidates
    }

    /// Retained maxima lacking sufficient valid shape support.
    #[must_use]
    pub const fn rejected_measurements(&self) -> usize {
        self.rejected_measurements
    }

    /// Measured sources touching the explicit saturation level.
    #[must_use]
    pub const fn saturated_stars(&self) -> usize {
        self.saturated_stars
    }

    /// Median major-axis FWHM over unsaturated measured stars.
    #[must_use]
    pub const fn median_fwhm_major_pixels(&self) -> Option<f64> {
        self.median_fwhm_major_pixels
    }

    /// Median eccentricity over unsaturated measured stars.
    #[must_use]
    pub const fn median_eccentricity(&self) -> Option<f64> {
        self.median_eccentricity
    }

    /// Measurements in descending detected-peak order.
    #[must_use]
    pub fn stars(&self) -> &[StarMeasurement] {
        &self.stars
    }
}

/// Failure to measure astronomical frame quality safely.
#[derive(Debug)]
pub enum FrameQualityError {
    /// Robust background estimation failed.
    Background(BackgroundError),
    /// Requested plane is outside the image.
    PlaneOutOfBounds {
        /// Requested plane.
        plane: usize,
        /// Available planes.
        planes: usize,
    },
    /// Detection threshold is not finite and positive.
    InvalidDetectionSigma {
        /// Rejected threshold.
        detection_sigma: f64,
    },
    /// Measurement floor is invalid or not below detection.
    InvalidMeasurementFloorSigma {
        /// Rejected floor.
        measurement_floor_sigma: f64,
        /// Detection threshold it must be below.
        detection_sigma: f64,
    },
    /// Aperture radius must be at least two pixels.
    InvalidMeasurementRadius {
        /// Rejected radius.
        measurement_radius: usize,
    },
    /// Source suppression requires a positive separation.
    ZeroMinimumSeparation,
    /// Shape moments require at least three pixels.
    InvalidMinimumMeasurementPixels {
        /// Rejected support threshold.
        minimum_measurement_pixels: usize,
    },
    /// Candidate storage must have a non-zero bound.
    ZeroMaximumCandidates,
    /// Saturation level must be finite when present.
    InvalidSaturationLevel,
    /// Image is too small to contain one complete measurement aperture.
    ImageTooSmall {
        /// Image width.
        width: usize,
        /// Image height.
        height: usize,
        /// Required border on every side.
        radius: usize,
    },
    /// A uniform retained background cannot define a sigma threshold.
    ZeroNoiseScale,
    /// Local maxima exceeded the explicit work bound.
    TooManyCandidates {
        /// Configured maximum.
        maximum: usize,
    },
    /// A work buffer could not be reserved.
    AllocationFailed {
        /// Requested element count.
        elements: usize,
    },
    /// Checked indexing or a derived measurement exceeded its finite domain.
    NumericalOverflow,
}

impl Display for FrameQualityError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Background(error) => write!(formatter, "cannot estimate background: {error}"),
            Self::PlaneOutOfBounds { plane, planes } => {
                write!(
                    formatter,
                    "quality plane {plane} is outside {planes} available planes"
                )
            }
            Self::InvalidDetectionSigma { detection_sigma } => write!(
                formatter,
                "detection sigma must be finite and positive, received {detection_sigma}"
            ),
            Self::InvalidMeasurementFloorSigma {
                measurement_floor_sigma,
                detection_sigma,
            } => write!(
                formatter,
                "measurement floor sigma {measurement_floor_sigma} must be finite, positive, and below detection sigma {detection_sigma}"
            ),
            Self::InvalidMeasurementRadius { measurement_radius } => write!(
                formatter,
                "measurement radius must be at least 2 pixels, received {measurement_radius}"
            ),
            Self::ZeroMinimumSeparation => {
                formatter.write_str("minimum source separation must be positive")
            }
            Self::InvalidMinimumMeasurementPixels {
                minimum_measurement_pixels,
            } => write!(
                formatter,
                "minimum measurement support must be at least 3 pixels, received {minimum_measurement_pixels}"
            ),
            Self::ZeroMaximumCandidates => {
                formatter.write_str("maximum candidate count must be positive")
            }
            Self::InvalidSaturationLevel => formatter.write_str("saturation level must be finite"),
            Self::ImageTooSmall {
                width,
                height,
                radius,
            } => write!(
                formatter,
                "image {width}x{height} cannot contain an aperture with radius {radius}"
            ),
            Self::ZeroNoiseScale => {
                formatter.write_str("stellar detection requires a non-zero background noise scale")
            }
            Self::TooManyCandidates { maximum } => {
                write!(
                    formatter,
                    "stellar candidates exceed the configured maximum {maximum}"
                )
            }
            Self::AllocationFailed { elements } => {
                write!(
                    formatter,
                    "cannot reserve {elements} frame-quality elements"
                )
            }
            Self::NumericalOverflow => {
                formatter.write_str("stellar measurement exceeds the finite numerical domain")
            }
        }
    }
}

impl Error for FrameQualityError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Background(error) => Some(error),
            _ => None,
        }
    }
}

impl From<BackgroundError> for FrameQualityError {
    fn from(value: BackgroundError) -> Self {
        Self::Background(value)
    }
}

#[derive(Clone, Copy, Debug)]
struct Candidate {
    index_in_plane: usize,
    x: usize,
    y: usize,
    peak: f64,
}

/// Measures robust background and stellar shapes on one linear image plane.
///
/// Candidates are deterministic 3x3 local maxima. Equal-valued plateaus retain
/// the lowest canonical pixel index. Brighter candidates suppress nearby lower
/// peaks before a fixed circular aperture calculates background-subtracted
/// centroids and covariance. Gaussian moments are corrected for the explicit
/// measurement floor. Saturated stars remain visible but do not enter aggregate
/// FWHM or eccentricity.
///
/// This initial strict algorithm expects a scientifically prepared monochrome
/// detection plane. Raw CFA mosaics require an explicit CFA-neutral detection
/// transform before camera presets can enable these metrics.
///
/// # Errors
///
/// Returns typed parameter, background, plane, allocation, work-bound, or
/// numerical failures. No partial quality result is returned.
pub fn measure_frame_quality(
    image: &ScientificImage,
    plane: usize,
    parameters: StarMeasurementParameters,
) -> Result<FrameQuality, FrameQualityError> {
    let parameters = StarMeasurementParameters::new(
        parameters.background,
        parameters.detection_sigma,
        parameters.measurement_floor_sigma,
        parameters.measurement_radius,
        parameters.minimum_separation,
        parameters.minimum_measurement_pixels,
        parameters.maximum_candidates,
        parameters.saturation_level,
    )?;
    let dimensions = image.dimensions();
    if plane >= dimensions.planes() {
        return Err(FrameQualityError::PlaneOutOfBounds {
            plane,
            planes: dimensions.planes(),
        });
    }
    let diameter = parameters
        .measurement_radius
        .checked_mul(2)
        .and_then(|value| value.checked_add(1))
        .ok_or(FrameQualityError::NumericalOverflow)?;
    if dimensions.width() < diameter || dimensions.height() < diameter {
        return Err(FrameQualityError::ImageTooSmall {
            width: dimensions.width(),
            height: dimensions.height(),
            radius: parameters.measurement_radius,
        });
    }

    let background = estimate_plane_background(image, plane, parameters.background)?;
    if background.noise_sigma() == 0.0 {
        return Err(FrameQualityError::ZeroNoiseScale);
    }
    let detection_threshold =
        background.location() + parameters.detection_sigma * background.noise_sigma();
    let measurement_floor = parameters.measurement_floor_sigma * background.noise_sigma();
    if !detection_threshold.is_finite() || !measurement_floor.is_finite() {
        return Err(FrameQualityError::NumericalOverflow);
    }

    let width = dimensions.width();
    let height = dimensions.height();
    let plane_area = width
        .checked_mul(height)
        .ok_or(FrameQualityError::NumericalOverflow)?;
    let plane_offset = plane
        .checked_mul(plane_area)
        .ok_or(FrameQualityError::NumericalOverflow)?;
    let candidates = collect_candidates(
        image,
        plane_offset,
        width,
        height,
        detection_threshold,
        parameters,
    )?;
    let raw_candidates = candidates.len();
    let mut suppressed = try_filled_vec(plane_area, 0_u8)?;
    let mut stars = try_vec(parameters.maximum_candidates.min(raw_candidates))?;
    let mut suppressed_candidates = 0_usize;
    let mut rejected_measurements = 0_usize;

    for candidate in candidates {
        if suppressed[candidate.index_in_plane] != 0 {
            suppressed_candidates += 1;
            continue;
        }
        mark_suppressed(
            &mut suppressed,
            width,
            height,
            candidate.x,
            candidate.y,
            parameters.minimum_separation,
        )?;
        match measure_candidate(
            image,
            plane_offset,
            width,
            height,
            candidate,
            background,
            measurement_floor,
            parameters,
        )? {
            Some(star) => stars.push(star),
            None => rejected_measurements += 1,
        }
    }

    let saturated_stars = stars.iter().filter(|star| star.saturated).count();
    let unsaturated_count = stars.len() - saturated_stars;
    let (median_fwhm_major_pixels, median_eccentricity) = if unsaturated_count == 0 {
        (None, None)
    } else {
        let mut values = try_f64_vec(unsaturated_count)?;
        values.extend(
            stars
                .iter()
                .filter(|star| !star.saturated)
                .map(|star| star.fwhm_major_pixels),
        );
        let fwhm = exact_median(&mut values);
        values.clear();
        values.extend(
            stars
                .iter()
                .filter(|star| !star.saturated)
                .map(|star| star.eccentricity),
        );
        (Some(fwhm), Some(exact_median(&mut values)))
    };

    Ok(FrameQuality {
        background,
        plane,
        detection_threshold,
        raw_candidates,
        suppressed_candidates,
        rejected_measurements,
        saturated_stars,
        median_fwhm_major_pixels,
        median_eccentricity,
        stars,
    })
}

fn collect_candidates(
    image: &ScientificImage,
    plane_offset: usize,
    width: usize,
    height: usize,
    threshold: f64,
    parameters: StarMeasurementParameters,
) -> Result<Vec<Candidate>, FrameQualityError> {
    let capacity = parameters
        .maximum_candidates
        .min(width.saturating_mul(height));
    let mut candidates = try_vec(capacity)?;
    let radius = parameters.measurement_radius;
    for y in radius..height - radius {
        for x in radius..width - radius {
            let index_in_plane = y
                .checked_mul(width)
                .and_then(|value| value.checked_add(x))
                .ok_or(FrameQualityError::NumericalOverflow)?;
            let index = plane_offset
                .checked_add(index_in_plane)
                .ok_or(FrameQualityError::NumericalOverflow)?;
            let value = image.pixels()[index];
            if value <= threshold
                || !value.is_finite()
                || !image.mask().as_slice()[index].is_clear()
                || !is_local_maximum(image, plane_offset, width, x, y, value)?
            {
                continue;
            }
            if candidates.len() == parameters.maximum_candidates {
                return Err(FrameQualityError::TooManyCandidates {
                    maximum: parameters.maximum_candidates,
                });
            }
            candidates.push(Candidate {
                index_in_plane,
                x,
                y,
                peak: value,
            });
        }
    }
    candidates.sort_by(|left, right| {
        right
            .peak
            .total_cmp(&left.peak)
            .then_with(|| left.index_in_plane.cmp(&right.index_in_plane))
    });
    Ok(candidates)
}

fn is_local_maximum(
    image: &ScientificImage,
    plane_offset: usize,
    width: usize,
    x: usize,
    y: usize,
    value: f64,
) -> Result<bool, FrameQualityError> {
    let center = y
        .checked_mul(width)
        .and_then(|row| row.checked_add(x))
        .ok_or(FrameQualityError::NumericalOverflow)?;
    for neighbor_y in y - 1..=y + 1 {
        for neighbor_x in x - 1..=x + 1 {
            let neighbor = neighbor_y
                .checked_mul(width)
                .and_then(|row| row.checked_add(neighbor_x))
                .ok_or(FrameQualityError::NumericalOverflow)?;
            if neighbor == center {
                continue;
            }
            let absolute = plane_offset
                .checked_add(neighbor)
                .ok_or(FrameQualityError::NumericalOverflow)?;
            if !image.mask().as_slice()[absolute].is_clear() {
                continue;
            }
            let neighbor_value = image.pixels()[absolute];
            if neighbor_value.is_finite() {
                match neighbor_value.total_cmp(&value) {
                    std::cmp::Ordering::Greater => return Ok(false),
                    std::cmp::Ordering::Equal if neighbor < center => return Ok(false),
                    std::cmp::Ordering::Equal | std::cmp::Ordering::Less => {}
                }
            }
        }
    }
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
fn measure_candidate(
    image: &ScientificImage,
    plane_offset: usize,
    width: usize,
    height: usize,
    candidate: Candidate,
    background: BackgroundEstimate,
    measurement_floor: f64,
    parameters: StarMeasurementParameters,
) -> Result<Option<StarMeasurement>, FrameQualityError> {
    let radius = parameters.measurement_radius;
    let radius_squared = radius
        .checked_mul(radius)
        .ok_or(FrameQualityError::NumericalOverflow)?;
    let mut flux = CompensatedSum::new();
    let mut weighted_x = CompensatedSum::new();
    let mut weighted_y = CompensatedSum::new();
    let mut support = 0_usize;
    let mut saturated = false;

    for y in candidate.y - radius..=candidate.y + radius {
        for x in candidate.x - radius..=candidate.x + radius {
            if y >= height || !inside_circle(x, y, candidate.x, candidate.y, radius_squared)? {
                continue;
            }
            let index = absolute_index(plane_offset, width, x, y)?;
            if !image.mask().as_slice()[index].is_clear() {
                continue;
            }
            let value = image.pixels()[index];
            if !value.is_finite() {
                continue;
            }
            let weight = value - background.location();
            if !weight.is_finite() {
                return Err(FrameQualityError::NumericalOverflow);
            }
            if weight <= measurement_floor {
                continue;
            }
            support += 1;
            flux.add(weight);
            weighted_x.add(weight * signed_delta(x, candidate.x)?);
            weighted_y.add(weight * signed_delta(y, candidate.y)?);
            saturated |= parameters
                .saturation_level
                .is_some_and(|level| value >= level);
        }
    }
    if support < parameters.minimum_measurement_pixels {
        return Ok(None);
    }
    let flux = flux.total();
    if !flux.is_finite() || flux <= 0.0 {
        return Err(FrameQualityError::NumericalOverflow);
    }
    let centroid_dx = weighted_x.total() / flux;
    let centroid_dy = weighted_y.total() / flux;
    if !centroid_dx.is_finite() || !centroid_dy.is_finite() {
        return Err(FrameQualityError::NumericalOverflow);
    }

    let mut xx = CompensatedSum::new();
    let mut yy = CompensatedSum::new();
    let mut xy = CompensatedSum::new();
    for y in candidate.y - radius..=candidate.y + radius {
        for x in candidate.x - radius..=candidate.x + radius {
            if y >= height || !inside_circle(x, y, candidate.x, candidate.y, radius_squared)? {
                continue;
            }
            let index = absolute_index(plane_offset, width, x, y)?;
            if !image.mask().as_slice()[index].is_clear() {
                continue;
            }
            let value = image.pixels()[index];
            if !value.is_finite() {
                continue;
            }
            let weight = value - background.location();
            if weight <= measurement_floor {
                continue;
            }
            let dx = signed_delta(x, candidate.x)? - centroid_dx;
            let dy = signed_delta(y, candidate.y)? - centroid_dy;
            xx.add(weight * dx * dx);
            yy.add(weight * dy * dy);
            xy.add(weight * dx * dy);
        }
    }

    let covariance_xx = xx.total() / flux;
    let covariance_yy = yy.total() / flux;
    let covariance_xy = xy.total() / flux;
    let trace = covariance_xx + covariance_yy;
    let difference = covariance_xx - covariance_yy;
    let discriminant = difference.mul_add(difference, 4.0 * covariance_xy * covariance_xy);
    if !trace.is_finite() || !discriminant.is_finite() {
        return Err(FrameQualityError::NumericalOverflow);
    }
    let root = discriminant.max(0.0).sqrt();
    let major_observed = ((trace + root) * 0.5).max(0.0);
    let minor_observed = ((trace - root) * 0.5).max(0.0);

    let peak_signal = candidate.peak - background.location();
    if !peak_signal.is_finite() || peak_signal <= measurement_floor {
        return Ok(None);
    }
    let truncation_u = (peak_signal / measurement_floor).ln();
    let excluded_fraction = (-truncation_u).exp();
    let denominator = 1.0 - excluded_fraction;
    let correction = 1.0 - ((truncation_u + 1.0) * excluded_fraction / denominator);
    if !correction.is_finite() || correction <= 0.0 {
        return Err(FrameQualityError::NumericalOverflow);
    }
    let major_variance = major_observed / correction;
    let minor_variance = minor_observed / correction;
    if !major_variance.is_finite()
        || !minor_variance.is_finite()
        || major_variance <= 0.0
        || minor_variance < 0.0
    {
        return Ok(None);
    }
    let fwhm_major_pixels = GAUSSIAN_FWHM_FACTOR * major_variance.sqrt();
    let fwhm_minor_pixels = GAUSSIAN_FWHM_FACTOR * minor_variance.sqrt();
    let eccentricity = (1.0 - minor_variance / major_variance)
        .clamp(0.0, 1.0)
        .sqrt();
    let background_snr = flux / (background.noise_sigma() * (support as f64).sqrt());
    let centroid_x = candidate.x as f64 + centroid_dx;
    let centroid_y = candidate.y as f64 + centroid_dy;
    if [
        fwhm_major_pixels,
        fwhm_minor_pixels,
        eccentricity,
        background_snr,
        centroid_x,
        centroid_y,
    ]
    .iter()
    .any(|value| !value.is_finite())
    {
        return Err(FrameQualityError::NumericalOverflow);
    }

    Ok(Some(StarMeasurement {
        centroid_x: canonical_zero(centroid_x),
        centroid_y: canonical_zero(centroid_y),
        peak: candidate.peak,
        flux_above_background: flux,
        background_snr,
        fwhm_major_pixels,
        fwhm_minor_pixels,
        eccentricity,
        measurement_pixels: support,
        saturated,
    }))
}

fn mark_suppressed(
    suppressed: &mut [u8],
    width: usize,
    height: usize,
    center_x: usize,
    center_y: usize,
    radius: usize,
) -> Result<(), FrameQualityError> {
    let radius_squared = radius
        .checked_mul(radius)
        .ok_or(FrameQualityError::NumericalOverflow)?;
    let minimum_x = center_x.saturating_sub(radius);
    let minimum_y = center_y.saturating_sub(radius);
    let maximum_x = center_x.saturating_add(radius).min(width - 1);
    let maximum_y = center_y.saturating_add(radius).min(height - 1);
    for y in minimum_y..=maximum_y {
        for x in minimum_x..=maximum_x {
            if inside_circle(x, y, center_x, center_y, radius_squared)? {
                let index = y
                    .checked_mul(width)
                    .and_then(|row| row.checked_add(x))
                    .ok_or(FrameQualityError::NumericalOverflow)?;
                suppressed[index] = 1;
            }
        }
    }
    Ok(())
}

fn inside_circle(
    x: usize,
    y: usize,
    center_x: usize,
    center_y: usize,
    radius_squared: usize,
) -> Result<bool, FrameQualityError> {
    let dx = x.abs_diff(center_x);
    let dy = y.abs_diff(center_y);
    let distance_squared = dx
        .checked_mul(dx)
        .and_then(|value| dy.checked_mul(dy).and_then(|dy| value.checked_add(dy)))
        .ok_or(FrameQualityError::NumericalOverflow)?;
    Ok(distance_squared <= radius_squared)
}

fn absolute_index(
    plane_offset: usize,
    width: usize,
    x: usize,
    y: usize,
) -> Result<usize, FrameQualityError> {
    plane_offset
        .checked_add(
            y.checked_mul(width)
                .and_then(|row| row.checked_add(x))
                .ok_or(FrameQualityError::NumericalOverflow)?,
        )
        .ok_or(FrameQualityError::NumericalOverflow)
}

fn signed_delta(value: usize, center: usize) -> Result<f64, FrameQualityError> {
    if value >= center {
        Ok((value - center) as f64)
    } else {
        Ok(-((center - value) as f64))
    }
}

fn try_vec<T>(elements: usize) -> Result<Vec<T>, FrameQualityError> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(elements)
        .map_err(|_| FrameQualityError::AllocationFailed { elements })?;
    Ok(output)
}

fn try_f64_vec(elements: usize) -> Result<Vec<f64>, FrameQualityError> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(elements)
        .map_err(|_| FrameQualityError::AllocationFailed { elements })?;
    Ok(output)
}

fn try_filled_vec<T: Clone>(elements: usize, value: T) -> Result<Vec<T>, FrameQualityError> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(elements)
        .map_err(|_| FrameQualityError::AllocationFailed { elements })?;
    output.resize(elements, value);
    Ok(output)
}

fn canonical_zero(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use aether_core::Dimensions;

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

    fn synthetic_image(
        width: usize,
        height: usize,
        stars: &[SyntheticStar],
    ) -> TestResult<ScientificImage> {
        let mut pixels = Vec::new();
        pixels.try_reserve_exact(width * height)?;
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
        Ok(ScientificImage::from_pixels(
            Dimensions::new(width, height, 1)?,
            pixels,
        )?)
    }

    fn parameters(saturation: Option<f64>) -> TestResult<StarMeasurementParameters> {
        Ok(StarMeasurementParameters::new(
            BackgroundParameters::new(3.0, 8, 100)?,
            6.0,
            2.0,
            10,
            8,
            9,
            1_000,
            saturation,
        )?)
    }

    #[test]
    fn measures_synthetic_elliptical_gaussian_shape() -> TestResult {
        let expected = SyntheticStar {
            x: 30.25,
            y: 29.75,
            amplitude: 500.0,
            sigma_x: 2.0,
            sigma_y: 3.0,
        };
        let image = synthetic_image(61, 61, &[expected])?;
        let quality = measure_frame_quality(&image, 0, parameters(None)?)?;

        assert_eq!(quality.stars().len(), 1);
        let measured = quality.stars()[0];
        assert!((measured.centroid_x() - expected.x).abs() < 0.1);
        assert!((measured.centroid_y() - expected.y).abs() < 0.1);
        assert!((measured.fwhm_major_pixels() - GAUSSIAN_FWHM_FACTOR * 3.0).abs() < 0.5);
        assert!((measured.fwhm_minor_pixels() - GAUSSIAN_FWHM_FACTOR * 2.0).abs() < 0.5);
        let expected_eccentricity = (1.0_f64 - 4.0 / 9.0).sqrt();
        assert!((measured.eccentricity() - expected_eccentricity).abs() < 0.05);
        assert_eq!(
            quality.median_fwhm_major_pixels(),
            Some(measured.fwhm_major_pixels())
        );
        assert_eq!(quality.median_eccentricity(), Some(measured.eccentricity()));
        Ok(())
    }

    #[test]
    fn detects_multiple_stars_and_suppresses_nearby_lower_maxima() -> TestResult {
        let stars = [
            SyntheticStar {
                x: 22.0,
                y: 25.0,
                amplitude: 500.0,
                sigma_x: 2.0,
                sigma_y: 2.0,
            },
            SyntheticStar {
                x: 48.0,
                y: 42.0,
                amplitude: 300.0,
                sigma_x: 2.5,
                sigma_y: 2.5,
            },
        ];
        let quality =
            measure_frame_quality(&synthetic_image(75, 75, &stars)?, 0, parameters(None)?)?;

        assert_eq!(quality.stars().len(), 2);
        assert!(quality.raw_candidates() >= quality.stars().len());
        assert!(quality.stars()[0].peak() > quality.stars()[1].peak());
        Ok(())
    }

    #[test]
    fn saturated_sources_remain_visible_but_leave_aggregate_shape() -> TestResult {
        let star = SyntheticStar {
            x: 30.0,
            y: 30.0,
            amplitude: 500.0,
            sigma_x: 2.0,
            sigma_y: 2.0,
        };
        let quality = measure_frame_quality(
            &synthetic_image(61, 61, &[star])?,
            0,
            parameters(Some(1_400.0))?,
        )?;

        assert_eq!(quality.stars().len(), 1);
        assert!(quality.stars()[0].saturated());
        assert_eq!(quality.saturated_stars(), 1);
        assert_eq!(quality.median_fwhm_major_pixels(), None);
        assert_eq!(quality.median_eccentricity(), None);
        Ok(())
    }

    #[test]
    fn rejects_uniform_background_and_out_of_range_plane() -> TestResult {
        let image = ScientificImage::from_pixels(Dimensions::new(25, 25, 1)?, vec![1_000.0; 625])?;
        assert!(matches!(
            measure_frame_quality(&image, 0, parameters(None)?),
            Err(FrameQualityError::ZeroNoiseScale)
        ));
        assert!(matches!(
            measure_frame_quality(&image, 1, parameters(None)?),
            Err(FrameQualityError::PlaneOutOfBounds {
                plane: 1,
                planes: 1,
            })
        ));
        Ok(())
    }

    #[test]
    fn validates_measurement_parameters_and_candidate_bound() -> TestResult {
        let background = BackgroundParameters::new(3.0, 4, 10)?;
        assert!(matches!(
            StarMeasurementParameters::new(background, 0.0, 1.0, 4, 2, 3, 10, None),
            Err(FrameQualityError::InvalidDetectionSigma { .. })
        ));
        assert!(matches!(
            StarMeasurementParameters::new(background, 5.0, 5.0, 4, 2, 3, 10, None),
            Err(FrameQualityError::InvalidMeasurementFloorSigma { .. })
        ));
        assert!(matches!(
            StarMeasurementParameters::new(background, 5.0, 1.0, 1, 2, 3, 10, None),
            Err(FrameQualityError::InvalidMeasurementRadius { .. })
        ));
        assert!(matches!(
            StarMeasurementParameters::new(background, 5.0, 1.0, 4, 2, 3, 0, None),
            Err(FrameQualityError::ZeroMaximumCandidates)
        ));

        let crowded = synthetic_image(
            41,
            41,
            &[
                SyntheticStar {
                    x: 12.0,
                    y: 12.0,
                    amplitude: 500.0,
                    sigma_x: 1.0,
                    sigma_y: 1.0,
                },
                SyntheticStar {
                    x: 28.0,
                    y: 28.0,
                    amplitude: 500.0,
                    sigma_x: 1.0,
                    sigma_y: 1.0,
                },
            ],
        )?;
        let bounded = StarMeasurementParameters::new(background, 6.0, 2.0, 4, 2, 3, 1, None)?;
        assert!(matches!(
            measure_frame_quality(&crowded, 0, bounded),
            Err(FrameQualityError::TooManyCandidates { maximum: 1 })
        ));
        Ok(())
    }
}
