use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs::File;
use std::mem::size_of;
use std::path::{Path, PathBuf};

use aether_calibration::{CalibrationError, CalibrationParameters, calibrate_dark_flat};
use aether_core::{
    CoreError, Dimensions, Halo, ImageStatistics, PixelFlags, ScientificImage, StatisticsError,
    Tile, TileGrid, image_statistics,
};
use aether_fits::{
    AtomicFitsWriteError, FitsOutputProvenance, FitsWriteSummary, HeaderReadOptions,
    ImageReadError, ImageRegion, PrimaryImageReader, SampleStatus, Severity, ValidationMode,
    write_f64_primary_atomic_new_with_provenance,
};
use aether_integration::{IntegrationError, PixelSupport, integrate_mean};

use crate::{
    CancellationToken, Cancelled, MemoryBudget, MemoryBudgetError, ProgressEvent,
    ProgressEventError, ProgressSequence, ProgressState, StageId, StageIdError,
};

/// Algorithm identifier embedded by the first strict CPU integration pipeline.
pub const STRICT_MEAN_ALGORITHM_ID: &str = "strict-mean-v1";

const DEFAULT_TILE_WIDTH: usize = 256;
const DEFAULT_TILE_HEIGHT: usize = 256;
const OUTPUT_BUFFER_BYTES: usize = 64 * 1_024;
const PIPELINE_STAGE_ID: &str = "strict-cpu-slice";

/// Input role reported by a strict-pipeline failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PipelineInput {
    /// Light or signal input at its stable reduction-order index.
    Signal {
        /// Zero-based index in the request's signal list.
        index: usize,
    },
    /// Dark master subtracted from every signal.
    Dark,
    /// Already normalized flat master used as the divisor.
    NormalizedFlat,
}

impl Display for PipelineInput {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Signal { index } => write!(formatter, "signal[{index}]"),
            Self::Dark => formatter.write_str("dark master"),
            Self::NormalizedFlat => formatter.write_str("normalized flat master"),
        }
    }
}

/// Validated inputs and explicit policy for one strict CPU stack.
#[derive(Clone, Debug)]
pub struct StrictPipelineRequest {
    signals: Vec<PathBuf>,
    dark: PathBuf,
    normalized_flat: PathBuf,
    output: PathBuf,
    provenance: FitsOutputProvenance,
    calibration: CalibrationParameters,
    tile_width: usize,
    tile_height: usize,
    header_options: HeaderReadOptions,
    validation_mode: ValidationMode,
}

impl StrictPipelineRequest {
    /// Builds a strict request in exact signal reduction order.
    ///
    /// The default tile shape is 256 by 256 pixels. FITS conformance defaults
    /// to strict validation with [`HeaderReadOptions::default`]. The output uses
    /// create-new semantics and is never allowed to replace an existing file.
    ///
    /// # Errors
    ///
    /// Returns a typed error for an empty or unrepresentable signal list, a
    /// provenance source count that does not match it, or an algorithm identifier
    /// other than [`STRICT_MEAN_ALGORITHM_ID`].
    pub fn new(
        signals: Vec<PathBuf>,
        dark: PathBuf,
        normalized_flat: PathBuf,
        output: PathBuf,
        provenance: FitsOutputProvenance,
        calibration: CalibrationParameters,
    ) -> Result<Self, StrictPipelineError> {
        if signals.is_empty() {
            return Err(StrictPipelineError::NoSignals);
        }
        let source_count =
            u32::try_from(signals.len()).map_err(|_| StrictPipelineError::TooManySignals {
                count: signals.len(),
            })?;
        if provenance.source_count() != source_count {
            return Err(StrictPipelineError::ProvenanceSourceCountMismatch {
                signals: source_count,
                provenance: provenance.source_count(),
            });
        }
        if provenance.algorithm_id() != STRICT_MEAN_ALGORITHM_ID {
            return Err(StrictPipelineError::ProvenanceAlgorithmMismatch);
        }

        Ok(Self {
            signals,
            dark,
            normalized_flat,
            output,
            provenance,
            calibration,
            tile_width: DEFAULT_TILE_WIDTH,
            tile_height: DEFAULT_TILE_HEIGHT,
            header_options: HeaderReadOptions::default(),
            validation_mode: ValidationMode::Strict,
        })
    }

    /// Replaces the default tile shape.
    ///
    /// Tile dimensions affect the working set and I/O pattern, but not the
    /// strict per-pixel reduction order.
    ///
    /// # Errors
    ///
    /// Returns an error when either extent is zero.
    pub fn with_tile_shape(
        mut self,
        width: usize,
        height: usize,
    ) -> Result<Self, StrictPipelineError> {
        if width == 0 || height == 0 {
            return Err(StrictPipelineError::TileGrid(CoreError::ZeroTileExtent {
                width,
                height,
            }));
        }
        self.tile_width = width;
        self.tile_height = height;
        Ok(self)
    }

    /// Replaces FITS header limits and the diagnostic acceptance policy.
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

    /// Signal paths in their deterministic integration order.
    #[must_use]
    pub fn signals(&self) -> &[PathBuf] {
        &self.signals
    }

    /// Destination path governed by atomic create-new publication.
    #[must_use]
    pub fn output(&self) -> &Path {
        &self.output
    }
}

/// Successful strict-pipeline measurements and output accounting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrictPipelineResult {
    statistics: ImageStatistics,
    write_summary: FitsWriteSummary,
    tiles_processed: u64,
    reserved_bytes: usize,
}

impl StrictPipelineResult {
    /// Statistics calculated from the complete integrated image before writing.
    #[must_use]
    pub const fn statistics(self) -> ImageStatistics {
        self.statistics
    }

    /// Exact FITS sample and byte accounting returned by the atomic writer.
    #[must_use]
    pub const fn write_summary(self) -> FitsWriteSummary {
        self.write_summary
    }

    /// Spatial-plane tiles completed in deterministic traversal order.
    #[must_use]
    pub const fn tiles_processed(self) -> u64 {
        self.tiles_processed
    }

    /// Logical peak working-set reservation held for the run.
    #[must_use]
    pub const fn reserved_bytes(self) -> usize {
        self.reserved_bytes
    }
}

/// Failure raised by the strict FITS-to-FITS CPU pipeline.
#[derive(Debug)]
pub enum StrictPipelineError {
    /// A mean integration requires at least one signal.
    NoSignals,
    /// Signal count cannot be represented by the provenance contract.
    TooManySignals {
        /// Received signal count.
        count: usize,
    },
    /// Provenance does not describe the complete signal list.
    ProvenanceSourceCountMismatch {
        /// Signal count in the request.
        signals: u32,
        /// Source count embedded in provenance.
        provenance: u32,
    },
    /// Provenance names an algorithm other than the one this pipeline executes.
    ProvenanceAlgorithmMismatch,
    /// A tile shape is invalid for the core traversal contract.
    TileGrid(CoreError),
    /// Derived work-unit or byte accounting overflowed.
    WorkSizeOverflow,
    /// The configured memory budget cannot reserve the planned working set.
    Memory(MemoryBudgetError),
    /// A source or master could not be opened.
    OpenInput {
        /// Input role, without disclosing its path.
        input: PipelineInput,
        /// Operating-system failure.
        source: std::io::Error,
    },
    /// A FITS header or pixel region could not be read.
    ReadInput {
        /// Input role, without disclosing its path.
        input: PipelineInput,
        /// Structured FITS failure.
        source: ImageReadError,
    },
    /// Strict policy rejected retained FITS conformance diagnostics.
    HeaderRejected {
        /// Rejected input role.
        input: PipelineInput,
        /// Number of error-severity diagnostics.
        errors: usize,
    },
    /// FITS axes cannot form the supported checked 2D or 3D image model.
    UnsupportedImageAxes {
        /// Input role.
        input: PipelineInput,
        /// Exact FITS axes that were rejected.
        axes: Vec<u64>,
    },
    /// FITS axes violate a core dimension invariant.
    InvalidImageDimensions {
        /// Input role.
        input: PipelineInput,
        /// Core validation failure.
        source: CoreError,
    },
    /// One source or master differs from the first signal's exact dimensions.
    DimensionMismatch {
        /// Mismatched input role.
        input: PipelineInput,
        /// Dimensions established by the first signal.
        expected: Dimensions,
        /// Dimensions observed for this input.
        actual: Dimensions,
    },
    /// A temporary vector could not reserve its bounded element count.
    AllocationFailed {
        /// Number of elements requested.
        elements: usize,
    },
    /// Dark-and-flat calibration failed.
    Calibration(CalibrationError),
    /// Strict mean integration failed.
    Integration(IntegrationError),
    /// Integrated output could not be allocated or assembled.
    OutputImage(CoreError),
    /// A supposedly valid tile violated an internal copy invariant.
    OutputAssemblyInvariant,
    /// Final strict statistics could not be represented.
    Statistics(StatisticsError),
    /// Atomic FITS publication failed.
    Publish(AtomicFitsWriteError),
    /// Execution stopped at a cooperative checkpoint.
    Cancelled(Cancelled),
    /// The fixed runtime stage identifier unexpectedly failed validation.
    StageId(StageIdError),
    /// A machine-readable progress event violated its invariant.
    Progress(ProgressEventError),
}

impl StrictPipelineError {
    fn code(&self) -> &'static str {
        match self {
            Self::NoSignals => "no-signals",
            Self::TooManySignals { .. } => "too-many-signals",
            Self::ProvenanceSourceCountMismatch { .. } => "provenance-source-count",
            Self::ProvenanceAlgorithmMismatch => "provenance-algorithm",
            Self::TileGrid(_) => "tile-grid",
            Self::WorkSizeOverflow => "work-size-overflow",
            Self::Memory(_) => "memory-budget",
            Self::OpenInput { .. } => "open-input",
            Self::ReadInput { .. } => "read-input",
            Self::HeaderRejected { .. } => "header-rejected",
            Self::UnsupportedImageAxes { .. } => "unsupported-image-axes",
            Self::InvalidImageDimensions { .. } => "invalid-image-dimensions",
            Self::DimensionMismatch { .. } => "dimension-mismatch",
            Self::AllocationFailed { .. } => "allocation-failed",
            Self::Calibration(_) => "calibration",
            Self::Integration(_) => "integration",
            Self::OutputImage(_) => "output-image",
            Self::OutputAssemblyInvariant => "output-assembly",
            Self::Statistics(_) => "statistics",
            Self::Publish(_) => "publish",
            Self::Cancelled(_) => "cancelled",
            Self::StageId(_) => "stage-id",
            Self::Progress(_) => "progress",
        }
    }
}

impl Display for StrictPipelineError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSignals => formatter.write_str("strict pipeline requires at least one signal"),
            Self::TooManySignals { count } => {
                write!(
                    formatter,
                    "signal count {count} exceeds the provenance limit"
                )
            }
            Self::ProvenanceSourceCountMismatch {
                signals,
                provenance,
            } => write!(
                formatter,
                "provenance represents {provenance} sources but request contains {signals} signals"
            ),
            Self::ProvenanceAlgorithmMismatch => write!(
                formatter,
                "provenance algorithm must be {STRICT_MEAN_ALGORITHM_ID}"
            ),
            Self::TileGrid(error) => Display::fmt(error, formatter),
            Self::WorkSizeOverflow => formatter.write_str("pipeline work size overflows"),
            Self::Memory(error) => Display::fmt(error, formatter),
            Self::OpenInput { input, source } => {
                write!(formatter, "cannot open {input}: {source}")
            }
            Self::ReadInput { input, source } => {
                write!(formatter, "cannot read {input}: {source}")
            }
            Self::HeaderRejected { input, errors } => write!(
                formatter,
                "{input} has {errors} error-severity FITS diagnostics rejected by policy"
            ),
            Self::UnsupportedImageAxes { input, axes } => {
                write!(formatter, "{input} has unsupported FITS axes {axes:?}")
            }
            Self::InvalidImageDimensions { input, source } => {
                write!(formatter, "{input} has invalid dimensions: {source}")
            }
            Self::DimensionMismatch {
                input,
                expected,
                actual,
            } => write!(
                formatter,
                "{input} dimensions {}x{}x{} do not match {}x{}x{}",
                actual.width(),
                actual.height(),
                actual.planes(),
                expected.width(),
                expected.height(),
                expected.planes()
            ),
            Self::AllocationFailed { elements } => {
                write!(
                    formatter,
                    "cannot reserve temporary vector for {elements} elements"
                )
            }
            Self::Calibration(error) => Display::fmt(error, formatter),
            Self::Integration(error) => Display::fmt(error, formatter),
            Self::OutputImage(error) => Display::fmt(error, formatter),
            Self::OutputAssemblyInvariant => {
                formatter.write_str("integrated tile cannot be copied into the output image")
            }
            Self::Statistics(error) => Display::fmt(error, formatter),
            Self::Publish(error) => Display::fmt(error, formatter),
            Self::Cancelled(error) => Display::fmt(error, formatter),
            Self::StageId(error) => Display::fmt(error, formatter),
            Self::Progress(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for StrictPipelineError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::TileGrid(error) => Some(error),
            Self::Memory(error) => Some(error),
            Self::OpenInput { source, .. } => Some(source),
            Self::ReadInput { source, .. } => Some(source),
            Self::InvalidImageDimensions { source, .. } => Some(source),
            Self::Calibration(error) => Some(error),
            Self::Integration(error) => Some(error),
            Self::OutputImage(error) => Some(error),
            Self::Statistics(error) => Some(error),
            Self::Publish(error) => Some(error),
            Self::Cancelled(error) => Some(error),
            Self::StageId(error) => Some(error),
            Self::Progress(error) => Some(error),
            Self::NoSignals
            | Self::TooManySignals { .. }
            | Self::ProvenanceSourceCountMismatch { .. }
            | Self::ProvenanceAlgorithmMismatch
            | Self::WorkSizeOverflow
            | Self::HeaderRejected { .. }
            | Self::UnsupportedImageAxes { .. }
            | Self::DimensionMismatch { .. }
            | Self::AllocationFailed { .. }
            | Self::OutputAssemblyInvariant => None,
        }
    }
}

/// Runs the strict reference path from FITS inputs to one atomic FITS product.
///
/// Each spatial-plane tile loads the two masters and all signals, calibrates in
/// stable request order, integrates with the strict mean oracle, and copies the
/// result into the final image. Reopening one signal at a time keeps file
/// descriptor use bounded independently of the group size. The supplied memory
/// budget reserves the complete logical pixel working set before allocation.
/// Header parsing remains separately bounded by [`HeaderReadOptions`].
///
/// Progress is emitted synchronously. The callback receives a started event,
/// running events after each tile and statistics pass, and exactly one terminal
/// event after execution begins. Cancellation is checked before input inspection,
/// between inputs, between tiles, and before publication. No destination is
/// created when cancellation is observed before publication.
///
/// # Errors
///
/// Returns a typed validation, I/O, scientific-kernel, memory, cancellation,
/// progress, statistics, or atomic-publication failure.
pub fn run_strict_pipeline<F>(
    request: &StrictPipelineRequest,
    cancellation: &CancellationToken,
    memory: &MemoryBudget,
    mut progress: F,
) -> Result<StrictPipelineResult, StrictPipelineError>
where
    F: FnMut(ProgressEvent),
{
    let stage = StageId::new(PIPELINE_STAGE_ID).map_err(StrictPipelineError::StageId)?;
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

    let mut completed_units = 0_u64;
    let mut total_units = None;
    let execution = execute_pipeline(
        request,
        cancellation,
        memory,
        &sequence,
        &stage,
        &mut completed_units,
        &mut total_units,
        &mut progress,
    );

    match execution {
        Ok(result) => {
            emit_progress(
                &sequence,
                &stage,
                ProgressState::Completed,
                completed_units,
                total_units,
                None,
                &mut progress,
            )?;
            Ok(result)
        }
        Err(error) => {
            let state = if matches!(error, StrictPipelineError::Cancelled(_)) {
                ProgressState::Cancelled
            } else {
                ProgressState::Failed
            };
            // Preserve the scientific or I/O failure if the best-effort
            // terminal progress event itself cannot be constructed.
            let _ignored = emit_progress(
                &sequence,
                &stage,
                state,
                completed_units,
                total_units,
                Some(error.code().to_owned()),
                &mut progress,
            );
            Err(error)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_pipeline<F>(
    request: &StrictPipelineRequest,
    cancellation: &CancellationToken,
    memory: &MemoryBudget,
    sequence: &ProgressSequence,
    stage: &StageId,
    completed_units: &mut u64,
    total_units: &mut Option<u64>,
    progress: &mut F,
) -> Result<StrictPipelineResult, StrictPipelineError>
where
    F: FnMut(ProgressEvent),
{
    cancellation
        .checkpoint()
        .map_err(StrictPipelineError::Cancelled)?;
    let dimensions = validate_input_dimensions(request, cancellation)?;
    let grid = TileGrid::new(
        dimensions,
        request.tile_width,
        request.tile_height,
        Halo::default(),
    )
    .map_err(StrictPipelineError::TileGrid)?;
    let tile_count = checked_tile_count(grid)?;
    let run_total = tile_count
        .checked_add(2)
        .ok_or(StrictPipelineError::WorkSizeOverflow)?;
    *total_units = Some(run_total);

    let reserved_bytes = planned_working_set_bytes(
        dimensions,
        request.tile_width,
        request.tile_height,
        request.signals.len(),
    )?;
    let _reservation = memory
        .try_reserve(reserved_bytes)
        .map_err(StrictPipelineError::Memory)?;
    let mut output =
        ScientificImage::filled(dimensions, f64::NAN).map_err(StrictPipelineError::OutputImage)?;

    for tile in grid.iter() {
        cancellation
            .checkpoint()
            .map_err(StrictPipelineError::Cancelled)?;
        process_tile(request, cancellation, dimensions, tile, &mut output)?;
        *completed_units = completed_units
            .checked_add(1)
            .ok_or(StrictPipelineError::WorkSizeOverflow)?;
        emit_progress(
            sequence,
            stage,
            ProgressState::Running,
            *completed_units,
            *total_units,
            None,
            progress,
        )?;
    }

    cancellation
        .checkpoint()
        .map_err(StrictPipelineError::Cancelled)?;
    let statistics = image_statistics(&output).map_err(StrictPipelineError::Statistics)?;
    *completed_units = completed_units
        .checked_add(1)
        .ok_or(StrictPipelineError::WorkSizeOverflow)?;
    emit_progress(
        sequence,
        stage,
        ProgressState::Running,
        *completed_units,
        *total_units,
        None,
        progress,
    )?;

    cancellation
        .checkpoint()
        .map_err(StrictPipelineError::Cancelled)?;
    let write_summary =
        write_f64_primary_atomic_new_with_provenance(&request.output, &output, &request.provenance)
            .map_err(StrictPipelineError::Publish)?;
    *completed_units = completed_units
        .checked_add(1)
        .ok_or(StrictPipelineError::WorkSizeOverflow)?;

    Ok(StrictPipelineResult {
        statistics,
        write_summary,
        tiles_processed: tile_count,
        reserved_bytes,
    })
}

fn validate_input_dimensions(
    request: &StrictPipelineRequest,
    cancellation: &CancellationToken,
) -> Result<Dimensions, StrictPipelineError> {
    let first_role = PipelineInput::Signal { index: 0 };
    let dimensions = inspect_dimensions(
        &request.signals[0],
        first_role,
        request.header_options,
        request.validation_mode,
    )?;

    for (index, path) in request.signals.iter().enumerate().skip(1) {
        cancellation
            .checkpoint()
            .map_err(StrictPipelineError::Cancelled)?;
        validate_dimensions(
            path,
            PipelineInput::Signal { index },
            dimensions,
            request.header_options,
            request.validation_mode,
        )?;
    }
    cancellation
        .checkpoint()
        .map_err(StrictPipelineError::Cancelled)?;
    validate_dimensions(
        &request.dark,
        PipelineInput::Dark,
        dimensions,
        request.header_options,
        request.validation_mode,
    )?;
    cancellation
        .checkpoint()
        .map_err(StrictPipelineError::Cancelled)?;
    validate_dimensions(
        &request.normalized_flat,
        PipelineInput::NormalizedFlat,
        dimensions,
        request.header_options,
        request.validation_mode,
    )?;
    Ok(dimensions)
}

fn validate_dimensions(
    path: &Path,
    input: PipelineInput,
    expected: Dimensions,
    options: HeaderReadOptions,
    mode: ValidationMode,
) -> Result<(), StrictPipelineError> {
    let actual = inspect_dimensions(path, input, options, mode)?;
    if actual != expected {
        return Err(StrictPipelineError::DimensionMismatch {
            input,
            expected,
            actual,
        });
    }
    Ok(())
}

fn inspect_dimensions(
    path: &Path,
    input: PipelineInput,
    options: HeaderReadOptions,
    mode: ValidationMode,
) -> Result<Dimensions, StrictPipelineError> {
    let reader = open_reader(path, input, options, mode)?;
    dimensions_from_axes(input, reader.descriptor().axes())
}

fn open_reader(
    path: &Path,
    input: PipelineInput,
    options: HeaderReadOptions,
    mode: ValidationMode,
) -> Result<PrimaryImageReader<File>, StrictPipelineError> {
    let file =
        File::open(path).map_err(|source| StrictPipelineError::OpenInput { input, source })?;
    let reader = PrimaryImageReader::open(file, options)
        .map_err(|source| StrictPipelineError::ReadInput { input, source })?;
    if !reader.report().is_accepted(mode) {
        let errors = reader
            .report()
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.severity() == Severity::Error)
            .count();
        return Err(StrictPipelineError::HeaderRejected { input, errors });
    }
    Ok(reader)
}

fn dimensions_from_axes(
    input: PipelineInput,
    axes: &[u64],
) -> Result<Dimensions, StrictPipelineError> {
    let (width, height, planes) = match axes {
        [width, height] => (*width, *height, 1),
        [width, height, planes] => (*width, *height, *planes),
        _ => {
            return Err(StrictPipelineError::UnsupportedImageAxes {
                input,
                axes: axes.to_vec(),
            });
        }
    };
    let convert = |value| usize::try_from(value).ok();
    let Some((width, height, planes)) = convert(width)
        .zip(convert(height))
        .zip(convert(planes))
        .map(|((width, height), planes)| (width, height, planes))
    else {
        return Err(StrictPipelineError::UnsupportedImageAxes {
            input,
            axes: axes.to_vec(),
        });
    };
    Dimensions::new(width, height, planes)
        .map_err(|source| StrictPipelineError::InvalidImageDimensions { input, source })
}

fn process_tile(
    request: &StrictPipelineRequest,
    cancellation: &CancellationToken,
    dimensions: Dimensions,
    tile: Tile,
    output: &mut ScientificImage,
) -> Result<(), StrictPipelineError> {
    let core = tile.core();
    let region = ImageRegion::new(
        u64::try_from(tile.plane()).map_err(|_| StrictPipelineError::WorkSizeOverflow)?,
        u64::try_from(core.x()).map_err(|_| StrictPipelineError::WorkSizeOverflow)?,
        u64::try_from(core.y()).map_err(|_| StrictPipelineError::WorkSizeOverflow)?,
        u64::try_from(core.width()).map_err(|_| StrictPipelineError::WorkSizeOverflow)?,
        u64::try_from(core.height()).map_err(|_| StrictPipelineError::WorkSizeOverflow)?,
    );
    let dark = read_tile(
        &request.dark,
        PipelineInput::Dark,
        request,
        dimensions,
        region,
    )?;
    let flat = read_tile(
        &request.normalized_flat,
        PipelineInput::NormalizedFlat,
        request,
        dimensions,
        region,
    )?;

    let mut calibrated = Vec::new();
    calibrated
        .try_reserve_exact(request.signals.len())
        .map_err(|_| StrictPipelineError::AllocationFailed {
            elements: request.signals.len(),
        })?;
    for (index, path) in request.signals.iter().enumerate() {
        cancellation
            .checkpoint()
            .map_err(StrictPipelineError::Cancelled)?;
        let signal = read_tile(
            path,
            PipelineInput::Signal { index },
            request,
            dimensions,
            region,
        )?;
        calibrated.push(
            calibrate_dark_flat(&signal, &dark, &flat, request.calibration)
                .map_err(StrictPipelineError::Calibration)?,
        );
    }

    let mut references = Vec::new();
    references
        .try_reserve_exact(calibrated.len())
        .map_err(|_| StrictPipelineError::AllocationFailed {
            elements: calibrated.len(),
        })?;
    references.extend(calibrated.iter());
    let integration = integrate_mean(&references).map_err(StrictPipelineError::Integration)?;
    copy_tile(dimensions, tile, integration.image(), output)
}

fn read_tile(
    path: &Path,
    input: PipelineInput,
    request: &StrictPipelineRequest,
    expected: Dimensions,
    region: ImageRegion,
) -> Result<ScientificImage, StrictPipelineError> {
    let mut reader = open_reader(path, input, request.header_options, request.validation_mode)?;
    let actual = dimensions_from_axes(input, reader.descriptor().axes())?;
    if actual != expected {
        return Err(StrictPipelineError::DimensionMismatch {
            input,
            expected,
            actual,
        });
    }
    reader
        .read_region_image(region)
        .map_err(|source| StrictPipelineError::ReadInput { input, source })
}

fn copy_tile(
    dimensions: Dimensions,
    tile: Tile,
    source: &ScientificImage,
    output: &mut ScientificImage,
) -> Result<(), StrictPipelineError> {
    let core = tile.core();
    let expected = Dimensions::new(core.width(), core.height(), 1)
        .map_err(StrictPipelineError::OutputImage)?;
    if source.dimensions() != expected || output.dimensions() != dimensions {
        return Err(StrictPipelineError::OutputAssemblyInvariant);
    }

    let (output_pixels, output_mask) = output.pixels_and_mask_mut();
    for row in 0..core.height() {
        let source_start = row
            .checked_mul(core.width())
            .ok_or(StrictPipelineError::OutputAssemblyInvariant)?;
        let source_end = source_start
            .checked_add(core.width())
            .ok_or(StrictPipelineError::OutputAssemblyInvariant)?;
        let destination_y = core
            .y()
            .checked_add(row)
            .ok_or(StrictPipelineError::OutputAssemblyInvariant)?;
        let destination_start = dimensions
            .linear_index(core.x(), destination_y, tile.plane())
            .map_err(StrictPipelineError::OutputImage)?;
        let destination_end = destination_start
            .checked_add(core.width())
            .ok_or(StrictPipelineError::OutputAssemblyInvariant)?;

        let source_pixels = source
            .pixels()
            .get(source_start..source_end)
            .ok_or(StrictPipelineError::OutputAssemblyInvariant)?;
        let source_flags = source
            .mask()
            .as_slice()
            .get(source_start..source_end)
            .ok_or(StrictPipelineError::OutputAssemblyInvariant)?;
        let destination_pixels = output_pixels
            .get_mut(destination_start..destination_end)
            .ok_or(StrictPipelineError::OutputAssemblyInvariant)?;
        let destination_flags = output_mask
            .as_mut_slice()
            .get_mut(destination_start..destination_end)
            .ok_or(StrictPipelineError::OutputAssemblyInvariant)?;
        destination_pixels.copy_from_slice(source_pixels);
        destination_flags.copy_from_slice(source_flags);
    }
    Ok(())
}

fn checked_tile_count(grid: TileGrid) -> Result<u64, StrictPipelineError> {
    let dimensions = grid.dimensions();
    let columns = dimensions.width().div_ceil(grid.tile_width());
    let rows = dimensions.height().div_ceil(grid.tile_height());
    columns
        .checked_mul(rows)
        .and_then(|tiles| tiles.checked_mul(dimensions.planes()))
        .and_then(|tiles| u64::try_from(tiles).ok())
        .ok_or(StrictPipelineError::WorkSizeOverflow)
}

fn planned_working_set_bytes(
    dimensions: Dimensions,
    tile_width: usize,
    tile_height: usize,
    signal_count: usize,
) -> Result<usize, StrictPipelineError> {
    let image_sample_bytes = size_of::<f64>()
        .checked_add(size_of::<PixelFlags>())
        .ok_or(StrictPipelineError::WorkSizeOverflow)?;
    let output_bytes = dimensions
        .pixel_count()
        .checked_mul(image_sample_bytes)
        .ok_or(StrictPipelineError::WorkSizeOverflow)?;
    let maximum_tile_samples = tile_width
        .min(dimensions.width())
        .checked_mul(tile_height.min(dimensions.height()))
        .ok_or(StrictPipelineError::WorkSizeOverflow)?;

    // At calibration peak, references, previously calibrated signals, the
    // current signal, and its result coexist. At integration peak, the same
    // image count coexists with the support map. Reserve the larger auxiliary.
    let tile_image_count = signal_count
        .checked_add(3)
        .ok_or(StrictPipelineError::WorkSizeOverflow)?;
    let tile_images = maximum_tile_samples
        .checked_mul(image_sample_bytes)
        .and_then(|bytes| bytes.checked_mul(tile_image_count))
        .ok_or(StrictPipelineError::WorkSizeOverflow)?;
    let tile_auxiliary = maximum_tile_samples
        .checked_mul(size_of::<PixelSupport>().max(size_of::<SampleStatus>()))
        .ok_or(StrictPipelineError::WorkSizeOverflow)?;
    let vector_storage = signal_count
        .checked_mul(
            size_of::<ScientificImage>()
                .checked_add(size_of::<&ScientificImage>())
                .ok_or(StrictPipelineError::WorkSizeOverflow)?,
        )
        .ok_or(StrictPipelineError::WorkSizeOverflow)?;

    output_bytes
        .checked_add(tile_images)
        .and_then(|bytes| bytes.checked_add(tile_auxiliary))
        .and_then(|bytes| bytes.checked_add(vector_storage))
        .and_then(|bytes| bytes.checked_add(OUTPUT_BUFFER_BYTES))
        .ok_or(StrictPipelineError::WorkSizeOverflow)
}

#[allow(clippy::too_many_arguments)]
fn emit_progress<F>(
    sequence: &ProgressSequence,
    stage: &StageId,
    state: ProgressState,
    completed_units: u64,
    total_units: Option<u64>,
    code: Option<String>,
    progress: &mut F,
) -> Result<(), StrictPipelineError>
where
    F: FnMut(ProgressEvent),
{
    let event = sequence
        .next(stage.clone(), state, completed_units, total_units, code)
        .map_err(StrictPipelineError::Progress)?;
    progress(event);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use aether_fits::{SampleStatus, write_f64_primary_atomic_new};

    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn StdError>>;

    static TEST_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new() -> std::io::Result<Self> {
            let sequence = TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "aether-runtime-pipeline-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path)?;
            Ok(Self { path })
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ignored = fs::remove_dir_all(&self.path);
        }
    }

    fn write_image(path: &Path, dimensions: Dimensions, pixels: Vec<f64>) -> TestResult {
        let image = ScientificImage::from_pixels(dimensions, pixels)?;
        write_f64_primary_atomic_new(path, &image)?;
        Ok(())
    }

    fn provenance(source_count: u32, algorithm: &str) -> TestResult<FitsOutputProvenance> {
        Ok(FitsOutputProvenance::new(
            "a".repeat(64),
            "b".repeat(64),
            algorithm,
            source_count,
        )?)
    }

    fn write_standard_inputs(directory: &TestDirectory) -> TestResult<Vec<PathBuf>> {
        let dimensions = Dimensions::new(4, 2, 1)?;
        let first = directory.path.join("signal-1.fits");
        let second = directory.path.join("signal-2.fits");
        let dark = directory.path.join("dark.fits");
        let flat = directory.path.join("flat.fits");
        write_image(
            &first,
            dimensions,
            vec![10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0],
        )?;
        write_image(
            &second,
            dimensions,
            vec![14.0, 22.0, 28.0, 42.0, 46.0, 62.0, 74.0, 78.0],
        )?;
        write_image(&dark, dimensions, vec![2.0; 8])?;
        write_image(
            &flat,
            dimensions,
            vec![2.0, 1.0, 0.5, 2.0, 2.0, 1.0, 0.5, 2.0],
        )?;
        Ok(vec![first, second, dark, flat])
    }

    #[test]
    fn runs_the_strict_tiled_pipeline_and_publishes_provenance() -> TestResult {
        let directory = TestDirectory::new()?;
        let paths = write_standard_inputs(&directory)?;
        let output = directory.path.join("stack.fits");
        let request = StrictPipelineRequest::new(
            vec![paths[0].clone(), paths[1].clone()],
            paths[2].clone(),
            paths[3].clone(),
            output.clone(),
            provenance(2, STRICT_MEAN_ALGORITHM_ID)?,
            CalibrationParameters::new(1.0e-12)?,
        )?
        .with_tile_shape(2, 1)?;
        let cancellation = CancellationToken::new();
        let memory = MemoryBudget::new(1_048_576)?;
        let mut events = Vec::new();

        let result = run_strict_pipeline(&request, &cancellation, &memory, |event| {
            events.push(event);
        })?;
        let expected = [5.0, 19.0, 54.0, 19.5, 23.0, 59.0, 140.0, 38.5];
        let expected_statistics = image_statistics(&ScientificImage::from_pixels(
            Dimensions::new(4, 2, 1)?,
            expected.to_vec(),
        )?)?;

        assert_eq!(result.tiles_processed(), 4);
        assert_eq!(result.write_summary().samples_written(), 8);
        assert_eq!(result.statistics().usable_samples(), 8);
        assert_eq!(
            result.statistics().mean().to_bits(),
            expected_statistics.mean().to_bits()
        );
        assert_eq!(memory.used(), 0);
        assert_eq!(memory.peak(), result.reserved_bytes());

        assert_eq!(events.len(), 7);
        assert_eq!(events[0].state(), ProgressState::Started);
        assert_eq!(events[0].total_units(), None);
        assert_eq!(events[6].state(), ProgressState::Completed);
        assert_eq!(events[6].completed_units(), 6);
        assert_eq!(events[6].total_units(), Some(6));
        for (index, event) in events.iter().enumerate() {
            assert_eq!(event.sequence(), (index + 1) as u64);
        }

        let mut reader =
            PrimaryImageReader::open(File::open(&output)?, HeaderReadOptions::default())?;
        assert!(reader.report().is_conformant());
        assert_eq!(
            reader.report().header().string("AETHALG"),
            Some(STRICT_MEAN_ALGORITHM_ID)
        );
        assert_eq!(reader.report().header().integer("AETHSRC"), Some(2));
        let integrated = reader.read_region_image(ImageRegion::new(0, 0, 0, 4, 2))?;
        assert_eq!(
            integrated
                .pixels()
                .iter()
                .copied()
                .map(f64::to_bits)
                .collect::<Vec<_>>(),
            expected.map(f64::to_bits)
        );
        assert!(
            integrated
                .mask()
                .as_slice()
                .iter()
                .all(|flags| flags.is_clear())
        );
        Ok(())
    }

    #[test]
    fn cancellation_before_inspection_creates_no_output() -> TestResult {
        let directory = TestDirectory::new()?;
        let paths = write_standard_inputs(&directory)?;
        let output = directory.path.join("stack.fits");
        let request = StrictPipelineRequest::new(
            vec![paths[0].clone(), paths[1].clone()],
            paths[2].clone(),
            paths[3].clone(),
            output.clone(),
            provenance(2, STRICT_MEAN_ALGORITHM_ID)?,
            CalibrationParameters::new(0.0)?,
        )?;
        let cancellation = CancellationToken::new();
        assert!(cancellation.cancel());
        let memory = MemoryBudget::new(1_048_576)?;
        let mut events = Vec::new();

        let result = run_strict_pipeline(&request, &cancellation, &memory, |event| {
            events.push(event);
        });

        assert!(matches!(result, Err(StrictPipelineError::Cancelled(_))));
        assert!(!output.exists());
        assert_eq!(memory.used(), 0);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].state(), ProgressState::Started);
        assert_eq!(events[1].state(), ProgressState::Cancelled);
        assert_eq!(events[1].code(), Some("cancelled"));
        Ok(())
    }

    #[test]
    fn cancellation_between_tiles_never_publishes_a_partial_output() -> TestResult {
        let directory = TestDirectory::new()?;
        let paths = write_standard_inputs(&directory)?;
        let output = directory.path.join("stack.fits");
        let request = StrictPipelineRequest::new(
            vec![paths[0].clone(), paths[1].clone()],
            paths[2].clone(),
            paths[3].clone(),
            output.clone(),
            provenance(2, STRICT_MEAN_ALGORITHM_ID)?,
            CalibrationParameters::new(0.0)?,
        )?
        .with_tile_shape(1, 1)?;
        let cancellation = CancellationToken::new();
        let callback_token = cancellation.clone();
        let memory = MemoryBudget::new(1_048_576)?;
        let mut events = Vec::new();

        let result = run_strict_pipeline(&request, &cancellation, &memory, |event| {
            if event.state() == ProgressState::Running && event.completed_units() == 1 {
                let _first_request = callback_token.cancel();
            }
            events.push(event);
        });

        assert!(matches!(result, Err(StrictPipelineError::Cancelled(_))));
        assert!(!output.exists());
        assert_eq!(memory.used(), 0);
        assert_eq!(
            events.last().map(ProgressEvent::state),
            Some(ProgressState::Cancelled)
        );
        assert_eq!(events.last().map(ProgressEvent::completed_units), Some(1));
        Ok(())
    }

    #[test]
    fn tile_shape_does_not_change_strict_output_bytes() -> TestResult {
        let directory = TestDirectory::new()?;
        let paths = write_standard_inputs(&directory)?;
        let small_output = directory.path.join("small-tiles.fits");
        let whole_output = directory.path.join("whole-image-tile.fits");
        let small_tiles = StrictPipelineRequest::new(
            vec![paths[0].clone(), paths[1].clone()],
            paths[2].clone(),
            paths[3].clone(),
            small_output.clone(),
            provenance(2, STRICT_MEAN_ALGORITHM_ID)?,
            CalibrationParameters::new(0.0)?,
        )?
        .with_tile_shape(1, 1)?;
        let whole_image = StrictPipelineRequest::new(
            vec![paths[0].clone(), paths[1].clone()],
            paths[2].clone(),
            paths[3].clone(),
            whole_output.clone(),
            provenance(2, STRICT_MEAN_ALGORITHM_ID)?,
            CalibrationParameters::new(0.0)?,
        )?
        .with_tile_shape(4, 2)?;
        let memory = MemoryBudget::new(1_048_576)?;

        run_strict_pipeline(&small_tiles, &CancellationToken::new(), &memory, |_| {})?;
        run_strict_pipeline(&whole_image, &CancellationToken::new(), &memory, |_| {})?;

        assert_eq!(fs::read(small_output)?, fs::read(whole_output)?);
        assert_eq!(memory.used(), 0);
        Ok(())
    }

    #[test]
    fn insufficient_memory_fails_before_output_allocation() -> TestResult {
        let directory = TestDirectory::new()?;
        let paths = write_standard_inputs(&directory)?;
        let output = directory.path.join("stack.fits");
        let request = StrictPipelineRequest::new(
            vec![paths[0].clone(), paths[1].clone()],
            paths[2].clone(),
            paths[3].clone(),
            output.clone(),
            provenance(2, STRICT_MEAN_ALGORITHM_ID)?,
            CalibrationParameters::new(0.0)?,
        )?;
        let memory = MemoryBudget::new(1)?;
        let mut events = Vec::new();

        let result = run_strict_pipeline(&request, &CancellationToken::new(), &memory, |event| {
            events.push(event)
        });

        assert!(matches!(result, Err(StrictPipelineError::Memory(_))));
        assert!(!output.exists());
        assert_eq!(memory.used(), 0);
        assert_eq!(
            events.last().and_then(ProgressEvent::code),
            Some("memory-budget")
        );
        assert_eq!(
            events.last().map(ProgressEvent::state),
            Some(ProgressState::Failed)
        );
        Ok(())
    }

    #[test]
    fn dimension_mismatch_identifies_the_input_role() -> TestResult {
        let directory = TestDirectory::new()?;
        let signal = directory.path.join("signal.fits");
        let dark = directory.path.join("dark.fits");
        let flat = directory.path.join("flat.fits");
        let output = directory.path.join("stack.fits");
        write_image(&signal, Dimensions::new(2, 1, 1)?, vec![1.0, 2.0])?;
        write_image(&dark, Dimensions::new(1, 1, 1)?, vec![0.0])?;
        write_image(&flat, Dimensions::new(2, 1, 1)?, vec![1.0, 1.0])?;
        let request = StrictPipelineRequest::new(
            vec![signal],
            dark,
            flat,
            output.clone(),
            provenance(1, STRICT_MEAN_ALGORITHM_ID)?,
            CalibrationParameters::new(0.0)?,
        )?;

        let result = run_strict_pipeline(
            &request,
            &CancellationToken::new(),
            &MemoryBudget::new(1_048_576)?,
            |_| {},
        );

        assert!(matches!(
            result,
            Err(StrictPipelineError::DimensionMismatch {
                input: PipelineInput::Dark,
                ..
            })
        ));
        assert!(!output.exists());
        Ok(())
    }

    #[test]
    fn request_rejects_incoherent_provenance_and_tile_shape() -> TestResult {
        let calibration = CalibrationParameters::new(0.0)?;
        let empty = StrictPipelineRequest::new(
            Vec::new(),
            PathBuf::from("dark.fits"),
            PathBuf::from("flat.fits"),
            PathBuf::from("output.fits"),
            provenance(1, STRICT_MEAN_ALGORITHM_ID)?,
            calibration,
        );
        assert!(matches!(empty, Err(StrictPipelineError::NoSignals)));

        let mismatch = StrictPipelineRequest::new(
            vec![PathBuf::from("signal.fits")],
            PathBuf::from("dark.fits"),
            PathBuf::from("flat.fits"),
            PathBuf::from("output.fits"),
            provenance(2, STRICT_MEAN_ALGORITHM_ID)?,
            calibration,
        );
        assert!(matches!(
            mismatch,
            Err(StrictPipelineError::ProvenanceSourceCountMismatch { .. })
        ));

        let wrong_algorithm = StrictPipelineRequest::new(
            vec![PathBuf::from("signal.fits")],
            PathBuf::from("dark.fits"),
            PathBuf::from("flat.fits"),
            PathBuf::from("output.fits"),
            provenance(1, "other-v1")?,
            calibration,
        );
        assert!(matches!(
            wrong_algorithm,
            Err(StrictPipelineError::ProvenanceAlgorithmMismatch)
        ));

        let valid = StrictPipelineRequest::new(
            vec![PathBuf::from("signal.fits")],
            PathBuf::from("dark.fits"),
            PathBuf::from("flat.fits"),
            PathBuf::from("output.fits"),
            provenance(1, STRICT_MEAN_ALGORITHM_ID)?,
            calibration,
        )?;
        assert!(matches!(
            valid.with_tile_shape(0, 1),
            Err(StrictPipelineError::TileGrid(
                CoreError::ZeroTileExtent { .. }
            ))
        ));
        Ok(())
    }

    #[test]
    fn output_readback_keeps_non_finite_samples_explicit() -> TestResult {
        let directory = TestDirectory::new()?;
        let signal = directory.path.join("signal.fits");
        let dark = directory.path.join("dark.fits");
        let flat = directory.path.join("flat.fits");
        let output = directory.path.join("stack.fits");
        let dimensions = Dimensions::new(2, 1, 1)?;
        write_image(&signal, dimensions, vec![10.0, 20.0])?;
        write_image(&dark, dimensions, vec![2.0, 2.0])?;
        write_image(&flat, dimensions, vec![2.0, 0.0])?;
        let request = StrictPipelineRequest::new(
            vec![signal],
            dark,
            flat,
            output.clone(),
            provenance(1, STRICT_MEAN_ALGORITHM_ID)?,
            CalibrationParameters::new(0.0)?,
        )?;

        let result = run_strict_pipeline(
            &request,
            &CancellationToken::new(),
            &MemoryBudget::new(1_048_576)?,
            |_| {},
        )?;

        assert_eq!(result.statistics().usable_samples(), 1);
        assert_eq!(result.statistics().masked_samples(), 1);
        let mut reader =
            PrimaryImageReader::open(File::open(output)?, HeaderReadOptions::default())?;
        let mut values = [0.0; 2];
        let mut statuses = [SampleStatus::Valid; 2];
        reader.read_physical_samples(0, &mut values, &mut statuses)?;
        assert_eq!(values[0].to_bits(), 4.0_f64.to_bits());
        assert!(values[1].is_nan());
        assert_eq!(statuses, [SampleStatus::Valid, SampleStatus::NonFinite]);
        Ok(())
    }
}
