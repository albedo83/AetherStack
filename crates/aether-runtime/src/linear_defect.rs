use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_calibration::{
    LinearDefectAxis, LinearDefectCorrectionParameters, LinearDefectDetectionParameters,
};
use aether_core::{Dimensions, PixelFlags};
use sha2::{Digest, Sha256};

const PARAMETER_DOMAIN: &[u8] = b"aetherstack-linear-defect-parameters-v1\0";
const PUBLICATION_BUFFER_BYTES: usize = 2 * 64 * 1_024;
const MAXIMUM_MEMORY_RADIUS: usize = 8;

/// Exact modeled heap peak for full-frame linear-defect processing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LinearDefectMemoryEstimate {
    pixel_count: usize,
    fits_decode_bytes: usize,
    detection_peak_bytes: usize,
    correction_peak_bytes: usize,
    publication_buffer_bytes: usize,
    reserved_peak_bytes: usize,
}

impl LinearDefectMemoryEstimate {
    /// Samples across every plane.
    #[must_use]
    pub const fn pixel_count(self) -> usize {
        self.pixel_count
    }
    /// One decoded binary64 image and its status mask.
    #[must_use]
    pub const fn fits_decode_bytes(self) -> usize {
        self.fits_decode_bytes
    }
    /// Reference, generated map, and two perpendicular scratch vectors.
    #[must_use]
    pub const fn detection_peak_bytes(self) -> usize {
        self.detection_peak_bytes
    }
    /// Source, output clone, immutable and retained maps, and repair scratch.
    #[must_use]
    pub const fn correction_peak_bytes(self) -> usize {
        self.correction_peak_bytes
    }
    /// Fixed buffering for two simultaneously staged FITS streams.
    #[must_use]
    pub const fn publication_buffer_bytes(self) -> usize {
        self.publication_buffer_bytes
    }
    /// Largest phase plus publication buffering, reserved before decoding.
    #[must_use]
    pub const fn reserved_peak_bytes(self) -> usize {
        self.reserved_peak_bytes
    }
}

/// Checked-arithmetic failure while planning linear-defect memory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LinearDefectMemoryEstimateError;

impl Display for LinearDefectMemoryEstimateError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("linear-defect memory estimate overflows usize")
    }
}

impl Error for LinearDefectMemoryEstimateError {}

/// Computes the complete strict CPU peak before any FITS pixels are opened.
pub fn estimate_linear_defect_memory(
    dimensions: Dimensions,
    maximum_perpendicular_radius: usize,
) -> Result<LinearDefectMemoryEstimate, LinearDefectMemoryEstimateError> {
    if maximum_perpendicular_radius == 0 || maximum_perpendicular_radius > MAXIMUM_MEMORY_RADIUS {
        return Err(LinearDefectMemoryEstimateError);
    }
    let pixel_count = dimensions.pixel_count();
    let image_bytes = checked_mul(pixel_count, size_of::<f64>())?;
    let mask_bytes = checked_mul(pixel_count, size_of::<PixelFlags>())?;
    let decoded_image_bytes = checked_add(image_bytes, mask_bytes)?;
    let map_bytes = mask_bytes;
    let support_samples = checked_mul(maximum_perpendicular_radius, 2)?;
    let support_bytes = checked_mul(support_samples, size_of::<f64>())?;
    let detection_scratch = checked_mul(support_bytes, 2)?;
    let detection_peak_bytes = checked_add(
        checked_add(decoded_image_bytes, map_bytes)?,
        detection_scratch,
    )?;
    let correction_peak_bytes = checked_add(
        checked_add(
            checked_mul(decoded_image_bytes, 2)?,
            checked_mul(map_bytes, 2)?,
        )?,
        support_bytes,
    )?;
    let phase_peak = detection_peak_bytes.max(correction_peak_bytes);
    let reserved_peak_bytes = checked_add(phase_peak, PUBLICATION_BUFFER_BYTES)?;
    Ok(LinearDefectMemoryEstimate {
        pixel_count,
        fits_decode_bytes: decoded_image_bytes,
        detection_peak_bytes,
        correction_peak_bytes,
        publication_buffer_bytes: PUBLICATION_BUFFER_BYTES,
        reserved_peak_bytes,
    })
}

fn checked_mul(left: usize, right: usize) -> Result<usize, LinearDefectMemoryEstimateError> {
    left.checked_mul(right)
        .ok_or(LinearDefectMemoryEstimateError)
}

fn checked_add(left: usize, right: usize) -> Result<usize, LinearDefectMemoryEstimateError> {
    left.checked_add(right)
        .ok_or(LinearDefectMemoryEstimateError)
}

/// Failure to construct one canonical linear-defect parameter identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinearDefectParameterSealError {
    /// Detection and correction target different line orientations.
    AxisMismatch,
    /// Detection and correction use different detector lattices.
    StrideMismatch,
}

impl Display for LinearDefectParameterSealError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AxisMismatch => {
                formatter.write_str("linear-defect detection and correction axes differ")
            }
            Self::StrideMismatch => {
                formatter.write_str("linear-defect detection and correction strides differ")
            }
        }
    }
}

impl Error for LinearDefectParameterSealError {}

/// Returns a path-free identity for every line-detection and repair control.
pub fn strict_linear_defect_parameters_sha256(
    detection: LinearDefectDetectionParameters,
    correction: LinearDefectCorrectionParameters,
) -> Result<String, LinearDefectParameterSealError> {
    if detection.axis() != correction.axis() {
        return Err(LinearDefectParameterSealError::AxisMismatch);
    }
    if detection.stride() != correction.stride() {
        return Err(LinearDefectParameterSealError::StrideMismatch);
    }
    let mut digest = Sha256::new();
    digest.update(PARAMETER_DOMAIN);
    digest.update([axis_tag(detection.axis())]);
    update_usize(&mut digest, detection.perpendicular_radius());
    update_usize(&mut digest, detection.stride());
    update_usize(&mut digest, detection.minimum_perpendicular_neighbours());
    update_usize(&mut digest, detection.minimum_affected_samples());
    digest.update(detection.minimum_affected_fraction_ppm().to_be_bytes());
    digest.update(detection.hot_sigma().to_bits().to_be_bytes());
    digest.update(detection.cold_sigma().to_bits().to_be_bytes());
    digest.update(
        detection
            .minimum_absolute_deviation()
            .to_bits()
            .to_be_bytes(),
    );
    update_usize(&mut digest, correction.perpendicular_radius());
    update_usize(&mut digest, correction.minimum_perpendicular_neighbours());
    Ok(lowercase_hex(&digest.finalize()))
}

const fn axis_tag(axis: LinearDefectAxis) -> u8 {
    match axis {
        LinearDefectAxis::Rows => 0,
        LinearDefectAxis::Columns => 1,
    }
}

fn update_usize(digest: &mut Sha256, value: usize) {
    digest.update((value as u128).to_be_bytes());
}

fn lowercase_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    type Policies = (
        LinearDefectDetectionParameters,
        LinearDefectCorrectionParameters,
    );

    fn policies(axis: LinearDefectAxis, stride: usize) -> Result<Policies, Box<dyn Error>> {
        Ok((
            LinearDefectDetectionParameters::new(axis, 2, stride, 3, 16, 500_000, 5.0, 4.0, -0.0)?,
            LinearDefectCorrectionParameters::new(axis, 3, stride, 4)?,
        ))
    }

    #[test]
    fn seal_is_stable_path_free_and_binds_every_policy_family() -> Result<(), Box<dyn Error>> {
        let (detection, correction) = policies(LinearDefectAxis::Rows, 2)?;
        let first = strict_linear_defect_parameters_sha256(detection, correction)?;
        let second = strict_linear_defect_parameters_sha256(detection, correction)?;
        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
        assert!(
            first
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        );

        let (columns, column_correction) = policies(LinearDefectAxis::Columns, 2)?;
        assert_ne!(
            first,
            strict_linear_defect_parameters_sha256(columns, column_correction)?
        );
        let changed = LinearDefectCorrectionParameters::new(LinearDefectAxis::Rows, 4, 2, 4)?;
        assert_ne!(
            first,
            strict_linear_defect_parameters_sha256(detection, changed)?
        );
        Ok(())
    }

    #[test]
    fn seal_rejects_mixed_axis_and_lattice_policies() -> Result<(), Box<dyn Error>> {
        let (detection, correction) = policies(LinearDefectAxis::Rows, 1)?;
        let columns = LinearDefectCorrectionParameters::new(LinearDefectAxis::Columns, 3, 1, 4)?;
        assert_eq!(
            strict_linear_defect_parameters_sha256(detection, columns),
            Err(LinearDefectParameterSealError::AxisMismatch)
        );
        let stride_two = LinearDefectCorrectionParameters::new(LinearDefectAxis::Rows, 3, 2, 4)?;
        assert_eq!(
            strict_linear_defect_parameters_sha256(detection, stride_two),
            Err(LinearDefectParameterSealError::StrideMismatch)
        );
        assert!(strict_linear_defect_parameters_sha256(detection, correction).is_ok());
        Ok(())
    }

    #[test]
    fn memory_peak_is_exact_for_representative_astro_cameras() -> Result<(), Box<dyn Error>> {
        let asi294 = estimate_linear_defect_memory(Dimensions::new(4_144, 2_822, 1)?, 8)?;
        assert_eq!(asi294.pixel_count(), 11_694_368);
        assert_eq!(asi294.fits_decode_bytes(), 105_249_312);
        assert_eq!(asi294.detection_peak_bytes(), 116_943_936);
        assert_eq!(asi294.correction_peak_bytes(), 233_887_488);
        assert_eq!(asi294.publication_buffer_bytes(), 131_072);
        assert_eq!(asi294.reserved_peak_bytes(), 234_018_560);

        let touptek585 = estimate_linear_defect_memory(Dimensions::new(3_840, 2_160, 1)?, 8)?;
        assert_eq!(touptek585.pixel_count(), 8_294_400);
        assert_eq!(touptek585.reserved_peak_bytes(), 166_019_200);
        Ok(())
    }

    #[test]
    fn memory_estimate_rejects_radius_and_arithmetic_overflow() -> Result<(), Box<dyn Error>> {
        let dimensions = Dimensions::new(16, 16, 1)?;
        assert_eq!(
            estimate_linear_defect_memory(dimensions, 0),
            Err(LinearDefectMemoryEstimateError)
        );
        let enormous = Dimensions::new(usize::MAX / 2, 1, 1)?;
        assert_eq!(
            estimate_linear_defect_memory(enormous, 8),
            Err(LinearDefectMemoryEstimateError)
        );
        Ok(())
    }
}
