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
use aether_localnorm::{
    ApplicationError, LOCAL_APPLICATION_ALGORITHM_ID, LocalApplicationEvidence, LocalCellFit,
    LocalFitError, LocalNormalizationParameters, LocalNormalizationPlan, PlanError,
    ProtectionError, SamplingError, SurfaceError, apply_local_surface_band, build_local_surface,
    build_protection_mask, fit_local_grid, protected_sources_from_stars, sample_local_grid,
};
use aether_quality::{FrameQualityError, measure_frame_quality};

use crate::pipeline::{dimensions_from_axes, open_reader, verify_source};
use crate::{
    CancellationToken, Cancelled, MemoryBudget, MemoryBudgetError, PipelineInput, PipelineSource,
    ProgressEvent, ProgressEventError, ProgressSequence, ProgressState, StageId, StageIdError,
    StrictPipelineError,
};

const STREAM_WRITER_BUFFER_BYTES: usize = 64 * 1_024;
const APPLICATION_BAND_HEIGHT: usize = 128;
const LOCAL_NORMALIZATION_STAGE_ID: &str = "local-normalization";

/// A source/reference local-normalization transaction sealed before FITS I/O.
#[derive(Clone, Debug)]
pub struct LocalNormalizationRequest {
    source: PipelineSource,
    reference: PipelineSource,
    output: PathBuf,
    provenance: FitsOutputProvenance,
    parameters: LocalNormalizationParameters,
    plan: LocalNormalizationPlan,
    header_options: HeaderReadOptions,
    validation_mode: ValidationMode,
}

impl LocalNormalizationRequest {
    /// Validates every identity that will be attached to the output product.
    ///
    /// The plan must bind the supplied source and reference byte fingerprints.
    /// FITS provenance must name the local-application algorithm, represent two
    /// inputs, and carry the exact canonical plan and parameter digests. The
    /// destination is reserved for a later create-new atomic publication and
    /// therefore never participates in scientific identity.
    ///
    /// # Errors
    ///
    /// Returns a typed mismatch before any source file is opened or output is
    /// created.
    pub fn new(
        source: PipelineSource,
        reference: PipelineSource,
        output: PathBuf,
        provenance: FitsOutputProvenance,
        parameters: LocalNormalizationParameters,
        plan: LocalNormalizationPlan,
    ) -> Result<Self, LocalNormalizationRequestError> {
        if source.fingerprint().sha256() != plan.source_sha256() {
            return Err(LocalNormalizationRequestError::SourcePlanMismatch);
        }
        if reference.fingerprint().sha256() != plan.reference_sha256() {
            return Err(LocalNormalizationRequestError::ReferencePlanMismatch);
        }
        let parameters_sha256 = parameters
            .canonical_sha256()
            .map_err(LocalNormalizationRequestError::Plan)?;
        if parameters_sha256 != plan.parameters_sha256() {
            return Err(LocalNormalizationRequestError::ParametersPlanMismatch);
        }
        if provenance.algorithm_id() != LOCAL_APPLICATION_ALGORITHM_ID {
            return Err(LocalNormalizationRequestError::ProvenanceAlgorithmMismatch);
        }
        if provenance.source_count() != 2 {
            return Err(LocalNormalizationRequestError::ProvenanceSourceCount {
                actual: provenance.source_count(),
            });
        }
        if provenance.plan_sha256() != Some(plan.plan_sha256()) {
            return Err(LocalNormalizationRequestError::ProvenancePlanMismatch);
        }
        if provenance.parameters_sha256() != Some(plan.parameters_sha256()) {
            return Err(LocalNormalizationRequestError::ProvenanceParametersMismatch);
        }
        Ok(Self {
            source,
            reference,
            output,
            provenance,
            parameters,
            plan,
            header_options: HeaderReadOptions::default(),
            validation_mode: ValidationMode::Strict,
        })
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

    /// Immutable image whose background and scale will be transformed.
    #[must_use]
    pub const fn source(&self) -> &PipelineSource {
        &self.source
    }

    /// Immutable registered image defining the target background and scale.
    #[must_use]
    pub const fn reference(&self) -> &PipelineSource {
        &self.reference
    }

    /// Atomic create-new destination.
    #[must_use]
    pub fn output(&self) -> &Path {
        &self.output
    }

    /// Provenance cards already cross-checked against the complete plan.
    #[must_use]
    pub const fn provenance(&self) -> &FitsOutputProvenance {
        &self.provenance
    }

    /// Exact scientific controls bound by the plan.
    #[must_use]
    pub const fn parameters(&self) -> LocalNormalizationParameters {
        self.parameters
    }

    /// Immutable identity of inputs, algorithms, and controls.
    #[must_use]
    pub const fn plan(&self) -> &LocalNormalizationPlan {
        &self.plan
    }

    /// FITS header resource limits selected for both inputs.
    #[must_use]
    pub const fn header_options(&self) -> HeaderReadOptions {
        self.header_options
    }

    /// Diagnostic acceptance policy selected for both inputs.
    #[must_use]
    pub const fn validation_mode(&self) -> ValidationMode {
        self.validation_mode
    }
}

/// Failure to construct a provenance-safe local-normalization transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalNormalizationRequestError {
    /// The image fingerprint differs from the image digest sealed in the plan.
    SourcePlanMismatch,
    /// The reference fingerprint differs from the reference digest in the plan.
    ReferencePlanMismatch,
    /// Re-encoding the supplied controls does not reproduce the plan digest.
    ParametersPlanMismatch,
    /// Provenance names a different scientific algorithm.
    ProvenanceAlgorithmMismatch,
    /// Local normalization must represent one source and one reference.
    ProvenanceSourceCount {
        /// Received source count.
        actual: u32,
    },
    /// Provenance does not carry the exact execution-plan digest.
    ProvenancePlanMismatch,
    /// Provenance does not carry the exact parameter digest.
    ProvenanceParametersMismatch,
    /// Canonical parameter encoding failed.
    Plan(PlanError),
}

impl Display for LocalNormalizationRequestError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SourcePlanMismatch => {
                formatter.write_str("local-normalization source does not match its sealed plan")
            }
            Self::ReferencePlanMismatch => {
                formatter.write_str("local-normalization reference does not match its sealed plan")
            }
            Self::ParametersPlanMismatch => formatter.write_str(
                "local-normalization parameters do not reproduce the sealed plan digest",
            ),
            Self::ProvenanceAlgorithmMismatch => formatter
                .write_str("local-normalization provenance names a different scientific algorithm"),
            Self::ProvenanceSourceCount { actual } => write!(
                formatter,
                "local-normalization provenance must represent two sources, received {actual}"
            ),
            Self::ProvenancePlanMismatch => formatter
                .write_str("local-normalization provenance does not carry the sealed plan digest"),
            Self::ProvenanceParametersMismatch => formatter.write_str(
                "local-normalization provenance does not carry the sealed parameter digest",
            ),
            Self::Plan(error) => write!(
                formatter,
                "cannot encode local-normalization parameters: {error}"
            ),
        }
    }
}

impl Error for LocalNormalizationRequestError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Plan(error) => Some(error),
            _ => None,
        }
    }
}

/// One plane-specific surface control retained for diagnostics.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocalNormalizationControlPoint {
    plane: usize,
    x: f64,
    y: f64,
    scale: f64,
    offset: f64,
    median_absolute_residual: f64,
}

/// One accepted or rejected grid-cell fit retained in canonical order.
#[derive(Clone, Debug, PartialEq)]
pub struct LocalNormalizationCellDiagnostic {
    plane: usize,
    fit: LocalCellFit,
}

impl LocalNormalizationCellDiagnostic {
    /// Zero-based FITS plane.
    #[must_use]
    pub const fn plane(&self) -> usize {
        self.plane
    }
    /// Bounds, sampling support, and accepted model or typed rejection.
    #[must_use]
    pub const fn fit(&self) -> &LocalCellFit {
        &self.fit
    }
}

impl LocalNormalizationControlPoint {
    /// Zero-based FITS plane.
    #[must_use]
    pub const fn plane(self) -> usize {
        self.plane
    }
    /// Horizontal source-pixel coordinate.
    #[must_use]
    pub const fn x(self) -> f64 {
        self.x
    }
    /// Vertical source-pixel coordinate.
    #[must_use]
    pub const fn y(self) -> f64 {
        self.y
    }
    /// Local multiplier.
    #[must_use]
    pub const fn scale(self) -> f64 {
        self.scale
    }
    /// Local offset.
    #[must_use]
    pub const fn offset(self) -> f64 {
        self.offset
    }
    /// Median absolute residual in reference units.
    #[must_use]
    pub const fn median_absolute_residual(self) -> f64 {
        self.median_absolute_residual
    }
}

/// Exact evidence retained after one atomic local-normalization publication.
#[derive(Clone, Debug, PartialEq)]
pub struct LocalNormalizationResult {
    dimensions: Dimensions,
    summary: FitsWriteSummary,
    application: LocalApplicationEvidence,
    measured_sources: usize,
    protected_pixels: usize,
    valid_control_points: usize,
    rejected_cells: usize,
    control_points: Vec<LocalNormalizationControlPoint>,
    cell_diagnostics: Vec<LocalNormalizationCellDiagnostic>,
    peak_reserved_bytes: usize,
}

impl LocalNormalizationResult {
    /// Dimensions preserved from the registered source and reference.
    #[must_use]
    pub const fn dimensions(&self) -> Dimensions {
        self.dimensions
    }

    /// Exact FITS sample, substitution, byte, and checksum accounting.
    #[must_use]
    pub const fn summary(&self) -> FitsWriteSummary {
        self.summary
    }

    /// Mutually exclusive accounting for every output sample.
    #[must_use]
    pub const fn application(&self) -> LocalApplicationEvidence {
        self.application
    }

    /// Stellar measurements retained across every plane.
    #[must_use]
    pub const fn measured_sources(&self) -> usize {
        self.measured_sources
    }

    /// Sum of plane-specific protected pixels.
    #[must_use]
    pub const fn protected_pixels(&self) -> usize {
        self.protected_pixels
    }

    /// Successful cell models retained by all coefficient surfaces.
    #[must_use]
    pub const fn valid_control_points(&self) -> usize {
        self.valid_control_points
    }

    /// Cell fits excluded from all coefficient surfaces.
    #[must_use]
    pub const fn rejected_cells(&self) -> usize {
        self.rejected_cells
    }

    /// Plane-specific controls in deterministic plane and grid order.
    #[must_use]
    pub fn control_points(&self) -> &[LocalNormalizationControlPoint] {
        &self.control_points
    }

    /// Complete accepted and rejected grid in deterministic plane-major order.
    #[must_use]
    pub fn cell_diagnostics(&self) -> &[LocalNormalizationCellDiagnostic] {
        &self.cell_diagnostics
    }

    /// Peak logical working set observed by the shared memory budget.
    #[must_use]
    pub const fn peak_reserved_bytes(&self) -> usize {
        self.peak_reserved_bytes
    }
}

/// Failure raised by a complete local-normalization transaction.
#[derive(Debug)]
pub enum LocalNormalizationPipelineError {
    /// Shared strict source or FITS validation failed.
    Input(StrictPipelineError),
    /// Registered source and reference dimensions differ.
    DimensionMismatch {
        /// Source dimensions.
        source: Dimensions,
        /// Reference dimensions.
        reference: Dimensions,
    },
    /// Checked memory or work accounting overflowed.
    WorkSizeOverflow,
    /// The configured memory budget cannot cover the complete planned peak.
    Memory(MemoryBudgetError),
    /// A registered image plane could not be decoded.
    ReadInput(ImageReadError),
    /// Deterministic background or stellar measurement failed.
    Quality(FrameQualityError),
    /// Stellar protection construction failed.
    Protection(ProtectionError),
    /// Deterministic grid sampling failed.
    Sampling(SamplingError),
    /// Robust local fitting failed before cell evidence could be retained.
    Fitting(LocalFitError),
    /// A guarded coefficient surface could not be built.
    Surface(SurfaceError),
    /// Full-image application failed.
    Application(ApplicationError),
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

impl LocalNormalizationPipelineError {
    const fn code(&self) -> &'static str {
        match self {
            Self::Input(_) => "localnorm-input",
            Self::DimensionMismatch { .. } => "localnorm-dimensions",
            Self::WorkSizeOverflow => "localnorm-work-size",
            Self::Memory(_) => "localnorm-memory",
            Self::ReadInput(_) => "localnorm-read",
            Self::Quality(_) => "localnorm-quality",
            Self::Protection(_) => "localnorm-protection",
            Self::Sampling(_) => "localnorm-sampling",
            Self::Fitting(_) => "localnorm-fitting",
            Self::Surface(_) => "localnorm-surface",
            Self::Application(_) => "localnorm-application",
            Self::Publish(_) => "localnorm-publish",
            Self::InvalidStagedOutput => "localnorm-readback",
            Self::Cancelled(_) => "cancelled",
            Self::StageId(_) => "localnorm-stage",
            Self::Progress(_) => "localnorm-progress",
        }
    }
}

impl Display for LocalNormalizationPipelineError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Input(error) => {
                write!(
                    formatter,
                    "cannot validate local-normalization input: {error}"
                )
            }
            Self::DimensionMismatch { source, reference } => write!(
                formatter,
                "local-normalization dimensions differ: source {source:?}, reference {reference:?}"
            ),
            Self::WorkSizeOverflow => {
                formatter.write_str("local-normalization work size cannot be represented")
            }
            Self::Memory(error) => {
                write!(
                    formatter,
                    "cannot reserve local-normalization memory: {error}"
                )
            }
            Self::ReadInput(error) => {
                write!(formatter, "cannot read local-normalization input: {error}")
            }
            Self::Quality(error) => {
                write!(
                    formatter,
                    "cannot measure local-normalization sources: {error}"
                )
            }
            Self::Protection(error) => {
                write!(
                    formatter,
                    "cannot protect local-normalization sources: {error}"
                )
            }
            Self::Sampling(error) => {
                write!(formatter, "cannot sample local-normalization grid: {error}")
            }
            Self::Fitting(error) => {
                write!(formatter, "cannot fit local-normalization grid: {error}")
            }
            Self::Surface(error) => {
                write!(
                    formatter,
                    "cannot build local-normalization surface: {error}"
                )
            }
            Self::Application(error) => {
                write!(
                    formatter,
                    "cannot apply local-normalization surface: {error}"
                )
            }
            Self::Publish(error) => {
                write!(formatter, "cannot publish local-normalized FITS: {error}")
            }
            Self::InvalidStagedOutput => formatter
                .write_str("private local-normalized FITS failed exact readback validation"),
            Self::Cancelled(error) => Display::fmt(error, formatter),
            Self::StageId(error) => write!(formatter, "invalid local-normalization stage: {error}"),
            Self::Progress(error) => {
                write!(formatter, "invalid local-normalization progress: {error}")
            }
        }
    }
}

impl Error for LocalNormalizationPipelineError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Input(error) => Some(error),
            Self::Memory(error) => Some(error),
            Self::ReadInput(error) => Some(error),
            Self::Quality(error) => Some(error),
            Self::Protection(error) => Some(error),
            Self::Sampling(error) => Some(error),
            Self::Fitting(error) => Some(error),
            Self::Surface(error) => Some(error),
            Self::Application(error) => Some(error),
            Self::Publish(error) => Some(error),
            Self::Cancelled(error) => Some(error),
            Self::StageId(error) => Some(error),
            Self::Progress(error) => Some(error),
            Self::DimensionMismatch { .. } | Self::WorkSizeOverflow | Self::InvalidStagedOutput => {
                None
            }
        }
    }
}

/// Executes one verified, bounded, cancellable, atomic normalization product.
///
/// Source and reference fingerprints are checked before decoding and again
/// after private-output readback. Each plane independently measures stellar
/// structure, protects it from fitting, builds a guarded coefficient surface,
/// and contributes exact evidence. Cancellation or any failure before the final
/// create-new publication leaves the destination absent.
///
/// This initial executor reserves its complete full-image peak up front. A
/// later banded implementation may reduce that peak but must reproduce identical
/// scientific pixels and provenance.
///
/// # Errors
///
/// Returns a typed source-integrity, FITS, memory, scientific, cancellation,
/// readback, or atomic-publication failure.
pub fn run_local_normalization(
    request: &LocalNormalizationRequest,
    cancellation: &CancellationToken,
    memory: &MemoryBudget,
) -> Result<LocalNormalizationResult, LocalNormalizationPipelineError> {
    run_local_normalization_with_progress(request, cancellation, memory, |_| {})
}

/// Executes local normalization while delivering validated lifecycle events.
///
/// Events use a stable stage identifier and strictly increasing sequence. Once
/// dimensions are known, the total covers complete input decoding, one unit per
/// image plane, private output validation, and atomic publication.
///
/// # Errors
///
/// Returns every error documented by [`run_local_normalization`], plus typed
/// stage and progress invariant failures.
pub fn run_local_normalization_with_progress<F>(
    request: &LocalNormalizationRequest,
    cancellation: &CancellationToken,
    memory: &MemoryBudget,
    mut progress: F,
) -> Result<LocalNormalizationResult, LocalNormalizationPipelineError>
where
    F: FnMut(ProgressEvent),
{
    let stage = StageId::new(LOCAL_NORMALIZATION_STAGE_ID)
        .map_err(LocalNormalizationPipelineError::StageId)?;
    let sequence = ProgressSequence::new();
    emit_progress(
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
    let execution = execute_local_normalization(
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
            emit_progress(
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
            let state = if matches!(error, LocalNormalizationPipelineError::Cancelled(_)) {
                ProgressState::Cancelled
            } else {
                ProgressState::Failed
            };
            let _ignored = emit_progress(
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
fn execute_local_normalization<F>(
    request: &LocalNormalizationRequest,
    cancellation: &CancellationToken,
    memory: &MemoryBudget,
    sequence: &ProgressSequence,
    stage: &StageId,
    completed: &mut u64,
    total: &mut Option<u64>,
    progress: &mut F,
) -> Result<LocalNormalizationResult, LocalNormalizationPipelineError>
where
    F: FnMut(ProgressEvent),
{
    cancellation
        .checkpoint()
        .map_err(LocalNormalizationPipelineError::Cancelled)?;
    let source_role = PipelineInput::Signal { index: 0 };
    let reference_role = PipelineInput::Signal { index: 1 };
    verify_source(request.source(), source_role).map_err(LocalNormalizationPipelineError::Input)?;
    verify_source(request.reference(), reference_role)
        .map_err(LocalNormalizationPipelineError::Input)?;

    let mut source_reader = open_reader(
        request.source().path(),
        source_role,
        request.header_options(),
        request.validation_mode(),
    )
    .map_err(LocalNormalizationPipelineError::Input)?;
    let mut reference_reader = open_reader(
        request.reference().path(),
        reference_role,
        request.header_options(),
        request.validation_mode(),
    )
    .map_err(LocalNormalizationPipelineError::Input)?;
    let dimensions = dimensions_from_axes(source_role, source_reader.descriptor().axes())
        .map_err(LocalNormalizationPipelineError::Input)?;
    let reference_dimensions =
        dimensions_from_axes(reference_role, reference_reader.descriptor().axes())
            .map_err(LocalNormalizationPipelineError::Input)?;
    if reference_dimensions != dimensions {
        return Err(LocalNormalizationPipelineError::DimensionMismatch {
            source: dimensions,
            reference: reference_dimensions,
        });
    }
    let work_units = u64::try_from(dimensions.planes())
        .ok()
        .and_then(|planes| planes.checked_add(3))
        .ok_or(LocalNormalizationPipelineError::WorkSizeOverflow)?;
    *total = Some(work_units);
    emit_progress(
        sequence,
        stage,
        ProgressState::Running,
        *completed,
        *total,
        None,
        progress,
    )?;

    let planned_bytes = estimate_local_normalization_peak_bytes(dimensions, request.parameters())?;
    let _reservation = memory
        .try_reserve(planned_bytes)
        .map_err(LocalNormalizationPipelineError::Memory)?;
    let mut writer = AtomicF64PrimaryStreamWriter::create_with_provenance(
        request.output(),
        dimensions,
        request.provenance(),
    )
    .map_err(LocalNormalizationPipelineError::Publish)?;
    advance_progress(sequence, stage, completed, *total, progress)?;

    let parameters = request.parameters();
    let mut measured_sources = 0_usize;
    let mut protected_pixels = 0_usize;
    let mut valid_control_points = 0_usize;
    let mut rejected_cells = 0_usize;
    let mut control_points = Vec::new();
    let mut cell_diagnostics = Vec::new();
    let mut application_evidence = LocalApplicationEvidence::default();
    let plane_width = u64::try_from(dimensions.width())
        .map_err(|_| LocalNormalizationPipelineError::WorkSizeOverflow)?;
    let plane_height = u64::try_from(dimensions.height())
        .map_err(|_| LocalNormalizationPipelineError::WorkSizeOverflow)?;
    for plane in 0..dimensions.planes() {
        cancellation
            .checkpoint()
            .map_err(LocalNormalizationPipelineError::Cancelled)?;
        let plane_index =
            u64::try_from(plane).map_err(|_| LocalNormalizationPipelineError::WorkSizeOverflow)?;
        let region = ImageRegion::new(plane_index, 0, 0, plane_width, plane_height);
        let source = source_reader
            .read_region_image(region)
            .map_err(LocalNormalizationPipelineError::ReadInput)?;
        let reference = reference_reader
            .read_region_image(region)
            .map_err(LocalNormalizationPipelineError::ReadInput)?;
        let quality = measure_frame_quality(&source, 0, parameters.detection())
            .map_err(LocalNormalizationPipelineError::Quality)?;
        let protected_sources = protected_sources_from_stars(
            quality.stars(),
            parameters.protection().maximum_sources(),
        )
        .map_err(LocalNormalizationPipelineError::Protection)?;
        let protection = build_protection_mask(
            source.dimensions(),
            0,
            &protected_sources,
            parameters.protection(),
        )
        .map_err(LocalNormalizationPipelineError::Protection)?;
        let samples = sample_local_grid(
            &source,
            &reference,
            Some(protection.mask()),
            0,
            parameters.sampling(),
        )
        .map_err(LocalNormalizationPipelineError::Sampling)?;
        let fits = fit_local_grid(&samples, parameters.fitting())
            .map_err(LocalNormalizationPipelineError::Fitting)?;
        cell_diagnostics
            .try_reserve(fits.len())
            .map_err(|_| LocalNormalizationPipelineError::WorkSizeOverflow)?;
        cell_diagnostics.extend(
            fits.iter()
                .cloned()
                .map(|fit| LocalNormalizationCellDiagnostic { plane, fit }),
        );
        let surface = build_local_surface(&fits, parameters.surface())
            .map_err(LocalNormalizationPipelineError::Surface)?;
        control_points
            .try_reserve(surface.control_point_count())
            .map_err(|_| LocalNormalizationPipelineError::WorkSizeOverflow)?;
        control_points.extend(surface.control_points().iter().map(|point| {
            LocalNormalizationControlPoint {
                plane,
                x: point.x(),
                y: point.y(),
                scale: point.scale(),
                offset: point.offset(),
                median_absolute_residual: point.median_absolute_residual(),
            }
        }));
        measured_sources = measured_sources
            .checked_add(quality.stars().len())
            .ok_or(LocalNormalizationPipelineError::WorkSizeOverflow)?;
        protected_pixels = protected_pixels
            .checked_add(protection.protected_pixels())
            .ok_or(LocalNormalizationPipelineError::WorkSizeOverflow)?;
        valid_control_points = valid_control_points
            .checked_add(surface.control_point_count())
            .ok_or(LocalNormalizationPipelineError::WorkSizeOverflow)?;
        rejected_cells = rejected_cells
            .checked_add(surface.rejected_cell_count())
            .ok_or(LocalNormalizationPipelineError::WorkSizeOverflow)?;
        for start_y in (0..dimensions.height()).step_by(APPLICATION_BAND_HEIGHT) {
            cancellation
                .checkpoint()
                .map_err(LocalNormalizationPipelineError::Cancelled)?;
            let row_count = APPLICATION_BAND_HEIGHT.min(dimensions.height() - start_y);
            let application = apply_local_surface_band(&source, &surface, start_y, row_count)
                .map_err(LocalNormalizationPipelineError::Application)?;
            application_evidence = application_evidence
                .checked_add(application.evidence())
                .ok_or(LocalNormalizationPipelineError::WorkSizeOverflow)?;
            writer
                .write_image_chunk(application.image())
                .map_err(LocalNormalizationPipelineError::Publish)?;
        }
        advance_progress(sequence, stage, completed, *total, progress)?;
    }

    cancellation
        .checkpoint()
        .map_err(LocalNormalizationPipelineError::Cancelled)?;
    let staged = writer
        .finish()
        .map_err(LocalNormalizationPipelineError::Publish)?;
    validate_staged_output(&staged, dimensions)?;
    advance_progress(sequence, stage, completed, *total, progress)?;
    cancellation
        .checkpoint()
        .map_err(LocalNormalizationPipelineError::Cancelled)?;
    verify_source(request.source(), source_role).map_err(LocalNormalizationPipelineError::Input)?;
    verify_source(request.reference(), reference_role)
        .map_err(LocalNormalizationPipelineError::Input)?;
    cancellation
        .checkpoint()
        .map_err(LocalNormalizationPipelineError::Cancelled)?;
    let summary = staged
        .publish()
        .map_err(LocalNormalizationPipelineError::Publish)?;
    advance_progress(sequence, stage, completed, *total, progress)?;
    Ok(LocalNormalizationResult {
        dimensions,
        summary,
        application: application_evidence,
        measured_sources,
        protected_pixels,
        valid_control_points,
        rejected_cells,
        control_points,
        cell_diagnostics,
        peak_reserved_bytes: memory.peak(),
    })
}

/// Computes the conservative peak reservation for one local-normalization job.
///
/// This pure preflight uses the same accounting path as execution, allowing a
/// caller to reject an undersized memory ceiling before reading image pixels.
///
/// # Errors
///
/// Returns [`LocalNormalizationPipelineError::WorkSizeOverflow`] when any
/// dimension or configured work ceiling cannot be represented safely.
pub fn estimate_local_normalization_peak_bytes(
    dimensions: Dimensions,
    parameters: LocalNormalizationParameters,
) -> Result<usize, LocalNormalizationPipelineError> {
    Ok(estimate_local_normalization_memory(dimensions, parameters)?.required_bytes())
}

/// Auditable components of the conservative local-normalization reservation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LocalNormalizationMemoryEstimate {
    plane_images_bytes: usize,
    application_band_bytes: usize,
    decode_status_bytes: usize,
    retained_samples_bytes: usize,
    diagnostics_bytes: usize,
    quality_bytes: usize,
    slope_bytes: usize,
    writer_buffer_bytes: usize,
    required_bytes: usize,
}

impl LocalNormalizationMemoryEstimate {
    /// Two complete active input planes.
    #[must_use]
    pub const fn plane_images_bytes(self) -> usize {
        self.plane_images_bytes
    }
    /// Fixed-height normalized output band.
    #[must_use]
    pub const fn application_band_bytes(self) -> usize {
        self.application_band_bytes
    }
    /// Temporary FITS sample-status decoding array.
    #[must_use]
    pub const fn decode_status_bytes(self) -> usize {
        self.decode_status_bytes
    }
    /// Maximum retained spatial sample storage.
    #[must_use]
    pub const fn retained_samples_bytes(self) -> usize {
        self.retained_samples_bytes
    }
    /// Accepted and rejected cell diagnostics retained across planes.
    #[must_use]
    pub const fn diagnostics_bytes(self) -> usize {
        self.diagnostics_bytes
    }
    /// Bounded stellar-quality candidate storage.
    #[must_use]
    pub const fn quality_bytes(self) -> usize {
        self.quality_bytes
    }
    /// Bounded pairwise-slope scratch storage.
    #[must_use]
    pub const fn slope_bytes(self) -> usize {
        self.slope_bytes
    }
    /// Buffered atomic FITS writer allocation.
    #[must_use]
    pub const fn writer_buffer_bytes(self) -> usize {
        self.writer_buffer_bytes
    }
    /// Conservative sum reserved before pixel decoding.
    #[must_use]
    pub const fn required_bytes(self) -> usize {
        self.required_bytes
    }
}

/// Computes an auditable conservative memory breakdown for preflight UIs.
///
/// # Errors
///
/// Returns a typed overflow or sampling-limit error under the same conditions
/// as [`estimate_local_normalization_peak_bytes`].
pub fn estimate_local_normalization_memory(
    dimensions: Dimensions,
    parameters: LocalNormalizationParameters,
) -> Result<LocalNormalizationMemoryEstimate, LocalNormalizationPipelineError> {
    let plane_samples = dimensions
        .width()
        .checked_mul(dimensions.height())
        .ok_or(LocalNormalizationPipelineError::WorkSizeOverflow)?;
    let plane_image_bytes = plane_samples
        .checked_mul(size_of::<f64>() + size_of::<PixelFlags>())
        .ok_or(LocalNormalizationPipelineError::WorkSizeOverflow)?;
    let decode_status_bytes = plane_samples
        .checked_mul(size_of::<SampleStatus>())
        .ok_or(LocalNormalizationPipelineError::WorkSizeOverflow)?;
    let band_samples = dimensions
        .width()
        .checked_mul(dimensions.height().min(APPLICATION_BAND_HEIGHT))
        .ok_or(LocalNormalizationPipelineError::WorkSizeOverflow)?;
    let band_image_bytes = band_samples
        .checked_mul(size_of::<f64>() + size_of::<PixelFlags>())
        .ok_or(LocalNormalizationPipelineError::WorkSizeOverflow)?;
    let sampling = parameters.sampling();
    let columns = dimensions
        .width()
        .checked_add(sampling.cell_width() - 1)
        .map(|value| value / sampling.cell_width())
        .ok_or(LocalNormalizationPipelineError::WorkSizeOverflow)?;
    let rows = dimensions
        .height()
        .checked_add(sampling.cell_height() - 1)
        .map(|value| value / sampling.cell_height())
        .ok_or(LocalNormalizationPipelineError::WorkSizeOverflow)?;
    let active_cells = columns
        .checked_mul(rows)
        .ok_or(LocalNormalizationPipelineError::WorkSizeOverflow)?;
    if active_cells > sampling.maximum_cells() {
        return Err(LocalNormalizationPipelineError::Sampling(
            SamplingError::CellLimitExceeded,
        ));
    }
    let retained_sample_bytes = active_cells
        .checked_mul(parameters.sampling().maximum_samples_per_cell())
        .and_then(|count| count.checked_mul(64))
        .ok_or(LocalNormalizationPipelineError::WorkSizeOverflow)?;
    let retained_diagnostic_bytes = active_cells
        .checked_mul(dimensions.planes())
        .and_then(|count| count.checked_mul(256))
        .ok_or(LocalNormalizationPipelineError::WorkSizeOverflow)?;
    let quality_bytes = parameters
        .detection()
        .maximum_candidates()
        .checked_mul(256)
        .ok_or(LocalNormalizationPipelineError::WorkSizeOverflow)?;
    let slope_bytes = parameters
        .fitting()
        .maximum_pairwise_slopes()
        .checked_mul(size_of::<f64>())
        .ok_or(LocalNormalizationPipelineError::WorkSizeOverflow)?;
    let plane_images_bytes = plane_image_bytes
        .checked_mul(2)
        .ok_or(LocalNormalizationPipelineError::WorkSizeOverflow)?;
    let required_bytes = plane_images_bytes
        .checked_add(band_image_bytes)
        .and_then(|bytes| bytes.checked_add(decode_status_bytes))
        .and_then(|bytes| bytes.checked_add(retained_sample_bytes))
        .and_then(|bytes| bytes.checked_add(retained_diagnostic_bytes))
        .and_then(|bytes| bytes.checked_add(quality_bytes))
        .and_then(|bytes| bytes.checked_add(slope_bytes))
        .and_then(|bytes| bytes.checked_add(STREAM_WRITER_BUFFER_BYTES))
        .ok_or(LocalNormalizationPipelineError::WorkSizeOverflow)?;
    Ok(LocalNormalizationMemoryEstimate {
        plane_images_bytes,
        application_band_bytes: band_image_bytes,
        decode_status_bytes,
        retained_samples_bytes: retained_sample_bytes,
        diagnostics_bytes: retained_diagnostic_bytes,
        quality_bytes,
        slope_bytes,
        writer_buffer_bytes: STREAM_WRITER_BUFFER_BYTES,
        required_bytes,
    })
}

fn validate_staged_output(
    staged: &aether_fits::CompletedAtomicFits,
    expected: Dimensions,
) -> Result<(), LocalNormalizationPipelineError> {
    let file = staged
        .try_clone_for_readback()
        .map_err(LocalNormalizationPipelineError::Publish)?;
    let mut reader = PrimaryImageReader::open(file, HeaderReadOptions::default())
        .map_err(LocalNormalizationPipelineError::ReadInput)?;
    let actual = dimensions_from_axes(
        PipelineInput::Signal { index: 0 },
        reader.descriptor().axes(),
    )
    .map_err(LocalNormalizationPipelineError::Input)?;
    let checksums = reader
        .verify_checksums()
        .map_err(LocalNormalizationPipelineError::ReadInput)?;
    if actual != expected || !checksums.is_fully_verified() {
        return Err(LocalNormalizationPipelineError::InvalidStagedOutput);
    }
    Ok(())
}

fn advance_progress<F>(
    sequence: &ProgressSequence,
    stage: &StageId,
    completed: &mut u64,
    total: Option<u64>,
    progress: &mut F,
) -> Result<(), LocalNormalizationPipelineError>
where
    F: FnMut(ProgressEvent),
{
    *completed = completed
        .checked_add(1)
        .ok_or(LocalNormalizationPipelineError::WorkSizeOverflow)?;
    emit_progress(
        sequence,
        stage,
        ProgressState::Running,
        *completed,
        total,
        None,
        progress,
    )
}

#[allow(clippy::too_many_arguments)]
fn emit_progress<F>(
    sequence: &ProgressSequence,
    stage: &StageId,
    state: ProgressState,
    completed: u64,
    total: Option<u64>,
    code: Option<String>,
    progress: &mut F,
) -> Result<(), LocalNormalizationPipelineError>
where
    F: FnMut(ProgressEvent),
{
    let event = sequence
        .next(stage.clone(), state, completed, total, code)
        .map_err(LocalNormalizationPipelineError::Progress)?;
    progress(event);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File};
    use std::sync::atomic::{AtomicU64, Ordering};

    use aether_core::ScientificImage;
    use aether_fits::{FitsProvenanceError, write_f64_primary_atomic_new};
    use aether_localnorm::{
        LocalFitParameters, ProtectionParameters, SamplingGridParameters, SurfaceParameters,
    };
    use aether_quality::{BackgroundParameters, StarMeasurementParameters};
    use aether_session::{SourceFingerprint, fingerprint_reader};

    use super::*;

    type TestResult = Result<(), Box<dyn Error>>;
    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> std::io::Result<Self> {
            let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "aether-local-normalization-runtime-{}-{sequence}",
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

    fn parameters(cell_width: usize) -> Result<LocalNormalizationParameters, Box<dyn Error>> {
        Ok(LocalNormalizationParameters::new(
            StarMeasurementParameters::new(
                BackgroundParameters::new(3.0, 8, 1_024)?,
                6.0,
                2.0,
                8,
                4,
                6,
                10_000,
                Some(65_000.0),
            )?,
            ProtectionParameters::new(1.5, 2.0, 2, 32, 10_000, 20_000_000)?,
            SamplingGridParameters::new(cell_width, 64, 256, 16_384)?,
            LocalFitParameters::new(32, 256, 32_640, 1.0e-6)?,
            SurfaceParameters::new(16, 4, 12, 256.0)?,
        ))
    }

    fn source(path: &str, digest: char) -> Result<PipelineSource, Box<dyn Error>> {
        Ok(PipelineSource::new(
            PathBuf::from(path),
            SourceFingerprint::new(1, digest.to_string().repeat(64))?,
        ))
    }

    #[test]
    fn priority_camera_peaks_use_active_geometry_and_stay_bounded() -> TestResult {
        let dimensions = Dimensions::new(4_144, 2_822, 1)?;
        let detection = StarMeasurementParameters::new(
            BackgroundParameters::new(3.0, 8, 1_024)?,
            6.0,
            2.0,
            6,
            4,
            6,
            24_576,
            None,
        )?;
        let make_parameters = |maximum_cells| -> Result<_, Box<dyn Error>> {
            Ok(LocalNormalizationParameters::new(
                detection,
                ProtectionParameters::new(1.5, 2.0, 2, 24, 24_576, 64_000_000)?,
                SamplingGridParameters::new(128, 128, 4_096, maximum_cells)?,
                LocalFitParameters::new(256, 4_096, 1_000_000, 1.0e-12)?,
                SurfaceParameters::new(16, 4, 16, 1_024.0)?,
            ))
        };
        let exact_grid = 33 * 23;
        let exact =
            estimate_local_normalization_peak_bytes(dimensions, make_parameters(exact_grid)?)?;
        let loose = estimate_local_normalization_peak_bytes(dimensions, make_parameters(262_144)?)?;
        let rgb = estimate_local_normalization_peak_bytes(
            Dimensions::new(4_144, 2_822, 3)?,
            make_parameters(262_144)?,
        )?;
        let touptek_585c = estimate_local_normalization_peak_bytes(
            Dimensions::new(3_840, 2_160, 1)?,
            make_parameters(262_144)?,
        )?;
        let breakdown = estimate_local_normalization_memory(dimensions, make_parameters(262_144)?)?;

        assert_eq!(exact, loose);
        assert_eq!(loose, 440_485_472);
        assert_eq!(touptek_585c, 310_198_272);
        assert_eq!(breakdown.required_bytes(), loose);
        assert_eq!(
            breakdown.required_bytes(),
            breakdown.plane_images_bytes()
                + breakdown.application_band_bytes()
                + breakdown.decode_status_bytes()
                + breakdown.retained_samples_bytes()
                + breakdown.diagnostics_bytes()
                + breakdown.quality_bytes()
                + breakdown.slope_bytes()
                + breakdown.writer_buffer_bytes()
        );
        assert!(loose < 512 * 1_024 * 1_024);
        assert!(touptek_585c < 512 * 1_024 * 1_024);
        assert!(rgb < loose * 2);
        assert!(matches!(
            estimate_local_normalization_peak_bytes(dimensions, make_parameters(exact_grid - 1)?),
            Err(LocalNormalizationPipelineError::Sampling(
                SamplingError::CellLimitExceeded
            ))
        ));
        Ok(())
    }

    fn execution_parameters() -> Result<LocalNormalizationParameters, Box<dyn Error>> {
        Ok(LocalNormalizationParameters::new(
            StarMeasurementParameters::new(
                BackgroundParameters::new(3.0, 8, 100)?,
                1_000.0,
                2.0,
                4,
                3,
                3,
                1_000,
                None,
            )?,
            ProtectionParameters::new(1.5, 2.0, 2, 16, 1_000, 2_000_000)?,
            SamplingGridParameters::new(16, 16, 256, 8)?,
            LocalFitParameters::new(32, 256, 32_640, 1.0e-12)?,
            SurfaceParameters::new(4, 1, 4, 100.0)?,
        ))
    }

    fn stellar_execution_parameters() -> Result<LocalNormalizationParameters, Box<dyn Error>> {
        Ok(LocalNormalizationParameters::new(
            StarMeasurementParameters::new(
                BackgroundParameters::new(3.0, 8, 1_000)?,
                6.0,
                2.0,
                6,
                4,
                6,
                1_000,
                Some(900.0),
            )?,
            ProtectionParameters::new(1.5, 2.0, 2, 24, 1_000, 4_000_000)?,
            SamplingGridParameters::new(16, 16, 256, 16)?,
            LocalFitParameters::new(32, 256, 32_640, 1.0e-12)?,
            SurfaceParameters::new(4, 1, 4, 128.0)?,
        ))
    }

    fn gradient_execution_parameters() -> Result<LocalNormalizationParameters, Box<dyn Error>> {
        Ok(LocalNormalizationParameters::new(
            StarMeasurementParameters::new(
                BackgroundParameters::new(3.0, 8, 4_000)?,
                6.0,
                2.0,
                6,
                4,
                6,
                4_000,
                Some(1_500.0),
            )?,
            ProtectionParameters::new(1.5, 2.0, 2, 32, 4_000, 16_000_000)?,
            SamplingGridParameters::new(16, 16, 256, 64)?,
            LocalFitParameters::new(32, 256, 32_640, 1.0e-12)?,
            SurfaceParameters::new(32, 4, 12, 256.0)?,
        ))
    }

    fn write_pipeline_source(
        path: PathBuf,
        image: &ScientificImage,
    ) -> Result<PipelineSource, Box<dyn Error>> {
        write_f64_primary_atomic_new(&path, image)?;
        let mut file = File::open(&path)?;
        let fingerprint = fingerprint_reader(&mut file)?;
        Ok(PipelineSource::new(path, fingerprint))
    }

    fn execution_request(
        source: PipelineSource,
        reference: PipelineSource,
        output: PathBuf,
        parameters: LocalNormalizationParameters,
    ) -> Result<LocalNormalizationRequest, Box<dyn Error>> {
        let plan = LocalNormalizationPlan::new(
            source.fingerprint().sha256(),
            reference.fingerprint().sha256(),
            parameters,
        )?;
        Ok(LocalNormalizationRequest::new(
            source,
            reference,
            output,
            provenance(&plan, LOCAL_APPLICATION_ALGORITHM_ID, 2)?,
            parameters,
            plan,
        )?)
    }

    fn affine_images(
        dimensions: Dimensions,
    ) -> Result<(ScientificImage, ScientificImage), Box<dyn Error>> {
        let source_pixels = (0..dimensions.pixel_count())
            .map(|index| (index % 509) as f64 - 127.0)
            .collect::<Vec<_>>();
        let reference_pixels = source_pixels
            .iter()
            .map(|&value| value.mul_add(2.0, 5.0))
            .collect::<Vec<_>>();
        Ok((
            ScientificImage::from_pixels(dimensions, source_pixels)?,
            ScientificImage::from_pixels(dimensions, reference_pixels)?,
        ))
    }

    fn stellar_affine_images() -> Result<(ScientificImage, ScientificImage), Box<dyn Error>> {
        let dimensions = Dimensions::new(64, 64, 1)?;
        let mut source_pixels = Vec::new();
        source_pixels.try_reserve_exact(dimensions.pixel_count())?;
        for y in 0..dimensions.height() {
            for x in 0..dimensions.width() {
                let background = ((y * dimensions.width() + x) % 17) as f64 * 0.01;
                let dx = x as f64 - 32.0;
                let dy = y as f64 - 32.0;
                let star = 1_000.0 * (-(dx.mul_add(dx, dy * dy)) / 12.5).exp();
                source_pixels.push(background + star);
            }
        }
        let reference_pixels = source_pixels
            .iter()
            .map(|&value| value.mul_add(2.0, 5.0))
            .collect::<Vec<_>>();
        Ok((
            ScientificImage::from_pixels(dimensions, source_pixels)?,
            ScientificImage::from_pixels(dimensions, reference_pixels)?,
        ))
    }

    fn gradient_stellar_images() -> Result<(ScientificImage, ScientificImage), Box<dyn Error>> {
        let dimensions = Dimensions::new(128, 128, 1)?;
        let mut source_pixels = Vec::new();
        let mut reference_pixels = Vec::new();
        source_pixels.try_reserve_exact(dimensions.pixel_count())?;
        reference_pixels.try_reserve_exact(dimensions.pixel_count())?;
        for y in 0..dimensions.height() {
            for x in 0..dimensions.width() {
                let background = 100.0 + ((x * 17 + y * 31) % 37) as f64;
                let dx = x as f64 - 64.0;
                let dy = y as f64 - 64.0;
                let star = 2_000.0 * (-(dx.mul_add(dx, dy * dy)) / 18.0).exp();
                let source = background + star;
                let scale = 0.92 + 0.16 * x as f64 / 127.0;
                let offset = -8.0 + 16.0 * y as f64 / 127.0;
                source_pixels.push(source);
                reference_pixels.push(source.mul_add(scale, offset));
            }
        }
        Ok((
            ScientificImage::from_pixels(dimensions, source_pixels)?,
            ScientificImage::from_pixels(dimensions, reference_pixels)?,
        ))
    }

    fn provenance(
        plan: &LocalNormalizationPlan,
        algorithm: &str,
        source_count: u32,
    ) -> Result<FitsOutputProvenance, FitsProvenanceError> {
        FitsOutputProvenance::new("c".repeat(64), "light-l", algorithm, source_count)?
            .with_plan_sha256(plan.plan_sha256())?
            .with_parameters_sha256(plan.parameters_sha256())
    }

    #[test]
    fn accepts_only_a_fully_cross_checked_request() -> TestResult {
        let parameters = parameters(64)?;
        let plan = LocalNormalizationPlan::new("a".repeat(64), "b".repeat(64), parameters)?;
        let request = LocalNormalizationRequest::new(
            source("source.fits", 'a')?,
            source("reference.fits", 'b')?,
            PathBuf::from("normalized.fits"),
            provenance(&plan, LOCAL_APPLICATION_ALGORITHM_ID, 2)?,
            parameters,
            plan.clone(),
        )?;
        assert_eq!(request.plan(), &plan);
        assert_eq!(request.output(), Path::new("normalized.fits"));
        assert_eq!(request.parameters(), parameters);
        assert_eq!(request.provenance().plan_sha256(), Some(plan.plan_sha256()));
        assert_eq!(request.header_options(), HeaderReadOptions::default());
        assert_eq!(request.validation_mode(), ValidationMode::Strict);
        Ok(())
    }

    #[test]
    fn rejects_input_and_parameter_identity_drift() -> TestResult {
        let controls = parameters(64)?;
        let changed_parameters = parameters(65)?;
        let plan = LocalNormalizationPlan::new("a".repeat(64), "b".repeat(64), controls)?;
        let bound = provenance(&plan, LOCAL_APPLICATION_ALGORITHM_ID, 2)?;

        let wrong_source = LocalNormalizationRequest::new(
            source("source.fits", 'd')?,
            source("reference.fits", 'b')?,
            PathBuf::from("normalized.fits"),
            bound.clone(),
            controls,
            plan.clone(),
        );
        assert_eq!(
            wrong_source.err(),
            Some(LocalNormalizationRequestError::SourcePlanMismatch)
        );

        let wrong_reference = LocalNormalizationRequest::new(
            source("source.fits", 'a')?,
            source("reference.fits", 'd')?,
            PathBuf::from("normalized.fits"),
            bound.clone(),
            controls,
            plan.clone(),
        );
        assert_eq!(
            wrong_reference.err(),
            Some(LocalNormalizationRequestError::ReferencePlanMismatch)
        );

        let wrong_parameters = LocalNormalizationRequest::new(
            source("source.fits", 'a')?,
            source("reference.fits", 'b')?,
            PathBuf::from("normalized.fits"),
            bound,
            changed_parameters,
            plan,
        );
        assert_eq!(
            wrong_parameters.err(),
            Some(LocalNormalizationRequestError::ParametersPlanMismatch)
        );
        Ok(())
    }

    #[test]
    fn rejects_incomplete_or_foreign_fits_provenance() -> TestResult {
        let parameters = parameters(64)?;
        let plan = LocalNormalizationPlan::new("a".repeat(64), "b".repeat(64), parameters)?;
        let image_source = source("source.fits", 'a')?;
        let image_reference = source("reference.fits", 'b')?;
        let make_request = |provenance| {
            LocalNormalizationRequest::new(
                image_source.clone(),
                image_reference.clone(),
                PathBuf::from("normalized.fits"),
                provenance,
                parameters,
                plan.clone(),
            )
        };

        assert_eq!(
            make_request(provenance(&plan, "foreign-v1", 2)?).err(),
            Some(LocalNormalizationRequestError::ProvenanceAlgorithmMismatch)
        );
        assert_eq!(
            make_request(provenance(&plan, LOCAL_APPLICATION_ALGORITHM_ID, 1)?).err(),
            Some(LocalNormalizationRequestError::ProvenanceSourceCount { actual: 1 })
        );
        let no_plan = FitsOutputProvenance::new(
            "c".repeat(64),
            "light-l",
            LOCAL_APPLICATION_ALGORITHM_ID,
            2,
        )?
        .with_parameters_sha256(plan.parameters_sha256())?;
        assert_eq!(
            make_request(no_plan).err(),
            Some(LocalNormalizationRequestError::ProvenancePlanMismatch)
        );
        let no_parameters = FitsOutputProvenance::new(
            "c".repeat(64),
            "light-l",
            LOCAL_APPLICATION_ALGORITHM_ID,
            2,
        )?
        .with_plan_sha256(plan.plan_sha256())?;
        assert_eq!(
            make_request(no_parameters).err(),
            Some(LocalNormalizationRequestError::ProvenanceParametersMismatch)
        );
        Ok(())
    }

    #[test]
    fn executes_all_planes_and_publishes_the_exact_affine_oracle() -> TestResult {
        let directory = TestDirectory::new()?;
        let dimensions = Dimensions::new(32, 32, 2)?;
        let (source_image, reference_image) = affine_images(dimensions)?;
        let source = write_pipeline_source(directory.0.join("source.fits"), &source_image)?;
        let reference =
            write_pipeline_source(directory.0.join("reference.fits"), &reference_image)?;
        let output = directory.0.join("normalized.fits");
        let request =
            execution_request(source, reference, output.clone(), execution_parameters()?)?;
        let estimated_peak =
            estimate_local_normalization_peak_bytes(dimensions, request.parameters())?;
        let budget = MemoryBudget::new(16 * 1_024 * 1_024)?;
        let mut events = Vec::new();
        let result = run_local_normalization_with_progress(
            &request,
            &CancellationToken::new(),
            &budget,
            |event| events.push(event),
        )?;

        assert_eq!(result.dimensions(), dimensions);
        assert_eq!(result.application().transformed(), dimensions.pixel_count());
        assert_eq!(
            result.application().classified_samples(),
            dimensions.pixel_count()
        );
        assert_eq!(result.measured_sources(), 0);
        assert_eq!(result.protected_pixels(), 0);
        assert_eq!(result.valid_control_points(), 8);
        assert_eq!(result.control_points().len(), 8);
        assert_eq!(result.cell_diagnostics().len(), 8);
        assert!(
            result
                .cell_diagnostics()
                .iter()
                .all(|cell| cell.fit().result().is_ok())
        );
        assert!(
            result
                .control_points()
                .iter()
                .all(|point| point.median_absolute_residual() == 0.0)
        );
        assert_eq!(result.rejected_cells(), 0);
        assert_eq!(result.peak_reserved_bytes(), budget.peak());
        assert_eq!(result.peak_reserved_bytes(), estimated_peak);
        assert_eq!(
            result.summary().samples_written(),
            dimensions.pixel_count() as u64
        );
        assert_eq!(
            events.first().map(ProgressEvent::state),
            Some(ProgressState::Started)
        );
        assert_eq!(events.first().and_then(ProgressEvent::total_units), None);
        assert_eq!(
            events.last().map(ProgressEvent::state),
            Some(ProgressState::Completed)
        );
        assert_eq!(events.last().map(ProgressEvent::completed_units), Some(5));
        assert_eq!(events.last().and_then(ProgressEvent::total_units), Some(5));
        assert!(
            events
                .windows(2)
                .all(|pair| pair[1].sequence() == pair[0].sequence() + 1)
        );
        assert!(
            events
                .iter()
                .all(|event| event.stage().as_str() == LOCAL_NORMALIZATION_STAGE_ID)
        );

        let file = File::open(output)?;
        let mut reader = PrimaryImageReader::open(file, HeaderReadOptions::default())?;
        assert!(reader.verify_checksums()?.is_fully_verified());
        for plane in 0..dimensions.planes() {
            let actual = reader.read_region_image(ImageRegion::new(
                plane as u64,
                0,
                0,
                dimensions.width() as u64,
                dimensions.height() as u64,
            ))?;
            let start = plane * dimensions.width() * dimensions.height();
            let end = start + dimensions.width() * dimensions.height();
            assert_eq!(actual.pixels(), &reference_image.pixels()[start..end]);
        }
        Ok(())
    }

    #[test]
    fn cancellation_and_memory_failure_publish_nothing() -> TestResult {
        let directory = TestDirectory::new()?;
        let dimensions = Dimensions::new(32, 32, 1)?;
        let (source_image, reference_image) = affine_images(dimensions)?;
        let source = write_pipeline_source(directory.0.join("source.fits"), &source_image)?;
        let reference =
            write_pipeline_source(directory.0.join("reference.fits"), &reference_image)?;
        let cancelled_output = directory.0.join("cancelled.fits");
        let cancelled_request = execution_request(
            source.clone(),
            reference.clone(),
            cancelled_output.clone(),
            execution_parameters()?,
        )?;
        let cancellation = CancellationToken::new();
        assert!(cancellation.cancel());
        let mut cancellation_events = Vec::new();
        let cancelled = run_local_normalization_with_progress(
            &cancelled_request,
            &cancellation,
            &MemoryBudget::new(16 * 1_024 * 1_024)?,
            |event| cancellation_events.push(event),
        );
        assert!(matches!(
            cancelled,
            Err(LocalNormalizationPipelineError::Cancelled(_))
        ));
        assert!(!cancelled_output.exists());
        assert_eq!(
            cancellation_events.last().map(ProgressEvent::state),
            Some(ProgressState::Cancelled)
        );
        assert_eq!(
            cancellation_events.last().and_then(ProgressEvent::code),
            Some("cancelled")
        );

        let memory_output = directory.0.join("memory.fits");
        let memory_request = execution_request(
            source,
            reference,
            memory_output.clone(),
            execution_parameters()?,
        )?;
        let mut memory_events = Vec::new();
        let failed = run_local_normalization_with_progress(
            &memory_request,
            &CancellationToken::new(),
            &MemoryBudget::new(1)?,
            |event| memory_events.push(event),
        );
        assert!(matches!(
            failed,
            Err(LocalNormalizationPipelineError::Memory(_))
        ));
        assert!(!memory_output.exists());
        assert_eq!(
            memory_events.last().map(ProgressEvent::state),
            Some(ProgressState::Failed)
        );
        assert_eq!(
            memory_events.last().and_then(ProgressEvent::code),
            Some("localnorm-memory")
        );
        Ok(())
    }

    #[test]
    fn dimension_mismatch_fails_before_destination_creation() -> TestResult {
        let directory = TestDirectory::new()?;
        let (source_image, _) = affine_images(Dimensions::new(32, 32, 1)?)?;
        let (_, reference_image) = affine_images(Dimensions::new(16, 32, 1)?)?;
        let source = write_pipeline_source(directory.0.join("source.fits"), &source_image)?;
        let reference =
            write_pipeline_source(directory.0.join("reference.fits"), &reference_image)?;
        let output = directory.0.join("normalized.fits");
        let request =
            execution_request(source, reference, output.clone(), execution_parameters()?)?;
        let result = run_local_normalization(
            &request,
            &CancellationToken::new(),
            &MemoryBudget::new(16 * 1_024 * 1_024)?,
        );
        assert!(matches!(
            result,
            Err(LocalNormalizationPipelineError::DimensionMismatch { .. })
        ));
        assert!(!output.exists());
        Ok(())
    }

    #[test]
    fn saturated_stellar_structure_is_measured_protected_and_excluded() -> TestResult {
        let directory = TestDirectory::new()?;
        let (source_image, reference_image) = stellar_affine_images()?;
        let dimensions = source_image.dimensions();
        let source = write_pipeline_source(directory.0.join("source.fits"), &source_image)?;
        let reference =
            write_pipeline_source(directory.0.join("reference.fits"), &reference_image)?;
        let output = directory.0.join("normalized.fits");
        let request = execution_request(
            source,
            reference,
            output.clone(),
            stellar_execution_parameters()?,
        )?;
        let result = run_local_normalization(
            &request,
            &CancellationToken::new(),
            &MemoryBudget::new(32 * 1_024 * 1_024)?,
        )?;

        assert_eq!(result.measured_sources(), 1);
        assert!(result.protected_pixels() > 100);
        assert_eq!(result.valid_control_points() + result.rejected_cells(), 16);
        assert_eq!(
            result.application().classified_samples(),
            dimensions.pixel_count()
        );
        let file = File::open(output)?;
        let mut reader = PrimaryImageReader::open(file, HeaderReadOptions::default())?;
        assert!(reader.verify_checksums()?.is_fully_verified());
        let actual = reader.read_region_image(ImageRegion::new(
            0,
            0,
            0,
            dimensions.width() as u64,
            dimensions.height() as u64,
        ))?;
        let maximum_error = actual
            .pixels()
            .iter()
            .zip(reference_image.pixels())
            .map(|(&actual, &expected)| (actual - expected).abs())
            .fold(0.0_f64, f64::max);
        assert!(maximum_error <= 1.0e-10, "maximum error: {maximum_error}");
        let stellar_peak = 32 * dimensions.width() + 32;
        assert!(actual.pixels()[stellar_peak] > 2_000.0);
        assert_eq!(
            actual
                .pixels()
                .iter()
                .enumerate()
                .max_by(|left, right| left.1.total_cmp(right.1))
                .map(|(index, _)| index),
            Some(stellar_peak)
        );
        Ok(())
    }

    #[test]
    fn spatial_gradient_is_recovered_without_suppressing_the_protected_star() -> TestResult {
        let directory = TestDirectory::new()?;
        let (source_image, reference_image) = gradient_stellar_images()?;
        let dimensions = source_image.dimensions();
        let source = write_pipeline_source(directory.0.join("source.fits"), &source_image)?;
        let reference =
            write_pipeline_source(directory.0.join("reference.fits"), &reference_image)?;
        let output = directory.0.join("normalized.fits");
        let request = execution_request(
            source,
            reference,
            output.clone(),
            gradient_execution_parameters()?,
        )?;
        let result = run_local_normalization(
            &request,
            &CancellationToken::new(),
            &MemoryBudget::new(64 * 1_024 * 1_024)?,
        )?;

        assert_eq!(result.measured_sources(), 1);
        assert!(result.protected_pixels() > 100);
        assert_eq!(result.valid_control_points(), 60);
        assert_eq!(result.rejected_cells(), 4);

        let file = File::open(output)?;
        let mut reader = PrimaryImageReader::open(file, HeaderReadOptions::default())?;
        let actual = reader.read_region_image(ImageRegion::new(
            0,
            0,
            0,
            dimensions.width() as u64,
            dimensions.height() as u64,
        ))?;
        let mut raw_squared_error = 0.0;
        let mut normalized_squared_error = 0.0;
        let mut background_samples = 0_u64;
        for y in 0..dimensions.height() {
            for x in 0..dimensions.width() {
                let dx = x as f64 - 64.0;
                let dy = y as f64 - 64.0;
                if dx.mul_add(dx, dy * dy) <= 144.0 {
                    continue;
                }
                let index = y * dimensions.width() + x;
                let expected = reference_image.pixels()[index];
                raw_squared_error += (source_image.pixels()[index] - expected).powi(2);
                normalized_squared_error += (actual.pixels()[index] - expected).powi(2);
                background_samples += 1;
            }
        }
        let raw_rmse = (raw_squared_error / background_samples as f64).sqrt();
        let normalized_rmse = (normalized_squared_error / background_samples as f64).sqrt();
        assert!(
            normalized_rmse < raw_rmse * 0.12,
            "raw RMSE {raw_rmse}, normalized RMSE {normalized_rmse}"
        );

        let stellar_peak = 64 * dimensions.width() + 64;
        let expected_peak = reference_image.pixels()[stellar_peak];
        let actual_peak = actual.pixels()[stellar_peak];
        assert!(
            (actual_peak - expected_peak).abs() / expected_peak < 0.002,
            "expected protected peak {expected_peak}, actual {actual_peak}"
        );
        assert_eq!(
            actual
                .pixels()
                .iter()
                .enumerate()
                .max_by(|left, right| left.1.total_cmp(right.1))
                .map(|(index, _)| index),
            Some(stellar_peak)
        );
        Ok(())
    }

    #[test]
    fn existing_destination_is_never_modified() -> TestResult {
        let directory = TestDirectory::new()?;
        let dimensions = Dimensions::new(32, 32, 1)?;
        let (source_image, reference_image) = affine_images(dimensions)?;
        let source = write_pipeline_source(directory.0.join("source.fits"), &source_image)?;
        let reference =
            write_pipeline_source(directory.0.join("reference.fits"), &reference_image)?;
        let output = directory.0.join("existing.fits");
        let sentinel = b"existing scientific product";
        fs::write(&output, sentinel)?;
        let request =
            execution_request(source, reference, output.clone(), execution_parameters()?)?;
        let result = run_local_normalization(
            &request,
            &CancellationToken::new(),
            &MemoryBudget::new(16 * 1_024 * 1_024)?,
        );
        assert!(matches!(
            result,
            Err(LocalNormalizationPipelineError::Publish(
                AtomicFitsWriteError::TargetExists
            ))
        ));
        assert_eq!(fs::read(output)?, sentinel);
        Ok(())
    }
}
