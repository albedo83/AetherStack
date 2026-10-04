use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs::{self, File};
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use aether_core::{Dimensions, PixelFlags};
use aether_fits::{
    AtomicF64PrimaryStreamWriter, AtomicFitsWriteError, FitsOutputProvenance, FitsProvenanceError,
    FitsWriteSummary, HeaderReadOptions, ImageReadError, ImageRegion, PrimaryImageReader,
    SampleStatus, ValidationMode,
};
use aether_registration::{
    AffineTransform, LANCZOS3_RESAMPLING_ALGORITHM_ID, Lanczos3BandPlan, Lanczos3SourceWindow,
    ProjectiveLanczos3BandPlan, ProjectiveRegistrationPlan, ProjectiveTransform, RegistrationPlan,
    ResampledBand, ResamplingError, ResamplingStatistics,
};
use aether_review::FrameId;

use crate::pipeline::{dimensions_from_axes, open_reader, verify_source};
use crate::{
    CancellationToken, Cancelled, MemoryBudget, MemoryBudgetError, PipelineInput, PipelineSource,
    ProgressEvent, ProgressEventError, ProgressSequence, ProgressState, StageId, StageIdError,
    StrictPipelineError,
};

const DEFAULT_BAND_HEIGHT: usize = 128;
const REGISTRATION_STAGE_ID: &str = "strict-registration";
const STREAM_WRITER_BUFFER_BYTES: usize = 64 * 1_024;
const MAX_STAGING_DIRECTORY_ATTEMPTS: usize = 128;
static STAGING_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Validated request for one bounded, atomic registration transaction.
#[derive(Clone, Debug)]
pub struct StrictRegistrationRequest {
    source: PipelineSource,
    output: PathBuf,
    provenance: FitsOutputProvenance,
    geometry: RegistrationGeometry,
    output_width: usize,
    output_height: usize,
    band_height: usize,
    header_options: HeaderReadOptions,
    validation_mode: ValidationMode,
    plan_source_dimensions: Option<(usize, usize)>,
    plan_source_frame_id: Option<FrameId>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum RegistrationGeometry {
    Affine(AffineTransform),
    Projective(ProjectiveTransform),
}

impl StrictRegistrationRequest {
    /// Builds a strict request using 128-row output bands.
    pub fn new(
        source: PipelineSource,
        output: PathBuf,
        provenance: FitsOutputProvenance,
        source_to_reference: AffineTransform,
        output_width: usize,
        output_height: usize,
    ) -> Result<Self, RegistrationPipelineError> {
        Self::new_with_geometry(
            source,
            output,
            provenance,
            RegistrationGeometry::Affine(source_to_reference),
            output_width,
            output_height,
        )
    }

    /// Builds a strict request using one canonical projective transform.
    pub fn new_projective(
        source: PipelineSource,
        output: PathBuf,
        provenance: FitsOutputProvenance,
        source_to_reference: ProjectiveTransform,
        output_width: usize,
        output_height: usize,
    ) -> Result<Self, RegistrationPipelineError> {
        Self::new_with_geometry(
            source,
            output,
            provenance,
            RegistrationGeometry::Projective(source_to_reference),
            output_width,
            output_height,
        )
    }

    fn new_with_geometry(
        source: PipelineSource,
        output: PathBuf,
        provenance: FitsOutputProvenance,
        geometry: RegistrationGeometry,
        output_width: usize,
        output_height: usize,
    ) -> Result<Self, RegistrationPipelineError> {
        if provenance.algorithm_id() != LANCZOS3_RESAMPLING_ALGORITHM_ID {
            return Err(RegistrationPipelineError::ProvenanceAlgorithmMismatch);
        }
        if provenance.source_count() != 1 {
            return Err(RegistrationPipelineError::ProvenanceSourceCount {
                actual: provenance.source_count(),
            });
        }
        if provenance.source_sha256() != Some(source.fingerprint().sha256()) {
            return Err(RegistrationPipelineError::ProvenanceSourceMismatch);
        }
        Dimensions::new(output_width, output_height, 1)
            .map_err(RegistrationPipelineError::OutputDimensions)?;
        Ok(Self {
            source,
            output,
            provenance,
            geometry,
            output_width,
            output_height,
            band_height: DEFAULT_BAND_HEIGHT,
            header_options: HeaderReadOptions::default(),
            validation_mode: ValidationMode::Strict,
            plan_source_dimensions: None,
            plan_source_frame_id: None,
        })
    }

    /// Builds a request directly from one immutable multi-frame plan entry.
    ///
    /// The portable path and recorded fingerprint are re-derived into the same
    /// [`FrameId`] used by review. Geometry and output dimensions come only
    /// from the plan, while FITS provenance must carry its exact canonical
    /// digest. This prevents callers from substituting an unreviewed transform.
    pub fn from_plan(
        source: PipelineSource,
        portable_relative_path: &str,
        output: PathBuf,
        provenance: FitsOutputProvenance,
        plan: &RegistrationPlan,
    ) -> Result<Self, RegistrationPipelineError> {
        if provenance.plan_sha256() != Some(plan.plan_sha256()) {
            return Err(RegistrationPipelineError::ProvenancePlanMismatch);
        }
        let frame_id = FrameId::derive(
            portable_relative_path,
            source.fingerprint().byte_length(),
            source.fingerprint().sha256(),
        )
        .map_err(|_| RegistrationPipelineError::PlanSourceIdentityMismatch)?;
        let planned = plan
            .frames()
            .iter()
            .find(|frame| frame.frame_id() == &frame_id)
            .ok_or(RegistrationPipelineError::PlanSourceIdentityMismatch)?;
        let expected_dimensions = (planned.source_width(), planned.source_height());
        let mut request = Self::new(
            source,
            output,
            provenance,
            planned.source_to_reference(),
            plan.reference_width(),
            plan.reference_height(),
        )?;
        request.plan_source_dimensions = Some(expected_dimensions);
        Ok(request)
    }

    /// Builds a request for a calibrated artifact carrying reviewed identity.
    ///
    /// The artifact bytes intentionally differ from the raw reviewed Light, so
    /// identity is read from the `AETHFID` provenance card instead of being
    /// re-derived from the immediate input fingerprint. This is the required
    /// path for registered linear RGB products from color cameras.
    pub fn from_plan_artifact(
        source: PipelineSource,
        reviewed_frame_id: FrameId,
        output: PathBuf,
        provenance: FitsOutputProvenance,
        plan: &RegistrationPlan,
    ) -> Result<Self, RegistrationPipelineError> {
        if provenance.plan_sha256() != Some(plan.plan_sha256()) {
            return Err(RegistrationPipelineError::ProvenancePlanMismatch);
        }
        let planned = plan
            .frames()
            .iter()
            .find(|frame| frame.frame_id() == &reviewed_frame_id)
            .ok_or(RegistrationPipelineError::PlanSourceIdentityMismatch)?;
        let expected_dimensions = (planned.source_width(), planned.source_height());
        let mut request = Self::new(
            source,
            output,
            provenance,
            planned.source_to_reference(),
            plan.reference_width(),
            plan.reference_height(),
        )?;
        request.plan_source_dimensions = Some(expected_dimensions);
        request.plan_source_frame_id = Some(reviewed_frame_id);
        Ok(request)
    }

    /// Builds a projective request for an identity-bound calibrated artifact.
    pub fn from_projective_plan_artifact(
        source: PipelineSource,
        reviewed_frame_id: FrameId,
        output: PathBuf,
        provenance: FitsOutputProvenance,
        plan: &ProjectiveRegistrationPlan,
    ) -> Result<Self, RegistrationPipelineError> {
        if provenance.plan_sha256() != Some(plan.plan_sha256()) {
            return Err(RegistrationPipelineError::ProvenancePlanMismatch);
        }
        let planned = plan
            .frames()
            .iter()
            .find(|frame| frame.frame_id() == &reviewed_frame_id)
            .ok_or(RegistrationPipelineError::PlanSourceIdentityMismatch)?;
        let expected_dimensions = (planned.source_width(), planned.source_height());
        let mut request = Self::new_projective(
            source,
            output,
            provenance,
            planned.source_to_reference(),
            plan.reference_width(),
            plan.reference_height(),
        )?;
        request.plan_source_dimensions = Some(expected_dimensions);
        request.plan_source_frame_id = Some(reviewed_frame_id);
        Ok(request)
    }

    /// Replaces the maximum number of output rows held by one band.
    pub fn with_band_height(
        mut self,
        band_height: usize,
    ) -> Result<Self, RegistrationPipelineError> {
        if band_height == 0 {
            return Err(RegistrationPipelineError::ZeroBandHeight);
        }
        self.band_height = band_height;
        Ok(self)
    }

    /// Replaces FITS header limits and diagnostic acceptance policy.
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

    /// Immutable registered source.
    #[must_use]
    pub const fn source(&self) -> &PipelineSource {
        &self.source
    }

    /// Atomic create-new destination.
    #[must_use]
    pub fn output(&self) -> &Path {
        &self.output
    }
}

/// Result of one completely published registered FITS product.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StrictRegistrationResult {
    dimensions: Dimensions,
    summary: FitsWriteSummary,
    statistics: ResamplingStatistics,
    peak_reserved_bytes: usize,
}

impl StrictRegistrationResult {
    /// Published reference-aligned dimensions.
    #[must_use]
    pub const fn dimensions(&self) -> Dimensions {
        self.dimensions
    }

    /// Exact FITS sample, substitution, byte, and checksum accounting.
    #[must_use]
    pub const fn summary(&self) -> FitsWriteSummary {
        self.summary
    }

    /// Complete interpolation and support accounting.
    #[must_use]
    pub const fn statistics(&self) -> ResamplingStatistics {
        self.statistics
    }

    /// Peak logical working set observed by the shared memory budget.
    #[must_use]
    pub const fn peak_reserved_bytes(&self) -> usize {
        self.peak_reserved_bytes
    }
}

/// One exact source supplied to a whole-plan registration transaction.
#[derive(Clone, Debug)]
pub struct RegistrationPlanSource {
    source: PipelineSource,
    portable_relative_path: Option<String>,
    reviewed_frame_id: Option<FrameId>,
}

impl RegistrationPlanSource {
    /// Associates a fingerprinted source with its portable session path.
    #[must_use]
    pub fn new(source: PipelineSource, portable_relative_path: impl Into<String>) -> Self {
        Self {
            source,
            portable_relative_path: Some(portable_relative_path.into()),
            reviewed_frame_id: None,
        }
    }

    /// Associates a derived calibrated artifact with its reviewed raw Light.
    #[must_use]
    pub const fn from_reviewed_artifact(source: PipelineSource, frame_id: FrameId) -> Self {
        Self {
            source,
            portable_relative_path: None,
            reviewed_frame_id: Some(frame_id),
        }
    }

    /// Fingerprinted local source.
    #[must_use]
    pub const fn source(&self) -> &PipelineSource {
        &self.source
    }

    /// Portable path used to derive the reviewed [`FrameId`].
    #[must_use]
    pub fn portable_relative_path(&self) -> Option<&str> {
        self.portable_relative_path.as_deref()
    }

    fn frame_id(&self) -> Result<FrameId, RegistrationPlanExecutionError> {
        if let Some(frame_id) = &self.reviewed_frame_id {
            return Ok(frame_id.clone());
        }
        FrameId::derive(
            self.portable_relative_path
                .as_deref()
                .ok_or(RegistrationPlanExecutionError::InvalidSourceIdentity)?,
            self.source.fingerprint().byte_length(),
            self.source.fingerprint().sha256(),
        )
        .map_err(|_| RegistrationPlanExecutionError::InvalidSourceIdentity)
    }
}

/// Complete immutable request for a rollback-safe registration plan run.
#[derive(Clone, Debug)]
pub struct RegistrationPlanExecutionRequest {
    plan: RegistrationPlan,
    sources: Vec<RegistrationPlanSource>,
    output_directory: PathBuf,
    manifest_sha256: String,
    group_id: String,
    band_height: usize,
    header_options: HeaderReadOptions,
    validation_mode: ValidationMode,
}

impl RegistrationPlanExecutionRequest {
    /// Creates a request whose public filenames are derived from frame IDs.
    pub fn new(
        plan: RegistrationPlan,
        sources: Vec<RegistrationPlanSource>,
        output_directory: PathBuf,
        manifest_sha256: impl Into<String>,
        group_id: impl Into<String>,
    ) -> Result<Self, RegistrationPlanExecutionError> {
        if sources.len() != plan.frames().len() {
            return Err(RegistrationPlanExecutionError::SourceSetMismatch);
        }
        let manifest_sha256 = manifest_sha256.into();
        let group_id = group_id.into();
        FitsOutputProvenance::new(
            &manifest_sha256,
            &group_id,
            LANCZOS3_RESAMPLING_ALGORITHM_ID,
            1,
        )
        .map_err(RegistrationPlanExecutionError::Provenance)?;
        validate_plan_source_set(&plan, &sources)?;
        Ok(Self {
            plan,
            sources,
            output_directory,
            manifest_sha256,
            group_id,
            band_height: DEFAULT_BAND_HEIGHT,
            header_options: HeaderReadOptions::default(),
            validation_mode: ValidationMode::Strict,
        })
    }

    /// Replaces the maximum number of output rows held by one frame worker.
    pub fn with_band_height(
        mut self,
        band_height: usize,
    ) -> Result<Self, RegistrationPlanExecutionError> {
        if band_height == 0 {
            return Err(RegistrationPlanExecutionError::FramePipeline {
                frame_id: self.plan.reference_frame_id().clone(),
                source: RegistrationPipelineError::ZeroBandHeight,
            });
        }
        self.band_height = band_height;
        Ok(self)
    }

    /// Replaces FITS header limits and diagnostic acceptance policy.
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

    /// Canonical plan executed by this transaction.
    #[must_use]
    pub const fn plan(&self) -> &RegistrationPlan {
        &self.plan
    }

    /// Directory that receives the complete public product set.
    #[must_use]
    pub fn output_directory(&self) -> &Path {
        &self.output_directory
    }
}

/// Progress of one canonical frame within a whole-plan transaction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegistrationPlanProgressEvent {
    frame_index: usize,
    frame_count: usize,
    frame_id: FrameId,
    stage: ProgressEvent,
}

impl RegistrationPlanProgressEvent {
    /// Zero-based position in canonical frame-ID order.
    #[must_use]
    pub const fn frame_index(&self) -> usize {
        self.frame_index
    }

    /// Total number of frames in this transaction.
    #[must_use]
    pub const fn frame_count(&self) -> usize {
        self.frame_count
    }

    /// Stable reviewed identity of the active frame.
    #[must_use]
    pub const fn frame_id(&self) -> &FrameId {
        &self.frame_id
    }

    /// Underlying bounded registration stage event.
    #[must_use]
    pub const fn stage(&self) -> &ProgressEvent {
        &self.stage
    }
}

/// One registered FITS published by a complete plan transaction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegisteredFrameExecutionResult {
    frame_id: FrameId,
    output: PathBuf,
    summary: FitsWriteSummary,
    statistics: ResamplingStatistics,
}

impl RegisteredFrameExecutionResult {
    /// Stable reviewed identity of this product.
    #[must_use]
    pub const fn frame_id(&self) -> &FrameId {
        &self.frame_id
    }

    /// Public create-new FITS path.
    #[must_use]
    pub fn output(&self) -> &Path {
        &self.output
    }

    /// Exact FITS write accounting.
    #[must_use]
    pub const fn summary(&self) -> FitsWriteSummary {
        self.summary
    }

    /// Complete interpolation and support accounting.
    #[must_use]
    pub const fn statistics(&self) -> ResamplingStatistics {
        self.statistics
    }
}

/// Complete canonically ordered result of a registration-plan transaction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegistrationPlanExecutionResult {
    plan_sha256: String,
    frames: Vec<RegisteredFrameExecutionResult>,
    peak_reserved_bytes: usize,
}

impl RegistrationPlanExecutionResult {
    /// Exact canonical plan digest carried by every output.
    #[must_use]
    pub fn plan_sha256(&self) -> &str {
        &self.plan_sha256
    }

    /// Published products in canonical frame-ID order.
    #[must_use]
    pub fn frames(&self) -> &[RegisteredFrameExecutionResult] {
        &self.frames
    }

    /// Peak logical working set observed by the shared memory budget.
    #[must_use]
    pub const fn peak_reserved_bytes(&self) -> usize {
        self.peak_reserved_bytes
    }
}

/// Failure while validating or executing a complete registration plan.
#[derive(Debug)]
pub enum RegistrationPlanExecutionError {
    /// A portable path and fingerprint cannot form a reviewed identity.
    InvalidSourceIdentity,
    /// Supplied identities are missing, duplicated, or absent from the plan.
    SourceSetMismatch,
    /// The output directory is absent or is not a directory.
    InvalidOutputDirectory,
    /// A final create-new destination already exists.
    DestinationExists(PathBuf),
    /// FITS provenance could not be constructed.
    Provenance(FitsProvenanceError),
    /// A private sibling staging directory could not be created.
    CreateStagingDirectory(std::io::Error),
    /// One frame failed before publication.
    FramePipeline {
        /// Reviewed frame that failed.
        frame_id: FrameId,
        /// Strict single-frame failure.
        source: RegistrationPipelineError,
    },
    /// Allocation for bounded transaction bookkeeping failed.
    AllocationFailed,
    /// Execution stopped at a cooperative checkpoint.
    Cancelled(Cancelled),
    /// A staged product could not be linked into the public directory.
    PublishProduct {
        /// Reviewed frame that could not be published.
        frame_id: FrameId,
        /// Atomic hard-link failure.
        source: std::io::Error,
    },
    /// The published directory entry set could not be durably synchronized.
    SyncOutputDirectory(std::io::Error),
    /// A link created by this transaction could not be removed during rollback.
    RollbackPublication {
        /// Public path that remains visible.
        path: PathBuf,
        /// Filesystem rollback failure.
        source: std::io::Error,
    },
}

impl Display for RegistrationPlanExecutionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSourceIdentity => formatter.write_str(
                "registration source path and fingerprint do not form a portable frame identity",
            ),
            Self::SourceSetMismatch => formatter
                .write_str("registration sources do not match the reviewed plan exactly once each"),
            Self::InvalidOutputDirectory => {
                formatter.write_str("registration output directory is absent or invalid")
            }
            Self::DestinationExists(path) => {
                write!(
                    formatter,
                    "registration destination already exists: {}",
                    path.display()
                )
            }
            Self::Provenance(error) => {
                write!(formatter, "invalid registration provenance: {error}")
            }
            Self::CreateStagingDirectory(error) => {
                write!(
                    formatter,
                    "cannot create private registration staging directory: {error}"
                )
            }
            Self::FramePipeline { frame_id, source } => {
                write!(
                    formatter,
                    "registration frame {} failed: {source}",
                    frame_id.as_str()
                )
            }
            Self::AllocationFailed => {
                formatter.write_str("cannot allocate registration transaction bookkeeping")
            }
            Self::Cancelled(error) => Display::fmt(error, formatter),
            Self::PublishProduct { frame_id, source } => write!(
                formatter,
                "cannot publish registration frame {}: {source}",
                frame_id.as_str()
            ),
            Self::SyncOutputDirectory(error) => {
                write!(
                    formatter,
                    "cannot synchronize registration output directory: {error}"
                )
            }
            Self::RollbackPublication { path, source } => write!(
                formatter,
                "cannot roll back published registration path {}: {source}",
                path.display()
            ),
        }
    }
}

impl Error for RegistrationPlanExecutionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Provenance(error) => Some(error),
            Self::CreateStagingDirectory(error)
            | Self::SyncOutputDirectory(error)
            | Self::RollbackPublication { source: error, .. }
            | Self::PublishProduct { source: error, .. } => Some(error),
            Self::FramePipeline { source, .. } => Some(source),
            Self::Cancelled(error) => Some(error),
            Self::InvalidSourceIdentity
            | Self::SourceSetMismatch
            | Self::InvalidOutputDirectory
            | Self::DestinationExists(_)
            | Self::AllocationFailed => None,
        }
    }
}

/// Failure raised by the bounded registration transaction.
#[derive(Debug)]
pub enum RegistrationPipelineError {
    /// Provenance names another numerical algorithm.
    ProvenanceAlgorithmMismatch,
    /// A registered frame must represent exactly one source.
    ProvenanceSourceCount {
        /// Received represented-source count.
        actual: u32,
    },
    /// Provenance does not carry the supplied source's exact SHA-256.
    ProvenanceSourceMismatch,
    /// FITS provenance is not bound to the reviewed registration plan.
    ProvenancePlanMismatch,
    /// Portable path plus fingerprint does not identify a frame in the plan.
    PlanSourceIdentityMismatch,
    /// Decoded source dimensions disagree with the plan entry.
    PlanSourceDimensionsMismatch,
    /// A derived artifact does not carry the reviewed frame identity in its header.
    PlanArtifactIdentityMismatch,
    /// Band height must be positive.
    ZeroBandHeight,
    /// The requested reference canvas violates the shared dimension contract.
    OutputDimensions(aether_core::CoreError),
    /// Shared strict FITS/source validation failed.
    Input(StrictPipelineError),
    /// Derived work-unit or byte accounting overflowed.
    WorkSizeOverflow,
    /// The configured memory budget cannot reserve a planned band peak.
    Memory(MemoryBudgetError),
    /// A planned FITS source window could not be decoded.
    ReadInput(ImageReadError),
    /// Exact Lanczos planning or execution failed.
    Resampling(ResamplingError),
    /// Private FITS construction or atomic publication failed.
    Publish(AtomicFitsWriteError),
    /// Complete private output failed structural or checksum readback.
    InvalidStagedOutput,
    /// Execution stopped at a cooperative checkpoint.
    Cancelled(Cancelled),
    /// The fixed stage identifier unexpectedly failed validation.
    StageId(StageIdError),
    /// A machine-readable progress event violated its invariant.
    Progress(ProgressEventError),
}

impl RegistrationPipelineError {
    const fn code(&self) -> &'static str {
        match self {
            Self::ProvenanceAlgorithmMismatch => "registration-provenance-algorithm",
            Self::ProvenanceSourceCount { .. } => "registration-provenance-count",
            Self::ProvenanceSourceMismatch => "registration-provenance-source",
            Self::ProvenancePlanMismatch => "registration-provenance-plan",
            Self::PlanSourceIdentityMismatch => "registration-plan-source",
            Self::PlanSourceDimensionsMismatch => "registration-plan-dimensions",
            Self::PlanArtifactIdentityMismatch => "registration-plan-artifact-identity",
            Self::ZeroBandHeight => "registration-band-height",
            Self::OutputDimensions(_) => "registration-output-dimensions",
            Self::Input(_) => "registration-input",
            Self::WorkSizeOverflow => "registration-work-size",
            Self::Memory(_) => "registration-memory",
            Self::ReadInput(_) => "registration-read",
            Self::Resampling(_) => "registration-kernel",
            Self::Publish(_) => "registration-publish",
            Self::InvalidStagedOutput => "registration-readback",
            Self::Cancelled(_) => "cancelled",
            Self::StageId(_) => "registration-stage",
            Self::Progress(_) => "registration-progress",
        }
    }
}

impl Display for RegistrationPipelineError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProvenanceAlgorithmMismatch => formatter
                .write_str("registration provenance does not name the strict Lanczos-3 algorithm"),
            Self::ProvenanceSourceCount { actual } => write!(
                formatter,
                "registration provenance must represent one source, received {actual}"
            ),
            Self::ProvenanceSourceMismatch => {
                formatter.write_str("registration provenance is not bound to the exact source")
            }
            Self::ProvenancePlanMismatch => formatter
                .write_str("registration provenance is not bound to the reviewed plan digest"),
            Self::PlanSourceIdentityMismatch => {
                formatter.write_str("registration source identity is absent from the reviewed plan")
            }
            Self::PlanSourceDimensionsMismatch => formatter
                .write_str("registration source dimensions disagree with the reviewed plan"),
            Self::PlanArtifactIdentityMismatch => formatter
                .write_str("registration artifact is not bound to the reviewed frame identity"),
            Self::ZeroBandHeight => {
                formatter.write_str("registration band height must be positive")
            }
            Self::OutputDimensions(error) => {
                write!(formatter, "invalid output dimensions: {error}")
            }
            Self::Input(error) => write!(formatter, "cannot validate registration input: {error}"),
            Self::WorkSizeOverflow => {
                formatter.write_str("registration work size cannot be represented")
            }
            Self::Memory(error) => write!(formatter, "cannot reserve registration memory: {error}"),
            Self::ReadInput(error) => {
                write!(formatter, "cannot read registration source window: {error}")
            }
            Self::Resampling(error) => {
                write!(formatter, "cannot resample registration band: {error}")
            }
            Self::Publish(error) => write!(formatter, "cannot publish registered FITS: {error}"),
            Self::InvalidStagedOutput => {
                formatter.write_str("private registered FITS failed exact readback validation")
            }
            Self::Cancelled(error) => Display::fmt(error, formatter),
            Self::StageId(error) => write!(formatter, "invalid registration stage: {error}"),
            Self::Progress(error) => write!(formatter, "invalid registration progress: {error}"),
        }
    }
}

impl Error for RegistrationPipelineError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::OutputDimensions(error) => Some(error),
            Self::Input(error) => Some(error),
            Self::Memory(error) => Some(error),
            Self::ReadInput(error) => Some(error),
            Self::Resampling(error) => Some(error),
            Self::Publish(error) => Some(error),
            Self::Cancelled(error) => Some(error),
            Self::StageId(error) => Some(error),
            Self::Progress(error) => Some(error),
            Self::ProvenanceAlgorithmMismatch
            | Self::ProvenanceSourceCount { .. }
            | Self::ProvenanceSourceMismatch
            | Self::ProvenancePlanMismatch
            | Self::PlanSourceIdentityMismatch
            | Self::PlanSourceDimensionsMismatch
            | Self::PlanArtifactIdentityMismatch
            | Self::ZeroBandHeight
            | Self::WorkSizeOverflow
            | Self::InvalidStagedOutput => None,
        }
    }
}

/// Executes every reviewed transform as one rollback-safe publication set.
///
/// All sources are identity-checked before calculation. Individual registered
/// FITS files are then built and checksum-validated in a private sibling
/// directory. The complete source set is fingerprinted again before any public
/// link is created. Publication uses create-new hard links, rolls back links
/// made by this transaction on failure, and never replaces an existing path.
pub fn run_registration_plan<F>(
    request: &RegistrationPlanExecutionRequest,
    cancellation: &CancellationToken,
    memory: &MemoryBudget,
    mut progress: F,
) -> Result<RegistrationPlanExecutionResult, RegistrationPlanExecutionError>
where
    F: FnMut(RegistrationPlanProgressEvent),
{
    validate_plan_source_set(&request.plan, &request.sources)?;
    if !request.output_directory.is_dir() {
        return Err(RegistrationPlanExecutionError::InvalidOutputDirectory);
    }
    let ordered = canonical_plan_sources(&request.plan, &request.sources)?;
    let destinations = planned_registration_destinations(&request.output_directory, &ordered)?;
    preflight_registration_destinations(&destinations)?;
    cancellation
        .checkpoint()
        .map_err(RegistrationPlanExecutionError::Cancelled)?;

    let staging = RegistrationStagingDirectory::create(&request.output_directory)?;
    let frame_count = ordered.len();
    let mut staged = BTreeMap::new();
    let mut frames = Vec::new();
    frames
        .try_reserve_exact(frame_count)
        .map_err(|_| RegistrationPlanExecutionError::AllocationFailed)?;

    for (frame_index, (frame_id, source)) in ordered.iter().enumerate() {
        cancellation
            .checkpoint()
            .map_err(RegistrationPlanExecutionError::Cancelled)?;
        let staged_output = staging.path().join(registration_file_name(frame_id));
        let provenance = FitsOutputProvenance::new(
            &request.manifest_sha256,
            &request.group_id,
            LANCZOS3_RESAMPLING_ALGORITHM_ID,
            1,
        )
        .and_then(|value| value.with_plan_sha256(request.plan.plan_sha256()))
        .and_then(|value| value.with_source_sha256(source.source.fingerprint().sha256()))
        .and_then(|value| value.with_frame_id_sha256(frame_id.as_str()))
        .map_err(RegistrationPlanExecutionError::Provenance)?;
        let pipeline = if let Some(reviewed_frame_id) = &source.reviewed_frame_id {
            StrictRegistrationRequest::from_plan_artifact(
                source.source.clone(),
                reviewed_frame_id.clone(),
                staged_output.clone(),
                provenance,
                &request.plan,
            )
        } else {
            StrictRegistrationRequest::from_plan(
                source.source.clone(),
                source
                    .portable_relative_path
                    .as_deref()
                    .ok_or(RegistrationPlanExecutionError::InvalidSourceIdentity)?,
                staged_output.clone(),
                provenance,
                &request.plan,
            )
        }
        .and_then(|value| value.with_band_height(request.band_height))
        .map(|value| value.with_header_policy(request.header_options, request.validation_mode))
        .map_err(|source| RegistrationPlanExecutionError::FramePipeline {
            frame_id: frame_id.clone(),
            source,
        })?;
        let completed =
            run_strict_registration_pipeline(&pipeline, cancellation, memory, |stage| {
                progress(RegistrationPlanProgressEvent {
                    frame_index,
                    frame_count,
                    frame_id: frame_id.clone(),
                    stage,
                });
            })
            .map_err(|source| RegistrationPlanExecutionError::FramePipeline {
                frame_id: frame_id.clone(),
                source,
            })?;
        let output = destinations
            .get(frame_id)
            .cloned()
            .ok_or(RegistrationPlanExecutionError::SourceSetMismatch)?;
        staged.insert(frame_id.clone(), staged_output);
        frames.push(RegisteredFrameExecutionResult {
            frame_id: frame_id.clone(),
            output,
            summary: completed.summary(),
            statistics: completed.statistics(),
        });
    }

    cancellation
        .checkpoint()
        .map_err(RegistrationPlanExecutionError::Cancelled)?;
    revalidate_registration_sources(&ordered)?;
    publish_registration_set(
        &staged,
        &destinations,
        &request.output_directory,
        cancellation,
    )?;
    Ok(RegistrationPlanExecutionResult {
        plan_sha256: request.plan.plan_sha256().to_owned(),
        frames,
        peak_reserved_bytes: memory.peak(),
    })
}

fn validate_plan_source_set(
    plan: &RegistrationPlan,
    sources: &[RegistrationPlanSource],
) -> Result<(), RegistrationPlanExecutionError> {
    if sources.len() != plan.frames().len() {
        return Err(RegistrationPlanExecutionError::SourceSetMismatch);
    }
    let mut identities = BTreeMap::new();
    for source in sources {
        let frame_id = source.frame_id()?;
        if identities.insert(frame_id, ()).is_some() {
            return Err(RegistrationPlanExecutionError::SourceSetMismatch);
        }
    }
    if plan
        .frames()
        .iter()
        .any(|frame| !identities.contains_key(frame.frame_id()))
    {
        return Err(RegistrationPlanExecutionError::SourceSetMismatch);
    }
    Ok(())
}

fn canonical_plan_sources<'a>(
    plan: &RegistrationPlan,
    sources: &'a [RegistrationPlanSource],
) -> Result<Vec<(FrameId, &'a RegistrationPlanSource)>, RegistrationPlanExecutionError> {
    let mut by_id = BTreeMap::new();
    for source in sources {
        if by_id.insert(source.frame_id()?, source).is_some() {
            return Err(RegistrationPlanExecutionError::SourceSetMismatch);
        }
    }
    let mut ordered = Vec::new();
    ordered
        .try_reserve_exact(plan.frames().len())
        .map_err(|_| RegistrationPlanExecutionError::AllocationFailed)?;
    for frame in plan.frames() {
        let source = by_id
            .remove(frame.frame_id())
            .ok_or(RegistrationPlanExecutionError::SourceSetMismatch)?;
        ordered.push((frame.frame_id().clone(), source));
    }
    if !by_id.is_empty() {
        return Err(RegistrationPlanExecutionError::SourceSetMismatch);
    }
    Ok(ordered)
}

fn planned_registration_destinations(
    output_directory: &Path,
    ordered: &[(FrameId, &RegistrationPlanSource)],
) -> Result<BTreeMap<FrameId, PathBuf>, RegistrationPlanExecutionError> {
    let mut destinations = BTreeMap::new();
    for (frame_id, _) in ordered {
        destinations.insert(
            frame_id.clone(),
            output_directory.join(registration_file_name(frame_id)),
        );
    }
    Ok(destinations)
}

fn registration_file_name(frame_id: &FrameId) -> String {
    format!("registered-{}.fits", frame_id.as_str())
}

fn preflight_registration_destinations(
    destinations: &BTreeMap<FrameId, PathBuf>,
) -> Result<(), RegistrationPlanExecutionError> {
    if let Some(path) = destinations.values().find(|path| path.exists()) {
        return Err(RegistrationPlanExecutionError::DestinationExists(
            path.clone(),
        ));
    }
    Ok(())
}

fn revalidate_registration_sources(
    ordered: &[(FrameId, &RegistrationPlanSource)],
) -> Result<(), RegistrationPlanExecutionError> {
    for (index, (frame_id, source)) in ordered.iter().enumerate() {
        verify_source(&source.source, PipelineInput::Signal { index }).map_err(|source| {
            RegistrationPlanExecutionError::FramePipeline {
                frame_id: frame_id.clone(),
                source: RegistrationPipelineError::Input(source),
            }
        })?;
    }
    Ok(())
}

fn publish_registration_set(
    staged: &BTreeMap<FrameId, PathBuf>,
    destinations: &BTreeMap<FrameId, PathBuf>,
    output_directory: &Path,
    cancellation: &CancellationToken,
) -> Result<(), RegistrationPlanExecutionError> {
    let mut published = Vec::new();
    published
        .try_reserve_exact(staged.len())
        .map_err(|_| RegistrationPlanExecutionError::AllocationFailed)?;
    for (frame_id, staged_path) in staged {
        if let Err(cancelled) = cancellation.checkpoint() {
            rollback_registration_publications(&published)?;
            return Err(RegistrationPlanExecutionError::Cancelled(cancelled));
        }
        let destination = destinations
            .get(frame_id)
            .ok_or(RegistrationPlanExecutionError::SourceSetMismatch)?;
        if let Err(source) = fs::hard_link(staged_path, destination) {
            rollback_registration_publications(&published)?;
            return Err(RegistrationPlanExecutionError::PublishProduct {
                frame_id: frame_id.clone(),
                source,
            });
        }
        published.push(destination.clone());
    }
    if let Err(source) = sync_registration_output_directory(output_directory) {
        rollback_registration_publications(&published)?;
        return Err(RegistrationPlanExecutionError::SyncOutputDirectory(source));
    }
    Ok(())
}

fn rollback_registration_publications(
    paths: &[PathBuf],
) -> Result<(), RegistrationPlanExecutionError> {
    for path in paths.iter().rev() {
        fs::remove_file(path).map_err(|source| {
            RegistrationPlanExecutionError::RollbackPublication {
                path: path.clone(),
                source,
            }
        })?;
    }
    Ok(())
}

#[cfg(unix)]
fn sync_registration_output_directory(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_registration_output_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

struct RegistrationStagingDirectory {
    path: PathBuf,
}

impl RegistrationStagingDirectory {
    fn create(output_directory: &Path) -> Result<Self, RegistrationPlanExecutionError> {
        for _ in 0..MAX_STAGING_DIRECTORY_ATTEMPTS {
            let sequence = STAGING_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = output_directory.join(format!(
                ".aether-registration-stage-{}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(RegistrationPlanExecutionError::CreateStagingDirectory(
                        error,
                    ));
                }
            }
        }
        Err(RegistrationPlanExecutionError::CreateStagingDirectory(
            std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "exhausted private registration staging names",
            ),
        ))
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for RegistrationStagingDirectory {
    fn drop(&mut self) {
        let _ignored = fs::remove_dir_all(&self.path);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum RegistrationBandPlan {
    Affine(Lanczos3BandPlan),
    Projective(ProjectiveLanczos3BandPlan),
}

impl RegistrationBandPlan {
    fn new(
        source_dimensions: Dimensions,
        output_width: usize,
        output_height: usize,
        reference_y: usize,
        band_height: usize,
        geometry: RegistrationGeometry,
    ) -> Result<Self, ResamplingError> {
        match geometry {
            RegistrationGeometry::Affine(transform) => Ok(Self::Affine(Lanczos3BandPlan::new(
                source_dimensions,
                output_width,
                output_height,
                reference_y,
                band_height,
                transform,
            )?)),
            RegistrationGeometry::Projective(transform) => {
                Ok(Self::Projective(ProjectiveLanczos3BandPlan::new(
                    source_dimensions,
                    output_width,
                    output_height,
                    reference_y,
                    band_height,
                    transform,
                )?))
            }
        }
    }

    const fn source_window(self) -> Option<Lanczos3SourceWindow> {
        match self {
            Self::Affine(plan) => plan.source_window(),
            Self::Projective(plan) => plan.source_window(),
        }
    }

    const fn output_width(self) -> usize {
        match self {
            Self::Affine(plan) => plan.output_width(),
            Self::Projective(plan) => plan.output_width(),
        }
    }

    const fn band_height(self) -> usize {
        match self {
            Self::Affine(plan) => plan.band_height(),
            Self::Projective(plan) => plan.band_height(),
        }
    }

    fn resample(
        self,
        source_window_image: Option<&aether_core::ScientificImage>,
    ) -> Result<ResampledBand, ResamplingError> {
        match self {
            Self::Affine(plan) => plan.resample(source_window_image),
            Self::Projective(plan) => plan.resample(source_window_image),
        }
    }
}

/// Executes one windowed, cancellable, atomically published registration.
pub fn run_strict_registration_pipeline<F>(
    request: &StrictRegistrationRequest,
    cancellation: &CancellationToken,
    memory: &MemoryBudget,
    mut progress: F,
) -> Result<StrictRegistrationResult, RegistrationPipelineError>
where
    F: FnMut(ProgressEvent),
{
    let stage = StageId::new(REGISTRATION_STAGE_ID).map_err(RegistrationPipelineError::StageId)?;
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
    let mut completed = 0_u64;
    let mut total = None;
    let execution = execute(
        request,
        cancellation,
        memory,
        &sequence,
        &stage,
        &mut completed,
        &mut total,
        &mut progress,
    );
    match execution {
        Ok(result) => {
            emit(
                &sequence,
                &stage,
                ProgressState::Completed,
                completed,
                total,
                None,
                &mut progress,
            )?;
            Ok(result)
        }
        Err(error) => {
            let state = if matches!(error, RegistrationPipelineError::Cancelled(_)) {
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
    request: &StrictRegistrationRequest,
    cancellation: &CancellationToken,
    memory: &MemoryBudget,
    sequence: &ProgressSequence,
    stage: &StageId,
    completed: &mut u64,
    total: &mut Option<u64>,
    progress: &mut F,
) -> Result<StrictRegistrationResult, RegistrationPipelineError>
where
    F: FnMut(ProgressEvent),
{
    cancellation
        .checkpoint()
        .map_err(RegistrationPipelineError::Cancelled)?;
    let input = PipelineInput::Signal { index: 0 };
    verify_source(&request.source, input).map_err(RegistrationPipelineError::Input)?;
    let mut reader = open_reader(
        request.source.path(),
        input,
        request.header_options,
        request.validation_mode,
    )
    .map_err(RegistrationPipelineError::Input)?;
    if let Some(expected_frame_id) = &request.plan_source_frame_id
        && reader.report().header().string("AETHFID") != Some(expected_frame_id.as_str())
    {
        return Err(RegistrationPipelineError::PlanArtifactIdentityMismatch);
    }
    let source_dimensions = dimensions_from_axes(input, reader.descriptor().axes())
        .map_err(RegistrationPipelineError::Input)?;
    if let Some(expected) = request.plan_source_dimensions
        && expected != (source_dimensions.width(), source_dimensions.height())
    {
        return Err(RegistrationPipelineError::PlanSourceDimensionsMismatch);
    }
    let output_dimensions = Dimensions::new(
        request.output_width,
        request.output_height,
        source_dimensions.planes(),
    )
    .map_err(RegistrationPipelineError::OutputDimensions)?;
    let bands_per_plane = request.output_height.div_ceil(request.band_height);
    let work_units = bands_per_plane
        .checked_mul(source_dimensions.planes())
        .and_then(|units| units.checked_add(1))
        .and_then(|units| u64::try_from(units).ok())
        .ok_or(RegistrationPipelineError::WorkSizeOverflow)?;
    *total = Some(work_units);
    emit(
        sequence,
        stage,
        ProgressState::Running,
        *completed,
        *total,
        None,
        progress,
    )?;

    let _writer_reservation = memory
        .try_reserve(STREAM_WRITER_BUFFER_BYTES)
        .map_err(RegistrationPipelineError::Memory)?;
    let mut writer = AtomicF64PrimaryStreamWriter::create_with_provenance(
        &request.output,
        output_dimensions,
        &request.provenance,
    )
    .map_err(RegistrationPipelineError::Publish)?;
    let plane_dimensions =
        Dimensions::new(source_dimensions.width(), source_dimensions.height(), 1)
            .map_err(RegistrationPipelineError::OutputDimensions)?;
    let mut statistics = ResamplingStatistics::default();

    for plane in 0..source_dimensions.planes() {
        for reference_y in (0..request.output_height).step_by(request.band_height) {
            cancellation
                .checkpoint()
                .map_err(RegistrationPipelineError::Cancelled)?;
            let height = (request.output_height - reference_y).min(request.band_height);
            let plan = RegistrationBandPlan::new(
                plane_dimensions,
                request.output_width,
                request.output_height,
                reference_y,
                height,
                request.geometry,
            )
            .map_err(RegistrationPipelineError::Resampling)?;
            let planned_bytes = planned_band_bytes(plan)?;
            let _band_reservation = memory
                .try_reserve(planned_bytes)
                .map_err(RegistrationPipelineError::Memory)?;
            let source_window = plan
                .source_window()
                .map(|window| {
                    let region = ImageRegion::new(
                        u64::try_from(plane)
                            .map_err(|_| RegistrationPipelineError::WorkSizeOverflow)?,
                        u64::try_from(window.x())
                            .map_err(|_| RegistrationPipelineError::WorkSizeOverflow)?,
                        u64::try_from(window.y())
                            .map_err(|_| RegistrationPipelineError::WorkSizeOverflow)?,
                        u64::try_from(window.width())
                            .map_err(|_| RegistrationPipelineError::WorkSizeOverflow)?,
                        u64::try_from(window.height())
                            .map_err(|_| RegistrationPipelineError::WorkSizeOverflow)?,
                    );
                    reader
                        .read_region_image(region)
                        .map_err(RegistrationPipelineError::ReadInput)
                })
                .transpose()?;
            let band = plan
                .resample(source_window.as_ref())
                .map_err(RegistrationPipelineError::Resampling)?;
            statistics = statistics
                .checked_add(band.statistics())
                .map_err(RegistrationPipelineError::Resampling)?;
            writer
                .write_image_chunk(band.image())
                .map_err(RegistrationPipelineError::Publish)?;
            *completed = completed
                .checked_add(1)
                .ok_or(RegistrationPipelineError::WorkSizeOverflow)?;
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

    let staged = writer
        .finish()
        .map_err(RegistrationPipelineError::Publish)?;
    validate_staged_output(&staged, output_dimensions)?;
    cancellation
        .checkpoint()
        .map_err(RegistrationPipelineError::Cancelled)?;
    verify_source(&request.source, input).map_err(RegistrationPipelineError::Input)?;
    *completed = completed
        .checked_add(1)
        .ok_or(RegistrationPipelineError::WorkSizeOverflow)?;
    emit(
        sequence,
        stage,
        ProgressState::Running,
        *completed,
        *total,
        None,
        progress,
    )?;
    cancellation
        .checkpoint()
        .map_err(RegistrationPipelineError::Cancelled)?;
    let summary = staged
        .publish()
        .map_err(RegistrationPipelineError::Publish)?;
    Ok(StrictRegistrationResult {
        dimensions: output_dimensions,
        summary,
        statistics,
        peak_reserved_bytes: memory.peak(),
    })
}

fn planned_band_bytes(plan: RegistrationBandPlan) -> Result<usize, RegistrationPipelineError> {
    let read_samples = plan
        .source_window()
        .map(|window| {
            window
                .width()
                .checked_mul(window.height())
                .ok_or(RegistrationPipelineError::WorkSizeOverflow)
        })
        .transpose()?
        .unwrap_or(0);
    let output_samples = plan
        .output_width()
        .checked_mul(plan.band_height())
        .ok_or(RegistrationPipelineError::WorkSizeOverflow)?;
    let image_sample_bytes = size_of::<f64>() + size_of::<PixelFlags>();
    let read_decode_peak = read_samples
        .checked_mul(image_sample_bytes + size_of::<SampleStatus>())
        .ok_or(RegistrationPipelineError::WorkSizeOverflow)?;
    let kernel_peak = read_samples
        .checked_mul(image_sample_bytes)
        .and_then(|bytes| {
            output_samples
                .checked_mul(image_sample_bytes)
                .and_then(|output| bytes.checked_add(output))
        })
        .ok_or(RegistrationPipelineError::WorkSizeOverflow)?;
    Ok(read_decode_peak.max(kernel_peak).max(1))
}

fn validate_staged_output(
    staged: &aether_fits::CompletedAtomicFits,
    expected: Dimensions,
) -> Result<(), RegistrationPipelineError> {
    let file = staged
        .try_clone_for_readback()
        .map_err(RegistrationPipelineError::Publish)?;
    let mut reader = PrimaryImageReader::open(file, HeaderReadOptions::default())
        .map_err(RegistrationPipelineError::ReadInput)?;
    let actual = dimensions_from_axes(
        PipelineInput::Signal { index: 0 },
        reader.descriptor().axes(),
    )
    .map_err(RegistrationPipelineError::Input)?;
    let checksums = reader
        .verify_checksums()
        .map_err(RegistrationPipelineError::ReadInput)?;
    if actual != expected || !checksums.is_fully_verified() {
        return Err(RegistrationPipelineError::InvalidStagedOutput);
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
) -> Result<(), RegistrationPipelineError>
where
    F: FnMut(ProgressEvent),
{
    let event = sequence
        .next(stage.clone(), state, completed, total, code)
        .map_err(RegistrationPipelineError::Progress)?;
    progress(event);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File};
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    use aether_core::ScientificImage;
    use aether_fits::{write_f64_primary_atomic_new, write_f64_primary_atomic_new_with_provenance};
    use aether_registration::{
        PlannedRegistrationFrame, RegistrationPlan, resample_lanczos3, resample_lanczos3_projective,
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
                "aether-registration-runtime-{}-{sequence}",
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

    fn source_and_provenance(
        directory: &TestDirectory,
    ) -> Result<(PipelineSource, FitsOutputProvenance, ScientificImage), Box<dyn Error>> {
        let dimensions = Dimensions::new(11, 9, 3)?;
        let pixels = (0..dimensions.pixel_count())
            .map(|index| ((index * 41 + 7) % 257) as f64 - 83.25)
            .collect();
        let image = ScientificImage::from_pixels(dimensions, pixels)?;
        let path = directory.0.join("source.fits");
        write_f64_primary_atomic_new(&path, &image)?;
        let mut file = File::open(&path)?;
        let fingerprint = fingerprint_reader(&mut file)?;
        let provenance = FitsOutputProvenance::new(
            "a".repeat(64),
            "registered-light",
            LANCZOS3_RESAMPLING_ALGORITHM_ID,
            1,
        )?
        .with_source_sha256(fingerprint.sha256())?;
        Ok((PipelineSource::new(path, fingerprint), provenance, image))
    }

    fn plan_for_source(
        source: &PipelineSource,
        source_width: usize,
        source_height: usize,
        transform: AffineTransform,
    ) -> Result<RegistrationPlan, Box<dyn Error>> {
        let source_id = FrameId::derive(
            "source.fits",
            source.fingerprint().byte_length(),
            source.fingerprint().sha256(),
        )?;
        let reference_id = FrameId::new("b".repeat(64))?;
        Ok(RegistrationPlan::new(
            reference_id.clone(),
            11,
            9,
            vec![
                PlannedRegistrationFrame::new(source_id, source_width, source_height, transform),
                PlannedRegistrationFrame::new(reference_id, 11, 9, AffineTransform::IDENTITY),
            ],
        )?)
    }

    fn two_source_registration_request(
        directory: &TestDirectory,
    ) -> Result<(RegistrationPlanExecutionRequest, Vec<FrameId>), Box<dyn Error>> {
        let (first, _, _) = source_and_provenance(directory)?;
        let second_path = directory.0.join("reference.fits");
        fs::copy(first.path(), &second_path)?;
        let second = PipelineSource::new(second_path, first.fingerprint().clone());
        let first_id = FrameId::derive(
            "source.fits",
            first.fingerprint().byte_length(),
            first.fingerprint().sha256(),
        )?;
        let second_id = FrameId::derive(
            "reference.fits",
            second.fingerprint().byte_length(),
            second.fingerprint().sha256(),
        )?;
        let plan = RegistrationPlan::new(
            second_id.clone(),
            11,
            9,
            vec![
                PlannedRegistrationFrame::new(first_id.clone(), 11, 9, AffineTransform::IDENTITY),
                PlannedRegistrationFrame::new(second_id.clone(), 11, 9, AffineTransform::IDENTITY),
            ],
        )?;
        let output_directory = directory.0.join("registered");
        fs::create_dir(&output_directory)?;
        let request = RegistrationPlanExecutionRequest::new(
            plan,
            vec![
                RegistrationPlanSource::new(second, "reference.fits"),
                RegistrationPlanSource::new(first, "source.fits"),
            ],
            output_directory,
            "c".repeat(64),
            "light-group",
        )?
        .with_band_height(3)?;
        let mut ids = vec![first_id, second_id];
        ids.sort();
        Ok((request, ids))
    }

    #[test]
    fn registration_plan_publishes_only_the_complete_canonical_set() -> TestResult {
        let directory = TestDirectory::new()?;
        let (request, expected_ids) = two_source_registration_request(&directory)?;
        let mut events = Vec::new();
        let result = run_registration_plan(
            &request,
            &CancellationToken::new(),
            &MemoryBudget::new(2_000_000)?,
            |event| events.push(event),
        )?;

        assert_eq!(result.plan_sha256(), request.plan().plan_sha256());
        assert_eq!(result.frames().len(), 2);
        assert_eq!(
            result
                .frames()
                .iter()
                .map(|frame| frame.frame_id().clone())
                .collect::<Vec<_>>(),
            expected_ids
        );
        assert!(result.frames().iter().all(|frame| frame.output().is_file()));
        assert_eq!(
            events
                .first()
                .map(RegistrationPlanProgressEvent::frame_index),
            Some(0)
        );
        assert_eq!(
            events
                .last()
                .map(RegistrationPlanProgressEvent::frame_index),
            Some(1)
        );
        assert!(events.iter().all(|event| event.frame_count() == 2));
        assert!(
            request
                .output_directory()
                .read_dir()?
                .all(|entry| entry.map_or(true, |entry| !entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with('.')))
        );
        Ok(())
    }

    #[test]
    fn registration_plan_accepts_identity_bound_linear_artifacts() -> TestResult {
        let directory = TestDirectory::new()?;
        let (_, _, image) = source_and_provenance(&directory)?;
        let first_id = FrameId::new("1".repeat(64))?;
        let second_id = FrameId::new("2".repeat(64))?;
        let plan = RegistrationPlan::new(
            first_id.clone(),
            11,
            9,
            vec![
                PlannedRegistrationFrame::new(second_id.clone(), 11, 9, AffineTransform::IDENTITY),
                PlannedRegistrationFrame::new(first_id.clone(), 11, 9, AffineTransform::IDENTITY),
            ],
        )?;
        let mut sources = Vec::new();
        for (index, frame_id) in [first_id, second_id].into_iter().enumerate() {
            let path = directory.0.join(format!("linear-{index}.fits"));
            let provenance =
                FitsOutputProvenance::new("a".repeat(64), "light-group", "linear-rgb-v1", 1)?
                    .with_frame_id_sha256(frame_id.as_str())?;
            write_f64_primary_atomic_new_with_provenance(&path, &image, &provenance)?;
            let mut file = File::open(&path)?;
            let fingerprint = fingerprint_reader(&mut file)?;
            sources.push(RegistrationPlanSource::from_reviewed_artifact(
                PipelineSource::new(path, fingerprint),
                frame_id,
            ));
        }
        let output = directory.0.join("registered-artifacts");
        fs::create_dir(&output)?;
        let request = RegistrationPlanExecutionRequest::new(
            plan,
            sources,
            output,
            "a".repeat(64),
            "light-group",
        )?;

        let result = run_registration_plan(
            &request,
            &CancellationToken::new(),
            &MemoryBudget::new(2_000_000)?,
            |_| {},
        )?;
        assert_eq!(result.frames().len(), 2);
        for frame in result.frames() {
            let reader = PrimaryImageReader::open(
                File::open(frame.output())?,
                HeaderReadOptions::default(),
            )?;
            assert_eq!(
                reader.report().header().string("AETHFID"),
                Some(frame.frame_id().as_str())
            );
        }
        Ok(())
    }

    #[test]
    fn registration_plan_rejects_incomplete_or_preexisting_products_before_work() -> TestResult {
        let directory = TestDirectory::new()?;
        let (request, _) = two_source_registration_request(&directory)?;
        let incomplete = RegistrationPlanExecutionRequest::new(
            request.plan.clone(),
            vec![request.sources[0].clone()],
            request.output_directory.clone(),
            request.manifest_sha256.clone(),
            request.group_id.clone(),
        );
        assert!(matches!(
            incomplete,
            Err(RegistrationPlanExecutionError::SourceSetMismatch)
        ));

        let first_id = request.plan.frames()[0].frame_id();
        let occupied = request
            .output_directory()
            .join(registration_file_name(first_id));
        File::create(&occupied)?;
        let mut events = Vec::new();
        assert!(matches!(
            run_registration_plan(
                &request,
                &CancellationToken::new(),
                &MemoryBudget::new(2_000_000)?,
                |event| events.push(event),
            ),
            Err(RegistrationPlanExecutionError::DestinationExists(path)) if path == occupied
        ));
        assert!(events.is_empty());
        Ok(())
    }

    #[test]
    fn cancellation_after_one_registered_frame_publishes_nothing() -> TestResult {
        let directory = TestDirectory::new()?;
        let (request, _) = two_source_registration_request(&directory)?;
        let cancellation = CancellationToken::new();
        let result = run_registration_plan(
            &request,
            &cancellation,
            &MemoryBudget::new(2_000_000)?,
            |event| {
                if event.frame_index() == 0 && event.stage().state() == ProgressState::Completed {
                    let _was_first_cancellation = cancellation.cancel();
                }
            },
        );

        assert!(matches!(
            result,
            Err(RegistrationPlanExecutionError::Cancelled(_))
        ));
        assert_eq!(request.output_directory().read_dir()?.count(), 0);
        Ok(())
    }

    #[test]
    fn source_mutation_after_staging_blocks_the_complete_plan() -> TestResult {
        let directory = TestDirectory::new()?;
        let (request, _) = two_source_registration_request(&directory)?;
        let first_path = canonical_plan_sources(&request.plan, &request.sources)?[0]
            .1
            .source
            .path()
            .to_path_buf();
        let mut mutated = false;
        let result = run_registration_plan(
            &request,
            &CancellationToken::new(),
            &MemoryBudget::new(2_000_000)?,
            |event| {
                if !mutated
                    && event.frame_index() == 0
                    && event.stage().state() == ProgressState::Completed
                    && let Ok(mut file) = fs::OpenOptions::new().append(true).open(&first_path)
                {
                    mutated = file.write_all(&[0]).is_ok();
                }
            },
        );

        assert!(mutated);
        assert!(matches!(
            result,
            Err(RegistrationPlanExecutionError::FramePipeline {
                source: RegistrationPipelineError::Input(_),
                ..
            })
        ));
        assert_eq!(request.output_directory().read_dir()?.count(), 0);
        Ok(())
    }

    #[test]
    fn plan_bound_request_derives_geometry_and_rejects_stale_identity() -> TestResult {
        let directory = TestDirectory::new()?;
        let (source, provenance, _) = source_and_provenance(&directory)?;
        let transform = AffineTransform::new(1.0, 0.0, 0.0, 1.0, 1.0, 0.0)?;
        let plan = plan_for_source(&source, 11, 9, transform)?;
        assert!(matches!(
            StrictRegistrationRequest::from_plan(
                source.clone(),
                "source.fits",
                directory.0.join("missing-plan.fits"),
                provenance.clone(),
                &plan,
            ),
            Err(RegistrationPipelineError::ProvenancePlanMismatch)
        ));

        let bound_provenance = provenance.clone().with_plan_sha256(plan.plan_sha256())?;
        assert!(matches!(
            StrictRegistrationRequest::from_plan(
                source.clone(),
                "different.fits",
                directory.0.join("wrong-source.fits"),
                bound_provenance.clone(),
                &plan,
            ),
            Err(RegistrationPipelineError::PlanSourceIdentityMismatch)
        ));

        let request = StrictRegistrationRequest::from_plan(
            source,
            "source.fits",
            directory.0.join("bound.fits"),
            bound_provenance,
            &plan,
        )?;
        assert_eq!(request.geometry, RegistrationGeometry::Affine(transform));
        assert_eq!((request.output_width, request.output_height), (11, 9));
        assert_eq!(request.plan_source_dimensions, Some((11, 9)));
        Ok(())
    }

    #[test]
    fn plan_bound_execution_rechecks_decoded_source_dimensions() -> TestResult {
        let directory = TestDirectory::new()?;
        let (source, provenance, _) = source_and_provenance(&directory)?;
        let plan = plan_for_source(&source, 10, 9, AffineTransform::IDENTITY)?;
        let output = directory.0.join("dimension-mismatch.fits");
        let request = StrictRegistrationRequest::from_plan(
            source,
            "source.fits",
            output.clone(),
            provenance.with_plan_sha256(plan.plan_sha256())?,
            &plan,
        )?;

        assert!(matches!(
            run_strict_registration_pipeline(
                &request,
                &CancellationToken::new(),
                &MemoryBudget::new(2_000_000)?,
                |_| {},
            ),
            Err(RegistrationPipelineError::PlanSourceDimensionsMismatch)
        ));
        assert!(!output.exists());
        Ok(())
    }

    #[test]
    fn projective_plan_bound_request_uses_only_sealed_geometry() -> TestResult {
        let directory = TestDirectory::new()?;
        let (source, provenance, _) = source_and_provenance(&directory)?;
        let source_id = FrameId::new("c".repeat(64))?;
        let reference_id = FrameId::new("d".repeat(64))?;
        let transform = ProjectiveTransform::new([
            [0.998, -0.017, 0.35],
            [0.019, 1.001, -0.28],
            [8.0e-5, -5.0e-5, 1.0],
        ])?;
        let plan = ProjectiveRegistrationPlan::new(
            reference_id.clone(),
            11,
            9,
            vec![
                aether_registration::ProjectivePlannedRegistrationFrame::new(
                    source_id.clone(),
                    11,
                    9,
                    transform,
                ),
                aether_registration::ProjectivePlannedRegistrationFrame::new(
                    reference_id,
                    11,
                    9,
                    ProjectiveTransform::IDENTITY,
                ),
            ],
        )?;
        let request = StrictRegistrationRequest::from_projective_plan_artifact(
            source,
            source_id.clone(),
            directory.0.join("projective-plan-bound.fits"),
            provenance.with_plan_sha256(plan.plan_sha256())?,
            &plan,
        )?;

        assert_eq!(
            request.geometry,
            RegistrationGeometry::Projective(transform)
        );
        assert_eq!(request.plan_source_dimensions, Some((11, 9)));
        assert_eq!(request.plan_source_frame_id, Some(source_id));
        Ok(())
    }

    #[test]
    fn reviewed_artifact_requires_matching_embedded_frame_identity() -> TestResult {
        let directory = TestDirectory::new()?;
        let (_, _, image) = source_and_provenance(&directory)?;
        let reviewed_id = FrameId::new("d".repeat(64))?;
        let reference_id = FrameId::new("e".repeat(64))?;
        let plan = RegistrationPlan::new(
            reference_id.clone(),
            11,
            9,
            vec![
                PlannedRegistrationFrame::new(
                    reviewed_id.clone(),
                    11,
                    9,
                    AffineTransform::IDENTITY,
                ),
                PlannedRegistrationFrame::new(reference_id, 11, 9, AffineTransform::IDENTITY),
            ],
        )?;
        let artifact_path = directory.0.join("linear-rgb.fits");
        let artifact_provenance =
            FitsOutputProvenance::new("a".repeat(64), "light-group", "linear-rgb-v1", 1)?
                .with_frame_id_sha256("f".repeat(64))?;
        write_f64_primary_atomic_new_with_provenance(&artifact_path, &image, &artifact_provenance)?;
        let mut file = File::open(&artifact_path)?;
        let fingerprint = fingerprint_reader(&mut file)?;
        let artifact = PipelineSource::new(artifact_path, fingerprint.clone());
        let output = directory.0.join("registered-artifact.fits");
        let output_provenance = FitsOutputProvenance::new(
            "a".repeat(64),
            "light-group",
            LANCZOS3_RESAMPLING_ALGORITHM_ID,
            1,
        )?
        .with_plan_sha256(plan.plan_sha256())?
        .with_source_sha256(fingerprint.sha256())?;
        let request = StrictRegistrationRequest::from_plan_artifact(
            artifact,
            reviewed_id,
            output.clone(),
            output_provenance,
            &plan,
        )?;

        assert!(matches!(
            run_strict_registration_pipeline(
                &request,
                &CancellationToken::new(),
                &MemoryBudget::new(2_000_000)?,
                |_| {},
            ),
            Err(RegistrationPipelineError::PlanArtifactIdentityMismatch)
        ));
        assert!(!output.exists());

        let valid_path = directory.0.join("bound-linear-rgb.fits");
        let valid_provenance =
            FitsOutputProvenance::new("a".repeat(64), "light-group", "linear-rgb-v1", 1)?
                .with_frame_id_sha256(
                    request
                        .plan_source_frame_id
                        .as_ref()
                        .ok_or("missing ID")?
                        .as_str(),
                )?;
        write_f64_primary_atomic_new_with_provenance(&valid_path, &image, &valid_provenance)?;
        let mut file = File::open(&valid_path)?;
        let fingerprint = fingerprint_reader(&mut file)?;
        let valid_output = directory.0.join("registered-bound-artifact.fits");
        let valid_output_provenance = FitsOutputProvenance::new(
            "a".repeat(64),
            "light-group",
            LANCZOS3_RESAMPLING_ALGORITHM_ID,
            1,
        )?
        .with_plan_sha256(plan.plan_sha256())?
        .with_source_sha256(fingerprint.sha256())?;
        let valid_request = StrictRegistrationRequest::from_plan_artifact(
            PipelineSource::new(valid_path, fingerprint),
            request
                .plan_source_frame_id
                .clone()
                .ok_or("missing reviewed identity")?,
            valid_output.clone(),
            valid_output_provenance,
            &plan,
        )?;
        run_strict_registration_pipeline(
            &valid_request,
            &CancellationToken::new(),
            &MemoryBudget::new(2_000_000)?,
            |_| {},
        )?;
        assert!(valid_output.is_file());
        Ok(())
    }

    #[test]
    fn band_height_does_not_change_registered_bytes_or_oracle_pixels() -> TestResult {
        let directory = TestDirectory::new()?;
        let (source, provenance, input) = source_and_provenance(&directory)?;
        let transform = AffineTransform::new(0.998, -0.017, 0.019, 1.001, 0.35, -0.28)?;
        let first_output = directory.0.join("registered-one-row.fits");
        let second_output = directory.0.join("registered-four-rows.fits");
        let first = StrictRegistrationRequest::new(
            source.clone(),
            first_output.clone(),
            provenance.clone(),
            transform,
            11,
            9,
        )?
        .with_band_height(1)?;
        let second = StrictRegistrationRequest::new(
            source,
            second_output.clone(),
            provenance,
            transform,
            11,
            9,
        )?
        .with_band_height(4)?;
        let mut events = Vec::new();
        let result = run_strict_registration_pipeline(
            &first,
            &CancellationToken::new(),
            &MemoryBudget::new(2_000_000)?,
            |event| events.push(event),
        )?;
        run_strict_registration_pipeline(
            &second,
            &CancellationToken::new(),
            &MemoryBudget::new(2_000_000)?,
            |_| {},
        )?;

        assert_eq!(fs::read(&first_output)?, fs::read(&second_output)?);
        assert_eq!(result.dimensions(), Dimensions::new(11, 9, 3)?);
        assert_eq!(result.summary().samples_written(), 297);
        assert_eq!(
            events.first().map(ProgressEvent::state),
            Some(ProgressState::Started)
        );
        assert_eq!(
            events.last().map(ProgressEvent::state),
            Some(ProgressState::Completed)
        );
        let oracle = resample_lanczos3(&input, 11, 9, transform)?;
        assert_eq!(result.statistics(), oracle.statistics());
        let file = File::open(first_output)?;
        let mut reader = PrimaryImageReader::open(file, HeaderReadOptions::default())?;
        let plane_area = 11 * 9;
        for plane in 0..3 {
            let actual = reader.read_region_image(ImageRegion::new(plane, 0, 0, 11, 9))?;
            for (&actual, &expected) in actual.pixels().iter().zip(
                &oracle.image().pixels()
                    [plane as usize * plane_area..(plane as usize + 1) * plane_area],
            ) {
                assert_eq!(actual.to_bits(), expected.to_bits());
            }
        }
        Ok(())
    }

    #[test]
    fn projective_pipeline_matches_oracle_and_is_band_height_independent() -> TestResult {
        let directory = TestDirectory::new()?;
        let (source, provenance, input) = source_and_provenance(&directory)?;
        let transform = ProjectiveTransform::new([
            [0.998, -0.017, 0.35],
            [0.019, 1.001, -0.28],
            [8.0e-5, -5.0e-5, 1.0],
        ])?;
        let one_row_output = directory.0.join("projective-one-row.fits");
        let four_row_output = directory.0.join("projective-four-rows.fits");
        let one_row = StrictRegistrationRequest::new_projective(
            source.clone(),
            one_row_output.clone(),
            provenance.clone(),
            transform,
            11,
            9,
        )?
        .with_band_height(1)?;
        let four_rows = StrictRegistrationRequest::new_projective(
            source,
            four_row_output.clone(),
            provenance,
            transform,
            11,
            9,
        )?
        .with_band_height(4)?;

        let result = run_strict_registration_pipeline(
            &one_row,
            &CancellationToken::new(),
            &MemoryBudget::new(2_000_000)?,
            |_| {},
        )?;
        run_strict_registration_pipeline(
            &four_rows,
            &CancellationToken::new(),
            &MemoryBudget::new(2_000_000)?,
            |_| {},
        )?;

        assert_eq!(fs::read(&one_row_output)?, fs::read(&four_row_output)?);
        let oracle = resample_lanczos3_projective(&input, 11, 9, transform)?;
        assert_eq!(result.statistics(), oracle.statistics());
        let mut reader =
            PrimaryImageReader::open(File::open(one_row_output)?, HeaderReadOptions::default())?;
        let plane_area = 11 * 9;
        for plane in 0..3 {
            let actual = reader.read_region_image(ImageRegion::new(plane, 0, 0, 11, 9))?;
            for (&actual, &expected) in actual.pixels().iter().zip(
                &oracle.image().pixels()
                    [plane as usize * plane_area..(plane as usize + 1) * plane_area],
            ) {
                assert_eq!(actual.to_bits(), expected.to_bits());
            }
        }
        Ok(())
    }

    #[test]
    fn cancellation_and_memory_failure_publish_nothing() -> TestResult {
        let directory = TestDirectory::new()?;
        let (source, provenance, _) = source_and_provenance(&directory)?;
        let cancelled_output = directory.0.join("cancelled.fits");
        let cancelled = StrictRegistrationRequest::new(
            source.clone(),
            cancelled_output.clone(),
            provenance.clone(),
            AffineTransform::IDENTITY,
            11,
            9,
        )?;
        let token = CancellationToken::new();
        assert!(token.cancel());
        assert!(matches!(
            run_strict_registration_pipeline(
                &cancelled,
                &token,
                &MemoryBudget::new(1_000_000)?,
                |_| {}
            ),
            Err(RegistrationPipelineError::Cancelled(_))
        ));
        assert!(!cancelled_output.exists());

        let memory_output = directory.0.join("memory.fits");
        let memory_request = StrictRegistrationRequest::new(
            source,
            memory_output.clone(),
            provenance,
            AffineTransform::IDENTITY,
            11,
            9,
        )?;
        assert!(matches!(
            run_strict_registration_pipeline(
                &memory_request,
                &CancellationToken::new(),
                &MemoryBudget::new(1)?,
                |_| {}
            ),
            Err(RegistrationPipelineError::Memory(_))
        ));
        assert!(!memory_output.exists());
        Ok(())
    }

    #[test]
    fn request_rejects_incoherent_provenance_dimensions_and_band_height() -> TestResult {
        let directory = TestDirectory::new()?;
        let (source, provenance, _) = source_and_provenance(&directory)?;
        let wrong = FitsOutputProvenance::new("a".repeat(64), "registered", "another-v1", 1)?
            .with_source_sha256(source.fingerprint().sha256())?;
        assert!(matches!(
            StrictRegistrationRequest::new(
                source.clone(),
                directory.0.join("wrong.fits"),
                wrong,
                AffineTransform::IDENTITY,
                11,
                9
            ),
            Err(RegistrationPipelineError::ProvenanceAlgorithmMismatch)
        ));
        assert!(matches!(
            StrictRegistrationRequest::new(
                source.clone(),
                directory.0.join("zero.fits"),
                provenance.clone(),
                AffineTransform::IDENTITY,
                0,
                9
            ),
            Err(RegistrationPipelineError::OutputDimensions(_))
        ));
        let valid = StrictRegistrationRequest::new(
            source,
            directory.0.join("valid.fits"),
            provenance,
            AffineTransform::IDENTITY,
            11,
            9,
        )?;
        assert!(matches!(
            valid.with_band_height(0),
            Err(RegistrationPipelineError::ZeroBandHeight)
        ));
        Ok(())
    }

    #[test]
    fn source_mutation_before_publication_is_detected() -> TestResult {
        let directory = TestDirectory::new()?;
        let (source, provenance, _) = source_and_provenance(&directory)?;
        let source_path = source.path().to_path_buf();
        let output = directory.0.join("mutated.fits");
        let request = StrictRegistrationRequest::new(
            source,
            output.clone(),
            provenance,
            AffineTransform::IDENTITY,
            11,
            9,
        )?
        .with_band_height(2)?;
        let mut changed = false;

        let result = run_strict_registration_pipeline(
            &request,
            &CancellationToken::new(),
            &MemoryBudget::new(2_000_000)?,
            |event| {
                if !changed
                    && event.state() == ProgressState::Running
                    && event.completed_units() == 1
                    && let Ok(mut file) = fs::OpenOptions::new().append(true).open(&source_path)
                {
                    changed = file.write_all(&[0]).is_ok();
                }
            },
        );

        assert!(changed);
        assert!(matches!(result, Err(RegistrationPipelineError::Input(_))));
        assert!(!output.exists());
        Ok(())
    }

    #[test]
    fn planned_memory_depends_on_band_height_not_full_image_height() -> TestResult {
        let short_dimensions = Dimensions::new(4_144, 128, 1)?;
        let asi_dimensions = Dimensions::new(4_144, 2_822, 1)?;
        let taller_dimensions = Dimensions::new(4_144, 20_000, 1)?;
        let short = planned_band_bytes(RegistrationBandPlan::Affine(Lanczos3BandPlan::new(
            short_dimensions,
            4_144,
            128,
            0,
            64,
            AffineTransform::IDENTITY,
        )?))?;
        let asi294 = planned_band_bytes(RegistrationBandPlan::Affine(Lanczos3BandPlan::new(
            asi_dimensions,
            4_144,
            2_822,
            0,
            64,
            AffineTransform::IDENTITY,
        )?))?;
        let taller = planned_band_bytes(RegistrationBandPlan::Affine(Lanczos3BandPlan::new(
            taller_dimensions,
            4_144,
            20_000,
            0,
            64,
            AffineTransform::IDENTITY,
        )?))?;
        assert_eq!(short, asi294);
        assert_eq!(asi294, taller);
        assert!(asi294 < 6 * 1_024 * 1_024);
        Ok(())
    }
}
