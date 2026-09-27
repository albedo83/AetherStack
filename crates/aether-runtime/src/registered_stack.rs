use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::mem::size_of;
use std::path::{Path, PathBuf};

use aether_core::{Dimensions, PixelFlags};
use aether_fits::{
    AtomicF64PrimaryStreamWriter, AtomicFitsWriteError, FitsOutputProvenance, FitsWriteSummary,
    HeaderReadOptions, ImageReadError, ImageRegion, PrimaryImageReader, SampleStatus,
    ValidationMode,
};
use aether_integration::{IntegrationError, PixelSupport, integrate_mean};
use aether_registration::RegistrationPlan;
use aether_review::FrameId;

use crate::pipeline::{dimensions_from_axes, open_reader, verify_source};
use crate::{
    CancellationToken, Cancelled, MemoryBudget, MemoryBudgetError, PipelineInput, PipelineSource,
    ProgressEvent, ProgressEventError, ProgressSequence, ProgressState, StageId, StageIdError,
    StrictPipelineError,
};

/// Strict estimator identity for a plan-bound registered common-crop stack.
pub const REGISTERED_CROP_MEAN_ALGORITHM_ID: &str = "registered-crop-mean-v1";
const REGISTERED_STACK_STAGE_ID: &str = "registered-stack";
const DEFAULT_BAND_HEIGHT: usize = 128;
const STREAM_WRITER_BUFFER_BYTES: usize = 64 * 1_024;

/// One immutable registered FITS artifact bound to its reviewed frame.
#[derive(Clone, Debug)]
pub struct RegisteredStackSource {
    frame_id: FrameId,
    source: PipelineSource,
}

impl RegisteredStackSource {
    /// Binds one registered artifact to the identity carried in `AETHFID`.
    #[must_use]
    pub const fn new(frame_id: FrameId, source: PipelineSource) -> Self {
        Self { frame_id, source }
    }

    /// Reviewed frame identity expected in the artifact header.
    #[must_use]
    pub const fn frame_id(&self) -> &FrameId {
        &self.frame_id
    }

    /// Fingerprinted registered FITS source.
    #[must_use]
    pub const fn source(&self) -> &PipelineSource {
        &self.source
    }
}

/// Validated request for one bounded common-crop registered integration.
#[derive(Clone, Debug)]
pub struct RegisteredStackRequest {
    plan: RegistrationPlan,
    sources: Vec<RegisteredStackSource>,
    output: PathBuf,
    provenance: FitsOutputProvenance,
    band_height: usize,
    header_options: HeaderReadOptions,
    validation_mode: ValidationMode,
}

impl RegisteredStackRequest {
    /// Builds a request and canonicalizes sources into plan identity order.
    pub fn new(
        plan: RegistrationPlan,
        sources: Vec<RegisteredStackSource>,
        output: PathBuf,
        provenance: FitsOutputProvenance,
    ) -> Result<Self, RegisteredStackError> {
        if sources.len() != plan.frames().len() {
            return Err(RegisteredStackError::SourceSetMismatch);
        }
        let source_count =
            u32::try_from(sources.len()).map_err(|_| RegisteredStackError::WorkSizeOverflow)?;
        if provenance.algorithm_id() != REGISTERED_CROP_MEAN_ALGORITHM_ID {
            return Err(RegisteredStackError::ProvenanceAlgorithmMismatch);
        }
        if provenance.source_count() != source_count {
            return Err(RegisteredStackError::ProvenanceSourceCountMismatch);
        }
        if provenance.plan_sha256() != Some(plan.plan_sha256()) {
            return Err(RegisteredStackError::ProvenancePlanMismatch);
        }
        let mut by_id = BTreeMap::new();
        for source in sources {
            if by_id.insert(source.frame_id.clone(), source).is_some() {
                return Err(RegisteredStackError::SourceSetMismatch);
            }
        }
        let mut canonical = Vec::new();
        canonical
            .try_reserve_exact(plan.frames().len())
            .map_err(|_| RegisteredStackError::AllocationFailed)?;
        for frame in plan.frames() {
            canonical.push(
                by_id
                    .remove(frame.frame_id())
                    .ok_or(RegisteredStackError::SourceSetMismatch)?,
            );
        }
        if !by_id.is_empty() {
            return Err(RegisteredStackError::SourceSetMismatch);
        }
        Ok(Self {
            plan,
            sources: canonical,
            output,
            provenance,
            band_height: DEFAULT_BAND_HEIGHT,
            header_options: HeaderReadOptions::default(),
            validation_mode: ValidationMode::Strict,
        })
    }

    /// Replaces the maximum output rows retained per plane band.
    pub fn with_band_height(mut self, height: usize) -> Result<Self, RegisteredStackError> {
        if height == 0 {
            return Err(RegisteredStackError::ZeroBandHeight);
        }
        self.band_height = height;
        Ok(self)
    }

    /// Replaces FITS header limits and conformance policy.
    #[must_use]
    pub const fn with_header_policy(
        mut self,
        options: HeaderReadOptions,
        mode: ValidationMode,
    ) -> Self {
        self.header_options = options;
        self.validation_mode = mode;
        self
    }

    /// Exact sealed registration plan.
    #[must_use]
    pub const fn plan(&self) -> &RegistrationPlan {
        &self.plan
    }

    /// Canonical registered source order.
    #[must_use]
    pub fn sources(&self) -> &[RegisteredStackSource] {
        &self.sources
    }

    /// Atomic create-new output path.
    #[must_use]
    pub fn output(&self) -> &Path {
        &self.output
    }
}

/// Completed common-crop registered stack.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegisteredStackResult {
    dimensions: Dimensions,
    summary: FitsWriteSummary,
    peak_reserved_bytes: usize,
}

impl RegisteredStackResult {
    /// Published cropped output dimensions.
    #[must_use]
    pub const fn dimensions(self) -> Dimensions {
        self.dimensions
    }

    /// Exact sample, substitution, byte, and checksum accounting.
    #[must_use]
    pub const fn summary(self) -> FitsWriteSummary {
        self.summary
    }

    /// Peak logical working set observed by the shared budget.
    #[must_use]
    pub const fn peak_reserved_bytes(self) -> usize {
        self.peak_reserved_bytes
    }
}

/// Failure of strict registered common-crop integration.
#[derive(Debug)]
pub enum RegisteredStackError {
    /// Supplied identities are missing, duplicated, or foreign to the plan.
    SourceSetMismatch,
    /// Output provenance names a different estimator.
    ProvenanceAlgorithmMismatch,
    /// Output provenance does not represent every registered source.
    ProvenanceSourceCountMismatch,
    /// Output provenance is not bound to the exact registration plan.
    ProvenancePlanMismatch,
    /// A band cannot contain zero rows.
    ZeroBandHeight,
    /// A registered artifact does not carry its expected reviewed identity.
    ArtifactIdentityMismatch {
        /// Expected reviewed frame.
        frame_id: FrameId,
    },
    /// A registered artifact was produced by another registration plan.
    ArtifactPlanMismatch {
        /// Artifact whose plan binding failed.
        frame_id: FrameId,
    },
    /// A registered artifact does not use the complete reference canvas.
    DimensionMismatch {
        /// Artifact whose dimensions failed.
        frame_id: FrameId,
        /// Required reference-canvas dimensions.
        expected: Dimensions,
        /// Decoded artifact dimensions.
        actual: Dimensions,
    },
    /// Registered artifacts disagree on their mono or planar-RGB shape.
    PlaneCountMismatch {
        /// Plane count established by the first canonical source.
        expected: usize,
        /// Conflicting plane count.
        actual: usize,
    },
    /// Shared strict FITS or fingerprint validation failed.
    Input(StrictPipelineError),
    /// A FITS pixel region or checksum could not be decoded.
    ReadInput(ImageReadError),
    /// Strict mean integration failed.
    Integration(IntegrationError),
    /// The logical working set exceeded the supplied budget.
    Memory(MemoryBudgetError),
    /// Private FITS construction or atomic publication failed.
    Publish(AtomicFitsWriteError),
    /// Complete private output or a registered input failed checksum validation.
    InvalidStagedOutput,
    /// Work-unit, coordinate, or byte accounting overflowed.
    WorkSizeOverflow,
    /// A bounded source or reference vector could not be allocated.
    AllocationFailed,
    /// Execution stopped at a cooperative checkpoint.
    Cancelled(Cancelled),
    /// The fixed stage identifier unexpectedly failed validation.
    StageId(StageIdError),
    /// A machine-readable progress event violated its invariant.
    Progress(ProgressEventError),
}

impl RegisteredStackError {
    const fn code(&self) -> &'static str {
        match self {
            Self::SourceSetMismatch => "registered-stack-source-set",
            Self::ProvenanceAlgorithmMismatch => "registered-stack-provenance-algorithm",
            Self::ProvenanceSourceCountMismatch => "registered-stack-provenance-count",
            Self::ProvenancePlanMismatch => "registered-stack-provenance-plan",
            Self::ZeroBandHeight => "registered-stack-band-height",
            Self::ArtifactIdentityMismatch { .. } => "registered-stack-artifact-identity",
            Self::ArtifactPlanMismatch { .. } => "registered-stack-artifact-plan",
            Self::DimensionMismatch { .. } => "registered-stack-dimensions",
            Self::PlaneCountMismatch { .. } => "registered-stack-planes",
            Self::Input(_) => "registered-stack-input",
            Self::ReadInput(_) => "registered-stack-read",
            Self::Integration(_) => "registered-stack-integration",
            Self::Memory(_) => "registered-stack-memory",
            Self::Publish(_) => "registered-stack-publish",
            Self::InvalidStagedOutput => "registered-stack-readback",
            Self::WorkSizeOverflow => "registered-stack-work-size",
            Self::AllocationFailed => "registered-stack-allocation",
            Self::Cancelled(_) => "cancelled",
            Self::StageId(_) => "registered-stack-stage",
            Self::Progress(_) => "registered-stack-progress",
        }
    }
}

impl Display for RegisteredStackError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SourceSetMismatch => {
                formatter.write_str("registered source set does not match the sealed plan")
            }
            Self::ProvenanceAlgorithmMismatch => {
                formatter.write_str("output provenance names another integration algorithm")
            }
            Self::ProvenanceSourceCountMismatch => {
                formatter.write_str("output provenance source count is inconsistent")
            }
            Self::ProvenancePlanMismatch => {
                formatter.write_str("output provenance does not bind the sealed registration plan")
            }
            Self::ZeroBandHeight => {
                formatter.write_str("registered stack band height must be positive")
            }
            Self::ArtifactIdentityMismatch { frame_id } => write!(
                formatter,
                "registered artifact identity does not match {}",
                frame_id.as_str()
            ),
            Self::ArtifactPlanMismatch { frame_id } => write!(
                formatter,
                "registered artifact for {} belongs to another plan",
                frame_id.as_str()
            ),
            Self::DimensionMismatch {
                frame_id,
                expected,
                actual,
            } => write!(
                formatter,
                "registered artifact {} is {}x{}x{}; expected {}x{}x{}",
                frame_id.as_str(),
                actual.width(),
                actual.height(),
                actual.planes(),
                expected.width(),
                expected.height(),
                expected.planes()
            ),
            Self::PlaneCountMismatch { expected, actual } => write!(
                formatter,
                "registered source plane count {actual} does not match {expected}"
            ),
            Self::Input(error) => Display::fmt(error, formatter),
            Self::ReadInput(error) => Display::fmt(error, formatter),
            Self::Integration(error) => Display::fmt(error, formatter),
            Self::Memory(error) => Display::fmt(error, formatter),
            Self::Publish(error) => Display::fmt(error, formatter),
            Self::InvalidStagedOutput => {
                formatter.write_str("private registered stack failed checksum readback")
            }
            Self::WorkSizeOverflow => {
                formatter.write_str("registered stack work accounting overflowed")
            }
            Self::AllocationFailed => formatter.write_str("registered stack allocation failed"),
            Self::Cancelled(error) => Display::fmt(error, formatter),
            Self::StageId(error) => Display::fmt(error, formatter),
            Self::Progress(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for RegisteredStackError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Input(error) => Some(error),
            Self::ReadInput(error) => Some(error),
            Self::Integration(error) => Some(error),
            Self::Memory(error) => Some(error),
            Self::Publish(error) => Some(error),
            Self::Cancelled(error) => Some(error),
            Self::StageId(error) => Some(error),
            Self::Progress(error) => Some(error),
            _ => None,
        }
    }
}

/// Integrates the sealed common crop and publishes one checksum-verified FITS.
pub fn run_registered_stack<F>(
    request: &RegisteredStackRequest,
    cancellation: &CancellationToken,
    memory: &MemoryBudget,
    mut progress: F,
) -> Result<RegisteredStackResult, RegisteredStackError>
where
    F: FnMut(ProgressEvent),
{
    let stage = StageId::new(REGISTERED_STACK_STAGE_ID).map_err(RegisteredStackError::StageId)?;
    let sequence = ProgressSequence::new();
    emit(
        &sequence,
        &stage,
        ProgressState::Started,
        0,
        None,
        None,
        &mut progress,
    )?;
    let mut completed = 0;
    let mut total = None;
    let result = execute(
        request,
        cancellation,
        memory,
        &sequence,
        &stage,
        &mut completed,
        &mut total,
        &mut progress,
    );
    match result {
        Ok(value) => {
            emit(
                &sequence,
                &stage,
                ProgressState::Completed,
                completed,
                total,
                None,
                &mut progress,
            )?;
            Ok(value)
        }
        Err(error) => {
            let state = if matches!(error, RegisteredStackError::Cancelled(_)) {
                ProgressState::Cancelled
            } else {
                ProgressState::Failed
            };
            let _ignored = emit(
                &sequence,
                &stage,
                state,
                completed,
                total,
                Some(error.code().to_owned()),
                &mut progress,
            );
            Err(error)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn execute<F>(
    request: &RegisteredStackRequest,
    cancellation: &CancellationToken,
    memory: &MemoryBudget,
    sequence: &ProgressSequence,
    stage: &StageId,
    completed: &mut u64,
    total: &mut Option<u64>,
    progress: &mut F,
) -> Result<RegisteredStackResult, RegisteredStackError>
where
    F: FnMut(ProgressEvent),
{
    cancellation
        .checkpoint()
        .map_err(RegisteredStackError::Cancelled)?;
    let crop = request
        .plan
        .common_footprint()
        .crop()
        .ok_or(RegisteredStackError::SourceSetMismatch)?;
    let planes = validate_sources(request, cancellation)?;
    let output_dimensions = Dimensions::new(crop.width(), crop.height(), planes)
        .map_err(|_| RegisteredStackError::WorkSizeOverflow)?;
    let bands = crop.height().div_ceil(request.band_height);
    *total = bands
        .checked_mul(planes)
        .and_then(|value| value.checked_add(2))
        .and_then(|value| u64::try_from(value).ok())
        .ok_or(RegisteredStackError::WorkSizeOverflow)
        .map(Some)?;
    emit(
        sequence,
        stage,
        ProgressState::Running,
        *completed,
        *total,
        None,
        progress,
    )?;

    let _writer = memory
        .try_reserve(STREAM_WRITER_BUFFER_BYTES)
        .map_err(RegisteredStackError::Memory)?;
    let mut writer = AtomicF64PrimaryStreamWriter::create_with_provenance(
        &request.output,
        output_dimensions,
        &request.provenance,
    )
    .map_err(RegisteredStackError::Publish)?;
    for plane in 0..planes {
        for offset_y in (0..crop.height()).step_by(request.band_height) {
            cancellation
                .checkpoint()
                .map_err(RegisteredStackError::Cancelled)?;
            let height = (crop.height() - offset_y).min(request.band_height);
            let reserved = planned_band_bytes(crop.width(), height, request.sources.len())?;
            let _band = memory
                .try_reserve(reserved)
                .map_err(RegisteredStackError::Memory)?;
            let mut images = Vec::new();
            images
                .try_reserve_exact(request.sources.len())
                .map_err(|_| RegisteredStackError::AllocationFailed)?;
            for (index, source) in request.sources.iter().enumerate() {
                let mut reader = open_reader(
                    source.source.path(),
                    PipelineInput::Signal { index },
                    request.header_options,
                    request.validation_mode,
                )
                .map_err(RegisteredStackError::Input)?;
                let region = ImageRegion::new(
                    u64::try_from(plane).map_err(|_| RegisteredStackError::WorkSizeOverflow)?,
                    u64::try_from(crop.x()).map_err(|_| RegisteredStackError::WorkSizeOverflow)?,
                    u64::try_from(crop.y() + offset_y)
                        .map_err(|_| RegisteredStackError::WorkSizeOverflow)?,
                    u64::try_from(crop.width())
                        .map_err(|_| RegisteredStackError::WorkSizeOverflow)?,
                    u64::try_from(height).map_err(|_| RegisteredStackError::WorkSizeOverflow)?,
                );
                images.push(
                    reader
                        .read_region_image(region)
                        .map_err(RegisteredStackError::ReadInput)?,
                );
            }
            let references = images.iter().collect::<Vec<_>>();
            let integrated =
                integrate_mean(&references).map_err(RegisteredStackError::Integration)?;
            writer
                .write_image_chunk(integrated.image())
                .map_err(RegisteredStackError::Publish)?;
            *completed = completed
                .checked_add(1)
                .ok_or(RegisteredStackError::WorkSizeOverflow)?;
            emit(
                sequence,
                stage,
                ProgressState::Running,
                *completed,
                *total,
                None,
                progress,
            )?;
        }
    }
    let staged = writer.finish().map_err(RegisteredStackError::Publish)?;
    validate_staged(&staged, output_dimensions)?;
    *completed = completed
        .checked_add(1)
        .ok_or(RegisteredStackError::WorkSizeOverflow)?;
    emit(
        sequence,
        stage,
        ProgressState::Running,
        *completed,
        *total,
        None,
        progress,
    )?;
    revalidate_sources(request, cancellation)?;
    cancellation
        .checkpoint()
        .map_err(RegisteredStackError::Cancelled)?;
    let summary = staged.publish().map_err(RegisteredStackError::Publish)?;
    *completed = completed
        .checked_add(1)
        .ok_or(RegisteredStackError::WorkSizeOverflow)?;
    Ok(RegisteredStackResult {
        dimensions: output_dimensions,
        summary,
        peak_reserved_bytes: memory.peak(),
    })
}

fn validate_sources(
    request: &RegisteredStackRequest,
    cancellation: &CancellationToken,
) -> Result<usize, RegisteredStackError> {
    let mut planes = None;
    for (index, source) in request.sources.iter().enumerate() {
        cancellation
            .checkpoint()
            .map_err(RegisteredStackError::Cancelled)?;
        let input = PipelineInput::Signal { index };
        verify_source(&source.source, input).map_err(RegisteredStackError::Input)?;
        let mut reader = open_reader(
            source.source.path(),
            input,
            request.header_options,
            request.validation_mode,
        )
        .map_err(RegisteredStackError::Input)?;
        if reader.report().header().string("AETHFID") != Some(source.frame_id.as_str()) {
            return Err(RegisteredStackError::ArtifactIdentityMismatch {
                frame_id: source.frame_id.clone(),
            });
        }
        if reader.report().header().string("AETHPLN") != Some(request.plan.plan_sha256()) {
            return Err(RegisteredStackError::ArtifactPlanMismatch {
                frame_id: source.frame_id.clone(),
            });
        }
        let actual = dimensions_from_axes(input, reader.descriptor().axes())
            .map_err(RegisteredStackError::Input)?;
        let expected = Dimensions::new(
            request.plan.reference_width(),
            request.plan.reference_height(),
            actual.planes(),
        )
        .map_err(|_| RegisteredStackError::WorkSizeOverflow)?;
        if actual != expected {
            return Err(RegisteredStackError::DimensionMismatch {
                frame_id: source.frame_id.clone(),
                expected,
                actual,
            });
        }
        if let Some(expected_planes) = planes
            && expected_planes != actual.planes()
        {
            return Err(RegisteredStackError::PlaneCountMismatch {
                expected: expected_planes,
                actual: actual.planes(),
            });
        }
        planes = Some(actual.planes());
        let checksums = reader
            .verify_checksums()
            .map_err(RegisteredStackError::ReadInput)?;
        if !checksums.is_fully_verified() {
            return Err(RegisteredStackError::InvalidStagedOutput);
        }
    }
    planes.ok_or(RegisteredStackError::SourceSetMismatch)
}

fn revalidate_sources(
    request: &RegisteredStackRequest,
    cancellation: &CancellationToken,
) -> Result<(), RegisteredStackError> {
    for (index, source) in request.sources.iter().enumerate() {
        cancellation
            .checkpoint()
            .map_err(RegisteredStackError::Cancelled)?;
        verify_source(&source.source, PipelineInput::Signal { index })
            .map_err(RegisteredStackError::Input)?;
    }
    Ok(())
}

fn planned_band_bytes(
    width: usize,
    height: usize,
    source_count: usize,
) -> Result<usize, RegisteredStackError> {
    let samples = width
        .checked_mul(height)
        .ok_or(RegisteredStackError::WorkSizeOverflow)?;
    let image = samples
        .checked_mul(size_of::<f64>() + size_of::<PixelFlags>())
        .ok_or(RegisteredStackError::WorkSizeOverflow)?;
    let sources = image
        .checked_mul(source_count)
        .ok_or(RegisteredStackError::WorkSizeOverflow)?;
    let output = image
        .checked_add(
            samples
                .checked_mul(size_of::<PixelSupport>())
                .ok_or(RegisteredStackError::WorkSizeOverflow)?,
        )
        .ok_or(RegisteredStackError::WorkSizeOverflow)?;
    let decode = samples
        .checked_mul(size_of::<SampleStatus>())
        .ok_or(RegisteredStackError::WorkSizeOverflow)?;
    let vector_storage = source_count
        .checked_mul(
            size_of::<aether_core::ScientificImage>() + size_of::<&aether_core::ScientificImage>(),
        )
        .ok_or(RegisteredStackError::WorkSizeOverflow)?;
    sources
        .checked_add(output)
        .and_then(|value| value.checked_add(decode))
        .and_then(|value| value.checked_add(vector_storage))
        .ok_or(RegisteredStackError::WorkSizeOverflow)
}

fn validate_staged(
    staged: &aether_fits::CompletedAtomicFits,
    expected: Dimensions,
) -> Result<(), RegisteredStackError> {
    let file = staged
        .try_clone_for_readback()
        .map_err(RegisteredStackError::Publish)?;
    let mut reader = PrimaryImageReader::open(file, HeaderReadOptions::default())
        .map_err(RegisteredStackError::ReadInput)?;
    let actual = dimensions_from_axes(
        PipelineInput::Signal { index: 0 },
        reader.descriptor().axes(),
    )
    .map_err(RegisteredStackError::Input)?;
    let checksums = reader
        .verify_checksums()
        .map_err(RegisteredStackError::ReadInput)?;
    if actual != expected || !checksums.is_fully_verified() {
        return Err(RegisteredStackError::InvalidStagedOutput);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn emit<F>(
    sequence: &ProgressSequence,
    stage: &StageId,
    state: ProgressState,
    completed: u64,
    total: Option<u64>,
    code: Option<String>,
    progress: &mut F,
) -> Result<(), RegisteredStackError>
where
    F: FnMut(ProgressEvent),
{
    let event = sequence
        .next(stage.clone(), state, completed, total, code)
        .map_err(RegisteredStackError::Progress)?;
    progress(event);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File};
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    use aether_core::ScientificImage;
    use aether_fits::write_f64_primary_atomic_new_with_provenance;
    use aether_registration::{
        AffineTransform, LANCZOS3_RESAMPLING_ALGORITHM_ID, PlannedRegistrationFrame,
    };
    use aether_session::fingerprint_reader;

    use super::*;

    type TestResult = Result<(), Box<dyn Error>>;
    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> std::io::Result<Self> {
            let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "aether-registered-stack-{}-{sequence}",
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

    fn id(digit: char) -> Result<FrameId, aether_review::ReviewError> {
        FrameId::new(digit.to_string().repeat(64))
    }

    fn plan() -> Result<RegistrationPlan, Box<dyn Error>> {
        Ok(RegistrationPlan::new(
            id('a')?,
            12,
            10,
            vec![
                PlannedRegistrationFrame::new(id('a')?, 12, 10, AffineTransform::IDENTITY),
                PlannedRegistrationFrame::new(
                    id('b')?,
                    12,
                    10,
                    AffineTransform::new(1.0, 0.0, 0.0, 1.0, 2.0, 1.0)?,
                ),
            ],
        )?)
    }

    fn registered_source(
        directory: &TestDirectory,
        plan: &RegistrationPlan,
        frame_id: FrameId,
        value: f64,
    ) -> Result<RegisteredStackSource, Box<dyn Error>> {
        let image = ScientificImage::filled(Dimensions::new(12, 10, 3)?, value)?;
        let path = directory
            .0
            .join(format!("registered-{}.fits", frame_id.as_str()));
        let provenance = FitsOutputProvenance::new(
            "a".repeat(64),
            "registered-light",
            LANCZOS3_RESAMPLING_ALGORITHM_ID,
            1,
        )?
        .with_plan_sha256(plan.plan_sha256())?
        .with_frame_id_sha256(frame_id.as_str())?;
        write_f64_primary_atomic_new_with_provenance(&path, &image, &provenance)?;
        let mut file = File::open(&path)?;
        let fingerprint = fingerprint_reader(&mut file)?;
        Ok(RegisteredStackSource::new(
            frame_id,
            PipelineSource::new(path, fingerprint),
        ))
    }

    fn stack_request(directory: &TestDirectory) -> Result<RegisteredStackRequest, Box<dyn Error>> {
        let plan = plan()?;
        let sources = vec![
            registered_source(directory, &plan, id('b')?, 4.0)?,
            registered_source(directory, &plan, id('a')?, 2.0)?,
        ];
        let provenance = FitsOutputProvenance::new(
            "a".repeat(64),
            "registered-stack",
            REGISTERED_CROP_MEAN_ALGORITHM_ID,
            2,
        )?
        .with_plan_sha256(plan.plan_sha256())?;
        Ok(RegisteredStackRequest::new(
            plan,
            sources,
            directory.0.join("integrated-registered.fits"),
            provenance,
        )?
        .with_band_height(2)?)
    }

    #[test]
    fn integrates_only_the_sealed_crop_and_publishes_checksums() -> TestResult {
        let directory = TestDirectory::new()?;
        let request = stack_request(&directory)?;
        let memory = MemoryBudget::new(16 * 1_024 * 1_024)?;
        let mut progress = Vec::new();

        let result = run_registered_stack(&request, &CancellationToken::new(), &memory, |event| {
            progress.push(event)
        })?;

        assert_eq!(result.dimensions(), Dimensions::new(10, 9, 3)?);
        assert!(result.peak_reserved_bytes() <= memory.limit());
        assert_eq!(
            progress.first().map(ProgressEvent::state),
            Some(ProgressState::Started)
        );
        assert_eq!(
            progress.last().map(ProgressEvent::state),
            Some(ProgressState::Completed)
        );
        let file = File::open(request.output())?;
        let mut reader = PrimaryImageReader::open(file, HeaderReadOptions::default())?;
        assert!(reader.verify_checksums()?.is_fully_verified());
        let output = reader.read_region_image(ImageRegion::new(0, 0, 0, 10, 9))?;
        assert!(
            output
                .pixels()
                .iter()
                .all(|value| value.to_bits() == 3.0_f64.to_bits())
        );
        Ok(())
    }

    #[test]
    fn cancellation_and_memory_failure_publish_nothing() -> TestResult {
        let directory = TestDirectory::new()?;
        let request = stack_request(&directory)?;
        let cancellation = CancellationToken::new();
        let _was_first_request = cancellation.cancel();
        assert!(matches!(
            run_registered_stack(
                &request,
                &cancellation,
                &MemoryBudget::new(16 * 1_024 * 1_024)?,
                |_| {},
            ),
            Err(RegisteredStackError::Cancelled(_))
        ));
        assert!(!request.output().exists());

        assert!(matches!(
            run_registered_stack(
                &request,
                &CancellationToken::new(),
                &MemoryBudget::new(1)?,
                |_| {},
            ),
            Err(RegisteredStackError::Memory(_))
        ));
        assert!(!request.output().exists());
        Ok(())
    }

    #[test]
    fn cancellation_between_bands_and_late_source_mutation_publish_nothing() -> TestResult {
        let directory = TestDirectory::new()?;
        let request = stack_request(&directory)?;
        let cancellation = CancellationToken::new();
        let cancellation_error = run_registered_stack(
            &request,
            &cancellation,
            &MemoryBudget::new(16 * 1_024 * 1_024)?,
            |event| {
                if event.state() == ProgressState::Running && event.completed_units() == 1 {
                    let _was_first_request = cancellation.cancel();
                }
            },
        )
        .err()
        .ok_or("mid-run cancellation unexpectedly published a stack")?;
        assert!(matches!(
            cancellation_error,
            RegisteredStackError::Cancelled(_)
        ));
        assert!(!request.output().exists());

        let mutation_directory = TestDirectory::new()?;
        let request = stack_request(&mutation_directory)?;
        let changed_path = request.sources()[0].source().path().to_owned();
        let final_band = 15;
        let error = run_registered_stack(
            &request,
            &CancellationToken::new(),
            &MemoryBudget::new(16 * 1_024 * 1_024)?,
            |event| {
                if event.state() == ProgressState::Running
                    && event.completed_units() == final_band
                    && let Ok(mut file) = fs::OpenOptions::new().append(true).open(&changed_path)
                {
                    let _ignored = file.write_all(&[0]);
                }
            },
        )
        .err()
        .ok_or("mutated source unexpectedly published a stack")?;
        assert!(matches!(error, RegisteredStackError::Input(_)));
        assert!(!request.output().exists());
        Ok(())
    }

    #[test]
    fn rejects_stale_plan_binding_before_output() -> TestResult {
        let directory = TestDirectory::new()?;
        let mut request = stack_request(&directory)?;
        request.plan = RegistrationPlan::new(
            id('a')?,
            12,
            10,
            vec![
                PlannedRegistrationFrame::new(id('a')?, 12, 10, AffineTransform::IDENTITY),
                PlannedRegistrationFrame::new(id('b')?, 12, 10, AffineTransform::IDENTITY),
            ],
        )?;

        assert!(matches!(
            run_registered_stack(
                &request,
                &CancellationToken::new(),
                &MemoryBudget::new(16 * 1_024 * 1_024)?,
                |_| {},
            ),
            Err(RegisteredStackError::ArtifactPlanMismatch { .. })
        ));
        assert!(!request.output().exists());
        Ok(())
    }
}
