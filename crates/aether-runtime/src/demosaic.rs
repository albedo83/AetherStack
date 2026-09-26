use std::error::Error;
use std::fmt::{Display, Formatter};
use std::mem::size_of;
use std::path::{Path, PathBuf};

use aether_core::{Dimensions, PixelFlags};
use aether_demosaic::{CfaWindow, DemosaicError, MALVAR_HE_CUTLER_ALGORITHM_ID, RgbChannel};
use aether_fits::{
    AtomicF64PrimaryStreamWriter, AtomicFitsWriteError, FitsOutputProvenance, FitsWriteSummary,
    HeaderReadOptions, ImageReadError, ImageRegion, PrimaryImageReader, SampleStatus,
    ValidationMode,
};
use aether_metadata::BayerPattern;

use crate::pipeline::{dimensions_from_axes, open_reader, verify_source};
use crate::{
    CancellationToken, Cancelled, MemoryBudget, MemoryBudgetError, PipelineInput, PipelineSource,
    ProgressEvent, ProgressEventError, ProgressSequence, ProgressState, StageId, StageIdError,
    StrictPipelineError,
};

const DEFAULT_BAND_HEIGHT: usize = 128;
const DEMOSAIC_STAGE_ID: &str = "strict-demosaic";
const STREAM_WRITER_BUFFER_BYTES: usize = 64 * 1_024;
const HALO_RADIUS: usize = 2;

/// Validated request for one bounded calibrated-CFA to linear-RGB transaction.
#[derive(Clone, Debug)]
pub struct StrictDemosaicRequest {
    source: PipelineSource,
    output: PathBuf,
    provenance: FitsOutputProvenance,
    pattern: BayerPattern,
    band_height: usize,
    header_options: HeaderReadOptions,
    validation_mode: ValidationMode,
}

impl StrictDemosaicRequest {
    /// Builds a strict request using 128-row cores and a two-row halo.
    ///
    /// Provenance must identify exactly the supplied calibrated source and use
    /// [`MALVAR_HE_CUTLER_ALGORITHM_ID`]. The destination is published with
    /// create-new semantics and can never replace an existing file.
    ///
    /// # Errors
    ///
    /// Returns a typed error for incoherent provenance or an unknown CFA phase.
    pub fn new(
        source: PipelineSource,
        output: PathBuf,
        provenance: FitsOutputProvenance,
        pattern: BayerPattern,
    ) -> Result<Self, DemosaicPipelineError> {
        if provenance.algorithm_id() != MALVAR_HE_CUTLER_ALGORITHM_ID {
            return Err(DemosaicPipelineError::ProvenanceAlgorithmMismatch);
        }
        if provenance.source_count() != 1 {
            return Err(DemosaicPipelineError::ProvenanceSourceCount {
                actual: provenance.source_count(),
            });
        }
        if provenance.source_sha256() != Some(source.fingerprint().sha256()) {
            return Err(DemosaicPipelineError::ProvenanceSourceMismatch);
        }
        if let BayerPattern::Other(name) = &pattern {
            return Err(DemosaicPipelineError::Demosaic(
                DemosaicError::UnsupportedPattern(name.clone()),
            ));
        }
        Ok(Self {
            source,
            output,
            provenance,
            pattern,
            band_height: DEFAULT_BAND_HEIGHT,
            header_options: HeaderReadOptions::default(),
            validation_mode: ValidationMode::Strict,
        })
    }

    /// Replaces the output-core height used for bounded execution.
    ///
    /// This execution parameter cannot change output pixels or FITS bytes.
    ///
    /// # Errors
    ///
    /// Returns [`DemosaicPipelineError::ZeroBandHeight`] for zero.
    pub fn with_band_height(mut self, band_height: usize) -> Result<Self, DemosaicPipelineError> {
        if band_height == 0 {
            return Err(DemosaicPipelineError::ZeroBandHeight);
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

    /// Immutable calibrated CFA source.
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

/// Result of one completely published strict RGB product.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StrictDemosaicResult {
    dimensions: Dimensions,
    summary: FitsWriteSummary,
    peak_reserved_bytes: usize,
}

impl StrictDemosaicResult {
    /// Published planar RGB dimensions.
    #[must_use]
    pub const fn dimensions(&self) -> Dimensions {
        self.dimensions
    }

    /// Exact FITS sample, substitution, byte, and checksum accounting.
    #[must_use]
    pub const fn summary(&self) -> FitsWriteSummary {
        self.summary
    }

    /// Peak logical working set observed by the shared memory budget.
    #[must_use]
    pub const fn peak_reserved_bytes(&self) -> usize {
        self.peak_reserved_bytes
    }
}

/// Failure raised by the bounded demosaicing transaction.
#[derive(Debug)]
pub enum DemosaicPipelineError {
    /// Provenance names another algorithm.
    ProvenanceAlgorithmMismatch,
    /// A demosaiced frame must represent exactly one input.
    ProvenanceSourceCount {
        /// Received provenance source count.
        actual: u32,
    },
    /// Provenance does not carry the supplied source's exact SHA-256.
    ProvenanceSourceMismatch,
    /// Band height must be positive.
    ZeroBandHeight,
    /// Shared strict FITS/source validation failed.
    Input(StrictPipelineError),
    /// The input must contain exactly one CFA plane.
    SourcePlaneCount {
        /// Number of planes found in the source.
        actual: usize,
    },
    /// Derived work-unit or byte accounting overflowed.
    WorkSizeOverflow,
    /// The configured memory budget cannot reserve the complete planned peak.
    Memory(MemoryBudgetError),
    /// A source band could not be decoded.
    ReadInput(ImageReadError),
    /// The strict scientific kernel rejected the window or CFA declaration.
    Demosaic(DemosaicError),
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

impl DemosaicPipelineError {
    const fn code(&self) -> &'static str {
        match self {
            Self::ProvenanceAlgorithmMismatch => "demosaic-provenance-algorithm",
            Self::ProvenanceSourceCount { .. } => "demosaic-provenance-count",
            Self::ProvenanceSourceMismatch => "demosaic-provenance-source",
            Self::ZeroBandHeight => "demosaic-band-height",
            Self::Input(_) => "demosaic-input",
            Self::SourcePlaneCount { .. } => "demosaic-source-planes",
            Self::WorkSizeOverflow => "demosaic-work-size",
            Self::Memory(_) => "demosaic-memory",
            Self::ReadInput(_) => "demosaic-read",
            Self::Demosaic(_) => "demosaic-kernel",
            Self::Publish(_) => "demosaic-publish",
            Self::InvalidStagedOutput => "demosaic-readback",
            Self::Cancelled(_) => "cancelled",
            Self::StageId(_) => "demosaic-stage",
            Self::Progress(_) => "demosaic-progress",
        }
    }
}

impl Display for DemosaicPipelineError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProvenanceAlgorithmMismatch => formatter.write_str(
                "demosaicing provenance does not name the strict Malvar-He-Cutler algorithm",
            ),
            Self::ProvenanceSourceCount { actual } => write!(
                formatter,
                "demosaicing provenance must represent one source, received {actual}"
            ),
            Self::ProvenanceSourceMismatch => formatter
                .write_str("demosaicing provenance is not bound to the exact calibrated source"),
            Self::ZeroBandHeight => formatter.write_str("demosaicing band height must be positive"),
            Self::Input(error) => write!(formatter, "cannot validate demosaicing input: {error}"),
            Self::SourcePlaneCount { actual } => write!(
                formatter,
                "demosaicing source must have one CFA plane, received {actual}"
            ),
            Self::WorkSizeOverflow => {
                formatter.write_str("demosaicing work size cannot be represented")
            }
            Self::Memory(error) => write!(formatter, "cannot reserve demosaicing memory: {error}"),
            Self::ReadInput(error) => write!(formatter, "cannot read CFA source band: {error}"),
            Self::Demosaic(error) => write!(formatter, "cannot demosaic CFA source: {error}"),
            Self::Publish(error) => write!(formatter, "cannot publish demosaiced FITS: {error}"),
            Self::InvalidStagedOutput => {
                formatter.write_str("private demosaiced FITS failed exact readback validation")
            }
            Self::Cancelled(error) => Display::fmt(error, formatter),
            Self::StageId(error) => write!(formatter, "invalid demosaicing stage: {error}"),
            Self::Progress(error) => write!(formatter, "invalid demosaicing progress: {error}"),
        }
    }
}

impl Error for DemosaicPipelineError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Input(error) => Some(error),
            Self::Memory(error) => Some(error),
            Self::ReadInput(error) => Some(error),
            Self::Demosaic(error) => Some(error),
            Self::Publish(error) => Some(error),
            Self::Cancelled(error) => Some(error),
            Self::StageId(error) => Some(error),
            Self::Progress(error) => Some(error),
            Self::ProvenanceAlgorithmMismatch
            | Self::ProvenanceSourceCount { .. }
            | Self::ProvenanceSourceMismatch
            | Self::ZeroBandHeight
            | Self::SourcePlaneCount { .. }
            | Self::WorkSizeOverflow
            | Self::InvalidStagedOutput => None,
        }
    }
}

/// Executes one bounded, cancellable, atomic CFA-to-linear-RGB transaction.
///
/// Channels are written in canonical planar RGB order. Each channel traverses
/// top-to-bottom bands with a clipped two-row halo; the complete source and RGB
/// result are never resident in memory. The source is fingerprinted before
/// decoding and again after complete private output validation. Cancellation or
/// any failure before publication leaves the destination absent.
///
/// # Errors
///
/// Returns a typed provenance, source-integrity, FITS, memory, scientific,
/// progress, cancellation, readback, or atomic-publication failure.
pub fn run_strict_demosaic_pipeline<F>(
    request: &StrictDemosaicRequest,
    cancellation: &CancellationToken,
    memory: &MemoryBudget,
    mut progress: F,
) -> Result<StrictDemosaicResult, DemosaicPipelineError>
where
    F: FnMut(ProgressEvent),
{
    let stage = StageId::new(DEMOSAIC_STAGE_ID).map_err(DemosaicPipelineError::StageId)?;
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
            let state = if matches!(error, DemosaicPipelineError::Cancelled(_)) {
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
    request: &StrictDemosaicRequest,
    cancellation: &CancellationToken,
    memory: &MemoryBudget,
    sequence: &ProgressSequence,
    stage: &StageId,
    completed: &mut u64,
    total: &mut Option<u64>,
    progress: &mut F,
) -> Result<StrictDemosaicResult, DemosaicPipelineError>
where
    F: FnMut(ProgressEvent),
{
    cancellation
        .checkpoint()
        .map_err(DemosaicPipelineError::Cancelled)?;
    let input = PipelineInput::Signal { index: 0 };
    verify_source(&request.source, input).map_err(DemosaicPipelineError::Input)?;
    let mut reader = open_reader(
        request.source.path(),
        input,
        request.header_options,
        request.validation_mode,
    )
    .map_err(DemosaicPipelineError::Input)?;
    let source_dimensions = dimensions_from_axes(input, reader.descriptor().axes())
        .map_err(DemosaicPipelineError::Input)?;
    if source_dimensions.planes() != 1 {
        return Err(DemosaicPipelineError::SourcePlaneCount {
            actual: source_dimensions.planes(),
        });
    }
    if source_dimensions.width() < 2 || source_dimensions.height() < 2 {
        return Err(DemosaicPipelineError::Demosaic(
            DemosaicError::ImageTooSmall {
                width: source_dimensions.width(),
                height: source_dimensions.height(),
            },
        ));
    }
    let output_dimensions =
        Dimensions::new(source_dimensions.width(), source_dimensions.height(), 3)
            .map_err(DemosaicError::Core)
            .map_err(DemosaicPipelineError::Demosaic)?;
    let bands_per_plane = source_dimensions.height().div_ceil(request.band_height);
    let work_units = bands_per_plane
        .checked_mul(3)
        .and_then(|units| units.checked_add(1))
        .and_then(|units| u64::try_from(units).ok())
        .ok_or(DemosaicPipelineError::WorkSizeOverflow)?;
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

    let planned_bytes = planned_peak_bytes(source_dimensions, request.band_height)?;
    let _reservation = memory
        .try_reserve(planned_bytes)
        .map_err(DemosaicPipelineError::Memory)?;
    let mut writer = AtomicF64PrimaryStreamWriter::create_with_provenance(
        &request.output,
        output_dimensions,
        &request.provenance,
    )
    .map_err(DemosaicPipelineError::Publish)?;

    for channel in [RgbChannel::Red, RgbChannel::Green, RgbChannel::Blue] {
        for core_y in (0..source_dimensions.height()).step_by(request.band_height) {
            cancellation
                .checkpoint()
                .map_err(DemosaicPipelineError::Cancelled)?;
            let core_height = (source_dimensions.height() - core_y).min(request.band_height);
            let read_y = core_y.saturating_sub(HALO_RADIUS);
            let read_bottom = core_y
                .checked_add(core_height)
                .and_then(|bottom| bottom.checked_add(HALO_RADIUS))
                .unwrap_or(usize::MAX)
                .min(source_dimensions.height());
            let read_height = read_bottom - read_y;
            let region = ImageRegion::new(
                0,
                0,
                u64::try_from(read_y).map_err(|_| DemosaicPipelineError::WorkSizeOverflow)?,
                u64::try_from(source_dimensions.width())
                    .map_err(|_| DemosaicPipelineError::WorkSizeOverflow)?,
                u64::try_from(read_height).map_err(|_| DemosaicPipelineError::WorkSizeOverflow)?,
            );
            let source_band = reader
                .read_region_image(region)
                .map_err(DemosaicPipelineError::ReadInput)?;
            let window = CfaWindow::new(
                &source_band,
                0,
                read_y,
                source_dimensions.width(),
                source_dimensions.height(),
            )
            .map_err(DemosaicPipelineError::Demosaic)?;
            let output_band = window
                .reconstruct_channel(
                    &request.pattern,
                    channel,
                    0,
                    core_y,
                    source_dimensions.width(),
                    core_height,
                )
                .map_err(DemosaicPipelineError::Demosaic)?;
            writer
                .write_image_chunk(&output_band)
                .map_err(DemosaicPipelineError::Publish)?;
            *completed = completed
                .checked_add(1)
                .ok_or(DemosaicPipelineError::WorkSizeOverflow)?;
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

    let staged = writer.finish().map_err(DemosaicPipelineError::Publish)?;
    validate_staged_output(&staged, output_dimensions)?;
    cancellation
        .checkpoint()
        .map_err(DemosaicPipelineError::Cancelled)?;
    verify_source(&request.source, input).map_err(DemosaicPipelineError::Input)?;
    *completed = completed
        .checked_add(1)
        .ok_or(DemosaicPipelineError::WorkSizeOverflow)?;
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
        .map_err(DemosaicPipelineError::Cancelled)?;
    let summary = staged.publish().map_err(DemosaicPipelineError::Publish)?;
    Ok(StrictDemosaicResult {
        dimensions: output_dimensions,
        summary,
        peak_reserved_bytes: memory.peak(),
    })
}

fn planned_peak_bytes(
    dimensions: Dimensions,
    band_height: usize,
) -> Result<usize, DemosaicPipelineError> {
    let read_rows = dimensions
        .height()
        .min(band_height.saturating_add(HALO_RADIUS * 2));
    let read_samples = dimensions
        .width()
        .checked_mul(read_rows)
        .ok_or(DemosaicPipelineError::WorkSizeOverflow)?;
    let output_samples = dimensions
        .width()
        .checked_mul(dimensions.height().min(band_height))
        .ok_or(DemosaicPipelineError::WorkSizeOverflow)?;
    let image_sample_bytes = size_of::<f64>() + size_of::<PixelFlags>();
    let read_decode_peak = read_samples
        .checked_mul(image_sample_bytes + size_of::<SampleStatus>())
        .ok_or(DemosaicPipelineError::WorkSizeOverflow)?;
    let kernel_peak = read_samples
        .checked_mul(image_sample_bytes)
        .and_then(|bytes| {
            output_samples
                .checked_mul(image_sample_bytes)
                .and_then(|output| bytes.checked_add(output))
        })
        .ok_or(DemosaicPipelineError::WorkSizeOverflow)?;
    read_decode_peak
        .max(kernel_peak)
        .checked_add(STREAM_WRITER_BUFFER_BYTES)
        .ok_or(DemosaicPipelineError::WorkSizeOverflow)
}

fn validate_staged_output(
    staged: &aether_fits::CompletedAtomicFits,
    expected: Dimensions,
) -> Result<(), DemosaicPipelineError> {
    let file = staged
        .try_clone_for_readback()
        .map_err(DemosaicPipelineError::Publish)?;
    let mut reader = PrimaryImageReader::open(file, HeaderReadOptions::default())
        .map_err(DemosaicPipelineError::ReadInput)?;
    let actual = dimensions_from_axes(
        PipelineInput::Signal { index: 0 },
        reader.descriptor().axes(),
    )
    .map_err(DemosaicPipelineError::Input)?;
    let checksums = reader
        .verify_checksums()
        .map_err(DemosaicPipelineError::ReadInput)?;
    if actual != expected || !checksums.is_fully_verified() {
        return Err(DemosaicPipelineError::InvalidStagedOutput);
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
) -> Result<(), DemosaicPipelineError>
where
    F: FnMut(ProgressEvent),
{
    let event = sequence
        .next(stage.clone(), state, completed, total, code)
        .map_err(DemosaicPipelineError::Progress)?;
    progress(event);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File};
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    use aether_core::ScientificImage;
    use aether_demosaic::demosaic_malvar_he_cutler;
    use aether_fits::{HeaderReadOptions, write_f64_primary_atomic_new};
    use aether_session::fingerprint_reader;

    use super::*;

    type TestResult = Result<(), Box<dyn Error>>;
    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> std::io::Result<Self> {
            let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "aether-demosaic-runtime-{}-{sequence}",
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
        let dimensions = Dimensions::new(9, 7, 1)?;
        let pixels = (0..dimensions.pixel_count())
            .map(|index| ((index * 37 + 5) % 113) as f64 - 31.0)
            .collect();
        let image = ScientificImage::from_pixels(dimensions, pixels)?;
        let path = directory.0.join("calibrated.fits");
        write_f64_primary_atomic_new(&path, &image)?;
        let mut file = File::open(&path)?;
        let fingerprint = fingerprint_reader(&mut file)?;
        let provenance = FitsOutputProvenance::new(
            "a".repeat(64),
            "light-rggb",
            MALVAR_HE_CUTLER_ALGORITHM_ID,
            1,
        )?
        .with_source_sha256(fingerprint.sha256())?;
        Ok((PipelineSource::new(path, fingerprint), provenance, image))
    }

    #[test]
    fn band_height_does_not_change_output_bytes_or_oracle_pixels() -> TestResult {
        let directory = TestDirectory::new()?;
        let (source, provenance, input) = source_and_provenance(&directory)?;
        let first_output = directory.0.join("rgb-one-row.fits");
        let second_output = directory.0.join("rgb-four-rows.fits");
        let first = StrictDemosaicRequest::new(
            source.clone(),
            first_output.clone(),
            provenance.clone(),
            BayerPattern::Rggb,
        )?
        .with_band_height(1)?;
        let second = StrictDemosaicRequest::new(
            source,
            second_output.clone(),
            provenance,
            BayerPattern::Rggb,
        )?
        .with_band_height(4)?;
        let first_budget = MemoryBudget::new(1_000_000)?;
        let second_budget = MemoryBudget::new(1_000_000)?;
        let mut events = Vec::new();
        let result = run_strict_demosaic_pipeline(
            &first,
            &CancellationToken::new(),
            &first_budget,
            |event| events.push(event),
        )?;
        run_strict_demosaic_pipeline(&second, &CancellationToken::new(), &second_budget, |_| {})?;
        assert_eq!(fs::read(&first_output)?, fs::read(&second_output)?);
        assert_eq!(result.dimensions(), Dimensions::new(9, 7, 3)?);
        assert_eq!(
            events.first().map(ProgressEvent::state),
            Some(ProgressState::Started)
        );
        assert_eq!(
            events.last().map(ProgressEvent::state),
            Some(ProgressState::Completed)
        );

        let expected = demosaic_malvar_he_cutler(&input, &BayerPattern::Rggb)?;
        let file = File::open(first_output)?;
        let mut reader = PrimaryImageReader::open(file, HeaderReadOptions::default())?;
        let actual = reader.read_region_image(ImageRegion::new(0, 0, 0, 9, 7))?;
        assert_eq!(actual.pixels(), &expected.pixels()[..63]);
        let green = reader.read_region_image(ImageRegion::new(1, 0, 0, 9, 7))?;
        assert_eq!(green.pixels(), &expected.pixels()[63..126]);
        let blue = reader.read_region_image(ImageRegion::new(2, 0, 0, 9, 7))?;
        assert_eq!(blue.pixels(), &expected.pixels()[126..]);
        Ok(())
    }

    #[test]
    fn cancellation_and_memory_failure_publish_nothing() -> TestResult {
        let directory = TestDirectory::new()?;
        let (source, provenance, _) = source_and_provenance(&directory)?;
        let cancelled_output = directory.0.join("cancelled.fits");
        let cancelled = StrictDemosaicRequest::new(
            source.clone(),
            cancelled_output.clone(),
            provenance.clone(),
            BayerPattern::Rggb,
        )?;
        let token = CancellationToken::new();
        assert!(token.cancel());
        assert!(matches!(
            run_strict_demosaic_pipeline(
                &cancelled,
                &token,
                &MemoryBudget::new(1_000_000)?,
                |_| {}
            ),
            Err(DemosaicPipelineError::Cancelled(_))
        ));
        assert!(!cancelled_output.exists());

        let memory_output = directory.0.join("memory.fits");
        let memory_request = StrictDemosaicRequest::new(
            source,
            memory_output.clone(),
            provenance,
            BayerPattern::Rggb,
        )?;
        assert!(matches!(
            run_strict_demosaic_pipeline(
                &memory_request,
                &CancellationToken::new(),
                &MemoryBudget::new(1)?,
                |_| {}
            ),
            Err(DemosaicPipelineError::Memory(_))
        ));
        assert!(!memory_output.exists());
        Ok(())
    }

    #[test]
    fn source_mutation_before_publication_is_detected() -> TestResult {
        let directory = TestDirectory::new()?;
        let (source, provenance, _) = source_and_provenance(&directory)?;
        let source_path = source.path().to_path_buf();
        let output = directory.0.join("mutated.fits");
        let request =
            StrictDemosaicRequest::new(source, output.clone(), provenance, BayerPattern::Rggb)?
                .with_band_height(2)?;
        let mut changed = false;
        let result = run_strict_demosaic_pipeline(
            &request,
            &CancellationToken::new(),
            &MemoryBudget::new(1_000_000)?,
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
        assert!(matches!(result, Err(DemosaicPipelineError::Input(_))));
        assert!(!output.exists());
        Ok(())
    }

    #[test]
    fn request_rejects_incoherent_provenance_and_band_height() -> TestResult {
        let directory = TestDirectory::new()?;
        let (source, provenance, _) = source_and_provenance(&directory)?;
        let wrong_algorithm =
            FitsOutputProvenance::new("a".repeat(64), "light-rggb", "another-v1", 1)?
                .with_source_sha256(source.fingerprint().sha256())?;
        assert!(matches!(
            StrictDemosaicRequest::new(
                source.clone(),
                directory.0.join("wrong.fits"),
                wrong_algorithm,
                BayerPattern::Rggb
            ),
            Err(DemosaicPipelineError::ProvenanceAlgorithmMismatch)
        ));
        let valid = StrictDemosaicRequest::new(
            source,
            directory.0.join("valid.fits"),
            provenance,
            BayerPattern::Rggb,
        )?;
        assert!(matches!(
            valid.with_band_height(0),
            Err(DemosaicPipelineError::ZeroBandHeight)
        ));
        Ok(())
    }

    #[test]
    fn planned_memory_depends_on_band_height_not_full_image_height() -> TestResult {
        let short = planned_peak_bytes(Dimensions::new(4_144, 128, 1)?, 64)?;
        let asi294 = planned_peak_bytes(Dimensions::new(4_144, 2_822, 1)?, 64)?;
        let taller = planned_peak_bytes(Dimensions::new(4_144, 20_000, 1)?, 64)?;
        assert_eq!(short, asi294);
        assert_eq!(asi294, taller);
        assert!(asi294 < 6 * 1_024 * 1_024);
        Ok(())
    }
}
