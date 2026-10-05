use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_core::{Dimensions, PixelFlags, PixelMask};
use aether_quality::StarMeasurement;

/// Stable identity of conservative stellar protection-mask rasterization.
pub const PROTECTED_SOURCE_MASK_ALGORITHM_ID: &str = "stellar-fwhm-protection-mask-v1";

/// One validated source footprint input.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProtectedSource {
    centroid_x: f64,
    centroid_y: f64,
    fwhm_major_pixels: f64,
    saturated: bool,
}

impl ProtectedSource {
    /// Builds one finite positive-width source description.
    pub fn new(
        centroid_x: f64,
        centroid_y: f64,
        fwhm_major_pixels: f64,
        saturated: bool,
    ) -> Result<Self, ProtectionError> {
        if !centroid_x.is_finite()
            || !centroid_y.is_finite()
            || !fwhm_major_pixels.is_finite()
            || fwhm_major_pixels <= 0.0
        {
            return Err(ProtectionError::InvalidSource);
        }
        Ok(Self {
            centroid_x,
            centroid_y,
            fwhm_major_pixels,
            saturated,
        })
    }

    /// Sub-pixel horizontal centroid.
    #[must_use]
    pub const fn centroid_x(self) -> f64 {
        self.centroid_x
    }

    /// Sub-pixel vertical centroid.
    #[must_use]
    pub const fn centroid_y(self) -> f64 {
        self.centroid_y
    }

    /// Major-axis full width at half maximum.
    #[must_use]
    pub const fn fwhm_major_pixels(self) -> f64 {
        self.fwhm_major_pixels
    }

    /// Whether the measurement touched the configured saturation level.
    #[must_use]
    pub const fn saturated(self) -> bool {
        self.saturated
    }
}

/// Explicit dilation and resource limits for stellar protection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProtectionParameters {
    growth_factor: f64,
    saturated_growth_factor: f64,
    minimum_radius: usize,
    maximum_radius: usize,
    maximum_sources: usize,
    maximum_pixel_visits: usize,
}

impl ProtectionParameters {
    /// Builds bounded protection controls.
    pub fn new(
        growth_factor: f64,
        saturated_growth_factor: f64,
        minimum_radius: usize,
        maximum_radius: usize,
        maximum_sources: usize,
        maximum_pixel_visits: usize,
    ) -> Result<Self, ProtectionError> {
        if !growth_factor.is_finite()
            || growth_factor <= 0.0
            || !saturated_growth_factor.is_finite()
            || saturated_growth_factor < 1.0
            || minimum_radius == 0
            || maximum_radius < minimum_radius
            || maximum_sources == 0
            || maximum_pixel_visits == 0
        {
            return Err(ProtectionError::InvalidParameters);
        }
        Ok(Self {
            growth_factor,
            saturated_growth_factor,
            minimum_radius,
            maximum_radius,
            maximum_sources,
            maximum_pixel_visits,
        })
    }
}

/// Rasterized mask and exact construction evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtectionMask {
    mask: PixelMask,
    source_count: usize,
    edge_clipped_sources: usize,
    candidate_pixel_visits: usize,
    protected_pixels: usize,
    overlap_hits: usize,
}

impl ProtectionMask {
    /// Mask whose protected pixels carry `REJECTED` on the selected plane.
    #[must_use]
    pub const fn mask(&self) -> &PixelMask {
        &self.mask
    }

    /// Consumes the report and returns its mask.
    #[must_use]
    pub fn into_mask(self) -> PixelMask {
        self.mask
    }

    /// Number of source footprints rasterized.
    #[must_use]
    pub const fn source_count(&self) -> usize {
        self.source_count
    }

    /// Footprints intersecting at least one image boundary.
    #[must_use]
    pub const fn edge_clipped_sources(&self) -> usize {
        self.edge_clipped_sources
    }

    /// Bounding-box pixels visited after the complete preflight.
    #[must_use]
    pub const fn candidate_pixel_visits(&self) -> usize {
        self.candidate_pixel_visits
    }

    /// Unique pixels protected on the selected plane.
    #[must_use]
    pub const fn protected_pixels(&self) -> usize {
        self.protected_pixels
    }

    /// In-circle writes landing on an already protected pixel.
    #[must_use]
    pub const fn overlap_hits(&self) -> usize {
        self.overlap_hits
    }
}

/// Failure to validate or rasterize a stellar protection mask.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtectionError {
    /// Growth, radius, source, or work controls are inconsistent.
    InvalidParameters,
    /// A source coordinate or width is non-finite or invalid.
    InvalidSource,
    /// A source centroid lies outside the image.
    SourceOutsideImage,
    /// The selected plane does not exist.
    PlaneOutOfBounds,
    /// The catalog exceeds its configured source bound.
    SourceLimitExceeded,
    /// Checked rasterization arithmetic overflowed.
    ArithmeticOverflow,
    /// Preflight bounding-box work exceeds its configured limit.
    WorkLimitExceeded,
    /// Result or preflight allocation failed.
    AllocationFailed,
}

impl Display for ProtectionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidParameters => "stellar protection parameters are inconsistent",
            Self::InvalidSource => "stellar protection source is not finite and positive-width",
            Self::SourceOutsideImage => "stellar protection centroid is outside the image",
            Self::PlaneOutOfBounds => "stellar protection plane is outside the image",
            Self::SourceLimitExceeded => "stellar protection source limit was exceeded",
            Self::ArithmeticOverflow => "stellar protection arithmetic overflowed",
            Self::WorkLimitExceeded => "stellar protection raster work limit was exceeded",
            Self::AllocationFailed => "stellar protection allocation failed",
        })
    }
}

impl Error for ProtectionError {}

/// Converts validated quality measurements without changing their coordinates.
pub fn protected_sources_from_stars(
    stars: &[StarMeasurement],
    maximum_sources: usize,
) -> Result<Vec<ProtectedSource>, ProtectionError> {
    if maximum_sources == 0 {
        return Err(ProtectionError::InvalidParameters);
    }
    if stars.len() > maximum_sources {
        return Err(ProtectionError::SourceLimitExceeded);
    }
    let mut sources = Vec::new();
    sources
        .try_reserve_exact(stars.len())
        .map_err(|_| ProtectionError::AllocationFailed)?;
    for star in stars {
        sources.push(ProtectedSource::new(
            star.centroid_x(),
            star.centroid_y(),
            star.fwhm_major_pixels(),
            star.saturated(),
        )?);
    }
    Ok(sources)
}

#[derive(Clone, Copy, Debug)]
struct RasterSource {
    source: ProtectedSource,
    radius: f64,
    left: usize,
    top: usize,
    right: usize,
    bottom: usize,
    edge_clipped: bool,
}

/// Rasterizes conservative circular source regions after a complete work preflight.
pub fn build_protection_mask(
    dimensions: Dimensions,
    plane: usize,
    sources: &[ProtectedSource],
    parameters: ProtectionParameters,
) -> Result<ProtectionMask, ProtectionError> {
    if plane >= dimensions.planes() {
        return Err(ProtectionError::PlaneOutOfBounds);
    }
    if sources.len() > parameters.maximum_sources {
        return Err(ProtectionError::SourceLimitExceeded);
    }
    let mut raster_sources = Vec::new();
    raster_sources
        .try_reserve_exact(sources.len())
        .map_err(|_| ProtectionError::AllocationFailed)?;
    let mut candidate_pixel_visits = 0_usize;
    let mut edge_clipped_sources = 0_usize;
    for &source in sources {
        if source.centroid_x < 0.0
            || source.centroid_x >= dimensions.width() as f64
            || source.centroid_y < 0.0
            || source.centroid_y >= dimensions.height() as f64
        {
            return Err(ProtectionError::SourceOutsideImage);
        }
        let saturation_growth = if source.saturated {
            parameters.saturated_growth_factor
        } else {
            1.0
        };
        let radius = (source.fwhm_major_pixels * parameters.growth_factor * saturation_growth)
            .max(parameters.minimum_radius as f64)
            .min(parameters.maximum_radius as f64);
        let unclipped_left = source.centroid_x - radius;
        let unclipped_top = source.centroid_y - radius;
        let unclipped_right = source.centroid_x + radius;
        let unclipped_bottom = source.centroid_y + radius;
        let edge_clipped = unclipped_left < 0.0
            || unclipped_top < 0.0
            || unclipped_right >= dimensions.width() as f64
            || unclipped_bottom >= dimensions.height() as f64;
        edge_clipped_sources += usize::from(edge_clipped);
        let left = unclipped_left.floor().max(0.0) as usize;
        let top = unclipped_top.floor().max(0.0) as usize;
        let right = (unclipped_right.ceil() + 1.0).min(dimensions.width() as f64) as usize;
        let bottom = (unclipped_bottom.ceil() + 1.0).min(dimensions.height() as f64) as usize;
        let visits = (right - left)
            .checked_mul(bottom - top)
            .ok_or(ProtectionError::ArithmeticOverflow)?;
        candidate_pixel_visits = candidate_pixel_visits
            .checked_add(visits)
            .ok_or(ProtectionError::ArithmeticOverflow)?;
        if candidate_pixel_visits > parameters.maximum_pixel_visits {
            return Err(ProtectionError::WorkLimitExceeded);
        }
        raster_sources.push(RasterSource {
            source,
            radius,
            left,
            top,
            right,
            bottom,
            edge_clipped,
        });
    }

    let mut mask = PixelMask::clear(dimensions).map_err(|_| ProtectionError::AllocationFailed)?;
    let plane_offset = plane
        .checked_mul(
            dimensions
                .width()
                .checked_mul(dimensions.height())
                .ok_or(ProtectionError::ArithmeticOverflow)?,
        )
        .ok_or(ProtectionError::ArithmeticOverflow)?;
    let mut protected_pixels = 0_usize;
    let mut overlap_hits = 0_usize;
    for raster in &raster_sources {
        let radius_squared = raster.radius * raster.radius;
        for y in raster.top..raster.bottom {
            for x in raster.left..raster.right {
                let dx = x as f64 - raster.source.centroid_x;
                let dy = y as f64 - raster.source.centroid_y;
                if dx.mul_add(dx, dy * dy) <= radius_squared {
                    let index = plane_offset + y * dimensions.width() + x;
                    if mask.as_slice()[index].is_clear() {
                        mask.as_mut_slice()[index] = PixelFlags::REJECTED;
                        protected_pixels += 1;
                    } else {
                        overlap_hits += 1;
                    }
                }
            }
        }
    }
    debug_assert_eq!(
        raster_sources
            .iter()
            .filter(|source| source.edge_clipped)
            .count(),
        edge_clipped_sources
    );
    Ok(ProtectionMask {
        mask,
        source_count: sources.len(),
        edge_clipped_sources,
        candidate_pixel_visits,
        protected_pixels,
        overlap_hits,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn Error>>;

    fn parameters(maximum_pixel_visits: usize) -> Result<ProtectionParameters, ProtectionError> {
        ProtectionParameters::new(1.0, 2.0, 1, 8, 8, maximum_pixel_visits)
    }

    #[test]
    fn rasterizes_overlap_deterministically_on_only_the_selected_plane() -> TestResult {
        let dimensions = Dimensions::new(7, 7, 2)?;
        let source = ProtectedSource::new(3.0, 3.0, 1.0, false)?;
        let report = build_protection_mask(dimensions, 1, &[source, source], parameters(100)?)?;
        assert_eq!(report.source_count(), 2);
        assert_eq!(report.edge_clipped_sources(), 0);
        assert_eq!(report.candidate_pixel_visits(), 18);
        assert_eq!(report.protected_pixels(), 5);
        assert_eq!(report.overlap_hits(), 5);
        assert!(report.mask().get(3, 3, 1)?.contains(PixelFlags::REJECTED));
        assert!(report.mask().get(3, 3, 0)?.is_clear());
        Ok(())
    }

    #[test]
    fn saturation_growth_and_edge_clipping_are_explicit() -> TestResult {
        let dimensions = Dimensions::new(9, 9, 1)?;
        let ordinary = build_protection_mask(
            dimensions,
            0,
            &[ProtectedSource::new(4.0, 4.0, 1.0, false)?],
            parameters(100)?,
        )?;
        let saturated = build_protection_mask(
            dimensions,
            0,
            &[ProtectedSource::new(4.0, 4.0, 1.0, true)?],
            parameters(100)?,
        )?;
        assert!(saturated.protected_pixels() > ordinary.protected_pixels());

        let edge = build_protection_mask(
            dimensions,
            0,
            &[ProtectedSource::new(0.0, 0.0, 2.0, false)?],
            parameters(100)?,
        )?;
        assert_eq!(edge.edge_clipped_sources(), 1);
        assert!(edge.protected_pixels() > 0);
        Ok(())
    }

    #[test]
    fn validation_and_work_limits_fail_before_rasterization() -> TestResult {
        assert_eq!(
            ProtectionParameters::new(0.0, 1.0, 1, 2, 1, 1),
            Err(ProtectionError::InvalidParameters)
        );
        assert_eq!(
            ProtectedSource::new(0.0, 0.0, f64::NAN, false),
            Err(ProtectionError::InvalidSource)
        );
        let dimensions = Dimensions::new(5, 5, 1)?;
        let source = ProtectedSource::new(2.0, 2.0, 2.0, false)?;
        assert_eq!(
            build_protection_mask(dimensions, 1, &[source], parameters(100)?),
            Err(ProtectionError::PlaneOutOfBounds)
        );
        assert_eq!(
            build_protection_mask(
                dimensions,
                0,
                &[ProtectedSource::new(5.0, 2.0, 1.0, false)?],
                parameters(100)?
            ),
            Err(ProtectionError::SourceOutsideImage)
        );
        assert_eq!(
            build_protection_mask(
                dimensions,
                0,
                &[source, source],
                ProtectionParameters::new(1.0, 1.0, 1, 2, 1, 100)?
            ),
            Err(ProtectionError::SourceLimitExceeded)
        );
        assert_eq!(
            build_protection_mask(dimensions, 0, &[source], parameters(24)?),
            Err(ProtectionError::WorkLimitExceeded)
        );
        Ok(())
    }
}
