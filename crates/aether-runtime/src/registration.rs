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
use aether_registration::{
    AffineTransform, LANCZOS3_RESAMPLING_ALGORITHM_ID, Lanczos3BandPlan, ResamplingError,
    ResamplingStatistics,
};

use crate::pipeline::{dimensions_from_axes, open_reader, verify_source};
use crate::{
    CancellationToken, Cancelled, MemoryBudget, MemoryBudgetError, PipelineInput, PipelineSource,
    ProgressEvent, ProgressEventError, ProgressSequence, ProgressState, StageId, StageIdError,
    StrictPipelineError,
};

const DEFAULT_BAND_HEIGHT: usize = 128;
const REGISTRATION_STAGE_ID: &str = "strict-registration";
const STREAM_WRITER_BUFFER_BYTES: usize = 64 * 1_024;

/// Validated request for one bounded, atomic registration transaction.
#[derive(Clone, Debug)]
pub struct StrictRegistrationRequest {
    source: PipelineSource,
    output: PathBuf,
    provenance: FitsOutputProvenance,
    source_to_reference: AffineTransform,
    output_width: usize,
    output_height: usize,
    band_height: usize,
    header_options: HeaderReadOptions,
    validation_mode: ValidationMode,
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
            source_to_reference,
            output_width,
            output_height,
            band_height: DEFAULT_BAND_HEIGHT,
            header_options: HeaderReadOptions::default(),
            validation_mode: ValidationMode::Strict,
        })
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
            | Self::ZeroBandHeight
            | Self::WorkSizeOverflow
            | Self::InvalidStagedOutput => None,
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
    let source_dimensions = dimensions_from_axes(input, reader.descriptor().axes())
        .map_err(RegistrationPipelineError::Input)?;
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
            let plan = Lanczos3BandPlan::new(
                plane_dimensions,
                request.output_width,
                request.output_height,
                reference_y,
                height,
                request.source_to_reference,
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

fn planned_band_bytes(plan: Lanczos3BandPlan) -> Result<usize, RegistrationPipelineError> {
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
    use aether_fits::write_f64_primary_atomic_new;
    use aether_registration::resample_lanczos3;
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
        let short = planned_band_bytes(Lanczos3BandPlan::new(
            short_dimensions,
            4_144,
            128,
            0,
            64,
            AffineTransform::IDENTITY,
        )?)?;
        let asi294 = planned_band_bytes(Lanczos3BandPlan::new(
            asi_dimensions,
            4_144,
            2_822,
            0,
            64,
            AffineTransform::IDENTITY,
        )?)?;
        let taller = planned_band_bytes(Lanczos3BandPlan::new(
            taller_dimensions,
            4_144,
            20_000,
            0,
            64,
            AffineTransform::IDENTITY,
        )?)?;
        assert_eq!(short, asi294);
        assert_eq!(asi294, taller);
        assert!(asi294 < 6 * 1_024 * 1_024);
        Ok(())
    }
}
