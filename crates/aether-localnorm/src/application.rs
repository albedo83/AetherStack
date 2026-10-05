use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_core::{Dimensions, PixelFlags, ScientificImage};

use crate::{LocalCoefficientSurface, SurfaceError};

/// Stable identity of guarded full-image coefficient application.
pub const LOCAL_APPLICATION_ALGORITHM_ID: &str = "local-surface-apply-f64-v1";

/// Complete accounting for one full-image local-normalization application.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LocalApplicationEvidence {
    transformed: usize,
    inherited_masked: usize,
    non_finite_input: usize,
    unsupported_surface: usize,
    non_finite_result: usize,
}

impl LocalApplicationEvidence {
    /// Adds disjoint plane evidence with checked accounting.
    #[must_use]
    pub fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            transformed: self.transformed.checked_add(other.transformed)?,
            inherited_masked: self.inherited_masked.checked_add(other.inherited_masked)?,
            non_finite_input: self.non_finite_input.checked_add(other.non_finite_input)?,
            unsupported_surface: self
                .unsupported_surface
                .checked_add(other.unsupported_surface)?,
            non_finite_result: self
                .non_finite_result
                .checked_add(other.non_finite_result)?,
        })
    }

    /// Clear finite samples transformed successfully.
    #[must_use]
    pub const fn transformed(self) -> usize {
        self.transformed
    }

    /// Samples skipped because an input quality flag was already present.
    #[must_use]
    pub const fn inherited_masked(self) -> usize {
        self.inherited_masked
    }

    /// Clear input samples found to contain NaN or infinity.
    #[must_use]
    pub const fn non_finite_input(self) -> usize {
        self.non_finite_input
    }

    /// Samples without enough valid local surface support.
    #[must_use]
    pub const fn unsupported_surface(self) -> usize {
        self.unsupported_surface
    }

    /// Samples whose finite input produced a non-finite affine result.
    #[must_use]
    pub const fn non_finite_result(self) -> usize {
        self.non_finite_result
    }

    /// Total samples classified by the mutually exclusive application paths.
    #[must_use]
    pub const fn classified_samples(self) -> usize {
        self.transformed
            + self.inherited_masked
            + self.non_finite_input
            + self.unsupported_surface
            + self.non_finite_result
    }
}

/// Normalized image and its exact application evidence.
#[derive(Clone, Debug, PartialEq)]
pub struct LocalApplication {
    image: ScientificImage,
    evidence: LocalApplicationEvidence,
}

impl LocalApplication {
    /// Normalized image with inherited and newly generated quality flags.
    #[must_use]
    pub const fn image(&self) -> &ScientificImage {
        &self.image
    }

    /// Consumes the report and returns the normalized image.
    #[must_use]
    pub fn into_image(self) -> ScientificImage {
        self.image
    }

    /// Complete mutually exclusive sample accounting.
    #[must_use]
    pub const fn evidence(&self) -> LocalApplicationEvidence {
        self.evidence
    }
}

/// Failure to validate or allocate a full-image local application.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplicationError {
    /// Exactly one coefficient surface is required per image plane.
    SurfaceCountMismatch,
    /// A requested application band is empty or outside the one-plane source.
    InvalidBand,
    /// Output or bounded interpolation scratch allocation failed.
    AllocationFailed,
    /// Reconstructing a same-sized output image unexpectedly failed.
    ImageConstructionFailed,
}

impl Display for ApplicationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::SurfaceCountMismatch => {
                "local application requires exactly one surface per image plane"
            }
            Self::InvalidBand => "local application band is outside its one-plane source",
            Self::AllocationFailed => "local application allocation failed",
            Self::ImageConstructionFailed => "local application could not construct its output",
        })
    }
}

/// Applies one surface to a consecutive row band of a one-plane source.
///
/// Surface coordinates remain global while the returned image is a compact
/// one-plane chunk suitable for canonical streaming FITS output.
pub fn apply_local_surface_band(
    source: &ScientificImage,
    surface: &LocalCoefficientSurface,
    start_y: usize,
    row_count: usize,
) -> Result<LocalApplication, ApplicationError> {
    let source_dimensions = source.dimensions();
    let end_y = start_y
        .checked_add(row_count)
        .ok_or(ApplicationError::InvalidBand)?;
    if source_dimensions.planes() != 1
        || row_count == 0
        || start_y >= source_dimensions.height()
        || end_y > source_dimensions.height()
    {
        return Err(ApplicationError::InvalidBand);
    }
    let dimensions = Dimensions::new(source_dimensions.width(), row_count, 1)
        .map_err(|_| ApplicationError::ImageConstructionFailed)?;
    let mut image = ScientificImage::filled(dimensions, 0.0)
        .map_err(|_| ApplicationError::ImageConstructionFailed)?;
    let mut neighbors = Vec::new();
    neighbors
        .try_reserve_exact(surface.scratch_capacity())
        .map_err(|_| ApplicationError::AllocationFailed)?;
    let mut evidence = LocalApplicationEvidence::default();
    for local_y in 0..row_count {
        let source_y = start_y + local_y;
        for x in 0..source_dimensions.width() {
            let source_index = source_y * source_dimensions.width() + x;
            let output_index = local_y * source_dimensions.width() + x;
            let source_flags = source.mask().as_slice()[source_index];
            image.mask_mut().as_mut_slice()[output_index] = source_flags;
            let source_value = source.pixels()[source_index];
            image.pixels_mut()[output_index] = source_value;
            if !source_flags.is_clear() {
                evidence.inherited_masked += 1;
                continue;
            }
            if !source_value.is_finite() {
                image.mask_mut().as_mut_slice()[output_index] |= PixelFlags::INVALID;
                evidence.non_finite_input += 1;
                continue;
            }
            match surface
                .evaluate_with_scratch(x as f64, source_y as f64, &mut neighbors)
                .and_then(|evaluation| evaluation.apply(source_value))
            {
                Ok(value) => {
                    image.pixels_mut()[output_index] = value;
                    evidence.transformed += 1;
                }
                Err(SurfaceError::NonFiniteModel) => {
                    image.mask_mut().as_mut_slice()[output_index] |= PixelFlags::INVALID;
                    evidence.non_finite_result += 1;
                }
                Err(_) => {
                    image.mask_mut().as_mut_slice()[output_index] |= PixelFlags::MISSING;
                    evidence.unsupported_surface += 1;
                }
            }
        }
    }
    Ok(LocalApplication { image, evidence })
}

impl Error for ApplicationError {}

/// Applies one guarded coefficient surface per plane without per-pixel allocation.
///
/// Existing masks always win. Unsupported surface coordinates retain their
/// source value and gain `MISSING`; non-finite inputs or results retain their
/// source value and gain `INVALID`. Consumers therefore cannot mistake an
/// untransformed value for valid normalized science.
pub fn apply_local_surfaces(
    source: &ScientificImage,
    surfaces: &[LocalCoefficientSurface],
) -> Result<LocalApplication, ApplicationError> {
    let dimensions = source.dimensions();
    if surfaces.len() != dimensions.planes() {
        return Err(ApplicationError::SurfaceCountMismatch);
    }

    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(source.pixels().len())
        .map_err(|_| ApplicationError::AllocationFailed)?;
    pixels.extend_from_slice(source.pixels());
    let mut image = ScientificImage::from_pixels(dimensions, pixels)
        .map_err(|_| ApplicationError::ImageConstructionFailed)?;
    image
        .mask_mut()
        .as_mut_slice()
        .copy_from_slice(source.mask().as_slice());

    let plane_area = dimensions.width() * dimensions.height();
    let maximum_scratch = surfaces
        .iter()
        .map(LocalCoefficientSurface::scratch_capacity)
        .max()
        .unwrap_or(0);
    let mut neighbors = Vec::new();
    neighbors
        .try_reserve_exact(maximum_scratch)
        .map_err(|_| ApplicationError::AllocationFailed)?;
    let mut evidence = LocalApplicationEvidence::default();

    for (plane, surface) in surfaces.iter().enumerate() {
        let plane_offset = plane * plane_area;
        for y in 0..dimensions.height() {
            for x in 0..dimensions.width() {
                let index = plane_offset + y * dimensions.width() + x;
                if !source.mask().as_slice()[index].is_clear() {
                    evidence.inherited_masked += 1;
                    continue;
                }
                let source_value = source.pixels()[index];
                if !source_value.is_finite() {
                    image.mask_mut().as_mut_slice()[index] |= PixelFlags::INVALID;
                    evidence.non_finite_input += 1;
                    continue;
                }
                match surface
                    .evaluate_with_scratch(x as f64, y as f64, &mut neighbors)
                    .and_then(|evaluation| evaluation.apply(source_value))
                {
                    Ok(value) => {
                        image.pixels_mut()[index] = value;
                        evidence.transformed += 1;
                    }
                    Err(SurfaceError::NonFiniteModel) => {
                        image.mask_mut().as_mut_slice()[index] |= PixelFlags::INVALID;
                        evidence.non_finite_result += 1;
                    }
                    Err(_) => {
                        image.mask_mut().as_mut_slice()[index] |= PixelFlags::MISSING;
                        evidence.unsupported_surface += 1;
                    }
                }
            }
        }
    }
    Ok(LocalApplication { image, evidence })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        LocalFitParameters, SamplingGridParameters, SurfaceParameters, build_local_surface,
        fit_local_grid, sample_local_grid,
    };
    use aether_core::Dimensions;

    type TestResult = Result<(), Box<dyn Error>>;

    fn constant_surface(maximum_distance: f64) -> Result<LocalCoefficientSurface, Box<dyn Error>> {
        let dimensions = Dimensions::new(3, 3, 1)?;
        let source_values = (0..9).map(f64::from).collect::<Vec<_>>();
        let reference_values = source_values
            .iter()
            .map(|value| 2.0 * value + 1.0)
            .collect();
        let source = ScientificImage::from_pixels(dimensions, source_values)?;
        let reference = ScientificImage::from_pixels(dimensions, reference_values)?;
        let samples = sample_local_grid(
            &source,
            &reference,
            None,
            0,
            SamplingGridParameters::new(3, 3, 9, 1)?,
        )?;
        let fits = fit_local_grid(&samples, LocalFitParameters::new(9, 9, 36, 1.0e-12)?)?;
        Ok(build_local_surface(
            &fits,
            SurfaceParameters::new(1, 1, 1, maximum_distance)?,
        )?)
    }

    #[test]
    fn plane_evidence_combines_without_losing_categories() -> TestResult {
        let first = LocalApplicationEvidence {
            transformed: 5,
            inherited_masked: 1,
            non_finite_input: 2,
            unsupported_surface: 3,
            non_finite_result: 4,
        };
        let Some(combined) = first.checked_add(first) else {
            return Err("small evidence counts unexpectedly overflowed".into());
        };
        assert_eq!(combined.transformed(), 10);
        assert_eq!(combined.inherited_masked(), 2);
        assert_eq!(combined.non_finite_input(), 4);
        assert_eq!(combined.unsupported_surface(), 6);
        assert_eq!(combined.non_finite_result(), 8);
        assert_eq!(combined.classified_samples(), 30);
        assert!(
            LocalApplicationEvidence {
                transformed: usize::MAX,
                ..LocalApplicationEvidence::default()
            }
            .checked_add(LocalApplicationEvidence {
                transformed: 1,
                ..LocalApplicationEvidence::default()
            })
            .is_none()
        );
        Ok(())
    }

    #[test]
    fn applies_all_clear_finite_samples_and_preserves_invalid_evidence() -> TestResult {
        let dimensions = Dimensions::new(3, 3, 1)?;
        let mut source = ScientificImage::from_pixels(
            dimensions,
            vec![0.0, 1.0, 2.0, 3.0, f64::NAN, 5.0, 6.0, 7.0, 8.0],
        )?;
        source.mark(0, 0, 0, PixelFlags::HOT)?;
        let application = apply_local_surfaces(&source, &[constant_surface(10.0)?])?;
        let evidence = application.evidence();
        assert_eq!(evidence.transformed(), 7);
        assert_eq!(evidence.inherited_masked(), 1);
        assert_eq!(evidence.non_finite_input(), 1);
        assert_eq!(evidence.unsupported_surface(), 0);
        assert_eq!(evidence.non_finite_result(), 0);
        assert_eq!(evidence.classified_samples(), 9);
        assert_eq!(application.image().pixels()[0].to_bits(), 0.0_f64.to_bits());
        assert_eq!(application.image().pixels()[1].to_bits(), 3.0_f64.to_bits());
        assert!(application.image().mask().as_slice()[0].contains(PixelFlags::HOT));
        assert!(application.image().mask().as_slice()[4].contains(PixelFlags::INVALID));
        Ok(())
    }

    #[test]
    fn row_bands_are_bit_exact_with_complete_application() -> TestResult {
        let dimensions = Dimensions::new(3, 3, 1)?;
        let source = ScientificImage::from_pixels(dimensions, (0..9).map(f64::from).collect())?;
        let surface = constant_surface(10.0)?;
        let complete = apply_local_surfaces(&source, std::slice::from_ref(&surface))?;
        let first = apply_local_surface_band(&source, &surface, 0, 1)?;
        let second = apply_local_surface_band(&source, &surface, 1, 2)?;
        let pixels = first
            .image()
            .pixels()
            .iter()
            .chain(second.image().pixels())
            .copied()
            .collect::<Vec<_>>();
        let flags = first
            .image()
            .mask()
            .as_slice()
            .iter()
            .chain(second.image().mask().as_slice())
            .copied()
            .collect::<Vec<_>>();
        assert_eq!(pixels, complete.image().pixels());
        assert_eq!(flags, complete.image().mask().as_slice());
        assert_eq!(
            first.evidence().checked_add(second.evidence()),
            Some(complete.evidence())
        );
        assert!(matches!(
            apply_local_surface_band(&source, &surface, 3, 1),
            Err(ApplicationError::InvalidBand)
        ));
        Ok(())
    }

    #[test]
    fn unsupported_pixels_are_retained_but_marked_missing() -> TestResult {
        let dimensions = Dimensions::new(3, 3, 1)?;
        let source =
            ScientificImage::from_pixels(dimensions, (0..9).map(f64::from).collect::<Vec<_>>())?;
        let application = apply_local_surfaces(&source, &[constant_surface(0.1)?])?;
        assert_eq!(application.evidence().transformed(), 1);
        assert_eq!(application.evidence().unsupported_surface(), 8);
        assert_eq!(application.image().pixels()[0].to_bits(), 0.0_f64.to_bits());
        assert!(application.image().mask().as_slice()[0].contains(PixelFlags::MISSING));
        assert_eq!(application.image().pixels()[4].to_bits(), 9.0_f64.to_bits());
        assert!(application.image().mask().as_slice()[4].is_clear());
        assert_eq!(
            apply_local_surfaces(&source, &[]),
            Err(ApplicationError::SurfaceCountMismatch)
        );
        Ok(())
    }
}
