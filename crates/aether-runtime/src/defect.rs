use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_calibration::{
    DefectCorrectionParameters, DefectDetectionEvidence, DefectDetectionParameters, DefectMap,
    DefectMapError, detect_local_defects, merge_defect_maps,
};
use aether_core::{Dimensions, PixelFlags};
use aether_fits::{HeaderReadOptions, ImageReadError, ImageRegion, SampleStatus, ValidationMode};
use sha2::{Digest, Sha256};

use crate::{
    CancellationToken, Cancelled, MemoryBudget, MemoryBudgetError, PipelineInput, PipelineSource,
    StrictPipelineError,
};

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

/// One immutable master FITS source and its role-specific detection policy.
#[derive(Clone, Debug)]
pub struct DefectFitsReference {
    source: PipelineSource,
    parameters: DefectReferenceParameters,
}

impl DefectFitsReference {
    /// Binds one manifest fingerprint to one master role and policy.
    #[must_use]
    pub fn new(source: PipelineSource, parameters: DefectReferenceParameters) -> Self {
        Self { source, parameters }
    }

    /// Immutable local source.
    #[must_use]
    pub const fn source(&self) -> &PipelineSource {
        &self.source
    }

    /// Master role and detection parameters.
    #[must_use]
    pub const fn parameters(&self) -> DefectReferenceParameters {
        self.parameters
    }
}

/// Complete in-memory result of verified master defect analysis.
#[derive(Clone, Debug)]
pub struct DefectAnalysisResult {
    map: DefectMap,
    evidence: Vec<(DefectReferenceKind, DefectDetectionEvidence)>,
    parameters_sha256: String,
    reserved_bytes: usize,
}

impl DefectAnalysisResult {
    /// Union of all master-derived HOT/COLD evidence.
    #[must_use]
    pub const fn map(&self) -> &DefectMap {
        &self.map
    }

    /// Per-master evidence in caller order.
    #[must_use]
    pub fn evidence(&self) -> &[(DefectReferenceKind, DefectDetectionEvidence)] {
        &self.evidence
    }

    /// Canonical path-free scientific parameter identity.
    #[must_use]
    pub fn parameters_sha256(&self) -> &str {
        &self.parameters_sha256
    }

    /// Bytes reserved before opening any image pixels.
    #[must_use]
    pub const fn reserved_bytes(&self) -> usize {
        self.reserved_bytes
    }
}

/// Failure while deriving one map from immutable FITS masters.
#[derive(Debug)]
pub enum DefectAnalysisError {
    /// Reference parameter sealing failed.
    Parameters(DefectParameterSealError),
    /// Cancellation was observed at a deterministic boundary.
    Cancelled(Cancelled),
    /// Header, fingerprint, or input opening failed.
    Input(StrictPipelineError),
    /// Embedded FITS checksums were absent or invalid.
    ChecksumNotVerified {
        /// Stable reference index.
        index: usize,
    },
    /// FITS pixel decoding failed.
    Read {
        /// Stable reference index.
        index: usize,
        /// Decoder failure.
        source: ImageReadError,
    },
    /// Reference dimensions differ.
    DimensionMismatch {
        /// Stable reference index.
        index: usize,
        /// First-reference dimensions.
        expected: Dimensions,
        /// Mismatched dimensions.
        actual: Dimensions,
    },
    /// Only one-plane calibration masters can define detector defects.
    UnsupportedPlaneCount {
        /// Stable reference index.
        index: usize,
        /// Received plane count.
        planes: usize,
    },
    /// Defect detection or map merging failed.
    Defect(DefectMapError),
    /// The exact memory estimate overflowed.
    MemoryEstimate(DefectMemoryEstimateError),
    /// The shared runtime budget rejected the complete peak.
    Memory(MemoryBudgetError),
    /// Evidence storage could not be allocated.
    AllocationFailed,
}

impl Display for DefectAnalysisError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parameters(error) => Display::fmt(error, formatter),
            Self::Cancelled(error) => Display::fmt(error, formatter),
            Self::Input(error) => Display::fmt(error, formatter),
            Self::ChecksumNotVerified { index } => write!(
                formatter,
                "defect reference {index} does not have fully verified FITS checksums"
            ),
            Self::Read { index, source } => {
                write!(formatter, "cannot read defect reference {index}: {source}")
            }
            Self::DimensionMismatch {
                index,
                expected,
                actual,
            } => write!(
                formatter,
                "defect reference {index} dimensions {}x{}x{} do not match {}x{}x{}",
                actual.width(),
                actual.height(),
                actual.planes(),
                expected.width(),
                expected.height(),
                expected.planes()
            ),
            Self::UnsupportedPlaneCount { index, planes } => write!(
                formatter,
                "defect reference {index} has {planes} planes; exactly one is required"
            ),
            Self::Defect(error) => Display::fmt(error, formatter),
            Self::MemoryEstimate(error) => Display::fmt(error, formatter),
            Self::Memory(error) => Display::fmt(error, formatter),
            Self::AllocationFailed => {
                formatter.write_str("cannot allocate defect-analysis evidence")
            }
        }
    }
}

impl Error for DefectAnalysisError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Parameters(error) => Some(error),
            Self::Cancelled(error) => Some(error),
            Self::Input(error) => Some(error),
            Self::Read { source, .. } => Some(source),
            Self::Defect(error) => Some(error),
            Self::MemoryEstimate(error) => Some(error),
            Self::Memory(error) => Some(error),
            Self::ChecksumNotVerified { .. }
            | Self::DimensionMismatch { .. }
            | Self::UnsupportedPlaneCount { .. }
            | Self::AllocationFailed => None,
        }
    }
}

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
        .checked_add(map_bytes.checked_mul(3).ok_or(DefectMemoryEstimateError)?)
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

/// Reads, verifies, detects, and merges one or two master-derived defect maps.
pub fn analyze_defect_references(
    references: &[DefectFitsReference],
    correction: DefectCorrectionParameters,
    cancellation: &CancellationToken,
    memory: &MemoryBudget,
) -> Result<DefectAnalysisResult, DefectAnalysisError> {
    let mut parameter_refs = Vec::new();
    parameter_refs
        .try_reserve_exact(references.len())
        .map_err(|_| DefectAnalysisError::AllocationFailed)?;
    parameter_refs.extend(references.iter().map(DefectFitsReference::parameters));
    let parameters_sha256 = strict_defect_parameters_sha256(&parameter_refs, correction)
        .map_err(DefectAnalysisError::Parameters)?;
    cancellation
        .checkpoint()
        .map_err(DefectAnalysisError::Cancelled)?;
    for (index, reference) in references.iter().enumerate() {
        crate::pipeline::verify_source(
            reference.source(),
            reference_input(reference.parameters().kind(), index),
        )
        .map_err(DefectAnalysisError::Input)?;
    }

    let first = references.first().ok_or(DefectAnalysisError::Parameters(
        DefectParameterSealError::NoReferences,
    ))?;
    let first_input = reference_input(first.parameters().kind(), 0);
    let first_reader = crate::pipeline::open_reader(
        first.source().path(),
        first_input,
        HeaderReadOptions::default(),
        ValidationMode::Strict,
    )
    .map_err(DefectAnalysisError::Input)?;
    let dimensions =
        crate::pipeline::dimensions_from_axes(first_input, first_reader.descriptor().axes())
            .map_err(DefectAnalysisError::Input)?;
    validate_single_plane(dimensions, 0)?;
    drop(first_reader);
    let estimate =
        estimate_defect_memory(dimensions).map_err(DefectAnalysisError::MemoryEstimate)?;
    let _reservation = memory
        .try_reserve(estimate.reserved_peak_bytes())
        .map_err(DefectAnalysisError::Memory)?;
    let mut merged: Option<DefectMap> = None;
    let mut evidence = Vec::new();
    evidence
        .try_reserve_exact(references.len())
        .map_err(|_| DefectAnalysisError::AllocationFailed)?;

    for (index, reference) in references.iter().enumerate() {
        cancellation
            .checkpoint()
            .map_err(DefectAnalysisError::Cancelled)?;
        let input = reference_input(reference.parameters().kind(), index);
        let mut reader = crate::pipeline::open_reader(
            reference.source().path(),
            input,
            HeaderReadOptions::default(),
            ValidationMode::Strict,
        )
        .map_err(DefectAnalysisError::Input)?;
        let actual = crate::pipeline::dimensions_from_axes(input, reader.descriptor().axes())
            .map_err(DefectAnalysisError::Input)?;
        validate_single_plane(actual, index)?;
        if actual != dimensions {
            return Err(DefectAnalysisError::DimensionMismatch {
                index,
                expected: dimensions,
                actual,
            });
        }
        if !reader
            .verify_checksums()
            .map_err(|source| DefectAnalysisError::Read { index, source })?
            .is_fully_verified()
        {
            return Err(DefectAnalysisError::ChecksumNotVerified { index });
        }
        let width = u64::try_from(dimensions.width())
            .map_err(|_| DefectAnalysisError::MemoryEstimate(DefectMemoryEstimateError))?;
        let height = u64::try_from(dimensions.height())
            .map_err(|_| DefectAnalysisError::MemoryEstimate(DefectMemoryEstimateError))?;
        let image = reader
            .read_region_image(ImageRegion::new(0, 0, 0, width, height))
            .map_err(|source| DefectAnalysisError::Read { index, source })?;
        let (detected, detected_evidence) =
            detect_local_defects(&image, reference.parameters().detection())
                .map_err(DefectAnalysisError::Defect)?;
        merged = Some(match merged.take() {
            Some(existing) => {
                merge_defect_maps(&[&existing, &detected])
                    .map_err(DefectAnalysisError::Defect)?
                    .0
            }
            None => detected,
        });
        evidence.push((reference.parameters().kind(), detected_evidence));
    }

    cancellation
        .checkpoint()
        .map_err(DefectAnalysisError::Cancelled)?;
    for (index, reference) in references.iter().enumerate() {
        crate::pipeline::verify_source(
            reference.source(),
            reference_input(reference.parameters().kind(), index),
        )
        .map_err(DefectAnalysisError::Input)?;
    }
    let map = merged.ok_or(DefectAnalysisError::Parameters(
        DefectParameterSealError::NoReferences,
    ))?;
    Ok(DefectAnalysisResult {
        map,
        evidence,
        parameters_sha256,
        reserved_bytes: estimate.reserved_peak_bytes(),
    })
}

fn validate_single_plane(dimensions: Dimensions, index: usize) -> Result<(), DefectAnalysisError> {
    if dimensions.planes() != 1 {
        return Err(DefectAnalysisError::UnsupportedPlaneCount {
            index,
            planes: dimensions.planes(),
        });
    }
    Ok(())
}

fn reference_input(kind: DefectReferenceKind, index: usize) -> PipelineInput {
    match kind {
        DefectReferenceKind::Dark => PipelineInput::Dark,
        DefectReferenceKind::Flat => PipelineInput::MasterSource { index },
    }
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
    use std::fs::{self, File};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use aether_core::ScientificImage;
    use aether_fits::write_f64_primary_atomic_new;
    use aether_session::fingerprint_reader;

    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn Error>>;
    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> std::io::Result<Self> {
            let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "aether-defect-runtime-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path)?;
            Ok(Self(path))
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ignored = fs::remove_dir_all(&self.0);
        }
    }

    fn detection(
        hot: f64,
    ) -> Result<DefectDetectionParameters, aether_calibration::DefectMapError> {
        DefectDetectionParameters::new(2, 2, 8, hot, 5.0, 1.0)
    }

    fn correction() -> Result<DefectCorrectionParameters, aether_calibration::DefectMapError> {
        DefectCorrectionParameters::new(2, 2, 8)
    }

    fn fits_source(
        directory: &TestDirectory,
        name: &str,
        image: &ScientificImage,
    ) -> Result<PipelineSource, Box<dyn Error>> {
        let path = directory.0.join(name);
        write_f64_primary_atomic_new(&path, image)?;
        let fingerprint = fingerprint_reader(&mut File::open(&path)?)?;
        Ok(PipelineSource::new(path, fingerprint))
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

    #[test]
    fn verified_fits_references_produce_one_merged_map() -> TestResult {
        let directory = TestDirectory::new()?;
        let dimensions = Dimensions::new(9, 9, 1)?;
        let mut dark = ScientificImage::filled(dimensions, 100.0)?;
        dark.pixels_mut()[4 * 9 + 4] = 1_000.0;
        let mut flat = ScientificImage::filled(dimensions, 1_000.0)?;
        flat.pixels_mut()[6 * 9 + 6] = 100.0;
        let detection = DefectDetectionParameters::new(2, 1, 8, 5.0, 5.0, 10.0)?;
        let references = [
            DefectFitsReference::new(
                fits_source(&directory, "dark.fits", &dark)?,
                DefectReferenceParameters::new(DefectReferenceKind::Dark, detection),
            ),
            DefectFitsReference::new(
                fits_source(&directory, "flat.fits", &flat)?,
                DefectReferenceParameters::new(DefectReferenceKind::Flat, detection),
            ),
        ];
        let estimate = estimate_defect_memory(dimensions)?;
        let budget = MemoryBudget::new(estimate.reserved_peak_bytes())?;
        let result = analyze_defect_references(
            &references,
            DefectCorrectionParameters::new(2, 1, 8)?,
            &CancellationToken::new(),
            &budget,
        )?;
        assert!(result.map().mask().get(4, 4, 0)?.contains(PixelFlags::HOT));
        assert!(result.map().mask().get(6, 6, 0)?.contains(PixelFlags::COLD));
        assert_eq!(result.evidence().len(), 2);
        assert_eq!(result.parameters_sha256().len(), 64);
        assert_eq!(result.reserved_bytes(), estimate.reserved_peak_bytes());
        assert_eq!(budget.used(), 0);
        Ok(())
    }
}
