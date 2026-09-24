//! Native desktop shell for AetherStack.
//!
//! Scientific and review behavior lives in the workspace crates. This crate is
//! deliberately limited to the operating-system window and typed IPC adapters,
//! preventing the web presenter from becoming a second processing engine.

use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs::File;
use std::io::{Read, Seek};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use aether_fits::{
    DEFAULT_STATISTICS_CHUNK_SAMPLES, FITS_STATISTICS_ALGORITHM_ID, HeaderReadOptions, ImageRegion,
    PrimaryImageReader, StoredSampleFormat, primary_image_statistics,
};
use aether_metadata::{BayerPattern, FrameType};
use aether_preview::{
    AUTO_STRETCH_ALGORITHM_ID, AutomaticDisplayTransform, FitsPreviewParameters, MissingPixelStyle,
    PreviewLimits, RgbaPreview, ScalarPreview, build_fits_preview, choose_reduction_level,
    estimate_display_transform, render_grayscale_rgba8,
};
use aether_quality::{
    BackgroundParameters, CFA_CELL_MEAN_ALGORITHM_ID, FrameQualityError,
    GLOBAL_BACKGROUND_ALGORITHM_ID, STAR_MEASUREMENT_ALGORITHM_ID, StarMeasurementParameters,
    measure_frame_quality, prepare_cfa_cell_mean,
};
use aether_review::{
    DecisionChange, DecisionDelta, DisplayTransform, FrameId, FrameMetrics, FrameSpec,
    MAX_UNDO_DEPTH, ManualDecision, ManualRejectionReason, MissingPlacement, ReviewBook,
    ReviewError, ReviewState, SortDirection as ReviewSortDirection, SortField as ReviewSortField,
    SortSpec, TransferFunction,
};
use aether_session::{
    ClassificationPolicy, DirectoryManifestOptions, DirectoryManifestReport, ManifestFile,
    generate_manifest_from_directory,
};
use serde::{Deserialize, Serialize};
use tauri::ipc::Response;

const MAX_DESKTOP_PREVIEW_PIXELS: usize = 2 * 1_024 * 1_024;
const DESKTOP_PREVIEW_IO_CHUNK_SAMPLES: usize = 256 * 1_024;
const MAX_DESKTOP_QUALITY_SOURCE_PIXELS: u64 = 64 * 1_024 * 1_024;
const DESKTOP_QUALITY_PROFILE_ID: &str = "desktop-diagnostic-quality-v1";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FitsPreviewRequest {
    path: PathBuf,
    plane: u64,
    maximum_width: usize,
    maximum_height: usize,
    black_point: f64,
    white_point: f64,
    midtone: f64,
    transfer: PreviewTransfer,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FitsPreviewEstimateRequest {
    path: PathBuf,
    plane: u64,
    maximum_width: usize,
    maximum_height: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EstimatedDisplayTransform {
    algorithm_id: &'static str,
    black_point: f64,
    white_point: f64,
    midtone: f64,
    finite_samples: usize,
    median: f64,
    scaled_mad: f64,
    high_quantile: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FitsStatisticsRequest {
    path: PathBuf,
}

/// Exact bounded-memory summary of the complete primary FITS array.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FitsStatisticsResponse {
    algorithm_id: &'static str,
    axes: Vec<u64>,
    stored_format: &'static str,
    header_conformant: bool,
    header_diagnostics: usize,
    total_samples: usize,
    usable_samples: usize,
    undefined_samples: usize,
    non_finite_samples: usize,
    minimum: f64,
    maximum: f64,
    mean: f64,
    population_standard_deviation: f64,
    sample_standard_deviation: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FrameQualityRequest {
    path: PathBuf,
    interpretation: QualityInterpretation,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
enum QualityInterpretation {
    Monochrome,
    BayerCellMean { pattern: BayerPatternWire },
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BayerPatternWire {
    Rggb,
    Bggr,
    Grbg,
    Gbrg,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FrameQualityResponse {
    profile_id: &'static str,
    background_algorithm_id: &'static str,
    star_algorithm_id: &'static str,
    detection_plane_algorithm_id: &'static str,
    interpretation: &'static str,
    source_pixel_scale: f64,
    diagnostic_only: bool,
    background: f64,
    noise: f64,
    initial_usable_samples: usize,
    retained_background_samples: usize,
    masked_samples: usize,
    non_finite_samples: usize,
    detected_stars: usize,
    usable_stars: usize,
    saturation_level: Option<f64>,
    saturated_stars: Option<usize>,
    raw_candidates: usize,
    suppressed_candidates: usize,
    rejected_measurements: usize,
    fwhm_pixels: Option<f64>,
    eccentricity: Option<f64>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
enum PreviewTransfer {
    Linear,
    Midtones,
    Asinh { softness: f64 },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PreviewCommandError {
    code: &'static str,
    message: &'static str,
}

/// Session data safe to expose to the display-only presenter.
///
/// Absolute paths are retained only in process memory so the selected source
/// can be opened later. They are never written to the repository or included in
/// a portable session manifest.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportedSession {
    name: String,
    root_path: String,
    frames: Vec<ImportedFrame>,
    files_considered: usize,
    classification_conflicts: usize,
    recoverable_failures: Vec<ImportedFailure>,
    unassigned_sources: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportedFrame {
    id: String,
    role: &'static str,
    label: String,
    relative_path: String,
    path: String,
    exposure_seconds: Option<f64>,
    temperature_celsius: Option<f64>,
    camera: Option<String>,
    filter: Option<String>,
    bayer_pattern: Option<&'static str>,
    axes: Vec<u64>,
    fits_diagnostic_count: usize,
    classification_conflict: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportedFailure {
    relative_path: String,
    code: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReviewSortRequest {
    frames: Vec<ReviewSortFrame>,
    field: ReviewSortFieldWire,
    direction: ReviewSortDirectionWire,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReviewSortFrame {
    id: String,
    label: String,
    fwhm_pixels: Option<f64>,
    eccentricity: Option<f64>,
    detected_stars: Option<usize>,
    background: Option<f64>,
    noise: Option<f64>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReviewSortFieldWire {
    ProcessingOrder,
    Label,
    FwhmMajor,
    Eccentricity,
    DetectedStars,
    Background,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReviewSortDirectionWire {
    Ascending,
    Descending,
}

/// Native owner of the current session's manual review decisions.
///
/// The browser presenter receives small immutable updates, while the audited
/// transaction history remains in Rust and cannot be bypassed by local DOM
/// state. Re-importing a session replaces this book atomically.
#[derive(Debug, Default)]
struct DesktopReviewState {
    book: Mutex<Option<ReviewBook>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReviewDecisionRequest {
    frame_id: String,
    action: ReviewDecisionAction,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
enum ReviewDecisionAction {
    Accept,
    Reject { reason: ReviewRejectionReasonWire },
    Clear,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReviewRejectionReasonWire {
    Blur,
    Trailing,
    Cloud,
    IntrusiveTrail,
    Gradient,
    Framing,
    Saturation,
}

/// Minimal decision patch returned to the presenter after one transaction.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReviewDecisionEntry {
    frame_id: String,
    state: ReviewState,
    rejection_reason: Option<ManualRejectionReason>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReviewDecisionUpdate {
    generation: u64,
    can_undo: bool,
    changes: Vec<ReviewDecisionEntry>,
}

impl PreviewCommandError {
    const fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }
}

impl Display for PreviewCommandError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message)
    }
}

impl Error for PreviewCommandError {}

#[tauri::command]
async fn render_fits_preview(request: FitsPreviewRequest) -> Result<Response, PreviewCommandError> {
    validate_runtime_source_path(&request.path)?;
    tauri::async_runtime::spawn_blocking(move || {
        let file = File::open(&request.path).map_err(|_| {
            PreviewCommandError::new(
                "fits_open_failed",
                "The selected FITS file could not be opened.",
            )
        })?;
        let png = render_fits_preview_png(file, &request)?;
        Ok(Response::new(png))
    })
    .await
    .map_err(|_| preview_worker_error())?
}

#[tauri::command]
async fn estimate_fits_preview_transform(
    request: FitsPreviewEstimateRequest,
) -> Result<EstimatedDisplayTransform, PreviewCommandError> {
    validate_runtime_source_path(&request.path)?;
    tauri::async_runtime::spawn_blocking(move || {
        let file = File::open(&request.path).map_err(|_| {
            PreviewCommandError::new(
                "fits_open_failed",
                "The selected FITS file could not be opened.",
            )
        })?;
        estimate_fits_preview_transform_from_reader(file, &request)
    })
    .await
    .map_err(|_| preview_worker_error())?
}

#[tauri::command]
async fn inspect_fits_statistics(
    request: FitsStatisticsRequest,
) -> Result<FitsStatisticsResponse, PreviewCommandError> {
    validate_runtime_source_path(&request.path)?;
    tauri::async_runtime::spawn_blocking(move || inspect_fits_statistics_sync(&request.path))
        .await
        .map_err(|_| {
            PreviewCommandError::new(
                "fits_statistics_interrupted",
                "The FITS statistics worker stopped before producing a result.",
            )
        })?
}

#[tauri::command]
async fn inspect_frame_quality(
    request: FrameQualityRequest,
) -> Result<FrameQualityResponse, PreviewCommandError> {
    validate_runtime_source_path(&request.path)?;
    tauri::async_runtime::spawn_blocking(move || inspect_frame_quality_sync(&request))
        .await
        .map_err(|_| {
            PreviewCommandError::new(
                "frame_quality_interrupted",
                "The frame-quality worker stopped before producing a result.",
            )
        })?
}

fn validate_runtime_source_path(path: &Path) -> Result<(), PreviewCommandError> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(PreviewCommandError::new(
            "fits_path_not_absolute",
            "A native FITS operation requires an absolute source path.",
        ))
    }
}

fn inspect_fits_statistics_sync(
    path: &Path,
) -> Result<FitsStatisticsResponse, PreviewCommandError> {
    let file = File::open(path).map_err(|_| {
        PreviewCommandError::new(
            "fits_open_failed",
            "The selected FITS file could not be opened.",
        )
    })?;
    let mut reader =
        PrimaryImageReader::open(file, HeaderReadOptions::default()).map_err(|_| {
            PreviewCommandError::new(
                "fits_statistics_header_failed",
                "The selected FITS primary header could not be inspected.",
            )
        })?;
    let axes = reader.descriptor().axes().to_vec();
    let stored_format = stored_format_name(reader.descriptor().sample_format());
    let header_conformant = reader.report().is_conformant();
    let header_diagnostics = reader.report().diagnostics().len();
    let chunk_samples = NonZeroUsize::new(DEFAULT_STATISTICS_CHUNK_SAMPLES)
        .ok_or_else(fits_statistics_configuration_error)?;
    let statistics = primary_image_statistics(&mut reader, chunk_samples).map_err(|_| {
        PreviewCommandError::new(
            "fits_statistics_failed",
            "Exact statistics could not be calculated for this FITS primary array.",
        )
    })?;
    let moments = statistics.moments();
    Ok(FitsStatisticsResponse {
        algorithm_id: FITS_STATISTICS_ALGORITHM_ID,
        axes,
        stored_format,
        header_conformant,
        header_diagnostics,
        total_samples: moments.total_samples(),
        usable_samples: moments.usable_samples(),
        undefined_samples: statistics.undefined_samples(),
        non_finite_samples: statistics.non_finite_samples(),
        minimum: moments.minimum(),
        maximum: moments.maximum(),
        mean: moments.mean(),
        population_standard_deviation: moments.population_standard_deviation(),
        sample_standard_deviation: moments.sample_standard_deviation(),
    })
}

fn inspect_frame_quality_sync(
    request: &FrameQualityRequest,
) -> Result<FrameQualityResponse, PreviewCommandError> {
    let file = File::open(&request.path).map_err(|_| {
        PreviewCommandError::new(
            "fits_open_failed",
            "The selected FITS file could not be opened.",
        )
    })?;
    let mut reader =
        PrimaryImageReader::open(file, HeaderReadOptions::default()).map_err(|_| {
            PreviewCommandError::new(
                "frame_quality_header_failed",
                "The selected FITS primary header could not be inspected for quality measurement.",
            )
        })?;
    let [width, height] = reader.descriptor().axes() else {
        return Err(PreviewCommandError::new(
            "frame_quality_axes_unsupported",
            "Frame quality currently requires one two-dimensional FITS primary array.",
        ));
    };
    let source_samples = width.checked_mul(*height).ok_or_else(|| {
        PreviewCommandError::new(
            "frame_quality_size_overflow",
            "The frame dimensions exceed the supported quality-measurement range.",
        )
    })?;
    if source_samples > MAX_DESKTOP_QUALITY_SOURCE_PIXELS {
        return Err(PreviewCommandError::new(
            "frame_quality_source_too_large",
            "The frame exceeds the documented in-memory quality-measurement limit.",
        ));
    }
    let source = reader
        .read_region_image(ImageRegion::new(0, 0, 0, *width, *height))
        .map_err(|_| {
            PreviewCommandError::new(
                "frame_quality_decode_failed",
                "The complete linear FITS plane could not be decoded for quality measurement.",
            )
        })?;
    let (detection_plane, detection_plane_algorithm_id, interpretation, source_pixel_scale) =
        match request.interpretation {
            QualityInterpretation::Monochrome => {
                (source, "identity-monochrome-v1", "monochrome", 1.0)
            }
            QualityInterpretation::BayerCellMean { pattern } => {
                let interpretation = bayer_interpretation_name(pattern);
                let plane = prepare_cfa_cell_mean(source).map_err(|_| {
                    PreviewCommandError::new(
                        "frame_quality_cfa_preparation_failed",
                        "The raw CFA source could not be converted into complete Bayer-cell means.",
                    )
                })?;
                (plane, CFA_CELL_MEAN_ALGORITHM_ID, interpretation, 2.0)
            }
        };
    let background =
        BackgroundParameters::new(3.0, 8, 100).map_err(|_| frame_quality_configuration_error())?;
    let parameters = StarMeasurementParameters::new(background, 6.0, 2.0, 8, 4, 6, 100_000, None)
        .map_err(|_| frame_quality_configuration_error())?;
    let quality = measure_frame_quality(&detection_plane, 0, parameters)
        .map_err(frame_quality_measurement_error)?;
    let background = quality.background();
    let detected_stars = quality.stars().len();
    Ok(FrameQualityResponse {
        profile_id: DESKTOP_QUALITY_PROFILE_ID,
        background_algorithm_id: GLOBAL_BACKGROUND_ALGORITHM_ID,
        star_algorithm_id: STAR_MEASUREMENT_ALGORITHM_ID,
        detection_plane_algorithm_id,
        interpretation,
        source_pixel_scale,
        diagnostic_only: true,
        background: background.location(),
        noise: background.noise_sigma(),
        initial_usable_samples: background.initial_usable_samples(),
        retained_background_samples: background.retained_samples(),
        masked_samples: background.masked_samples(),
        non_finite_samples: background.non_finite_samples(),
        detected_stars,
        usable_stars: detected_stars,
        saturation_level: None,
        saturated_stars: None,
        raw_candidates: quality.raw_candidates(),
        suppressed_candidates: quality.suppressed_candidates(),
        rejected_measurements: quality.rejected_measurements(),
        fwhm_pixels: quality
            .median_fwhm_major_pixels()
            .map(|value| value * source_pixel_scale),
        eccentricity: quality.median_eccentricity(),
    })
}

const fn bayer_interpretation_name(pattern: BayerPatternWire) -> &'static str {
    match pattern {
        BayerPatternWire::Rggb => "raw CFA · RGGB",
        BayerPatternWire::Bggr => "raw CFA · BGGR",
        BayerPatternWire::Grbg => "raw CFA · GRBG",
        BayerPatternWire::Gbrg => "raw CFA · GBRG",
    }
}

const fn frame_quality_configuration_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "frame_quality_configuration_invalid",
        "The built-in diagnostic quality profile is invalid.",
    )
}

const fn frame_quality_measurement_error(error: FrameQualityError) -> PreviewCommandError {
    match error {
        FrameQualityError::TooManyCandidates { .. } => PreviewCommandError::new(
            "frame_quality_candidate_limit",
            "The frame exceeds the diagnostic profile's explicit stellar-candidate limit.",
        ),
        FrameQualityError::ZeroNoiseScale => PreviewCommandError::new(
            "frame_quality_zero_noise",
            "The prepared detection plane has no measurable robust noise scale.",
        ),
        FrameQualityError::Background(_) => PreviewCommandError::new(
            "frame_quality_background_failed",
            "The prepared detection plane has insufficient valid support for robust background estimation.",
        ),
        FrameQualityError::AllocationFailed { .. } => PreviewCommandError::new(
            "frame_quality_allocation_failed",
            "The diagnostic quality measurement could not reserve its bounded work buffers.",
        ),
        FrameQualityError::NumericalOverflow => PreviewCommandError::new(
            "frame_quality_numerical_failure",
            "The diagnostic quality measurement exceeded its finite numerical domain.",
        ),
        FrameQualityError::PlaneOutOfBounds { .. }
        | FrameQualityError::InvalidDetectionSigma { .. }
        | FrameQualityError::InvalidMeasurementFloorSigma { .. }
        | FrameQualityError::InvalidMeasurementRadius { .. }
        | FrameQualityError::ZeroMinimumSeparation
        | FrameQualityError::InvalidMinimumMeasurementPixels { .. }
        | FrameQualityError::ZeroMaximumCandidates
        | FrameQualityError::InvalidSaturationLevel
        | FrameQualityError::ImageTooSmall { .. } => frame_quality_configuration_error(),
    }
}

const fn stored_format_name(format: StoredSampleFormat) -> &'static str {
    match format {
        StoredSampleFormat::Unsigned8 => "unsigned 8-bit integer",
        StoredSampleFormat::Signed16 => "signed 16-bit integer",
        StoredSampleFormat::Signed32 => "signed 32-bit integer",
        StoredSampleFormat::Signed64 => "signed 64-bit integer",
        StoredSampleFormat::Float32 => "IEEE 754 binary32",
        StoredSampleFormat::Float64 => "IEEE 754 binary64",
    }
}

const fn fits_statistics_configuration_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "fits_statistics_configuration_invalid",
        "The FITS statistics buffer configuration is invalid.",
    )
}

#[tauri::command]
async fn import_session_directory(
    path: PathBuf,
    review_state: tauri::State<'_, DesktopReviewState>,
) -> Result<ImportedSession, PreviewCommandError> {
    if !path.is_absolute() {
        return Err(PreviewCommandError::new(
            "session_path_not_absolute",
            "The selected session directory must use an absolute path.",
        ));
    }
    let imported =
        tauri::async_runtime::spawn_blocking(move || import_session_directory_sync(&path))
            .await
            .map_err(|_| {
                PreviewCommandError::new(
                    "session_import_interrupted",
                    "The session import worker stopped before producing a result.",
                )
            })??;
    install_review_book(&review_state, &imported)?;
    Ok(imported)
}

#[tauri::command]
fn apply_review_decision(
    request: ReviewDecisionRequest,
    review_state: tauri::State<'_, DesktopReviewState>,
) -> Result<ReviewDecisionUpdate, PreviewCommandError> {
    apply_review_decision_sync(&review_state, request)
}

#[tauri::command]
fn undo_review_decision(
    review_state: tauri::State<'_, DesktopReviewState>,
) -> Result<ReviewDecisionUpdate, PreviewCommandError> {
    undo_review_decision_sync(&review_state)
}

fn install_review_book(
    review_state: &DesktopReviewState,
    imported: &ImportedSession,
) -> Result<(), PreviewCommandError> {
    let mut frames = review_entry_buffer(imported.frames.len())?;
    for frame in &imported.frames {
        let id = FrameId::new(frame.id.clone()).map_err(|_| review_state_input_error())?;
        frames.push(
            FrameSpec::new(id, frame.label.clone(), FrameMetrics::default())
                .map_err(|_| review_state_input_error())?,
        );
    }
    let book = if frames.is_empty() {
        None
    } else {
        Some(ReviewBook::new(frames, MAX_UNDO_DEPTH).map_err(|_| review_state_input_error())?)
    };
    *lock_review_state(review_state)? = book;
    Ok(())
}

fn apply_review_decision_sync(
    review_state: &DesktopReviewState,
    request: ReviewDecisionRequest,
) -> Result<ReviewDecisionUpdate, PreviewCommandError> {
    let frame_id = FrameId::new(request.frame_id).map_err(|_| review_decision_input_error())?;
    let change = match request.action {
        ReviewDecisionAction::Accept => DecisionChange::set(
            frame_id.clone(),
            ManualDecision::accept(None).map_err(|_| review_decision_input_error())?,
        ),
        ReviewDecisionAction::Reject { reason } => DecisionChange::set(
            frame_id.clone(),
            ManualDecision::reject(rejection_reason(reason), None)
                .map_err(|_| review_decision_input_error())?,
        ),
        ReviewDecisionAction::Clear => DecisionChange::clear(frame_id.clone()),
    };
    let mut state = lock_review_state(review_state)?;
    let book = state.as_mut().ok_or_else(review_state_missing_error)?;
    let preview = match book.preview_changes(&[change]) {
        Ok(preview) => preview,
        Err(ReviewError::NoEffectiveChanges) => {
            let mut changes = review_entry_buffer(1)?;
            changes.push(review_decision_entry(book, &frame_id)?);
            return Ok(review_decision_update(book, changes));
        }
        Err(_) => return Err(review_decision_failed_error()),
    };

    // Reserve the outbound patch before mutating the transaction engine. If
    // memory is exhausted, the browser and native state remain synchronized.
    let mut changes = review_entry_buffer(preview.changes().len())?;
    for delta in preview.changes() {
        changes.push(review_decision_entry_from_delta(delta));
    }
    book.apply_preview(preview)
        .map_err(|_| review_decision_failed_error())?;
    Ok(review_decision_update(book, changes))
}

fn undo_review_decision_sync(
    review_state: &DesktopReviewState,
) -> Result<ReviewDecisionUpdate, PreviewCommandError> {
    let mut state = lock_review_state(review_state)?;
    let book = state.as_mut().ok_or_else(review_state_missing_error)?;
    let change_count = book
        .pending_undo_change_count()
        .ok_or_else(review_nothing_to_undo_error)?;
    let mut changes = review_entry_buffer(change_count)?;
    let inverse = book.undo_with_deltas().map_err(|error| match error {
        ReviewError::NothingToUndo => review_nothing_to_undo_error(),
        _ => review_decision_failed_error(),
    })?;
    changes.extend(inverse.iter().map(review_decision_entry_from_delta));
    Ok(review_decision_update(book, changes))
}

fn review_decision_update(
    book: &ReviewBook,
    changes: Vec<ReviewDecisionEntry>,
) -> ReviewDecisionUpdate {
    ReviewDecisionUpdate {
        generation: book.generation(),
        can_undo: book.can_undo(),
        changes,
    }
}

fn review_decision_entry(
    book: &ReviewBook,
    frame_id: &FrameId,
) -> Result<ReviewDecisionEntry, PreviewCommandError> {
    let state = book
        .state(frame_id)
        .ok_or_else(review_decision_input_error)?;
    Ok(ReviewDecisionEntry {
        frame_id: frame_id.as_str().to_owned(),
        state,
        rejection_reason: book
            .decision(frame_id)
            .and_then(ManualDecision::rejection_reason),
    })
}

fn review_decision_entry_from_delta(delta: &DecisionDelta) -> ReviewDecisionEntry {
    let decision = delta.after();
    ReviewDecisionEntry {
        frame_id: delta.frame_id().as_str().to_owned(),
        state: decision.map_or(ReviewState::Undecided, ManualDecision::state),
        rejection_reason: decision.and_then(ManualDecision::rejection_reason),
    }
}

fn rejection_reason(reason: ReviewRejectionReasonWire) -> ManualRejectionReason {
    match reason {
        ReviewRejectionReasonWire::Blur => ManualRejectionReason::Blur,
        ReviewRejectionReasonWire::Trailing => ManualRejectionReason::Trailing,
        ReviewRejectionReasonWire::Cloud => ManualRejectionReason::Cloud,
        ReviewRejectionReasonWire::IntrusiveTrail => ManualRejectionReason::IntrusiveTrail,
        ReviewRejectionReasonWire::Gradient => ManualRejectionReason::Gradient,
        ReviewRejectionReasonWire::Framing => ManualRejectionReason::Framing,
        ReviewRejectionReasonWire::Saturation => ManualRejectionReason::Saturation,
    }
}

fn review_entry_buffer<T>(elements: usize) -> Result<Vec<T>, PreviewCommandError> {
    let mut output = Vec::new();
    output.try_reserve_exact(elements).map_err(|_| {
        PreviewCommandError::new(
            "review_state_allocation_failed",
            "The native review state could not reserve its bounded response buffer.",
        )
    })?;
    Ok(output)
}

fn lock_review_state(
    state: &DesktopReviewState,
) -> Result<MutexGuard<'_, Option<ReviewBook>>, PreviewCommandError> {
    state.book.lock().map_err(|_| {
        PreviewCommandError::new(
            "review_state_unavailable",
            "The native review state is unavailable after an internal synchronization failure.",
        )
    })
}

const fn review_state_input_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "review_state_input_invalid",
        "The imported session cannot initialize a valid native review state.",
    )
}

const fn review_state_missing_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "review_state_missing",
        "Import a non-empty FITS session before editing review decisions.",
    )
}

const fn review_decision_input_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "review_decision_input_invalid",
        "The requested review decision contains an invalid or unknown frame identity.",
    )
}

const fn review_decision_failed_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "review_decision_failed",
        "The native review transaction could not be applied atomically.",
    )
}

const fn review_nothing_to_undo_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "review_nothing_to_undo",
        "No applied review transaction remains to undo.",
    )
}

#[tauri::command]
async fn sort_review_frames(
    request: ReviewSortRequest,
) -> Result<Vec<String>, PreviewCommandError> {
    tauri::async_runtime::spawn_blocking(move || sort_review_frames_sync(request))
        .await
        .map_err(|_| {
            PreviewCommandError::new(
                "review_sort_interrupted",
                "The review sorting worker stopped before producing a result.",
            )
        })?
}

fn sort_review_frames_sync(request: ReviewSortRequest) -> Result<Vec<String>, PreviewCommandError> {
    let mut frames = Vec::new();
    frames
        .try_reserve_exact(request.frames.len())
        .map_err(|_| {
            PreviewCommandError::new(
                "review_sort_allocation_failed",
                "The review set is too large to sort in memory.",
            )
        })?;
    for frame in request.frames {
        let id = FrameId::new(frame.id).map_err(|_| review_sort_input_error())?;
        let metrics = FrameMetrics::new(
            frame.background,
            frame.noise,
            frame.detected_stars,
            None,
            frame.fwhm_pixels,
            frame.eccentricity,
        )
        .map_err(|_| review_sort_input_error())?;
        frames
            .push(FrameSpec::new(id, frame.label, metrics).map_err(|_| review_sort_input_error())?);
    }
    let review = ReviewBook::new(frames, 1).map_err(|_| review_sort_input_error())?;
    let field = match request.field {
        ReviewSortFieldWire::ProcessingOrder => ReviewSortField::ProcessingOrder,
        ReviewSortFieldWire::Label => ReviewSortField::Label,
        ReviewSortFieldWire::FwhmMajor => ReviewSortField::FwhmMajor,
        ReviewSortFieldWire::Eccentricity => ReviewSortField::Eccentricity,
        ReviewSortFieldWire::DetectedStars => ReviewSortField::DetectedStars,
        ReviewSortFieldWire::Background => ReviewSortField::Background,
    };
    let direction = match request.direction {
        ReviewSortDirectionWire::Ascending => ReviewSortDirection::Ascending,
        ReviewSortDirectionWire::Descending => ReviewSortDirection::Descending,
    };
    let sorted = review
        .sorted_frame_ids(SortSpec::new(field, direction, MissingPlacement::Last))
        .map_err(|_| {
            PreviewCommandError::new(
                "review_sort_failed",
                "The review frames could not be sorted deterministically.",
            )
        })?;
    Ok(sorted
        .into_iter()
        .map(|identity| identity.as_str().to_owned())
        .collect())
}

const fn review_sort_input_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "review_sort_input_invalid",
        "The review set contains an invalid identity, label, or metric.",
    )
}

fn import_session_directory_sync(root: &Path) -> Result<ImportedSession, PreviewCommandError> {
    let root_path = root.to_str().ok_or_else(|| {
        PreviewCommandError::new(
            "session_path_not_unicode",
            "The selected session path cannot be represented as Unicode.",
        )
    })?;
    let options = DirectoryManifestOptions {
        // Interactive directory import is an explicit structured-folder
        // operation. Prefer its nearest recognized role when a capture program
        // wrote a contradictory IMAGETYP, while retaining the conflict below.
        classification_policy: ClassificationPolicy::PreferDirectory,
        ..DirectoryManifestOptions::default()
    };
    let report = generate_manifest_from_directory(root, options).map_err(|_| {
        PreviewCommandError::new(
            "session_scan_failed",
            "The selected directory could not be scanned into a complete session.",
        )
    })?;
    imported_session_from_report(root, root_path, &report)
}

fn imported_session_from_report(
    root: &Path,
    root_path: &str,
    report: &DirectoryManifestReport,
) -> Result<ImportedSession, PreviewCommandError> {
    let name = root
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("Imported session")
        .to_owned();
    let mut frames = Vec::new();
    frames
        .try_reserve_exact(report.manifest().files().len())
        .map_err(|_| {
            PreviewCommandError::new(
                "session_allocation_failed",
                "The imported session is too large to represent in memory.",
            )
        })?;
    for file in report.manifest().files() {
        if let Some(frame) = imported_frame(root, file)? {
            frames.push(frame);
        }
    }

    let recoverable_failures = report
        .failures()
        .iter()
        .map(|failure| ImportedFailure {
            relative_path: failure.relative_path().to_string_lossy().into_owned(),
            code: failure.code().to_string(),
        })
        .collect();
    Ok(ImportedSession {
        name,
        root_path: root_path.to_owned(),
        frames,
        files_considered: report.fits_files_considered(),
        classification_conflicts: report
            .manifest()
            .files()
            .iter()
            .filter(|file| file.classification().has_conflict())
            .count(),
        recoverable_failures,
        unassigned_sources: report.unassigned_sources().to_vec(),
    })
}

fn imported_frame(
    root: &Path,
    file: &ManifestFile,
) -> Result<Option<ImportedFrame>, PreviewCommandError> {
    let Some(resolution) = file.resolution() else {
        return Ok(None);
    };
    let Some(role) = frame_role(resolution.frame_type()) else {
        return Ok(None);
    };
    let fingerprint = file.fingerprint();
    let id = FrameId::derive(
        file.relative_path(),
        fingerprint.byte_length(),
        fingerprint.sha256(),
    )
    .map_err(|_| {
        PreviewCommandError::new(
            "session_frame_identity_failed",
            "A session source could not be assigned a stable frame identity.",
        )
    })?;
    let source_path = join_portable_path(root, file.relative_path());
    let source_path = source_path.to_str().ok_or_else(|| {
        PreviewCommandError::new(
            "session_source_path_not_unicode",
            "An imported source path cannot be represented as Unicode.",
        )
    })?;
    let metadata = file.metadata();
    let temperature_celsius = metadata
        .sensor_temperature_c
        .as_ref()
        .or(metadata.set_temperature_c.as_ref())
        .map(|value| *value.value());
    let label = Path::new(file.relative_path())
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(file.relative_path())
        .to_owned();
    Ok(Some(ImportedFrame {
        id: id.as_str().to_owned(),
        role,
        label,
        relative_path: file.relative_path().to_owned(),
        path: source_path.to_owned(),
        exposure_seconds: metadata
            .exposure_seconds
            .as_ref()
            .map(|value| *value.value()),
        temperature_celsius,
        camera: metadata
            .camera
            .as_ref()
            .map(|value| value.value().canonical_name().to_owned()),
        filter: metadata
            .filter
            .as_ref()
            .map(|value| value.value().to_owned()),
        bayer_pattern: metadata
            .bayer_pattern
            .as_ref()
            .and_then(|value| standard_bayer_pattern(value.value())),
        axes: file.axes().to_vec(),
        fits_diagnostic_count: file.fits_diagnostics().len(),
        classification_conflict: file.classification().has_conflict(),
    }))
}

const fn standard_bayer_pattern(pattern: &BayerPattern) -> Option<&'static str> {
    match pattern {
        BayerPattern::Rggb => Some("rggb"),
        BayerPattern::Bggr => Some("bggr"),
        BayerPattern::Grbg => Some("grbg"),
        BayerPattern::Gbrg => Some("gbrg"),
        BayerPattern::Other(_) => None,
    }
}

/// Rebuilds an operating-system path from the manifest's portable separator.
///
/// Manifest paths always use `/`, including on Windows. Joining the complete
/// string would retain that separator in the display path on Windows, so each
/// already-validated component is joined independently.
fn join_portable_path(root: &Path, relative_path: &str) -> PathBuf {
    relative_path
        .split('/')
        .fold(root.to_owned(), |path, component| path.join(component))
}

const fn frame_role(frame_type: &FrameType) -> Option<&'static str> {
    match frame_type {
        FrameType::Bias => Some("bias"),
        FrameType::Dark => Some("dark"),
        FrameType::Flat => Some("flat"),
        FrameType::Light => Some("light"),
        FrameType::Other(_) => None,
    }
}

fn render_fits_preview_png<R: Read + Seek>(
    input: R,
    request: &FitsPreviewRequest,
) -> Result<Vec<u8>, PreviewCommandError> {
    let scalar = build_scalar_preview(
        input,
        request.plane,
        request.maximum_width,
        request.maximum_height,
    )?;
    let transfer_function = match request.transfer {
        PreviewTransfer::Linear => TransferFunction::Linear,
        PreviewTransfer::Midtones => TransferFunction::Midtones,
        PreviewTransfer::Asinh { softness } => TransferFunction::Asinh { softness },
    };
    let transform = DisplayTransform::new(
        request.black_point,
        request.white_point,
        request.midtone,
        transfer_function,
    )
    .map_err(|_| {
        PreviewCommandError::new(
            "display_transform_invalid",
            "The requested display transform is invalid.",
        )
    })?;
    let rgba = render_grayscale_rgba8(&scalar, transform, MissingPixelStyle::Checkerboard)
        .map_err(|_| {
            PreviewCommandError::new(
                "preview_mapping_failed",
                "The scalar preview could not be mapped for display.",
            )
        })?;
    encode_png(&rgba)
}

fn estimate_fits_preview_transform_from_reader<R: Read + Seek>(
    input: R,
    request: &FitsPreviewEstimateRequest,
) -> Result<EstimatedDisplayTransform, PreviewCommandError> {
    let scalar = build_scalar_preview(
        input,
        request.plane,
        request.maximum_width,
        request.maximum_height,
    )?;
    let estimate = estimate_display_transform(&scalar).map_err(|_| {
        PreviewCommandError::new(
            "preview_stretch_failed",
            "A robust display stretch could not be estimated from this frame.",
        )
    })?;
    Ok(estimated_transform(estimate))
}

fn estimated_transform(estimate: AutomaticDisplayTransform) -> EstimatedDisplayTransform {
    let transform = estimate.transform();
    EstimatedDisplayTransform {
        algorithm_id: AUTO_STRETCH_ALGORITHM_ID,
        black_point: transform.black_point(),
        white_point: transform.white_point(),
        midtone: transform.midtone(),
        finite_samples: estimate.finite_samples(),
        median: estimate.median(),
        scaled_mad: estimate.scaled_mad(),
        high_quantile: estimate.high_quantile(),
    }
}

fn build_scalar_preview<R: Read + Seek>(
    input: R,
    plane: u64,
    maximum_width: usize,
    maximum_height: usize,
) -> Result<ScalarPreview, PreviewCommandError> {
    let requested_pixels = maximum_width
        .checked_mul(maximum_height)
        .ok_or_else(preview_bounds_error)?;
    let limits = PreviewLimits::new(
        maximum_width,
        maximum_height,
        requested_pixels.min(MAX_DESKTOP_PREVIEW_PIXELS),
    )
    .map_err(|_| preview_bounds_error())?;

    let mut reader =
        PrimaryImageReader::open(input, HeaderReadOptions::default()).map_err(|_| {
            PreviewCommandError::new(
                "fits_layout_unsupported",
                "The file does not contain a supported primary FITS image.",
            )
        })?;
    let axes = reader.descriptor().axes();
    let (source_width, source_height) = match axes {
        [width, height] | [width, height, _] => (*width, *height),
        _ => {
            return Err(PreviewCommandError::new(
                "fits_axes_unsupported",
                "The FITS preview requires a two- or three-axis primary image.",
            ));
        }
    };
    let reduction_level =
        choose_reduction_level(source_width, source_height, limits).map_err(|_| {
            PreviewCommandError::new(
                "preview_bounds_unsatisfied",
                "The image cannot be reduced within the preview safety limits.",
            )
        })?;
    let parameters = FitsPreviewParameters::new(
        plane,
        reduction_level,
        limits.maximum_pixels(),
        DESKTOP_PREVIEW_IO_CHUNK_SAMPLES,
    )
    .map_err(|_| preview_bounds_error())?;
    build_fits_preview(&mut reader, parameters).map_err(|_| {
        PreviewCommandError::new(
            "preview_decode_failed",
            "The FITS pixels could not be decoded into a bounded preview.",
        )
    })
}

fn encode_png(preview: &RgbaPreview) -> Result<Vec<u8>, PreviewCommandError> {
    let width = u32::try_from(preview.width()).map_err(|_| preview_encoding_error())?;
    let height = u32::try_from(preview.height()).map_err(|_| preview_encoding_error())?;
    let mut output = Vec::new();
    let mut encoder = png::Encoder::new(&mut output, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|_| preview_encoding_error())?;
    writer
        .write_image_data(preview.pixels())
        .map_err(|_| preview_encoding_error())?;
    writer.finish().map_err(|_| preview_encoding_error())?;
    Ok(output)
}

const fn preview_bounds_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "preview_bounds_invalid",
        "The requested preview dimensions are outside the supported bounds.",
    )
}

const fn preview_encoding_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "preview_encoding_failed",
        "The display preview could not be encoded.",
    )
}

const fn preview_worker_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "preview_worker_interrupted",
        "The preview worker stopped before producing a result.",
    )
}

/// Runs the native AetherStack application until its final window closes.
///
/// # Errors
///
/// Returns a Tauri runtime error if the desktop shell cannot be initialized or
/// the native event loop terminates abnormally.
pub fn run() -> Result<(), tauri::Error> {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(DesktopReviewState::default())
        .invoke_handler(tauri::generate_handler![
            apply_review_decision,
            estimate_fits_preview_transform,
            import_session_directory,
            inspect_frame_quality,
            inspect_fits_statistics,
            render_fits_preview,
            sort_review_frames,
            undo_review_decision
        ])
        .run(tauri::generate_context!())
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File};
    use std::io::Cursor;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};

    use aether_core::{Dimensions, ScientificImage};
    use aether_fits::write_f64_primary;

    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new() -> std::io::Result<Self> {
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "aether-desktop-import-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path)?;
            Ok(Self { path })
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ignored = fs::remove_dir_all(&self.path);
        }
    }

    fn request(transfer: PreviewTransfer) -> FitsPreviewRequest {
        FitsPreviewRequest {
            path: PathBuf::from("unused-in-memory-test.fits"),
            plane: 0,
            maximum_width: 4,
            maximum_height: 2,
            black_point: 0.0,
            white_point: 8.0,
            midtone: 0.5,
            transfer,
        }
    }

    fn estimate_request() -> FitsPreviewEstimateRequest {
        FitsPreviewEstimateRequest {
            path: PathBuf::from("unused-in-memory-test.fits"),
            plane: 0,
            maximum_width: 4,
            maximum_height: 2,
        }
    }

    fn fits_bytes() -> TestResult<Vec<u8>> {
        let image = ScientificImage::from_pixels(
            Dimensions::new(4, 2, 1)?,
            (0..8).map(f64::from).collect(),
        )?;
        let mut bytes = Vec::new();
        write_f64_primary(&mut bytes, &image)?;
        Ok(bytes)
    }

    fn quality_fits_bytes() -> TestResult<Vec<u8>> {
        const CELL_WIDTH: usize = 64;
        const CELL_HEIGHT: usize = 64;
        let mut pixels = Vec::new();
        pixels.try_reserve_exact(CELL_WIDTH * CELL_HEIGHT * 4)?;
        for source_y in 0..CELL_HEIGHT * 2 {
            for source_x in 0..CELL_WIDTH * 2 {
                let x = (source_x / 2) as f64;
                let y = (source_y / 2) as f64;
                let dx = x - 31.75;
                let dy = y - 32.25;
                let noise = ((source_x / 2 + 3 * (source_y / 2)) % 5) as f64 - 2.0;
                pixels.push(1_000.0 + noise + 500.0 * (-(dx * dx + dy * dy) / 18.0).exp());
            }
        }
        let image = ScientificImage::from_pixels(
            Dimensions::new(CELL_WIDTH * 2, CELL_HEIGHT * 2, 1)?,
            pixels,
        )?;
        let mut bytes = Vec::new();
        write_f64_primary(&mut bytes, &image)?;
        Ok(bytes)
    }

    #[test]
    fn renders_a_png_with_the_bounded_preview_dimensions() -> TestResult {
        let encoded = render_fits_preview_png(
            Cursor::new(fits_bytes()?),
            &request(PreviewTransfer::Linear),
        )?;

        assert_eq!(&encoded[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&encoded[12..16], b"IHDR");
        assert_eq!(u32::from_be_bytes(encoded[16..20].try_into()?), 4);
        assert_eq!(u32::from_be_bytes(encoded[20..24].try_into()?), 2);
        Ok(())
    }

    #[test]
    fn rejects_an_invalid_display_transform_before_mapping() -> TestResult {
        let mut invalid = request(PreviewTransfer::Midtones);
        invalid.white_point = invalid.black_point;

        let Err(error) = render_fits_preview_png(Cursor::new(fits_bytes()?), &invalid) else {
            return Err("equal display points were accepted".into());
        };
        assert_eq!(error.code, "display_transform_invalid");
        Ok(())
    }

    #[test]
    fn rejects_zero_output_bounds() -> TestResult {
        let mut invalid = request(PreviewTransfer::Asinh { softness: 0.1 });
        invalid.maximum_width = 0;

        let Err(error) = render_fits_preview_png(Cursor::new(fits_bytes()?), &invalid) else {
            return Err("zero preview width was accepted".into());
        };
        assert_eq!(error.code, "preview_bounds_invalid");
        Ok(())
    }

    #[test]
    fn native_fits_operations_require_absolute_source_paths() -> TestResult {
        let error = validate_runtime_source_path(Path::new("relative/frame.fits"))
            .err()
            .ok_or("relative source paths must be rejected")?;

        assert_eq!(error.code, "fits_path_not_absolute");
        Ok(())
    }

    #[test]
    fn exposes_only_supported_standard_bayer_patterns() {
        assert_eq!(standard_bayer_pattern(&BayerPattern::Rggb), Some("rggb"));
        assert_eq!(standard_bayer_pattern(&BayerPattern::Bggr), Some("bggr"));
        assert_eq!(standard_bayer_pattern(&BayerPattern::Grbg), Some("grbg"));
        assert_eq!(standard_bayer_pattern(&BayerPattern::Gbrg), Some("gbrg"));
        assert_eq!(
            standard_bayer_pattern(&BayerPattern::Other("XTRANS".to_owned())),
            None
        );
    }

    #[test]
    fn estimates_an_inspectable_automatic_transform() -> TestResult {
        let estimate = estimate_fits_preview_transform_from_reader(
            Cursor::new(fits_bytes()?),
            &estimate_request(),
        )?;

        assert_eq!(estimate.algorithm_id, AUTO_STRETCH_ALGORITHM_ID);
        assert_eq!(estimate.finite_samples, 8);
        assert!(estimate.black_point < estimate.median);
        assert!(estimate.white_point > estimate.median);
        assert!(estimate.scaled_mad > 0.0);
        assert!((0.0..1.0).contains(&estimate.midtone));
        Ok(())
    }

    #[test]
    fn calculates_exact_bounded_statistics_for_the_primary_array() -> TestResult {
        let directory = TestDirectory::new()?;
        let path = directory.path().join("statistics.fits");
        fs::write(&path, fits_bytes()?)?;

        let statistics = inspect_fits_statistics_sync(&path)?;

        assert_eq!(statistics.algorithm_id, FITS_STATISTICS_ALGORITHM_ID);
        assert_eq!(statistics.axes, [4, 2]);
        assert_eq!(statistics.stored_format, "IEEE 754 binary64");
        assert!(statistics.header_conformant);
        assert_eq!(statistics.header_diagnostics, 0);
        assert_eq!(statistics.total_samples, 8);
        assert_eq!(statistics.usable_samples, 8);
        assert_eq!(statistics.undefined_samples, 0);
        assert_eq!(statistics.non_finite_samples, 0);
        assert_eq!(statistics.minimum.to_bits(), 0.0_f64.to_bits());
        assert_eq!(statistics.maximum.to_bits(), 7.0_f64.to_bits());
        assert_eq!(statistics.mean.to_bits(), 3.5_f64.to_bits());
        assert!(statistics.population_standard_deviation > 2.0);
        assert!(statistics.sample_standard_deviation.is_some());
        Ok(())
    }

    #[test]
    fn measures_a_bayer_frame_on_a_phase_neutral_detection_plane() -> TestResult {
        let directory = TestDirectory::new()?;
        let path = directory.path().join("quality.fits");
        fs::write(&path, quality_fits_bytes()?)?;
        let request = FrameQualityRequest {
            path,
            interpretation: QualityInterpretation::BayerCellMean {
                pattern: BayerPatternWire::Rggb,
            },
        };

        let quality = inspect_frame_quality_sync(&request)?;

        assert_eq!(quality.profile_id, DESKTOP_QUALITY_PROFILE_ID);
        assert_eq!(
            quality.detection_plane_algorithm_id,
            CFA_CELL_MEAN_ALGORITHM_ID
        );
        assert_eq!(quality.interpretation, "raw CFA · RGGB");
        assert_eq!(quality.source_pixel_scale.to_bits(), 2.0_f64.to_bits());
        assert!(quality.diagnostic_only);
        assert!(quality.noise > 0.0);
        assert_eq!(quality.detected_stars, 1);
        assert_eq!(quality.usable_stars, 1);
        assert!(quality.fwhm_pixels.is_some_and(|value| value > 10.0));
        assert!(quality.eccentricity.is_some_and(|value| value < 0.2));
        Ok(())
    }

    #[test]
    fn sorts_review_rows_in_rust_with_missing_metrics_last() -> TestResult {
        let frame = |digit: char, label: &str, fwhm_pixels: Option<f64>| ReviewSortFrame {
            id: digit.to_string().repeat(64),
            label: label.to_owned(),
            fwhm_pixels,
            eccentricity: None,
            detected_stars: None,
            background: None,
            noise: None,
        };
        let sorted = sort_review_frames_sync(ReviewSortRequest {
            frames: vec![
                frame('a', "third", Some(3.0)),
                frame('b', "missing", None),
                frame('c', "first", Some(2.0)),
            ],
            field: ReviewSortFieldWire::FwhmMajor,
            direction: ReviewSortDirectionWire::Ascending,
        })?;

        assert_eq!(sorted, vec!["c".repeat(64), "a".repeat(64), "b".repeat(64)]);
        Ok(())
    }

    #[test]
    fn imports_a_directory_classified_light_with_stable_identity() -> TestResult {
        let directory = TestDirectory::new()?;
        let lights = directory.path().join("LIGHTS");
        fs::create_dir(&lights)?;
        let source_path = lights.join("frame-0001.fits");
        let image = ScientificImage::from_pixels(
            Dimensions::new(4, 2, 1)?,
            (0..8).map(f64::from).collect(),
        )?;
        let mut source = File::create(&source_path)?;
        write_f64_primary(&mut source, &image)?;
        drop(source);

        let imported = import_session_directory_sync(directory.path())?;

        assert_eq!(imported.files_considered, 1);
        assert!(imported.recoverable_failures.is_empty());
        assert_eq!(imported.frames.len(), 1);
        let frame = &imported.frames[0];
        assert_eq!(frame.role, "light");
        assert_eq!(frame.label, "frame-0001.fits");
        assert_eq!(frame.relative_path, "LIGHTS/frame-0001.fits");
        assert_eq!(frame.axes, [4, 2]);
        assert_eq!(frame.id.len(), 64);
        assert_eq!(frame.path, source_path.to_string_lossy());
        Ok(())
    }

    #[test]
    fn applies_idempotent_review_decisions_and_undoes_native_transactions() -> TestResult {
        let imported = ImportedSession {
            name: "transaction test".to_owned(),
            root_path: "/runtime-only".to_owned(),
            frames: vec![
                imported_review_test_frame('a', "first.fits"),
                imported_review_test_frame('b', "second.fits"),
            ],
            files_considered: 2,
            classification_conflicts: 0,
            recoverable_failures: Vec::new(),
            unassigned_sources: Vec::new(),
        };
        let state = DesktopReviewState::default();
        install_review_book(&state, &imported)?;

        let accepted = apply_review_decision_sync(
            &state,
            ReviewDecisionRequest {
                frame_id: "a".repeat(64),
                action: ReviewDecisionAction::Accept,
            },
        )?;
        assert_eq!(accepted.generation, 1);
        assert!(accepted.can_undo);
        assert_eq!(accepted.changes.len(), 1);
        assert_eq!(accepted.changes[0].state, ReviewState::Accepted);

        let repeated = apply_review_decision_sync(
            &state,
            ReviewDecisionRequest {
                frame_id: "a".repeat(64),
                action: ReviewDecisionAction::Accept,
            },
        )?;
        assert_eq!(repeated.generation, 1);
        assert_eq!(repeated.changes[0].state, ReviewState::Accepted);

        let rejected = apply_review_decision_sync(
            &state,
            ReviewDecisionRequest {
                frame_id: "b".repeat(64),
                action: ReviewDecisionAction::Reject {
                    reason: ReviewRejectionReasonWire::Trailing,
                },
            },
        )?;
        assert_eq!(rejected.generation, 2);
        assert_eq!(rejected.changes[0].state, ReviewState::Rejected);
        assert_eq!(
            rejected.changes[0].rejection_reason,
            Some(ManualRejectionReason::Trailing)
        );

        let first_undo = undo_review_decision_sync(&state)?;
        assert_eq!(first_undo.generation, 3);
        assert!(first_undo.can_undo);
        assert_eq!(first_undo.changes[0].frame_id, "b".repeat(64));
        assert_eq!(first_undo.changes[0].state, ReviewState::Undecided);

        let second_undo = undo_review_decision_sync(&state)?;
        assert_eq!(second_undo.generation, 4);
        assert!(!second_undo.can_undo);
        assert_eq!(second_undo.changes[0].frame_id, "a".repeat(64));
        assert_eq!(second_undo.changes[0].state, ReviewState::Undecided);
        let Err(error) = undo_review_decision_sync(&state) else {
            return Err("an empty undo history was accepted".into());
        };
        assert_eq!(error.code, "review_nothing_to_undo");
        Ok(())
    }

    fn imported_review_test_frame(digit: char, label: &str) -> ImportedFrame {
        ImportedFrame {
            id: digit.to_string().repeat(64),
            role: "light",
            label: label.to_owned(),
            relative_path: format!("LIGHTS/{label}"),
            path: format!("/runtime-only/LIGHTS/{label}"),
            exposure_seconds: Some(60.0),
            temperature_celsius: Some(-5.0),
            camera: Some("Synthetic camera".to_owned()),
            filter: None,
            bayer_pattern: Some("rggb"),
            axes: vec![4, 2],
            fits_diagnostic_count: 0,
            classification_conflict: false,
        }
    }

    #[test]
    #[ignore = "requires AETHERSTACK_TEST_SESSION to reference a representative local corpus"]
    fn imports_an_external_session_corpus() -> TestResult {
        let root = std::env::var_os("AETHERSTACK_TEST_SESSION")
            .ok_or("AETHERSTACK_TEST_SESSION is not configured")?;

        let imported = import_session_directory_sync(Path::new(&root))?;

        assert!(imported.files_considered > 0);
        assert!(!imported.frames.is_empty());
        for expected_role in ["dark", "flat", "light"] {
            assert!(
                imported
                    .frames
                    .iter()
                    .any(|frame| frame.role == expected_role),
                "representative corpus has no resolved {expected_role} frames"
            );
        }
        let light = imported
            .frames
            .iter()
            .find(|frame| frame.role == "light")
            .ok_or("representative corpus has no light preview source")?;
        let estimate_request = FitsPreviewEstimateRequest {
            path: PathBuf::from(&light.path),
            plane: 0,
            maximum_width: 800,
            maximum_height: 600,
        };
        let estimate = estimate_fits_preview_transform_from_reader(
            File::open(&estimate_request.path)?,
            &estimate_request,
        )?;
        let transform = FitsPreviewRequest {
            path: estimate_request.path.clone(),
            plane: 0,
            maximum_width: 800,
            maximum_height: 600,
            black_point: estimate.black_point,
            white_point: estimate.white_point,
            midtone: estimate.midtone,
            transfer: PreviewTransfer::Midtones,
        };
        let encoded = render_fits_preview_png(File::open(&transform.path)?, &transform)?;
        assert_eq!(&encoded[..8], b"\x89PNG\r\n\x1a\n");
        assert!(u32::from_be_bytes(encoded[16..20].try_into()?) <= 800);
        assert!(u32::from_be_bytes(encoded[20..24].try_into()?) <= 600);
        let statistics = inspect_fits_statistics_sync(&transform.path)?;
        assert_eq!(statistics.total_samples, statistics.usable_samples);
        assert_eq!(statistics.undefined_samples, 0);
        assert_eq!(statistics.non_finite_samples, 0);
        assert!(statistics.maximum > statistics.minimum);
        assert_eq!(light.bayer_pattern, Some("rggb"));
        if let Some(output) = std::env::var_os("AETHERSTACK_TEST_PREVIEW_OUTPUT") {
            fs::write(output, encoded)?;
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires AETHERSTACK_TEST_QUALITY_FRAME to reference a representative CFA light"]
    fn measures_an_external_cfa_light() -> TestResult {
        let path = std::env::var_os("AETHERSTACK_TEST_QUALITY_FRAME")
            .ok_or("AETHERSTACK_TEST_QUALITY_FRAME is not configured")?;
        let quality = inspect_frame_quality_sync(&FrameQualityRequest {
            path: PathBuf::from(path),
            interpretation: QualityInterpretation::BayerCellMean {
                pattern: BayerPatternWire::Rggb,
            },
        })?;

        assert!(quality.detected_stars > 0);
        assert!(quality.usable_stars > 0);
        assert!(quality.fwhm_pixels.is_some());
        assert!(quality.eccentricity.is_some());
        Ok(())
    }
}
