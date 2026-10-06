use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_calibration::{DefectCorrectionParameters, DefectDetectionParameters};
use aether_core::{Dimensions, PixelFlags};
use aether_fits::SampleStatus;
use sha2::{Digest, Sha256};

const PARAMETER_DOMAIN: &[u8] = b"aetherstack-defect-parameters-v1\0";
const MAX_NEIGHBOUR_SAMPLES: usize = 288;
const PUBLICATION_BUFFER_BYTES: usize = 2 * 64 * 1_024;

/// Calibration-master role supplying detector-defect evidence.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum DefectReferenceKind {
    /// Dark master, primarily carrying hot-pixel evidence.
    Dark = 0,
    /// Normalized flat master, primarily carrying cold-response evidence.
    Flat = 1,
}

/// Role-tagged local detection controls.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DefectReferenceParameters {
    kind: DefectReferenceKind,
    detection: DefectDetectionParameters,
}

impl DefectReferenceParameters {
    /// Associates validated detection controls with one master role.
    #[must_use]
    pub const fn new(kind: DefectReferenceKind, detection: DefectDetectionParameters) -> Self {
        Self { kind, detection }
    }

    /// Master role.
    #[must_use]
    pub const fn kind(self) -> DefectReferenceKind {
        self.kind
    }

    /// Local robust-detection controls.
    #[must_use]
    pub const fn detection(self) -> DefectDetectionParameters {
        self.detection
    }
}

/// Failure to build one canonical defect-parameter identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DefectParameterSealError {
    /// At least one master-derived detector reference is required.
    NoReferences,
    /// A master role appeared more than once.
    DuplicateReference {
        /// Duplicated role.
        kind: DefectReferenceKind,
    },
}

impl Display for DefectParameterSealError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoReferences => formatter.write_str("at least one defect reference is required"),
            Self::DuplicateReference { kind } => {
                write!(formatter, "duplicate {kind:?} defect reference")
            }
        }
    }
}

impl Error for DefectParameterSealError {}

/// Exact modeled heap-element peak for full-frame defect processing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DefectMemoryEstimate {
    pixel_count: usize,
    fits_decode_bytes: usize,
    detection_peak_bytes: usize,
    correction_peak_bytes: usize,
    publication_buffer_bytes: usize,
    reserved_peak_bytes: usize,
}

impl DefectMemoryEstimate {
    /// Samples in all congruent images and maps.
    #[must_use]
    pub const fn pixel_count(self) -> usize {
        self.pixel_count
    }
    /// One image, one decode-status vector, and one accumulated map.
    #[must_use]
    pub const fn fits_decode_bytes(self) -> usize {
        self.fits_decode_bytes
    }
    /// Reference, accumulated/new maps, and two robust-statistics scratch vectors.
    #[must_use]
    pub const fn detection_peak_bytes(self) -> usize {
        self.detection_peak_bytes
    }
    /// Immutable source, corrected clone, original/cloned map, and median scratch.
    #[must_use]
    pub const fn correction_peak_bytes(self) -> usize {
        self.correction_peak_bytes
    }
    /// Fixed buffering for two simultaneously staged FITS streams.
    #[must_use]
    pub const fn publication_buffer_bytes(self) -> usize {
        self.publication_buffer_bytes
    }
    /// Largest phase plus publication buffers, reserved before allocation.
    #[must_use]
    pub const fn reserved_peak_bytes(self) -> usize {
        self.reserved_peak_bytes
    }
}

/// Checked-arithmetic failure while planning detector correction memory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DefectMemoryEstimateError;

impl Display for DefectMemoryEstimateError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("detector defect memory estimate overflows usize")
    }
}

impl Error for DefectMemoryEstimateError {}

/// Calculates the complete full-frame reservation before pixel I/O.
pub fn estimate_defect_memory(
    dimensions: Dimensions,
) -> Result<DefectMemoryEstimate, DefectMemoryEstimateError> {
    let pixels = dimensions.pixel_count();
    let image_sample_bytes = std::mem::size_of::<f64>()
        .checked_add(std::mem::size_of::<PixelFlags>())
        .ok_or(DefectMemoryEstimateError)?;
    let image_bytes = pixels
        .checked_mul(image_sample_bytes)
        .ok_or(DefectMemoryEstimateError)?;
    let map_bytes = pixels
        .checked_mul(std::mem::size_of::<PixelFlags>())
        .ok_or(DefectMemoryEstimateError)?;
    let status_bytes = pixels
        .checked_mul(std::mem::size_of::<SampleStatus>())
        .ok_or(DefectMemoryEstimateError)?;
    let detection_scratch = MAX_NEIGHBOUR_SAMPLES
        .checked_mul(std::mem::size_of::<f64>())
        .and_then(|bytes| bytes.checked_mul(2))
        .ok_or(DefectMemoryEstimateError)?;
    let correction_scratch = MAX_NEIGHBOUR_SAMPLES
        .checked_mul(std::mem::size_of::<f64>())
        .ok_or(DefectMemoryEstimateError)?;
    let fits_decode_bytes = image_bytes
        .checked_add(status_bytes)
        .and_then(|bytes| bytes.checked_add(map_bytes))
        .ok_or(DefectMemoryEstimateError)?;
    let detection_peak_bytes = image_bytes
        .checked_add(map_bytes.checked_mul(2).ok_or(DefectMemoryEstimateError)?)
        .and_then(|bytes| bytes.checked_add(detection_scratch))
        .ok_or(DefectMemoryEstimateError)?;
    let correction_peak_bytes = image_bytes
        .checked_mul(2)
        .and_then(|bytes| bytes.checked_add(map_bytes.checked_mul(2)?))
        .and_then(|bytes| bytes.checked_add(correction_scratch))
        .ok_or(DefectMemoryEstimateError)?;
    let phase_peak = fits_decode_bytes
        .max(detection_peak_bytes)
        .max(correction_peak_bytes);
    let reserved_peak_bytes = phase_peak
        .checked_add(PUBLICATION_BUFFER_BYTES)
        .ok_or(DefectMemoryEstimateError)?;
    Ok(DefectMemoryEstimate {
        pixel_count: pixels,
        fits_decode_bytes,
        detection_peak_bytes,
        correction_peak_bytes,
        publication_buffer_bytes: PUBLICATION_BUFFER_BYTES,
        reserved_peak_bytes,
    })
}

/// Hashes the complete detection and correction policy in canonical role order.
///
/// The representation uses domain-separated fixed-width big-endian integers and
/// exact IEEE 754 payloads. Paths, UI labels, and caller iteration order cannot
/// influence the seal.
pub fn strict_defect_parameters_sha256(
    references: &[DefectReferenceParameters],
    correction: DefectCorrectionParameters,
) -> Result<String, DefectParameterSealError> {
    if references.is_empty() {
        return Err(DefectParameterSealError::NoReferences);
    }
    let mut dark = None;
    let mut flat = None;
    for reference in references.iter().copied() {
        let slot = match reference.kind {
            DefectReferenceKind::Dark => &mut dark,
            DefectReferenceKind::Flat => &mut flat,
        };
        if slot.replace(reference).is_some() {
            return Err(DefectParameterSealError::DuplicateReference {
                kind: reference.kind,
            });
        }
    }

    let mut hasher = Sha256::new();
    hasher.update(PARAMETER_DOMAIN);
    for reference in dark.into_iter().chain(flat) {
        hasher.update([reference.kind as u8]);
        hash_detection(&mut hasher, reference.detection);
    }
    hash_usize(&mut hasher, correction.radius());
    hash_usize(&mut hasher, correction.stride());
    hash_usize(&mut hasher, correction.minimum_neighbours());
    Ok(encode_lower_hex(&hasher.finalize()))
}

fn hash_detection(hasher: &mut Sha256, parameters: DefectDetectionParameters) {
    hash_usize(hasher, parameters.radius());
    hash_usize(hasher, parameters.stride());
    hash_usize(hasher, parameters.minimum_neighbours());
    hasher.update(parameters.hot_sigma().to_bits().to_be_bytes());
    hasher.update(parameters.cold_sigma().to_bits().to_be_bytes());
    hasher.update(
        parameters
            .minimum_absolute_deviation()
            .to_bits()
            .to_be_bytes(),
    );
}

fn hash_usize(hasher: &mut Sha256, value: usize) {
    hasher.update((value as u64).to_be_bytes());
}

fn encode_lower_hex(bytes: &[u8]) -> String {
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

    type TestResult<T = ()> = Result<T, Box<dyn Error>>;

    fn detection(
        hot: f64,
    ) -> Result<DefectDetectionParameters, aether_calibration::DefectMapError> {
        DefectDetectionParameters::new(2, 2, 8, hot, 5.0, 1.0)
    }

    fn correction() -> Result<DefectCorrectionParameters, aether_calibration::DefectMapError> {
        DefectCorrectionParameters::new(2, 2, 8)
    }

    #[test]
    fn seal_is_role_order_independent_and_parameter_sensitive() -> TestResult {
        let dark = DefectReferenceParameters::new(DefectReferenceKind::Dark, detection(8.0)?);
        let flat = DefectReferenceParameters::new(DefectReferenceKind::Flat, detection(6.0)?);
        let forward = strict_defect_parameters_sha256(&[dark, flat], correction()?)?;
        let reverse = strict_defect_parameters_sha256(&[flat, dark], correction()?)?;
        let changed = strict_defect_parameters_sha256(
            &[
                DefectReferenceParameters::new(DefectReferenceKind::Dark, detection(8.5)?),
                flat,
            ],
            correction()?,
        )?;
        assert_eq!(forward, reverse);
        assert_ne!(forward, changed);
        assert_eq!(forward.len(), 64);
        Ok(())
    }

    #[test]
    fn seal_rejects_empty_and_duplicate_roles() -> TestResult {
        assert_eq!(
            strict_defect_parameters_sha256(&[], correction()?),
            Err(DefectParameterSealError::NoReferences)
        );
        let dark = DefectReferenceParameters::new(DefectReferenceKind::Dark, detection(8.0)?);
        assert_eq!(
            strict_defect_parameters_sha256(&[dark, dark], correction()?),
            Err(DefectParameterSealError::DuplicateReference {
                kind: DefectReferenceKind::Dark
            })
        );
        Ok(())
    }

    #[test]
    fn priority_camera_memory_baselines_are_checked_and_bounded() -> TestResult {
        let asi294 = estimate_defect_memory(Dimensions::new(4_144, 2_822, 1)?)?;
        let touptek585 = estimate_defect_memory(Dimensions::new(3_840, 2_160, 1)?)?;
        assert_eq!(asi294.pixel_count(), 11_694_368);
        assert_eq!(asi294.reserved_peak_bytes(), 234_020_736);
        assert_eq!(touptek585.reserved_peak_bytes(), 166_021_376);
        assert!(asi294.reserved_peak_bytes() < 256 * 1_024 * 1_024);
        assert!(touptek585.reserved_peak_bytes() < 256 * 1_024 * 1_024);
        Ok(())
    }

    #[test]
    fn estimate_exposes_the_exact_largest_phase() -> TestResult {
        let estimate = estimate_defect_memory(Dimensions::new(10, 10, 1)?)?;
        assert!(estimate.detection_peak_bytes() > estimate.fits_decode_bytes());
        assert_eq!(
            estimate.publication_buffer_bytes(),
            PUBLICATION_BUFFER_BYTES
        );
        assert_eq!(
            estimate.reserved_peak_bytes(),
            estimate
                .detection_peak_bytes()
                .max(estimate.correction_peak_bytes())
                + estimate.publication_buffer_bytes()
        );
        Ok(())
    }
}
