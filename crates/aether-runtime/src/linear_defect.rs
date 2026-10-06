use std::error::Error;
use std::fmt::{Display, Formatter};
use std::path::PathBuf;

use aether_calibration::{
    DefectMapSummary, LinearDefectAxis, LinearDefectCorrectionEvidence,
    LinearDefectCorrectionParameters, LinearDefectDetectionEvidence,
    LinearDefectDetectionParameters, LinearDefectError, correct_linear_defects,
    detect_linear_defects,
};
use aether_core::{Dimensions, PixelFlags};
use aether_fits::{
    AtomicF64PrimaryStreamWriter, AtomicFitsSetWriteError, AtomicFitsWriteError,
    FitsOutputProvenance, FitsWriteSummary, HeaderReadOptions, ImageReadError, ImageRegion,
    PrimaryImageReader, ValidationMode, publish_atomic_fits_set,
};
use sha2::{Digest, Sha256};

use crate::{
    CancellationToken, Cancelled, MemoryBudget, MemoryBudgetError, PipelineInput, PipelineSource,
    ProgressEvent, ProgressEventError, ProgressSequence, ProgressState, StageId, StageIdError,
    StrictPipelineError,
};

const PARAMETER_DOMAIN: &[u8] = b"aetherstack-linear-defect-parameters-v1\0";
const PUBLICATION_BUFFER_BYTES: usize = 2 * 64 * 1_024;
const MAXIMUM_MEMORY_RADIUS: usize = 8;

/// Algorithm identity for line-corrected calibrated-Light pixels.
pub const STRICT_LINEAR_DEFECT_CORRECTED_ALGORITHM_ID: &str = "strict-line-defect-corrected-v1";
/// Algorithm identity for the exact line HOT/COLD companion map.
pub const STRICT_LINEAR_DEFECT_MAP_ALGORITHM_ID: &str = "strict-line-defect-map-v1";
/// Stable progress stage for one complete line-correction transaction.
pub const STRICT_LINEAR_DEFECT_STAGE_ID: &str = "strict-linear-defect-correction";

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

/// Immutable request for one line-corrected Light and its evidence map.
#[derive(Clone, Debug)]
pub struct StrictLinearDefectCorrectionRequest {
    source: PipelineSource,
    corrected_output: PathBuf,
    map_output: PathBuf,
    corrected_provenance: FitsOutputProvenance,
    map_provenance: FitsOutputProvenance,
    detection: LinearDefectDetectionParameters,
    correction: LinearDefectCorrectionParameters,
    parameters_sha256: String,
}

impl StrictLinearDefectCorrectionRequest {
    /// Validates paths, companion provenance, and the complete parameter seal.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        source: PipelineSource,
        corrected_output: PathBuf,
        map_output: PathBuf,
        corrected_provenance: FitsOutputProvenance,
        map_provenance: FitsOutputProvenance,
        detection: LinearDefectDetectionParameters,
        correction: LinearDefectCorrectionParameters,
    ) -> Result<Self, LinearDefectPipelineError> {
        if corrected_output == map_output {
            return Err(LinearDefectPipelineError::DuplicateOutput);
        }
        let parameters_sha256 = strict_linear_defect_parameters_sha256(detection, correction)
            .map_err(LinearDefectPipelineError::Parameters)?;
        validate_provenance(
            &corrected_provenance,
            STRICT_LINEAR_DEFECT_CORRECTED_ALGORITHM_ID,
            &parameters_sha256,
        )?;
        validate_provenance(
            &map_provenance,
            STRICT_LINEAR_DEFECT_MAP_ALGORITHM_ID,
            &parameters_sha256,
        )?;
        if corrected_provenance.manifest_sha256() != map_provenance.manifest_sha256()
            || corrected_provenance.plan_sha256() != map_provenance.plan_sha256()
            || corrected_provenance.group_id() != map_provenance.group_id()
        {
            return Err(LinearDefectPipelineError::ProvenanceMismatch);
        }
        Ok(Self {
            source,
            corrected_output,
            map_output,
            corrected_provenance,
            map_provenance,
            detection,
            correction,
            parameters_sha256,
        })
    }

    /// Canonical path-free parameter identity required by both products.
    #[must_use]
    pub fn parameters_sha256(&self) -> &str {
        &self.parameters_sha256
    }
}

/// Published line-corrected Light and exact companion evidence.
#[derive(Clone, Debug)]
pub struct LinearDefectPipelineResult {
    corrected: FitsWriteSummary,
    map: FitsWriteSummary,
    detection: LinearDefectDetectionEvidence,
    correction: LinearDefectCorrectionEvidence,
    map_summary: DefectMapSummary,
    parameters_sha256: String,
    reserved_bytes: usize,
}

impl LinearDefectPipelineResult {
    /// Corrected FITS summary.
    #[must_use]
    pub const fn corrected(&self) -> FitsWriteSummary {
        self.corrected
    }
    /// HOT/COLD companion FITS summary.
    #[must_use]
    pub const fn map(&self) -> FitsWriteSummary {
        self.map
    }
    /// Complete coherent-line detection accounting.
    #[must_use]
    pub const fn detection(&self) -> LinearDefectDetectionEvidence {
        self.detection
    }
    /// Complete perpendicular replacement accounting.
    #[must_use]
    pub const fn correction(&self) -> LinearDefectCorrectionEvidence {
        self.correction
    }
    /// Exact categories in the published companion map.
    #[must_use]
    pub const fn map_summary(&self) -> DefectMapSummary {
        self.map_summary
    }
    /// Canonical parameter identity embedded in both products.
    #[must_use]
    pub fn parameters_sha256(&self) -> &str {
        &self.parameters_sha256
    }
    /// Peak bytes reserved before decoding.
    #[must_use]
    pub const fn reserved_bytes(&self) -> usize {
        self.reserved_bytes
    }
}

/// Failure of the all-or-nothing linear-defect transaction.
#[derive(Debug)]
pub enum LinearDefectPipelineError {
    /// Stable progress-stage construction failed.
    StageId(StageIdError),
    /// A progress event violated lifecycle invariants.
    Progress(ProgressEventError),
    /// Parameter sealing failed.
    Parameters(LinearDefectParameterSealError),
    /// Output destinations are identical.
    DuplicateOutput,
    /// A product has the wrong algorithm, source count, or parameter identity.
    InvalidProductProvenance,
    /// Companion products do not share manifest, plan, and group identities.
    ProvenanceMismatch,
    /// Source fingerprint, header, or opening failed.
    Input(StrictPipelineError),
    /// Source FITS checksums were absent or invalid.
    ChecksumNotVerified,
    /// The strict file transaction currently accepts one detector plane.
    UnsupportedPlaneCount {
        /// Rejected plane count.
        planes: usize,
    },
    /// Pixel decoding failed.
    Read(ImageReadError),
    /// Detection or correction failed.
    LinearDefect(LinearDefectError),
    /// Memory planning overflowed.
    MemoryEstimate(LinearDefectMemoryEstimateError),
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

impl Display for LinearDefectPipelineError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StageId(error) => Display::fmt(error, formatter),
            Self::Progress(error) => Display::fmt(error, formatter),
            Self::Parameters(error) => Display::fmt(error, formatter),
            Self::DuplicateOutput => formatter
                .write_str("line-corrected image and line map require distinct output paths"),
            Self::InvalidProductProvenance => {
                formatter.write_str("linear-defect product provenance does not match the request")
            }
            Self::ProvenanceMismatch => formatter
                .write_str("linear-defect companions do not identify one common product set"),
            Self::Input(error) => Display::fmt(error, formatter),
            Self::ChecksumNotVerified => {
                formatter.write_str("linear-defect source lacks verified FITS checksums")
            }
            Self::UnsupportedPlaneCount { planes } => {
                write!(
                    formatter,
                    "linear-defect FITS execution received {planes} planes"
                )
            }
            Self::Read(error) => Display::fmt(error, formatter),
            Self::LinearDefect(error) => Display::fmt(error, formatter),
            Self::MemoryEstimate(error) => Display::fmt(error, formatter),
            Self::Cancelled(error) => Display::fmt(error, formatter),
            Self::Memory(error) => Display::fmt(error, formatter),
            Self::Stage(error) => Display::fmt(error, formatter),
            Self::Readback(message) => {
                write!(formatter, "linear-defect readback failed: {message}")
            }
            Self::Publish(error) => Display::fmt(error, formatter),
            Self::AllocationFailed => {
                formatter.write_str("cannot allocate linear-defect transaction bookkeeping")
            }
        }
    }
}

impl Error for LinearDefectPipelineError {}

/// Runs one verified all-or-nothing line-correction transaction.
pub fn run_strict_linear_defect_correction(
    request: &StrictLinearDefectCorrectionRequest,
    cancellation: &CancellationToken,
    memory: &MemoryBudget,
) -> Result<LinearDefectPipelineResult, LinearDefectPipelineError> {
    run_strict_linear_defect_correction_with_progress(request, cancellation, memory, |_| {})
}

/// Runs one transaction while reporting four deterministic publication phases.
pub fn run_strict_linear_defect_correction_with_progress<F>(
    request: &StrictLinearDefectCorrectionRequest,
    cancellation: &CancellationToken,
    memory: &MemoryBudget,
    mut progress: F,
) -> Result<LinearDefectPipelineResult, LinearDefectPipelineError>
where
    F: FnMut(ProgressEvent),
{
    let stage =
        StageId::new(STRICT_LINEAR_DEFECT_STAGE_ID).map_err(LinearDefectPipelineError::StageId)?;
    let sequence = ProgressSequence::new();
    emit_progress(
        &sequence,
        &stage,
        ProgressState::Started,
        0,
        None,
        &mut progress,
    )?;
    let mut completed = 0_u64;
    let execution = execute_linear_defect(request, cancellation, memory, &mut || {
        completed += 1;
        emit_progress(
            &sequence,
            &stage,
            ProgressState::Running,
            completed,
            None,
            &mut progress,
        )
    });
    match execution {
        Ok(result) => {
            emit_progress(
                &sequence,
                &stage,
                ProgressState::Completed,
                completed,
                None,
                &mut progress,
            )?;
            Ok(result)
        }
        Err(error) => {
            let state = if matches!(error, LinearDefectPipelineError::Cancelled(_)) {
                ProgressState::Cancelled
            } else {
                ProgressState::Failed
            };
            let _ignored = emit_progress(
                &sequence,
                &stage,
                state,
                completed,
                Some(linear_error_code(&error).to_owned()),
                &mut progress,
            );
            Err(error)
        }
    }
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

fn execute_linear_defect<F>(
    request: &StrictLinearDefectCorrectionRequest,
    cancellation: &CancellationToken,
    memory: &MemoryBudget,
    advance: &mut F,
) -> Result<LinearDefectPipelineResult, LinearDefectPipelineError>
where
    F: FnMut() -> Result<(), LinearDefectPipelineError>,
{
    crate::pipeline::verify_source(&request.source, PipelineInput::Signal { index: 0 })
        .map_err(LinearDefectPipelineError::Input)?;
    let mut reader = crate::pipeline::open_reader(
        request.source.path(),
        PipelineInput::Signal { index: 0 },
        HeaderReadOptions::default(),
        ValidationMode::Strict,
    )
    .map_err(LinearDefectPipelineError::Input)?;
    let dimensions = crate::pipeline::dimensions_from_axes(
        PipelineInput::Signal { index: 0 },
        reader.descriptor().axes(),
    )
    .map_err(LinearDefectPipelineError::Input)?;
    if dimensions.planes() != 1 {
        return Err(LinearDefectPipelineError::UnsupportedPlaneCount {
            planes: dimensions.planes(),
        });
    }
    if !reader
        .verify_checksums()
        .map_err(LinearDefectPipelineError::Read)?
        .is_fully_verified()
    {
        return Err(LinearDefectPipelineError::ChecksumNotVerified);
    }
    let maximum_radius = request
        .detection
        .perpendicular_radius()
        .max(request.correction.perpendicular_radius());
    let estimate = estimate_linear_defect_memory(dimensions, maximum_radius)
        .map_err(LinearDefectPipelineError::MemoryEstimate)?;
    let _reservation = memory
        .try_reserve(estimate.reserved_peak_bytes())
        .map_err(LinearDefectPipelineError::Memory)?;
    let width = u64::try_from(dimensions.width())
        .map_err(|_| LinearDefectPipelineError::MemoryEstimate(LinearDefectMemoryEstimateError))?;
    let height = u64::try_from(dimensions.height())
        .map_err(|_| LinearDefectPipelineError::MemoryEstimate(LinearDefectMemoryEstimateError))?;
    let source = reader
        .read_region_image(ImageRegion::new(0, 0, 0, width, height))
        .map_err(LinearDefectPipelineError::Read)?;
    let (map, detection) = detect_linear_defects(&source, request.detection)
        .map_err(LinearDefectPipelineError::LinearDefect)?;
    let corrected = correct_linear_defects(&source, &map, request.correction)
        .map_err(LinearDefectPipelineError::LinearDefect)?;
    drop(source);
    advance()?;
    cancellation
        .checkpoint()
        .map_err(LinearDefectPipelineError::Cancelled)?;
    crate::pipeline::verify_source(&request.source, PipelineInput::Signal { index: 0 })
        .map_err(LinearDefectPipelineError::Input)?;

    let map_summary = corrected.defect_map().summary();
    let correction = corrected.evidence();
    let map_image = corrected
        .defect_map()
        .to_transport_image()
        .map_err(|error| {
            LinearDefectPipelineError::LinearDefect(LinearDefectError::DefectMap(error))
        })?;
    let mut corrected_writer = AtomicF64PrimaryStreamWriter::create_with_provenance(
        &request.corrected_output,
        dimensions,
        &request.corrected_provenance,
    )
    .map_err(LinearDefectPipelineError::Stage)?;
    corrected_writer
        .write_image_chunk(corrected.image())
        .map_err(LinearDefectPipelineError::Stage)?;
    let corrected_staged = corrected_writer
        .finish()
        .map_err(LinearDefectPipelineError::Stage)?;
    let mut map_writer = AtomicF64PrimaryStreamWriter::create_with_provenance(
        &request.map_output,
        dimensions,
        &request.map_provenance,
    )
    .map_err(LinearDefectPipelineError::Stage)?;
    map_writer
        .write_image_chunk(&map_image)
        .map_err(LinearDefectPipelineError::Stage)?;
    let map_staged = map_writer
        .finish()
        .map_err(LinearDefectPipelineError::Stage)?;
    advance()?;
    validate_staged(&corrected_staged, dimensions)?;
    validate_staged(&map_staged, dimensions)?;
    cancellation
        .checkpoint()
        .map_err(LinearDefectPipelineError::Cancelled)?;
    crate::pipeline::verify_source(&request.source, PipelineInput::Signal { index: 0 })
        .map_err(LinearDefectPipelineError::Input)?;
    advance()?;
    let mut products = Vec::new();
    products
        .try_reserve_exact(2)
        .map_err(|_| LinearDefectPipelineError::AllocationFailed)?;
    products.push(corrected_staged);
    products.push(map_staged);
    let summaries =
        publish_atomic_fits_set(products).map_err(LinearDefectPipelineError::Publish)?;
    advance()?;
    let [corrected_summary, map_summary_fits] = summaries.as_slice() else {
        return Err(LinearDefectPipelineError::AllocationFailed);
    };
    Ok(LinearDefectPipelineResult {
        corrected: *corrected_summary,
        map: *map_summary_fits,
        detection,
        correction,
        map_summary,
        parameters_sha256: request.parameters_sha256.clone(),
        reserved_bytes: estimate.reserved_peak_bytes(),
    })
}

fn emit_progress<F>(
    sequence: &ProgressSequence,
    stage: &StageId,
    state: ProgressState,
    completed: u64,
    code: Option<String>,
    progress: &mut F,
) -> Result<(), LinearDefectPipelineError>
where
    F: FnMut(ProgressEvent),
{
    let event = sequence
        .next(stage.clone(), state, completed, Some(4), code)
        .map_err(LinearDefectPipelineError::Progress)?;
    progress(event);
    Ok(())
}

fn validate_provenance(
    provenance: &FitsOutputProvenance,
    algorithm: &str,
    parameters_sha256: &str,
) -> Result<(), LinearDefectPipelineError> {
    if provenance.algorithm_id() != algorithm
        || provenance.source_count() != 1
        || provenance.parameters_sha256() != Some(parameters_sha256)
    {
        return Err(LinearDefectPipelineError::InvalidProductProvenance);
    }
    Ok(())
}

fn validate_staged(
    staged: &aether_fits::CompletedAtomicFits,
    expected: Dimensions,
) -> Result<(), LinearDefectPipelineError> {
    let file = staged
        .try_clone_for_readback()
        .map_err(|error| LinearDefectPipelineError::Readback(error.to_string()))?;
    let mut reader = PrimaryImageReader::open(file, HeaderReadOptions::default())
        .map_err(|error| LinearDefectPipelineError::Readback(error.to_string()))?;
    let actual = match reader.descriptor().axes() {
        [width, height] => Dimensions::new(
            usize::try_from(*width)
                .map_err(|error| LinearDefectPipelineError::Readback(error.to_string()))?,
            usize::try_from(*height)
                .map_err(|error| LinearDefectPipelineError::Readback(error.to_string()))?,
            1,
        )
        .map_err(|error| LinearDefectPipelineError::Readback(error.to_string()))?,
        _ => {
            return Err(LinearDefectPipelineError::Readback(
                "unexpected output axes".to_owned(),
            ));
        }
    };
    let checksums = reader
        .verify_checksums()
        .map_err(|error| LinearDefectPipelineError::Readback(error.to_string()))?;
    if actual != expected || !checksums.is_fully_verified() {
        return Err(LinearDefectPipelineError::Readback(
            "output dimensions or checksums do not match".to_owned(),
        ));
    }
    Ok(())
}

const fn linear_error_code(error: &LinearDefectPipelineError) -> &'static str {
    match error {
        LinearDefectPipelineError::Cancelled(_) => "cancelled",
        LinearDefectPipelineError::Memory(_) => "memory_budget_exceeded",
        LinearDefectPipelineError::ChecksumNotVerified => "checksum_not_verified",
        LinearDefectPipelineError::UnsupportedPlaneCount { .. } => "unsupported_plane_count",
        LinearDefectPipelineError::Publish(_) => "publication_failed",
        LinearDefectPipelineError::Stage(_) => "staging_failed",
        LinearDefectPipelineError::Readback(_) => "readback_failed",
        LinearDefectPipelineError::Read(_) => "fits_read_failed",
        LinearDefectPipelineError::Input(_) => "source_integrity_failed",
        LinearDefectPipelineError::LinearDefect(_) => "linear_defect_failed",
        LinearDefectPipelineError::MemoryEstimate(_) => "memory_estimate_failed",
        LinearDefectPipelineError::Parameters(_)
        | LinearDefectPipelineError::DuplicateOutput
        | LinearDefectPipelineError::InvalidProductProvenance
        | LinearDefectPipelineError::ProvenanceMismatch => "configuration_invalid",
        LinearDefectPipelineError::AllocationFailed => "allocation_failed",
        LinearDefectPipelineError::StageId(_) | LinearDefectPipelineError::Progress(_) => {
            "progress_invariant_failed"
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use aether_calibration::DefectMap;
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
                "aether-linear-defect-runtime-{}-{sequence}",
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

    fn execution_policies() -> Result<Policies, Box<dyn Error>> {
        Ok((
            LinearDefectDetectionParameters::new(
                LinearDefectAxis::Rows,
                2,
                1,
                2,
                5,
                500_000,
                5.0,
                5.0,
                1.0,
            )?,
            LinearDefectCorrectionParameters::new(LinearDefectAxis::Rows, 2, 1, 2)?,
        ))
    }

    fn provenance(algorithm: &str, digest: &str) -> Result<FitsOutputProvenance, Box<dyn Error>> {
        Ok(
            FitsOutputProvenance::new("a".repeat(64), "light-l", algorithm, 1)?
                .with_parameters_sha256(digest.to_owned())?,
        )
    }

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
    fn seal_is_stable_path_free_and_binds_every_policy_family() -> TestResult {
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
    fn seal_rejects_mixed_axis_and_lattice_policies() -> TestResult {
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
    fn memory_peak_is_exact_for_representative_astro_cameras() -> TestResult {
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
    fn memory_estimate_rejects_radius_and_arithmetic_overflow() -> TestResult {
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

    #[test]
    fn transaction_publishes_verified_science_and_map_together() -> TestResult {
        let directory = TestDirectory::new()?;
        let dimensions = Dimensions::new(9, 9, 1)?;
        let mut source = ScientificImage::filled(dimensions, 100.0)?;
        for x in 0..9 {
            source.pixels_mut()[4 * 9 + x] = 500.0;
        }
        let (detection, correction) = execution_policies()?;
        let digest = strict_linear_defect_parameters_sha256(detection, correction)?;
        let corrected_path = directory.0.join("corrected.fits");
        let map_path = directory.0.join("line-map.fits");
        let request = StrictLinearDefectCorrectionRequest::new(
            fits_source(&directory, "source.fits", &source)?,
            corrected_path.clone(),
            map_path.clone(),
            provenance(STRICT_LINEAR_DEFECT_CORRECTED_ALGORITHM_ID, &digest)?,
            provenance(STRICT_LINEAR_DEFECT_MAP_ALGORITHM_ID, &digest)?,
            detection,
            correction,
        )?;
        let estimate = estimate_linear_defect_memory(dimensions, 2)?;
        let budget = MemoryBudget::new(estimate.reserved_peak_bytes())?;
        let mut progress = Vec::new();
        let result = run_strict_linear_defect_correction_with_progress(
            &request,
            &CancellationToken::new(),
            &budget,
            |event| progress.push(event),
        )?;
        assert_eq!(result.detection().hot_lines(), 1);
        assert_eq!(result.correction().requested_samples(), 9);
        assert_eq!(result.correction().corrected_samples(), 9);
        assert_eq!(result.map_summary().hot_samples(), 9);
        assert_eq!(result.parameters_sha256(), digest);
        assert_eq!(result.reserved_bytes(), estimate.reserved_peak_bytes());
        assert_eq!(progress.len(), 6);
        assert_eq!(
            progress.first().map(ProgressEvent::state),
            Some(ProgressState::Started)
        );
        assert_eq!(
            progress.last().map(ProgressEvent::state),
            Some(ProgressState::Completed)
        );
        assert_eq!(progress.last().map(ProgressEvent::completed_units), Some(4));
        assert_eq!(budget.used(), 0);

        let mut corrected_reader =
            PrimaryImageReader::open(File::open(&corrected_path)?, HeaderReadOptions::default())?;
        let corrected = corrected_reader.read_region_image(ImageRegion::new(0, 0, 0, 9, 9))?;
        for x in 0..9 {
            assert_eq!(corrected.pixels()[4 * 9 + x].to_bits(), 100.0_f64.to_bits());
        }
        let mut map_reader =
            PrimaryImageReader::open(File::open(&map_path)?, HeaderReadOptions::default())?;
        let map_image = map_reader.read_region_image(ImageRegion::new(0, 0, 0, 9, 9))?;
        assert_eq!(
            DefectMap::from_transport_image(&map_image)?
                .summary()
                .hot_samples(),
            9
        );
        Ok(())
    }

    #[test]
    fn companion_collision_rolls_back_the_science_product() -> TestResult {
        let directory = TestDirectory::new()?;
        let dimensions = Dimensions::new(9, 9, 1)?;
        let source = ScientificImage::filled(dimensions, 100.0)?;
        let (detection, correction) = execution_policies()?;
        let digest = strict_linear_defect_parameters_sha256(detection, correction)?;
        let corrected_path = directory.0.join("must-not-exist.fits");
        let map_path = directory.0.join("existing-map.fits");
        fs::write(&map_path, b"existing")?;
        let request = StrictLinearDefectCorrectionRequest::new(
            fits_source(&directory, "source.fits", &source)?,
            corrected_path.clone(),
            map_path.clone(),
            provenance(STRICT_LINEAR_DEFECT_CORRECTED_ALGORITHM_ID, &digest)?,
            provenance(STRICT_LINEAR_DEFECT_MAP_ALGORITHM_ID, &digest)?,
            detection,
            correction,
        )?;
        let budget =
            MemoryBudget::new(estimate_linear_defect_memory(dimensions, 2)?.reserved_peak_bytes())?;
        let mut progress = Vec::new();
        assert!(matches!(
            run_strict_linear_defect_correction_with_progress(
                &request,
                &CancellationToken::new(),
                &budget,
                |event| progress.push(event),
            ),
            Err(LinearDefectPipelineError::Stage(
                AtomicFitsWriteError::TargetExists
            ))
        ));
        assert!(!corrected_path.exists());
        assert_eq!(fs::read(map_path)?, b"existing");
        assert_eq!(
            progress.last().map(ProgressEvent::state),
            Some(ProgressState::Failed)
        );
        assert_eq!(
            progress.last().and_then(ProgressEvent::code),
            Some("staging_failed")
        );
        Ok(())
    }
}
