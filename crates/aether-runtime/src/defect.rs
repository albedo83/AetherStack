use std::error::Error;
use std::fmt::{Display, Formatter};
use std::path::PathBuf;

use aether_calibration::{
    DefectCorrectionEvidence, DefectCorrectionParameters, DefectDetectionEvidence,
    DefectDetectionParameters, DefectMap, DefectMapError, correct_defects, detect_local_defects,
    merge_defect_maps,
};
use aether_core::{Dimensions, PixelFlags};
use aether_fits::{
    AtomicF64PrimaryStreamWriter, AtomicFitsSetWriteError, AtomicFitsWriteError,
    FitsOutputProvenance, FitsWriteSummary, HeaderReadOptions, ImageReadError, ImageRegion,
    PrimaryImageReader, SampleStatus, ValidationMode, publish_atomic_fits_set,
};
use sha2::{Digest, Sha256};

use crate::{
    CancellationToken, Cancelled, MemoryBudget, MemoryBudgetError, PipelineInput, PipelineSource,
    StrictPipelineError,
};

const PARAMETER_DOMAIN: &[u8] = b"aetherstack-defect-parameters-v1\0";
const MAX_NEIGHBOUR_SAMPLES: usize = 288;
const PUBLICATION_BUFFER_BYTES: usize = 2 * 64 * 1_024;

/// Algorithm identity for corrected calibrated-Light pixels.
pub const STRICT_DEFECT_CORRECTED_ALGORITHM_ID: &str = "strict-defect-corrected-v1";
/// Algorithm identity for the exact HOT/COLD companion map.
pub const STRICT_DEFECT_MAP_ALGORITHM_ID: &str = "strict-defect-map-v1";

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

/// Immutable request for one corrected Light and its exact defect companion.
#[derive(Clone, Debug)]
pub struct StrictDefectCorrectionRequest {
    light: PipelineSource,
    references: Vec<DefectFitsReference>,
    corrected_output: PathBuf,
    map_output: PathBuf,
    corrected_provenance: FitsOutputProvenance,
    map_provenance: FitsOutputProvenance,
    correction: DefectCorrectionParameters,
}

impl StrictDefectCorrectionRequest {
    /// Validates product roles, shared provenance, paths, and parameter seal.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        light: PipelineSource,
        references: Vec<DefectFitsReference>,
        corrected_output: PathBuf,
        map_output: PathBuf,
        corrected_provenance: FitsOutputProvenance,
        map_provenance: FitsOutputProvenance,
        correction: DefectCorrectionParameters,
    ) -> Result<Self, DefectCorrectionPipelineError> {
        if corrected_output == map_output {
            return Err(DefectCorrectionPipelineError::DuplicateOutput);
        }
        let mut parameter_refs = Vec::new();
        parameter_refs
            .try_reserve_exact(references.len())
            .map_err(|_| DefectCorrectionPipelineError::AllocationFailed)?;
        parameter_refs.extend(references.iter().map(DefectFitsReference::parameters));
        let seal = strict_defect_parameters_sha256(&parameter_refs, correction)
            .map_err(DefectCorrectionPipelineError::Parameters)?;
        let expected_sources = u32::try_from(references.len().saturating_add(1))
            .map_err(|_| DefectCorrectionPipelineError::TooManySources)?;
        validate_product_provenance(
            &corrected_provenance,
            STRICT_DEFECT_CORRECTED_ALGORITHM_ID,
            expected_sources,
            &seal,
        )?;
        validate_product_provenance(
            &map_provenance,
            STRICT_DEFECT_MAP_ALGORITHM_ID,
            expected_sources,
            &seal,
        )?;
        if corrected_provenance.manifest_sha256() != map_provenance.manifest_sha256()
            || corrected_provenance.plan_sha256() != map_provenance.plan_sha256()
            || corrected_provenance.group_id() != map_provenance.group_id()
        {
            return Err(DefectCorrectionPipelineError::ProvenanceMismatch);
        }
        Ok(Self {
            light,
            references,
            corrected_output,
            map_output,
            corrected_provenance,
            map_provenance,
            correction,
        })
    }
}

/// Published corrected Light and exact companion-map evidence.
#[derive(Clone, Debug)]
pub struct DefectCorrectionPipelineResult {
    corrected: FitsWriteSummary,
    map: FitsWriteSummary,
    correction: DefectCorrectionEvidence,
    parameters_sha256: String,
    reserved_bytes: usize,
}

impl DefectCorrectionPipelineResult {
    /// Corrected calibrated-Light FITS summary.
    #[must_use]
    pub const fn corrected(&self) -> FitsWriteSummary {
        self.corrected
    }
    /// HOT/COLD companion FITS summary.
    #[must_use]
    pub const fn map(&self) -> FitsWriteSummary {
        self.map
    }
    /// Complete replacement accounting.
    #[must_use]
    pub const fn correction(&self) -> DefectCorrectionEvidence {
        self.correction
    }
    /// Canonical parameter identity embedded in both products.
    #[must_use]
    pub fn parameters_sha256(&self) -> &str {
        &self.parameters_sha256
    }
    /// Peak bytes reserved for each full-frame phase.
    #[must_use]
    pub const fn reserved_bytes(&self) -> usize {
        self.reserved_bytes
    }
}

/// Failure of the all-or-nothing defect correction transaction.
#[derive(Debug)]
pub enum DefectCorrectionPipelineError {
    /// Parameter sealing failed.
    Parameters(DefectParameterSealError),
    /// Output destinations are identical.
    DuplicateOutput,
    /// Too many sources for FITS provenance.
    TooManySources,
    /// A product has the wrong algorithm, count, or parameter identity.
    InvalidProductProvenance,
    /// Companion products do not share the same manifest, plan, and group.
    ProvenanceMismatch,
    /// Master analysis failed.
    Analysis(DefectAnalysisError),
    /// Light fingerprint, header, or opening failed.
    Input(StrictPipelineError),
    /// Light FITS checksums were absent or invalid.
    ChecksumNotVerified,
    /// Light dimensions do not match the defect map.
    DimensionMismatch {
        /// Calibrated-Light dimensions.
        light: Dimensions,
        /// Derived map dimensions.
        map: Dimensions,
    },
    /// Light decoding failed.
    Read(ImageReadError),
    /// Defect replacement or transport failed.
    Defect(DefectMapError),
    /// Cancellation was observed before publication.
    Cancelled(Cancelled),
    /// Memory reservation failed.
    Memory(MemoryBudgetError),
    /// Private output staging failed.
    Stage(AtomicFitsWriteError),
    /// Private product readback failed.
    Readback(String),
    /// Atomic product-set publication failed.
    Publish(AtomicFitsSetWriteError),
    /// Small transaction bookkeeping allocation failed.
    AllocationFailed,
}

impl Display for DefectCorrectionPipelineError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parameters(error) => Display::fmt(error, formatter),
            Self::DuplicateOutput => {
                formatter.write_str("corrected image and defect map require distinct outputs")
            }
            Self::TooManySources => {
                formatter.write_str("defect correction source count exceeds u32")
            }
            Self::InvalidProductProvenance => {
                formatter.write_str("defect product provenance does not match the sealed request")
            }
            Self::ProvenanceMismatch => formatter
                .write_str("defect companion provenance does not identify one common product set"),
            Self::Analysis(error) => Display::fmt(error, formatter),
            Self::Input(error) => Display::fmt(error, formatter),
            Self::ChecksumNotVerified => {
                formatter.write_str("calibrated Light does not have fully verified FITS checksums")
            }
            Self::DimensionMismatch { light, map } => write!(
                formatter,
                "Light dimensions {}x{}x{} do not match defect map {}x{}x{}",
                light.width(),
                light.height(),
                light.planes(),
                map.width(),
                map.height(),
                map.planes()
            ),
            Self::Read(error) => Display::fmt(error, formatter),
            Self::Defect(error) => Display::fmt(error, formatter),
            Self::Cancelled(error) => Display::fmt(error, formatter),
            Self::Memory(error) => Display::fmt(error, formatter),
            Self::Stage(error) => Display::fmt(error, formatter),
            Self::Readback(message) => {
                write!(formatter, "defect product readback failed: {message}")
            }
            Self::Publish(error) => Display::fmt(error, formatter),
            Self::AllocationFailed => {
                formatter.write_str("cannot allocate defect publication bookkeeping")
            }
        }
    }
}

impl Error for DefectCorrectionPipelineError {}

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

/// Corrects one calibrated Light and atomically publishes it with its map.
pub fn run_strict_defect_correction(
    request: &StrictDefectCorrectionRequest,
    cancellation: &CancellationToken,
    memory: &MemoryBudget,
) -> Result<DefectCorrectionPipelineResult, DefectCorrectionPipelineError> {
    let analysis = analyze_defect_references(
        &request.references,
        request.correction,
        cancellation,
        memory,
    )
    .map_err(DefectCorrectionPipelineError::Analysis)?;
    cancellation
        .checkpoint()
        .map_err(DefectCorrectionPipelineError::Cancelled)?;
    crate::pipeline::verify_source(&request.light, PipelineInput::Signal { index: 0 })
        .map_err(DefectCorrectionPipelineError::Input)?;
    let mut reader = crate::pipeline::open_reader(
        request.light.path(),
        PipelineInput::Signal { index: 0 },
        HeaderReadOptions::default(),
        ValidationMode::Strict,
    )
    .map_err(DefectCorrectionPipelineError::Input)?;
    let light_dimensions = crate::pipeline::dimensions_from_axes(
        PipelineInput::Signal { index: 0 },
        reader.descriptor().axes(),
    )
    .map_err(DefectCorrectionPipelineError::Input)?;
    if light_dimensions != analysis.map().dimensions() {
        return Err(DefectCorrectionPipelineError::DimensionMismatch {
            light: light_dimensions,
            map: analysis.map().dimensions(),
        });
    }
    if !reader
        .verify_checksums()
        .map_err(DefectCorrectionPipelineError::Read)?
        .is_fully_verified()
    {
        return Err(DefectCorrectionPipelineError::ChecksumNotVerified);
    }
    let estimate = estimate_defect_memory(light_dimensions).map_err(|error| {
        DefectCorrectionPipelineError::Analysis(DefectAnalysisError::MemoryEstimate(error))
    })?;
    let _reservation = memory
        .try_reserve(estimate.reserved_peak_bytes())
        .map_err(DefectCorrectionPipelineError::Memory)?;
    let width = u64::try_from(light_dimensions.width()).map_err(|_| {
        DefectCorrectionPipelineError::Analysis(DefectAnalysisError::MemoryEstimate(
            DefectMemoryEstimateError,
        ))
    })?;
    let height = u64::try_from(light_dimensions.height()).map_err(|_| {
        DefectCorrectionPipelineError::Analysis(DefectAnalysisError::MemoryEstimate(
            DefectMemoryEstimateError,
        ))
    })?;
    let light = reader
        .read_region_image(ImageRegion::new(0, 0, 0, width, height))
        .map_err(DefectCorrectionPipelineError::Read)?;
    let corrected = correct_defects(&light, analysis.map(), request.correction)
        .map_err(DefectCorrectionPipelineError::Defect)?;
    drop(light);
    cancellation
        .checkpoint()
        .map_err(DefectCorrectionPipelineError::Cancelled)?;
    verify_all_sources(request).map_err(DefectCorrectionPipelineError::Input)?;

    let map_image = corrected
        .defect_map()
        .to_transport_image()
        .map_err(DefectCorrectionPipelineError::Defect)?;
    let mut corrected_writer = AtomicF64PrimaryStreamWriter::create_with_provenance(
        &request.corrected_output,
        light_dimensions,
        &request.corrected_provenance,
    )
    .map_err(DefectCorrectionPipelineError::Stage)?;
    corrected_writer
        .write_image_chunk(corrected.image())
        .map_err(DefectCorrectionPipelineError::Stage)?;
    let corrected_staged = corrected_writer
        .finish()
        .map_err(DefectCorrectionPipelineError::Stage)?;
    let mut map_writer = AtomicF64PrimaryStreamWriter::create_with_provenance(
        &request.map_output,
        light_dimensions,
        &request.map_provenance,
    )
    .map_err(DefectCorrectionPipelineError::Stage)?;
    map_writer
        .write_image_chunk(&map_image)
        .map_err(DefectCorrectionPipelineError::Stage)?;
    let map_staged = map_writer
        .finish()
        .map_err(DefectCorrectionPipelineError::Stage)?;
    validate_defect_staged(&corrected_staged, light_dimensions)?;
    validate_defect_staged(&map_staged, light_dimensions)?;
    cancellation
        .checkpoint()
        .map_err(DefectCorrectionPipelineError::Cancelled)?;
    verify_all_sources(request).map_err(DefectCorrectionPipelineError::Input)?;
    let mut products = Vec::new();
    products
        .try_reserve_exact(2)
        .map_err(|_| DefectCorrectionPipelineError::AllocationFailed)?;
    products.push(corrected_staged);
    products.push(map_staged);
    let summaries =
        publish_atomic_fits_set(products).map_err(DefectCorrectionPipelineError::Publish)?;
    let [corrected_summary, map_summary] = summaries.as_slice() else {
        return Err(DefectCorrectionPipelineError::AllocationFailed);
    };
    Ok(DefectCorrectionPipelineResult {
        corrected: *corrected_summary,
        map: *map_summary,
        correction: corrected.evidence(),
        parameters_sha256: analysis.parameters_sha256().to_owned(),
        reserved_bytes: estimate.reserved_peak_bytes(),
    })
}

fn validate_product_provenance(
    provenance: &FitsOutputProvenance,
    algorithm: &str,
    source_count: u32,
    parameters_sha256: &str,
) -> Result<(), DefectCorrectionPipelineError> {
    if provenance.algorithm_id() != algorithm
        || provenance.source_count() != source_count
        || provenance.parameters_sha256() != Some(parameters_sha256)
    {
        return Err(DefectCorrectionPipelineError::InvalidProductProvenance);
    }
    Ok(())
}

fn verify_all_sources(request: &StrictDefectCorrectionRequest) -> Result<(), StrictPipelineError> {
    crate::pipeline::verify_source(&request.light, PipelineInput::Signal { index: 0 })?;
    for (index, reference) in request.references.iter().enumerate() {
        crate::pipeline::verify_source(
            reference.source(),
            reference_input(reference.parameters().kind(), index),
        )?;
    }
    Ok(())
}

fn validate_defect_staged(
    staged: &aether_fits::CompletedAtomicFits,
    expected: Dimensions,
) -> Result<(), DefectCorrectionPipelineError> {
    let file = staged
        .try_clone_for_readback()
        .map_err(|error| DefectCorrectionPipelineError::Readback(error.to_string()))?;
    let mut reader = PrimaryImageReader::open(file, HeaderReadOptions::default())
        .map_err(|error| DefectCorrectionPipelineError::Readback(error.to_string()))?;
    let actual = match reader.descriptor().axes() {
        [width, height] => Dimensions::new(
            usize::try_from(*width)
                .map_err(|error| DefectCorrectionPipelineError::Readback(error.to_string()))?,
            usize::try_from(*height)
                .map_err(|error| DefectCorrectionPipelineError::Readback(error.to_string()))?,
            1,
        )
        .map_err(|error| DefectCorrectionPipelineError::Readback(error.to_string()))?,
        _ => {
            return Err(DefectCorrectionPipelineError::Readback(
                "unexpected output axes".to_owned(),
            ));
        }
    };
    let checksums = reader
        .verify_checksums()
        .map_err(|error| DefectCorrectionPipelineError::Readback(error.to_string()))?;
    if actual != expected || !checksums.is_fully_verified() {
        return Err(DefectCorrectionPipelineError::Readback(
            "output dimensions or checksums do not match".to_owned(),
        ));
    }
    Ok(())
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

    #[test]
    fn correction_publishes_verified_science_and_map_as_one_set() -> TestResult {
        let directory = TestDirectory::new()?;
        let dimensions = Dimensions::new(9, 9, 1)?;
        let mut dark = ScientificImage::filled(dimensions, 100.0)?;
        dark.pixels_mut()[4 * 9 + 4] = 1_000.0;
        let mut flat = ScientificImage::filled(dimensions, 1_000.0)?;
        flat.pixels_mut()[6 * 9 + 6] = 100.0;
        let mut light = ScientificImage::filled(dimensions, 50.0)?;
        light.pixels_mut()[4 * 9 + 4] = 900.0;
        light.pixels_mut()[6 * 9 + 6] = 2.0;
        let detection = DefectDetectionParameters::new(2, 1, 8, 5.0, 5.0, 10.0)?;
        let reference_parameters = [
            DefectReferenceParameters::new(DefectReferenceKind::Dark, detection),
            DefectReferenceParameters::new(DefectReferenceKind::Flat, detection),
        ];
        let correction = DefectCorrectionParameters::new(2, 1, 8)?;
        let parameter_digest = strict_defect_parameters_sha256(&reference_parameters, correction)?;
        let references = vec![
            DefectFitsReference::new(
                fits_source(&directory, "dark.fits", &dark)?,
                reference_parameters[0],
            ),
            DefectFitsReference::new(
                fits_source(&directory, "flat.fits", &flat)?,
                reference_parameters[1],
            ),
        ];
        let light_source = fits_source(&directory, "light.fits", &light)?;
        let corrected_path = directory.0.join("corrected.fits");
        let map_path = directory.0.join("defects.fits");
        let corrected_provenance = FitsOutputProvenance::new(
            "a".repeat(64),
            "light-l",
            STRICT_DEFECT_CORRECTED_ALGORITHM_ID,
            3,
        )?
        .with_parameters_sha256(parameter_digest.clone())?;
        let map_provenance = FitsOutputProvenance::new(
            "a".repeat(64),
            "light-l",
            STRICT_DEFECT_MAP_ALGORITHM_ID,
            3,
        )?
        .with_parameters_sha256(parameter_digest.clone())?;
        let request = StrictDefectCorrectionRequest::new(
            light_source,
            references,
            corrected_path.clone(),
            map_path.clone(),
            corrected_provenance,
            map_provenance,
            correction,
        )?;
        let estimate = estimate_defect_memory(dimensions)?;
        let budget = MemoryBudget::new(estimate.reserved_peak_bytes())?;
        let result = run_strict_defect_correction(&request, &CancellationToken::new(), &budget)?;
        assert_eq!(result.correction().requested(), 2);
        assert_eq!(result.correction().corrected(), 2);
        assert_eq!(result.parameters_sha256(), parameter_digest);
        assert_eq!(result.reserved_bytes(), estimate.reserved_peak_bytes());
        assert!(corrected_path.is_file());
        assert!(map_path.is_file());

        let mut corrected_reader =
            PrimaryImageReader::open(File::open(&corrected_path)?, HeaderReadOptions::default())?;
        let corrected = corrected_reader.read_region_image(ImageRegion::new(0, 0, 0, 9, 9))?;
        assert_eq!(corrected.pixels()[4 * 9 + 4].to_bits(), 50.0_f64.to_bits());
        assert_eq!(corrected.pixels()[6 * 9 + 6].to_bits(), 50.0_f64.to_bits());
        let mut map_reader =
            PrimaryImageReader::open(File::open(&map_path)?, HeaderReadOptions::default())?;
        let map_image = map_reader.read_region_image(ImageRegion::new(0, 0, 0, 9, 9))?;
        let map = DefectMap::from_transport_image(&map_image)?;
        assert!(map.mask().get(4, 4, 0)?.contains(PixelFlags::HOT));
        assert!(map.mask().get(6, 6, 0)?.contains(PixelFlags::COLD));
        assert_eq!(budget.used(), 0);
        Ok(())
    }

    #[test]
    fn companion_collision_publishes_no_corrected_product() -> TestResult {
        let directory = TestDirectory::new()?;
        let dimensions = Dimensions::new(7, 7, 1)?;
        let mut dark = ScientificImage::filled(dimensions, 10.0)?;
        dark.pixels_mut()[3 * 7 + 3] = 100.0;
        let light = ScientificImage::filled(dimensions, 20.0)?;
        let detection = DefectDetectionParameters::new(2, 1, 8, 5.0, 5.0, 1.0)?;
        let reference_parameters =
            DefectReferenceParameters::new(DefectReferenceKind::Dark, detection);
        let correction = DefectCorrectionParameters::new(2, 1, 8)?;
        let digest = strict_defect_parameters_sha256(&[reference_parameters], correction)?;
        let corrected_path = directory.0.join("must-not-exist.fits");
        let map_path = directory.0.join("existing-map.fits");
        fs::write(&map_path, b"existing")?;
        let science_provenance = FitsOutputProvenance::new(
            "b".repeat(64),
            "light-l",
            STRICT_DEFECT_CORRECTED_ALGORITHM_ID,
            2,
        )?
        .with_parameters_sha256(digest.clone())?;
        let map_provenance = FitsOutputProvenance::new(
            "b".repeat(64),
            "light-l",
            STRICT_DEFECT_MAP_ALGORITHM_ID,
            2,
        )?
        .with_parameters_sha256(digest)?;
        let request = StrictDefectCorrectionRequest::new(
            fits_source(&directory, "light-collision.fits", &light)?,
            vec![DefectFitsReference::new(
                fits_source(&directory, "dark-collision.fits", &dark)?,
                reference_parameters,
            )],
            corrected_path.clone(),
            map_path.clone(),
            science_provenance,
            map_provenance,
            correction,
        )?;
        let budget = MemoryBudget::new(estimate_defect_memory(dimensions)?.reserved_peak_bytes())?;
        assert!(matches!(
            run_strict_defect_correction(&request, &CancellationToken::new(), &budget),
            Err(DefectCorrectionPipelineError::Stage(
                AtomicFitsWriteError::TargetExists
            ))
        ));
        assert!(!corrected_path.exists());
        assert_eq!(fs::read(map_path)?, b"existing");
        Ok(())
    }
}
