//! Native desktop shell for AetherStack.
//!
//! Scientific and review behavior lives in the workspace crates. This crate is
//! deliberately limited to the operating-system window and typed IPC adapters,
//! preventing the web presenter from becoming a second processing engine.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs::{self, File, OpenOptions};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::num::{NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use aether_cache::{ArtifactStore, CacheKey};
use aether_calibration::{CalibrationParameters, FlatNormalizationParameters};
use aether_fits::{
    DEFAULT_STATISTICS_CHUNK_SAMPLES, DatasumVerification, FITS_STATISTICS_ALGORITHM_ID,
    FitsOutputProvenance, FitsWriteSummary, HduChecksumVerification, HeaderReadOptions,
    ImageRegion, PrimaryImageReader, SampleStatus, StoredSampleFormat, primary_image_statistics,
};
use aether_metadata::{BayerPattern, FrameType};
use aether_preview::{
    AUTO_STRETCH_ALGORITHM_ID, AutomaticDisplayTransform, FitsPreviewParameters, MissingPixelStyle,
    PreviewLimits, RgbaPreview, ScalarPalette, ScalarPreview, build_fits_preview,
    choose_reduction_level, estimate_display_transform, estimate_rgb_display_transform,
    render_false_color_rgba8, render_grayscale_rgba8, render_rgb_rgba8,
};
use aether_quality::{
    BackgroundParameters, CFA_CELL_MEAN_ALGORITHM_ID, FrameQualityError,
    GLOBAL_BACKGROUND_ALGORITHM_ID, RGB_LUMINANCE_ALGORITHM_ID, RgbLuminanceBuilder,
    RgbLuminanceChannel, STAR_MEASUREMENT_ALGORITHM_ID, StarMeasurementParameters,
    measure_frame_quality, prepare_cfa_cell_mean,
};
use aether_register::{RegistrationDiagnostic, diagnose_paths};
use aether_registration::{AffineTransform, PlannedRegistrationFrame, RegistrationPlan};
use aether_review::{
    DecisionChange, DecisionDelta, DisplayTransform, FrameId, FrameMetrics,
    FrameSelectionComparator, FrameSelectionMetric, FrameSelectionPlan, FrameSelectionProposal,
    FrameSelectionRule, FrameSpec, MAX_FRAME_SELECTION_PLAN_FRAMES, MAX_UNDO_DEPTH, ManualDecision,
    ManualRejectionReason, MissingMetricPolicy, MissingPlacement, ReviewBook, ReviewError,
    ReviewState, SortDirection as ReviewSortDirection, SortField as ReviewSortField, SortSpec,
    TransferFunction,
};
use aether_runtime::{
    BALANCED_PSF_WEIGHT_ALGORITHM_ID, CancellationToken, LightPlanExecutionError,
    LightPlanExecutionRequest, MasterPlanExecutionError, MasterPlanExecutionRequest, MemoryBudget,
    PERCENTILE_REJECTION_MAP_ALGORITHM_ID, PercentileClipParameters, PipelineSource, ProgressState,
    QualityWeightMetrics, RegisteredFrameQuality, RegisteredRejectionMapOutput,
    RegisteredStackError, RegisteredStackEstimator, RegisteredStackRequest, RegisteredStackSource,
    RegisteredWeightSet, RegistrationPlanExecutionError, RegistrationPlanExecutionRequest,
    RegistrationPlanSource, run_calibrated_light_plan, run_demosaiced_light_plan, run_light_plan,
    run_master_plan, run_registered_stack, run_registration_plan,
};
use aether_session::{
    ClassificationPolicy, DirectoryManifestOptions, DirectoryManifestReport,
    FlatPedestalAssociation, FlatPedestalBlockingReason, FlatPedestalPolicy, LightCalibrationPlan,
    LightCalibrationPlanOptions, LightMasterAssociation, LightMasterBlockingReason,
    LightMasterCandidateCompatibility, LightMasterKind, LightMasterMatchField,
    LightMasterMismatchReason, ManifestFile, ManifestGroup, MasterPlan, MasterPlanOptions,
    MasterProductKind, PedestalCandidateCompatibility, PedestalMatchField, PedestalMismatchReason,
    PedestalSourceKind, SessionManifest, TemperatureBasis, fingerprint_reader,
    fingerprint_reader_with_progress, generate_manifest_from_directory,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::Manager;
use tauri::ipc::Response;

const MAX_DESKTOP_PREVIEW_PIXELS: usize = 2 * 1_024 * 1_024;
const DESKTOP_PREVIEW_IO_CHUNK_SAMPLES: usize = 256 * 1_024;
const REJECTION_HISTOGRAM_ALGORITHM_ID: &str = "rejection-count-histogram-v1";
const MAX_REJECTION_HISTOGRAM_BINS: usize = 4_096;
const MAX_DESKTOP_QUALITY_SOURCE_PIXELS: u64 = 64 * 1_024 * 1_024;
const DESKTOP_QUALITY_PROFILE_ID: &str = "desktop-diagnostic-quality-v1";
const QUALITY_EVIDENCE_SCHEMA_VERSION: u16 = 1;
const QUALITY_EVIDENCE_CACHE_DOMAIN: &str = "frame-quality-evidence-v1";
const MAX_QUALITY_EVIDENCE_BYTES: u64 = 64 * 1_024;
const REGISTERED_STACK_REPORT_ALGORITHM_ID: &str = "registered-stack-report-v1";
const MAX_REGISTERED_STACK_REPORT_BYTES: u64 = 4 * 1_024 * 1_024;
const SOURCE_VERIFICATION_PROGRESS_BYTES: u64 = 8 * 1_024 * 1_024;
static REGISTERED_STACK_REPORT_TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FitsPreviewRequest {
    path: PathBuf,
    content: FitsPreviewContent,
    maximum_width: usize,
    maximum_height: usize,
    black_point: f64,
    white_point: f64,
    midtone: f64,
    transfer: PreviewTransfer,
    #[serde(default)]
    palette: PreviewPalette,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FitsPreviewEstimateRequest {
    path: PathBuf,
    content: FitsPreviewContent,
    maximum_width: usize,
    maximum_height: usize,
}

/// Explicit interpretation of the primary FITS array for display.
///
/// A tagged value prevents a planar RGB product from being silently treated as
/// one grayscale plane, and preserves an exact plane choice for scientific
/// mono or cube inspection.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum FitsPreviewContent {
    Scalar { plane: u64 },
    Rgb,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PreviewPalette {
    #[default]
    Grayscale,
    #[serde(rename = "rejection_low")]
    LowRejection,
    #[serde(rename = "rejection_high")]
    HighRejection,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RejectionHistogramRequest {
    path: PathBuf,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RejectionHistogramBin {
    rejected_count: u32,
    samples: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RejectionHistogramResponse {
    algorithm_id: &'static str,
    total_samples: u64,
    zero_samples: u64,
    rejected_samples: u64,
    maximum_rejected_count: u32,
    bins: Vec<RejectionHistogramBin>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StackPixelInspectionRequest {
    science_path: PathBuf,
    low_rejection_path: Option<PathBuf>,
    high_rejection_path: Option<PathBuf>,
    x: u64,
    y: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StackPixelInspectionResponse {
    x: u64,
    y: u64,
    science_values: Vec<Option<f64>>,
    low_rejection_counts: Option<Vec<Option<u32>>>,
    high_rejection_counts: Option<Vec<Option<u32>>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegistrationDiagnosticRequest {
    source_path: PathBuf,
    reference_path: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegistrationPlanPreviewRequest {
    reference_frame_id: String,
    source_frame_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RegistrationPlanPreviewResponse {
    schema_version: u32,
    plan_sha256: String,
    reference_frame_id: String,
    reference_width: usize,
    reference_height: usize,
    covered_pixels: usize,
    autocrop: RegistrationCropResponse,
    frames: Vec<RegistrationPlannedFrameResponse>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RegistrationPlannedFrameResponse {
    frame_id: String,
    source_width: usize,
    source_height: usize,
    transform_coefficients_source_pixels: [f64; 6],
    reference: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RegistrationCropResponse {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegistrationArtifactInput {
    frame_id: String,
    path: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegistrationPlanExecutionCommandRequest {
    planning: RegistrationPlanPreviewRequest,
    expected_plan_sha256: String,
    artifacts: Vec<RegistrationArtifactInput>,
    output_directory: PathBuf,
    band_height: usize,
    memory_limit_bytes: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RegistrationExecutionProgress {
    frame_index: usize,
    frame_count: usize,
    frame_id: String,
    sequence: u64,
    stage: String,
    state: &'static str,
    completed_units: u64,
    total_units: Option<u64>,
    code: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RegistrationPlanExecutionResponse {
    plan_sha256: String,
    memory_limit_bytes: usize,
    peak_reserved_bytes: usize,
    frames: Vec<ExecutedRegisteredFrame>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExecutedRegisteredFrame {
    frame_id: String,
    output_path: String,
    samples_written: u64,
    substituted_samples: u64,
    bytes_written: u64,
    interpolated_samples: usize,
    outside_footprint_samples: usize,
    masked_support_samples: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegisteredStackCommandRequest {
    planning: RegistrationPlanPreviewRequest,
    expected_plan_sha256: String,
    artifacts: Vec<RegistrationArtifactInput>,
    #[serde(default)]
    quality_evidence: Vec<RegisteredFrameQualityInput>,
    #[serde(default)]
    quality_reference_frame_id: Option<String>,
    output_path: PathBuf,
    band_height: usize,
    memory_limit_bytes: u64,
    integration: RegisteredStackIntegrationSettings,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum RegisteredStackEstimatorInput {
    StrictMean,
    WeightedMean,
    PercentileClipped,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegisteredFrameQualityInput {
    frame_id: String,
    signal_to_noise: f64,
    fwhm_pixels: f64,
    eccentricity: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegisteredWeightPreflightRequest {
    expected_plan_sha256: String,
    frame_ids: Vec<String>,
    reference_frame_id: String,
    quality_evidence: Vec<RegisteredFrameQualityInput>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RegisteredWeightPreflightResponse {
    schema_version: u32,
    plan_sha256: String,
    algorithm_id: &'static str,
    parameters_sha256: String,
    reference_frame_id: String,
    weights: Vec<RegisteredWeightPreflightEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegisteredWeightPreflightEntry {
    frame_id: String,
    weight: f64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegisteredStackIntegrationSettings {
    estimator: RegisteredStackEstimatorInput,
    low_fraction: f64,
    high_fraction: f64,
    minimum_retained_samples: u32,
    generate_rejection_maps: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RegisteredStackProgress {
    sequence: u64,
    stage: String,
    state: &'static str,
    completed_units: u64,
    total_units: Option<u64>,
    code: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RegisteredStackResponse {
    plan_sha256: String,
    output_path: String,
    width: usize,
    height: usize,
    planes: usize,
    samples_written: u64,
    substituted_samples: u64,
    bytes_written: u64,
    memory_limit_bytes: usize,
    peak_reserved_bytes: usize,
    estimator: &'static str,
    low_rejection_map_path: Option<String>,
    high_rejection_map_path: Option<String>,
    rejection_map_samples_written: Option<u64>,
    report_path: String,
    report_sha256: String,
}

/// Stable machine-readable evidence emitted beside every integrated product.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegisteredStackReport {
    algorithm_id: String,
    plan_sha256: String,
    manifest_sha256: String,
    integration: RegisteredStackIntegrationSettings,
    band_height: usize,
    memory_limit_bytes: usize,
    peak_reserved_bytes: usize,
    dimensions: RegisteredStackReportDimensions,
    sources: Vec<RegisteredStackReportSource>,
    weights: Option<RegisteredStackReportWeights>,
    products: Vec<RegisteredStackReportProduct>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegisteredStackReportDimensions {
    width: usize,
    height: usize,
    planes: usize,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegisteredStackReportSource {
    frame_id: String,
    file_name: String,
    byte_length: u64,
    sha256: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegisteredStackReportWeights {
    algorithm_id: String,
    parameters_sha256: String,
    reference_frame_id: String,
    entries: Vec<RegisteredWeightPreflightEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegisteredStackReportProduct {
    role: String,
    file_name: String,
    samples_written: u64,
    substituted_samples: u64,
    bytes_written: u64,
    data_sum: Option<u32>,
    checksum: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegisteredStackReportEnvelope {
    schema_version: u32,
    report_sha256: String,
    report: RegisteredStackReport,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegisteredStackReportInspectionRequest {
    path: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegisteredStackSourceVerificationRequest {
    report_path: PathBuf,
    source_directory: PathBuf,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RegisteredStackReportInspectionResponse {
    schema_version: u32,
    report_sha256: String,
    plan_sha256: String,
    manifest_sha256: String,
    estimator: RegisteredStackEstimatorInput,
    width: usize,
    height: usize,
    planes: usize,
    source_count: usize,
    product_count: usize,
    weighted: bool,
    all_products_verified: bool,
    sources: Vec<RegisteredStackReportSourceInspection>,
    products: Vec<RegisteredStackReportProductInspection>,
}

/// Path-independent source identity retained by the deterministic report.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RegisteredStackReportSourceInspection {
    frame_id: String,
    file_name: String,
    byte_length: u64,
    sha256: String,
}

/// On-disk verification result for one product named by an integration report.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RegisteredStackReportProductInspection {
    role: String,
    file_name: String,
    path: String,
    bytes_written: u64,
    status: RegisteredStackReportProductStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RegisteredStackReportProductStatus {
    Verified,
    Missing,
    NonRegular,
    ByteLengthMismatch,
    InvalidFits,
    MetadataMismatch,
    ChecksumMismatch,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RegisteredStackSourceVerificationResponse {
    report_sha256: String,
    source_directory: String,
    all_sources_verified: bool,
    sources: Vec<RegisteredStackSourceVerification>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RegisteredStackSourceVerificationProgress {
    sequence: u64,
    state: &'static str,
    completed_sources: usize,
    total_sources: usize,
    current_file_name: Option<String>,
    completed_bytes: u64,
    total_bytes: u64,
    current_file_bytes: u64,
    current_file_total_bytes: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RegisteredStackSourceVerification {
    frame_id: String,
    file_name: String,
    path: String,
    byte_length: u64,
    status: RegisteredStackSourceVerificationStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RegisteredStackSourceVerificationStatus {
    Verified,
    Missing,
    NonRegular,
    ByteLengthMismatch,
    ReadFailed,
    FingerprintMismatch,
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
    frame_id: String,
    path: PathBuf,
    interpretation: QualityInterpretation,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
enum QualityInterpretation {
    Monochrome,
    BayerCellMean { pattern: BayerPatternWire },
    RgbLuminance,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BayerPatternWire {
    Rggb,
    Bggr,
    Grbg,
    Gbrg,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FrameQualityResponse {
    profile_id: String,
    background_algorithm_id: String,
    star_algorithm_id: String,
    detection_plane_algorithm_id: String,
    interpretation: String,
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
    signal_to_noise: Option<f64>,
    fwhm_pixels: Option<f64>,
    eccentricity: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredFrameQualityEvidence {
    schema_version: u16,
    frame_id: String,
    source_byte_length: u64,
    source_sha256: String,
    response: FrameQualityResponse,
}

fn quality_evidence_key(
    frame_id: &FrameId,
    source_byte_length: u64,
    source_sha256: &str,
) -> Result<CacheKey, PreviewCommandError> {
    if source_byte_length == 0 || !is_canonical_sha256(source_sha256) {
        return Err(frame_quality_result_error());
    }
    let mut descriptor = Vec::new();
    descriptor
        .try_reserve_exact(2 + 64 + 8 + 64 + DESKTOP_QUALITY_PROFILE_ID.len() + 96)
        .map_err(|_| frame_quality_result_error())?;
    descriptor.extend_from_slice(&QUALITY_EVIDENCE_SCHEMA_VERSION.to_be_bytes());
    descriptor.extend_from_slice(frame_id.as_str().as_bytes());
    descriptor.extend_from_slice(&source_byte_length.to_be_bytes());
    descriptor.extend_from_slice(source_sha256.as_bytes());
    for identity in [
        DESKTOP_QUALITY_PROFILE_ID,
        GLOBAL_BACKGROUND_ALGORITHM_ID,
        STAR_MEASUREMENT_ALGORITHM_ID,
    ] {
        let length = u64::try_from(identity.len()).map_err(|_| frame_quality_result_error())?;
        descriptor.extend_from_slice(&length.to_be_bytes());
        descriptor.extend_from_slice(identity.as_bytes());
    }
    CacheKey::derive(QUALITY_EVIDENCE_CACHE_DOMAIN, &descriptor)
        .map_err(|_| frame_quality_result_error())
}

fn publish_quality_evidence(
    cache_root: &Path,
    frame_id: &FrameId,
    source_byte_length: u64,
    source_sha256: &str,
    response: &FrameQualityResponse,
) -> Result<(), PreviewCommandError> {
    validate_quality_response(response)?;
    let key = quality_evidence_key(frame_id, source_byte_length, source_sha256)?;
    let evidence = StoredFrameQualityEvidence {
        schema_version: QUALITY_EVIDENCE_SCHEMA_VERSION,
        frame_id: frame_id.as_str().to_owned(),
        source_byte_length,
        source_sha256: source_sha256.to_owned(),
        response: response.clone(),
    };
    let payload = serde_json::to_vec(&evidence).map_err(|_| frame_quality_result_error())?;
    if u64::try_from(payload.len()).unwrap_or(u64::MAX) > MAX_QUALITY_EVIDENCE_BYTES {
        return Err(frame_quality_result_error());
    }
    let store =
        ArtifactStore::new(cache_root.to_owned()).map_err(|_| frame_quality_cache_error())?;
    let mut reader = Cursor::new(payload);
    store
        .publish(&key, &mut reader)
        .map_err(|_| frame_quality_cache_error())?;
    Ok(())
}

#[derive(Debug, PartialEq)]
enum QualityEvidenceRestore {
    Missing,
    Restored(Box<FrameQualityResponse>),
    Rejected,
}

fn restore_quality_evidence(cache_root: &Path, frame: &ImportedFrame) -> QualityEvidenceRestore {
    let Ok(frame_id) = FrameId::new(frame.id.clone()) else {
        return QualityEvidenceRestore::Rejected;
    };
    let Ok(derived) = FrameId::derive(
        &frame.relative_path,
        frame.source_byte_length,
        &frame.source_sha256,
    ) else {
        return QualityEvidenceRestore::Rejected;
    };
    if derived != frame_id {
        return QualityEvidenceRestore::Rejected;
    }
    let Ok(key) = quality_evidence_key(&frame_id, frame.source_byte_length, &frame.source_sha256)
    else {
        return QualityEvidenceRestore::Rejected;
    };
    let Ok(store) = ArtifactStore::new(cache_root.to_owned()) else {
        return QualityEvidenceRestore::Rejected;
    };
    let mut artifact = match store.lookup_verified(&key) {
        Ok(Some(artifact)) => artifact,
        Ok(None) => return QualityEvidenceRestore::Missing,
        Err(_) => return QualityEvidenceRestore::Rejected,
    };
    if artifact.payload_bytes() > MAX_QUALITY_EVIDENCE_BYTES {
        return QualityEvidenceRestore::Rejected;
    }
    let Ok(capacity) = usize::try_from(artifact.payload_bytes()) else {
        return QualityEvidenceRestore::Rejected;
    };
    let mut payload = Vec::new();
    if payload.try_reserve_exact(capacity).is_err() || artifact.read_to_end(&mut payload).is_err() {
        return QualityEvidenceRestore::Rejected;
    }
    if payload.len() != capacity {
        return QualityEvidenceRestore::Rejected;
    }
    let Ok(evidence) = serde_json::from_slice::<StoredFrameQualityEvidence>(&payload) else {
        return QualityEvidenceRestore::Rejected;
    };
    if evidence.schema_version != QUALITY_EVIDENCE_SCHEMA_VERSION
        || evidence.frame_id != frame.id
        || evidence.source_byte_length != frame.source_byte_length
        || evidence.source_sha256 != frame.source_sha256
        || validate_quality_response(&evidence.response).is_err()
    {
        return QualityEvidenceRestore::Rejected;
    }
    QualityEvidenceRestore::Restored(Box::new(evidence.response))
}

fn validate_quality_response(
    response: &FrameQualityResponse,
) -> Result<FrameMetrics, PreviewCommandError> {
    let accounted_candidates = response
        .suppressed_candidates
        .checked_add(response.rejected_measurements)
        .and_then(|count| count.checked_add(response.detected_stars));
    let valid_saturation = match (response.saturation_level, response.saturated_stars) {
        (None, None) => true,
        (Some(level), Some(stars)) => {
            level.is_finite() && level > 0.0 && stars <= response.detected_stars
        }
        _ => false,
    };
    if response.profile_id != DESKTOP_QUALITY_PROFILE_ID
        || response.background_algorithm_id != GLOBAL_BACKGROUND_ALGORITHM_ID
        || response.star_algorithm_id != STAR_MEASUREMENT_ALGORITHM_ID
        || !response.source_pixel_scale.is_finite()
        || response.source_pixel_scale <= 0.0
        || !response.diagnostic_only
        || response.usable_stars > response.detected_stars
        || response.retained_background_samples > response.initial_usable_samples
        || accounted_candidates != Some(response.raw_candidates)
        || !valid_saturation
    {
        return Err(frame_quality_result_error());
    }
    let valid_interpretation = match response.interpretation.as_str() {
        "monochrome" => {
            response.detection_plane_algorithm_id == "identity-monochrome-v1"
                && response.source_pixel_scale.to_bits() == 1.0_f64.to_bits()
        }
        "raw CFA · RGGB" | "raw CFA · BGGR" | "raw CFA · GRBG" | "raw CFA · GBRG" => {
            response.detection_plane_algorithm_id == CFA_CELL_MEAN_ALGORITHM_ID
                && response.source_pixel_scale.to_bits() == 2.0_f64.to_bits()
        }
        "calibrated RGB · linear Rec. 709 luminance" => {
            response.detection_plane_algorithm_id == RGB_LUMINANCE_ALGORITHM_ID
                && response.source_pixel_scale.to_bits() == 1.0_f64.to_bits()
        }
        _ => false,
    };
    if !valid_interpretation {
        return Err(frame_quality_result_error());
    }
    FrameMetrics::new(
        Some(response.background),
        Some(response.noise),
        Some(response.detected_stars),
        Some(response.usable_stars),
        response.fwhm_pixels,
        response.eccentricity,
    )
    .and_then(|metrics| metrics.with_signal_to_noise(response.signal_to_noise))
    .map_err(|_| frame_quality_result_error())
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
    quality_evidence_restored: usize,
    quality_evidence_missing: usize,
    quality_evidence_rejected: usize,
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
    #[serde(skip_serializing)]
    source_byte_length: u64,
    #[serde(skip_serializing)]
    source_sha256: String,
    quality: Option<FrameQualityResponse>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportedFailure {
    relative_path: String,
    code: String,
}

fn restore_session_quality_evidence(cache_root: &Path, session: &mut ImportedSession) {
    session.quality_evidence_restored = 0;
    session.quality_evidence_missing = 0;
    session.quality_evidence_rejected = 0;
    for frame in &mut session.frames {
        // Raw quality diagnostics currently require a declared Bayer phase.
        // Calibration and non-Light sources deliberately do not inflate the
        // cache-miss count because they are not eligible for this evidence.
        if frame.role != "light" || frame.bayer_pattern.is_none() {
            continue;
        }
        match restore_quality_evidence(cache_root, frame) {
            QualityEvidenceRestore::Missing => session.quality_evidence_missing += 1,
            QualityEvidenceRestore::Restored(response) => {
                frame.quality = Some(*response);
                session.quality_evidence_restored += 1;
            }
            QualityEvidenceRestore::Rejected => session.quality_evidence_rejected += 1,
        }
    }
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
    signal_to_noise: Option<f64>,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FrameSelectionFrameWire {
    frame_id: String,
    source_path: PathBuf,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FrameSelectionMetricWire {
    Background,
    Noise,
    SignalToNoise,
    DetectedStars,
    UsableStars,
    FwhmPixels,
    Eccentricity,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FrameSelectionComparatorWire {
    LessThan,
    GreaterThan,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MissingMetricPolicyWire {
    Retain,
    Reject,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum FrameSelectionThresholdWire {
    Scalar(f64),
    Count(u64),
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FrameSelectionRuleWire {
    metric: FrameSelectionMetricWire,
    comparator: FrameSelectionComparatorWire,
    threshold: FrameSelectionThresholdWire,
    missing_policy: MissingMetricPolicyWire,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FrameSelectionPreviewRequest {
    frames: Vec<FrameSelectionFrameWire>,
    rules: Vec<FrameSelectionRuleWire>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FrameSelectionApplyRequest {
    frames: Vec<FrameSelectionFrameWire>,
    rules: Vec<FrameSelectionRuleWire>,
    plan_sha256: String,
}

/// Native owner of the current session's manual review decisions.
///
/// The browser presenter receives small immutable updates, while the audited
/// transaction history remains in Rust and cannot be bypassed by local DOM
/// state. Re-importing a session replaces this book atomically.
type NativeQualityMetrics = BTreeMap<(FrameId, PathBuf), FrameMetrics>;

#[derive(Debug, Default)]
struct DesktopReviewState {
    book: Mutex<Option<ReviewBook>>,
    quality_metrics: Mutex<NativeQualityMetrics>,
}

/// Native owner of the exact imported manifest and its runtime-only root.
///
/// The browser never sends a reconstructed manifest back for scientific
/// planning. Every preview is derived from this immutable native snapshot.
#[derive(Debug, Default)]
struct DesktopSessionState {
    session: Mutex<Option<Arc<ImportedNativeSession>>>,
}

/// Single active native-work slot for bounded execution and cancellation.
///
/// Serializing master, Light, registration, integration, and evidence work
/// avoids accidental memory and disk-I/O multiplication and makes each visible
/// Cancel control unambiguous.
#[derive(Debug, Default)]
struct DesktopCalibrationExecutionState {
    cancellation: Mutex<Option<CancellationToken>>,
}

#[derive(Debug)]
struct ImportedNativeSession {
    root: PathBuf,
    manifest: Arc<SessionManifest>,
}

#[derive(Debug)]
struct ImportedSessionBundle {
    presentation: ImportedSession,
    native: ImportedNativeSession,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FlatPedestalPolicyWire {
    RequireMatchedDark,
    RequireBias,
    PreferMatchedDarkThenBias,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MasterPlanPreviewRequest {
    flat_pedestal_policy: FlatPedestalPolicyWire,
    maximum_exposure_delta_seconds: f64,
    maximum_temperature_delta_c: f64,
    maximum_light_dark_temperature_delta_c: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MasterPlanExecutionCommandRequest {
    output_directory: PathBuf,
    planning: MasterPlanPreviewRequest,
    expected_manifest_sha256: String,
    expected_plan_sha256: String,
    minimum_flat_normalization_samples: usize,
    minimum_positive_flat_median: f64,
    tile_width: usize,
    tile_height: usize,
    memory_limit_bytes: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LightPlanExecutionCommandRequest {
    master_directory: PathBuf,
    output_directory: PathBuf,
    planning: MasterPlanPreviewRequest,
    expected_manifest_sha256: String,
    expected_master_plan_sha256: String,
    expected_light_plan_sha256: String,
    minimum_absolute_flat: f64,
    tile_width: usize,
    tile_height: usize,
    memory_limit_bytes: u64,
    output_mode: LightOutputMode,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum LightOutputMode {
    CalibratedFrames,
    Integrated,
}

impl LightOutputMode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::CalibratedFrames => "calibrated_frames",
            Self::Integrated => "integrated",
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MasterPlanPreviewResponse {
    schema_version: u32,
    manifest_sha256: String,
    plan_sha256: String,
    ready: bool,
    products: Vec<MasterProductPreview>,
    light_plan: Option<LightCalibrationPlanPreview>,
}

/// Display projection of the immutable Light-to-master association plan.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LightCalibrationPlanPreview {
    schema_version: u32,
    plan_sha256: String,
    ready: bool,
    products: Vec<LightCalibrationProductPreview>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LightCalibrationProductPreview {
    group_id: String,
    dark: LightMasterAssociationPreview,
    flat: LightMasterAssociationPreview,
    candidates: Vec<LightMasterCandidatePreview>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LightMasterAssociationPreview {
    kind: &'static str,
    status: &'static str,
    selected_group_id: Option<String>,
    temperature_basis: Option<&'static str>,
    temperature_delta_celsius: Option<f64>,
    blocking_reason: Option<&'static str>,
    ambiguous_group_ids: Vec<String>,
    missing_fields: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LightMasterCandidatePreview {
    group_id: String,
    kind: &'static str,
    status: &'static str,
    temperature_basis: Option<&'static str>,
    temperature_delta_celsius: Option<f64>,
    mismatches: Vec<LightMasterMismatchPreview>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LightMasterMismatchPreview {
    field: &'static str,
    reason: &'static str,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MasterExecutionProgress {
    product_index: usize,
    product_count: usize,
    group_id: String,
    kind: &'static str,
    sequence: u64,
    stage: String,
    state: &'static str,
    completed_units: u64,
    total_units: Option<u64>,
    code: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LightExecutionProgress {
    product_index: usize,
    product_count: usize,
    group_id: String,
    sequence: u64,
    stage: String,
    state: &'static str,
    completed_units: u64,
    total_units: Option<u64>,
    code: Option<String>,
    source_index: Option<usize>,
    source_count: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MasterPlanExecutionResponse {
    manifest_sha256: String,
    plan_sha256: String,
    memory_limit_bytes: usize,
    peak_reserved_bytes: usize,
    products: Vec<ExecutedMasterProduct>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LightPlanExecutionResponse {
    manifest_sha256: String,
    master_plan_sha256: String,
    light_plan_sha256: String,
    memory_limit_bytes: usize,
    peak_reserved_bytes: usize,
    output_mode: &'static str,
    products: Vec<ExecutedLightProduct>,
    calibrated_frames: Vec<ExecutedCalibratedLightFrame>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExecutedCalibratedLightFrame {
    group_id: String,
    source_index: usize,
    source_frame_id: String,
    source_label: String,
    source_sha256: String,
    output_path: String,
    rgb_output_path: Option<String>,
    total_samples: usize,
    usable_samples: usize,
    masked_samples: usize,
    non_finite_samples: usize,
    minimum: f64,
    maximum: f64,
    mean: f64,
    population_standard_deviation: f64,
    samples_written: u64,
    substituted_samples: u64,
    bytes_written: u64,
    tiles_processed: u64,
    tiles_reused: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExecutedLightProduct {
    group_id: String,
    dark_group_id: String,
    flat_group_id: String,
    output_path: String,
    total_samples: usize,
    usable_samples: usize,
    masked_samples: usize,
    non_finite_samples: usize,
    minimum: f64,
    maximum: f64,
    mean: f64,
    population_standard_deviation: f64,
    samples_written: u64,
    substituted_samples: u64,
    bytes_written: u64,
    tiles_processed: u64,
    tiles_reused: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExecutedMasterProduct {
    group_id: String,
    kind: &'static str,
    output_path: String,
    total_samples: usize,
    usable_samples: usize,
    masked_samples: usize,
    non_finite_samples: usize,
    minimum: f64,
    maximum: f64,
    mean: f64,
    population_standard_deviation: f64,
    samples_written: u64,
    substituted_samples: u64,
    bytes_written: u64,
    normalization: Option<f64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MasterProductPreview {
    group_id: String,
    kind: &'static str,
    frame_count: usize,
    camera: Option<String>,
    axes: Vec<u64>,
    exposure_seconds: Option<f64>,
    sensor_temperature_celsius: Option<f64>,
    set_temperature_celsius: Option<f64>,
    gain: Option<f64>,
    offset: Option<f64>,
    binning: Option<MasterBinningPreview>,
    filter: Option<String>,
    bayer_pattern: Option<String>,
    pedestal: MasterPedestalPreview,
    candidates: Vec<MasterPedestalCandidatePreview>,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct MasterBinningPreview {
    x: u32,
    y: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MasterPedestalPreview {
    status: &'static str,
    selected_group_id: Option<String>,
    exposure_delta_seconds: Option<f64>,
    temperature_basis: Option<&'static str>,
    temperature_delta_celsius: Option<f64>,
    blocking_reason: Option<&'static str>,
    ambiguous_group_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MasterPedestalCandidatePreview {
    group_id: String,
    source_kind: &'static str,
    status: &'static str,
    exposure_delta_seconds: Option<f64>,
    temperature_basis: Option<&'static str>,
    temperature_delta_celsius: Option<f64>,
    mismatches: Vec<MasterPedestalMismatchPreview>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MasterPedestalMismatchPreview {
    field: &'static str,
    reason: &'static str,
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
async fn inspect_rejection_histogram(
    request: RejectionHistogramRequest,
) -> Result<RejectionHistogramResponse, PreviewCommandError> {
    validate_runtime_source_path(&request.path)?;
    tauri::async_runtime::spawn_blocking(move || {
        let file = File::open(&request.path).map_err(|_| {
            PreviewCommandError::new(
                "fits_open_failed",
                "The rejection-map FITS file could not be opened.",
            )
        })?;
        inspect_rejection_histogram_reader(file)
    })
    .await
    .map_err(|_| preview_worker_error())?
}

#[tauri::command]
async fn inspect_stack_pixel(
    request: StackPixelInspectionRequest,
) -> Result<StackPixelInspectionResponse, PreviewCommandError> {
    validate_runtime_source_path(&request.science_path)?;
    if let Some(path) = &request.low_rejection_path {
        validate_runtime_source_path(path)?;
    }
    if let Some(path) = &request.high_rejection_path {
        validate_runtime_source_path(path)?;
    }
    tauri::async_runtime::spawn_blocking(move || inspect_stack_pixel_sync(&request))
        .await
        .map_err(|_| preview_worker_error())?
}

#[tauri::command]
async fn inspect_registered_stack_report(
    request: RegisteredStackReportInspectionRequest,
) -> Result<RegisteredStackReportInspectionResponse, PreviewCommandError> {
    validate_runtime_source_path(&request.path)?;
    tauri::async_runtime::spawn_blocking(move || {
        inspect_registered_stack_report_sync(&request.path)
    })
    .await
    .map_err(|_| preview_worker_error())?
}

#[tauri::command]
async fn verify_registered_stack_sources(
    request: RegisteredStackSourceVerificationRequest,
    on_progress: tauri::ipc::Channel<RegisteredStackSourceVerificationProgress>,
    execution_state: tauri::State<'_, DesktopCalibrationExecutionState>,
) -> Result<RegisteredStackSourceVerificationResponse, PreviewCommandError> {
    validate_runtime_source_path(&request.report_path)?;
    validate_runtime_source_path(&request.source_directory)?;
    let cancellation = begin_calibration_execution(&execution_state)?;
    let worker_cancellation = cancellation.clone();
    let execution = tauri::async_runtime::spawn_blocking(move || {
        verify_registered_stack_sources_sync(
            &request.report_path,
            &request.source_directory,
            &worker_cancellation,
            |progress| {
                let _ignored = on_progress.send(progress);
            },
        )
    })
    .await;
    finish_calibration_execution(&execution_state)?;
    execution.map_err(|_| preview_worker_error())?
}

#[tauri::command]
fn cancel_registered_stack_source_verification(
    execution_state: tauri::State<'_, DesktopCalibrationExecutionState>,
) -> Result<bool, PreviewCommandError> {
    cancel_calibration_execution(
        &execution_state,
        "registered_stack_source_verification_missing",
    )
}

#[tauri::command]
async fn inspect_frame_quality(
    request: FrameQualityRequest,
    app: tauri::AppHandle,
    review_state: tauri::State<'_, DesktopReviewState>,
    session_state: tauri::State<'_, DesktopSessionState>,
) -> Result<FrameQualityResponse, PreviewCommandError> {
    validate_runtime_source_path(&request.path)?;
    let frame_id =
        FrameId::new(request.frame_id.clone()).map_err(|_| frame_quality_identity_error())?;
    ensure_review_frame_exists(&review_state, &frame_id)?;
    let artifact_path = request.path.clone();
    let source_identity = imported_quality_source(&session_state, &frame_id, &artifact_path)?;
    let cache_root = quality_cache_root(&app)?;
    let response =
        tauri::async_runtime::spawn_blocking(move || inspect_frame_quality_sync(&request))
            .await
            .map_err(|_| {
                PreviewCommandError::new(
                    "frame_quality_interrupted",
                    "The frame-quality worker stopped before producing a result.",
                )
            })??;
    if let Some((source_byte_length, source_sha256)) = source_identity {
        publish_quality_evidence(
            &cache_root,
            &frame_id,
            source_byte_length,
            &source_sha256,
            &response,
        )?;
    }
    record_frame_quality(&review_state, frame_id, artifact_path, &response)?;
    Ok(response)
}

#[tauri::command]
async fn diagnose_fits_registration(
    request: RegistrationDiagnosticRequest,
) -> Result<RegistrationDiagnostic, PreviewCommandError> {
    validate_runtime_source_path(&request.source_path)?;
    validate_runtime_source_path(&request.reference_path)?;
    tauri::async_runtime::spawn_blocking(move || {
        diagnose_paths(&request.source_path, &request.reference_path).map_err(|_| {
            PreviewCommandError::new(
                "registration_diagnostic_failed",
                "The selected FITS pair could not produce a registration diagnostic.",
            )
        })
    })
    .await
    .map_err(|_| {
        PreviewCommandError::new(
            "registration_diagnostic_interrupted",
            "The registration diagnostic worker stopped before producing a result.",
        )
    })?
}

#[tauri::command]
async fn preview_registration_plan(
    request: RegistrationPlanPreviewRequest,
    session_state: tauri::State<'_, DesktopSessionState>,
    review_state: tauri::State<'_, DesktopReviewState>,
) -> Result<RegistrationPlanPreviewResponse, PreviewCommandError> {
    let session = lock_session_state(&session_state)?
        .clone()
        .ok_or_else(session_state_missing_error)?;
    let eligible_ids = reviewed_registration_frame_ids(&review_state, &session)?;
    tauri::async_runtime::spawn_blocking(move || {
        preview_registration_plan_for_ids_sync(&session, request, &eligible_ids)
    })
    .await
    .map_err(|_| {
        PreviewCommandError::new(
            "registration_plan_interrupted",
            "The registration-plan worker stopped before producing a result.",
        )
    })?
}

#[tauri::command]
fn preview_registered_weights(
    request: RegisteredWeightPreflightRequest,
) -> Result<RegisteredWeightPreflightResponse, PreviewCommandError> {
    preview_registered_weights_sync(request)
}

#[tauri::command]
async fn execute_registration_plan(
    request: RegistrationPlanExecutionCommandRequest,
    on_progress: tauri::ipc::Channel<RegistrationExecutionProgress>,
    session_state: tauri::State<'_, DesktopSessionState>,
    review_state: tauri::State<'_, DesktopReviewState>,
    execution_state: tauri::State<'_, DesktopCalibrationExecutionState>,
) -> Result<RegistrationPlanExecutionResponse, PreviewCommandError> {
    if !request.output_directory.is_absolute()
        || request
            .artifacts
            .iter()
            .any(|artifact| !artifact.path.is_absolute())
    {
        return Err(registration_execution_configuration_error());
    }
    let session = lock_session_state(&session_state)?
        .clone()
        .ok_or_else(session_state_missing_error)?;
    let eligible_ids = reviewed_registration_frame_ids(&review_state, &session)?;
    let cancellation = begin_calibration_execution(&execution_state)?;
    let worker_cancellation = cancellation.clone();
    let execution = tauri::async_runtime::spawn_blocking(move || {
        execute_registration_plan_for_ids_sync(
            &session,
            request,
            &eligible_ids,
            &worker_cancellation,
            |event| {
                let _ignored = on_progress.send(event);
            },
        )
    })
    .await;
    finish_calibration_execution(&execution_state)?;
    execution.map_err(|_| {
        PreviewCommandError::new(
            "registration_execution_interrupted",
            "The registration worker stopped before producing a result.",
        )
    })?
}

#[tauri::command]
fn cancel_registration_plan(
    execution_state: tauri::State<'_, DesktopCalibrationExecutionState>,
) -> Result<bool, PreviewCommandError> {
    cancel_calibration_execution(&execution_state, "registration_execution_missing")
}

#[tauri::command]
async fn execute_registered_stack(
    request: RegisteredStackCommandRequest,
    on_progress: tauri::ipc::Channel<RegisteredStackProgress>,
    session_state: tauri::State<'_, DesktopSessionState>,
    review_state: tauri::State<'_, DesktopReviewState>,
    execution_state: tauri::State<'_, DesktopCalibrationExecutionState>,
) -> Result<RegisteredStackResponse, PreviewCommandError> {
    if !request.output_path.is_absolute()
        || request
            .artifacts
            .iter()
            .any(|artifact| !artifact.path.is_absolute())
    {
        return Err(registered_stack_configuration_error());
    }
    let session = lock_session_state(&session_state)?
        .clone()
        .ok_or_else(session_state_missing_error)?;
    let eligible_ids = reviewed_registration_frame_ids(&review_state, &session)?;
    let cancellation = begin_calibration_execution(&execution_state)?;
    let worker_cancellation = cancellation.clone();
    let execution = tauri::async_runtime::spawn_blocking(move || {
        execute_registered_stack_for_ids_sync(
            &session,
            request,
            &eligible_ids,
            &worker_cancellation,
            |event| {
                let _ignored = on_progress.send(event);
            },
        )
    })
    .await;
    finish_calibration_execution(&execution_state)?;
    execution.map_err(|_| {
        PreviewCommandError::new(
            "registered_stack_interrupted",
            "The registered-stack worker stopped before producing a result.",
        )
    })?
}

#[tauri::command]
fn cancel_registered_stack(
    execution_state: tauri::State<'_, DesktopCalibrationExecutionState>,
) -> Result<bool, PreviewCommandError> {
    cancel_calibration_execution(&execution_state, "registered_stack_execution_missing")
}

#[derive(Debug)]
struct RegistrationNativeSource {
    path: PathBuf,
    width: usize,
    height: usize,
}

#[cfg(test)]
fn preview_registration_plan_sync(
    session: &ImportedNativeSession,
    request: RegistrationPlanPreviewRequest,
) -> Result<RegistrationPlanPreviewResponse, PreviewCommandError> {
    let eligible_ids = registration_native_sources(session)?
        .into_keys()
        .collect::<BTreeSet<_>>();
    preview_registration_plan_for_ids_sync(session, request, &eligible_ids)
}

fn preview_registration_plan_for_ids_sync(
    session: &ImportedNativeSession,
    request: RegistrationPlanPreviewRequest,
    eligible_ids: &BTreeSet<FrameId>,
) -> Result<RegistrationPlanPreviewResponse, PreviewCommandError> {
    let plan = build_registration_plan_for_ids_sync(session, &request, eligible_ids)?;
    registration_plan_response(&plan)
}

#[cfg(test)]
fn build_registration_plan_sync(
    session: &ImportedNativeSession,
    request: &RegistrationPlanPreviewRequest,
) -> Result<RegistrationPlan, PreviewCommandError> {
    let eligible_ids = registration_native_sources(session)?
        .into_keys()
        .collect::<BTreeSet<_>>();
    build_registration_plan_for_ids_sync(session, request, &eligible_ids)
}

fn build_registration_plan_for_ids_sync(
    session: &ImportedNativeSession,
    request: &RegistrationPlanPreviewRequest,
    eligible_ids: &BTreeSet<FrameId>,
) -> Result<RegistrationPlan, PreviewCommandError> {
    let mut sources = registration_native_sources(session)?;
    if !eligible_ids
        .iter()
        .all(|frame_id| sources.contains_key(frame_id))
    {
        return Err(registration_plan_input_error());
    }
    sources.retain(|frame_id, _| eligible_ids.contains(frame_id));
    if sources.len() < 2 {
        return Err(registration_plan_input_error());
    }
    let reference_id = FrameId::new(request.reference_frame_id.clone())
        .map_err(|_| registration_plan_input_error())?;
    let reference = sources
        .get(&reference_id)
        .ok_or_else(registration_plan_input_error)?;
    let requested_count = request.source_frame_ids.len();
    let requested_ids = request
        .source_frame_ids
        .iter()
        .cloned()
        .map(|value| FrameId::new(value).map_err(|_| registration_plan_input_error()))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let expected_ids = sources
        .keys()
        .filter(|frame_id| *frame_id != &reference_id)
        .cloned()
        .collect::<BTreeSet<_>>();
    if requested_ids.len() != requested_count || requested_ids != expected_ids {
        return Err(registration_plan_input_error());
    }

    let mut planned = Vec::new();
    planned
        .try_reserve_exact(sources.len())
        .map_err(|_| registration_plan_allocation_error())?;
    planned.push(PlannedRegistrationFrame::new(
        reference_id.clone(),
        reference.width,
        reference.height,
        AffineTransform::IDENTITY,
    ));
    for frame_id in requested_ids {
        let source = sources
            .get(&frame_id)
            .ok_or_else(registration_plan_input_error)?;
        let diagnostic = diagnose_paths(&source.path, &reference.path)
            .map_err(|_| registration_plan_diagnostic_error())?;
        let accepted = diagnostic
            .accepted_plan()
            .ok_or_else(registration_plan_rejected_error)?;
        if accepted.reference_width() != reference.width
            || accepted.reference_height() != reference.height
        {
            return Err(registration_plan_diagnostic_error());
        }
        let [m00, m01, m10, m11, tx, ty] = accepted.source_to_reference_coefficients();
        let transform = AffineTransform::new(m00, m01, m10, m11, tx, ty)
            .map_err(|_| registration_plan_diagnostic_error())?;
        planned.push(PlannedRegistrationFrame::new(
            frame_id,
            source.width,
            source.height,
            transform,
        ));
    }
    RegistrationPlan::new(
        reference_id.clone(),
        reference.width,
        reference.height,
        planned,
    )
    .map_err(|_| registration_plan_geometry_error())
}

fn registration_plan_response(
    plan: &RegistrationPlan,
) -> Result<RegistrationPlanPreviewResponse, PreviewCommandError> {
    let crop = plan
        .common_footprint()
        .crop()
        .ok_or_else(registration_plan_geometry_error)?;
    let frames = plan
        .frames()
        .iter()
        .map(|frame| RegistrationPlannedFrameResponse {
            frame_id: frame.frame_id().as_str().to_owned(),
            source_width: frame.source_width(),
            source_height: frame.source_height(),
            transform_coefficients_source_pixels: frame.source_to_reference().coefficients(),
            reference: frame.frame_id() == plan.reference_frame_id(),
        })
        .collect();
    Ok(RegistrationPlanPreviewResponse {
        schema_version: 1,
        plan_sha256: plan.plan_sha256().to_owned(),
        reference_frame_id: plan.reference_frame_id().as_str().to_owned(),
        reference_width: plan.reference_width(),
        reference_height: plan.reference_height(),
        covered_pixels: plan.common_footprint().covered_pixels(),
        autocrop: RegistrationCropResponse {
            x: crop.x(),
            y: crop.y(),
            width: crop.width(),
            height: crop.height(),
        },
        frames,
    })
}

#[cfg(test)]
fn execute_registration_plan_sync<F>(
    session: &ImportedNativeSession,
    request: RegistrationPlanExecutionCommandRequest,
    cancellation: &CancellationToken,
    progress: F,
) -> Result<RegistrationPlanExecutionResponse, PreviewCommandError>
where
    F: FnMut(RegistrationExecutionProgress),
{
    let eligible_ids = registration_native_sources(session)?
        .into_keys()
        .collect::<BTreeSet<_>>();
    execute_registration_plan_for_ids_sync(session, request, &eligible_ids, cancellation, progress)
}

fn execute_registration_plan_for_ids_sync<F>(
    session: &ImportedNativeSession,
    request: RegistrationPlanExecutionCommandRequest,
    eligible_ids: &BTreeSet<FrameId>,
    cancellation: &CancellationToken,
    mut progress: F,
) -> Result<RegistrationPlanExecutionResponse, PreviewCommandError>
where
    F: FnMut(RegistrationExecutionProgress),
{
    if !session.root.is_absolute()
        || !request.output_directory.is_absolute()
        || request.band_height == 0
    {
        return Err(registration_execution_configuration_error());
    }
    request.output_directory.to_str().ok_or_else(|| {
        PreviewCommandError::new(
            "registration_output_path_not_unicode",
            "The registration output directory cannot be represented as Unicode.",
        )
    })?;
    let plan = build_registration_plan_for_ids_sync(session, &request.planning, eligible_ids)?;
    if plan.plan_sha256() != request.expected_plan_sha256 {
        return Err(PreviewCommandError::new(
            "registration_plan_stale",
            "The reviewed registration digest no longer matches native evidence.",
        ));
    }
    if request.artifacts.len() != plan.frames().len() {
        return Err(registration_artifact_set_error());
    }
    let mut artifacts = BTreeMap::new();
    for artifact in request.artifacts {
        if !artifact.path.is_absolute() {
            return Err(registration_execution_configuration_error());
        }
        let frame_id =
            FrameId::new(artifact.frame_id).map_err(|_| registration_artifact_set_error())?;
        if artifacts.insert(frame_id, artifact.path).is_some() {
            return Err(registration_artifact_set_error());
        }
    }
    let expected_ids = plan
        .frames()
        .iter()
        .map(|frame| frame.frame_id().clone())
        .collect::<BTreeSet<_>>();
    if artifacts.keys().cloned().collect::<BTreeSet<_>>() != expected_ids {
        return Err(registration_artifact_set_error());
    }

    let memory_limit = usize::try_from(request.memory_limit_bytes)
        .map_err(|_| registration_execution_configuration_error())?;
    let memory = MemoryBudget::new(memory_limit)
        .map_err(|_| registration_execution_configuration_error())?;
    let mut sources = Vec::new();
    sources
        .try_reserve_exact(artifacts.len())
        .map_err(|_| registration_execution_allocation_error())?;
    for (frame_id, path) in artifacts {
        let mut input = File::open(&path).map_err(|_| registration_artifact_error())?;
        let fingerprint =
            fingerprint_reader(&mut input).map_err(|_| registration_artifact_error())?;
        sources.push(RegistrationPlanSource::from_reviewed_artifact(
            PipelineSource::new(path, fingerprint),
            frame_id,
        ));
    }
    let manifest_sha256 = session
        .manifest
        .canonical_sha256()
        .map_err(|_| registration_artifact_error())?;
    let execution = RegistrationPlanExecutionRequest::new(
        plan,
        sources,
        request.output_directory,
        manifest_sha256,
        "registration-all-lights",
    )
    .and_then(|value| value.with_band_height(request.band_height))
    .map_err(registration_execution_error)?;
    let result = run_registration_plan(&execution, cancellation, &memory, |event| {
        let stage = event.stage();
        progress(RegistrationExecutionProgress {
            frame_index: event.frame_index(),
            frame_count: event.frame_count(),
            frame_id: event.frame_id().as_str().to_owned(),
            sequence: stage.sequence(),
            stage: stage.stage().as_str().to_owned(),
            state: progress_state_name(stage.state()),
            completed_units: stage.completed_units(),
            total_units: stage.total_units(),
            code: stage.code().map(str::to_owned),
        });
    })
    .map_err(registration_execution_error)?;
    let mut frames = Vec::new();
    frames
        .try_reserve_exact(result.frames().len())
        .map_err(|_| registration_execution_allocation_error())?;
    for frame in result.frames() {
        let output_path = frame.output().to_str().map(str::to_owned).ok_or_else(|| {
            PreviewCommandError::new(
                "registration_output_path_not_unicode",
                "A registered output path cannot be represented as Unicode.",
            )
        })?;
        let write = frame.summary();
        let statistics = frame.statistics();
        frames.push(ExecutedRegisteredFrame {
            frame_id: frame.frame_id().as_str().to_owned(),
            output_path,
            samples_written: write.samples_written(),
            substituted_samples: write.substituted_samples(),
            bytes_written: write.bytes_written(),
            interpolated_samples: statistics.interpolated_samples(),
            outside_footprint_samples: statistics.outside_footprint_samples(),
            masked_support_samples: statistics.masked_support_samples(),
        });
    }
    Ok(RegistrationPlanExecutionResponse {
        plan_sha256: result.plan_sha256().to_owned(),
        memory_limit_bytes: memory.limit(),
        peak_reserved_bytes: result.peak_reserved_bytes(),
        frames,
    })
}

#[cfg(test)]
fn execute_registered_stack_sync<F>(
    session: &ImportedNativeSession,
    request: RegisteredStackCommandRequest,
    cancellation: &CancellationToken,
    progress: F,
) -> Result<RegisteredStackResponse, PreviewCommandError>
where
    F: FnMut(RegisteredStackProgress),
{
    let eligible_ids = registration_native_sources(session)?
        .into_keys()
        .collect::<BTreeSet<_>>();
    execute_registered_stack_for_ids_sync(session, request, &eligible_ids, cancellation, progress)
}

fn execute_registered_stack_for_ids_sync<F>(
    session: &ImportedNativeSession,
    request: RegisteredStackCommandRequest,
    eligible_ids: &BTreeSet<FrameId>,
    cancellation: &CancellationToken,
    mut progress: F,
) -> Result<RegisteredStackResponse, PreviewCommandError>
where
    F: FnMut(RegisteredStackProgress),
{
    if request.band_height == 0 || !request.output_path.is_absolute() {
        return Err(registered_stack_configuration_error());
    }
    let report_path = registered_stack_report_path(&request.output_path)?;
    require_absent_registered_report(&report_path)?;
    let integration = request.integration;
    if matches!(
        integration.estimator,
        RegisteredStackEstimatorInput::StrictMean | RegisteredStackEstimatorInput::WeightedMean
    ) && integration.generate_rejection_maps
    {
        return Err(registered_stack_configuration_error());
    }
    let estimator = match integration.estimator {
        RegisteredStackEstimatorInput::StrictMean => RegisteredStackEstimator::StrictMean,
        RegisteredStackEstimatorInput::WeightedMean => RegisteredStackEstimator::WeightedMean,
        RegisteredStackEstimatorInput::PercentileClipped => {
            RegisteredStackEstimator::PercentileClipped(
                PercentileClipParameters::new(
                    integration.low_fraction,
                    integration.high_fraction,
                    integration.minimum_retained_samples,
                )
                .map_err(|_| registered_stack_configuration_error())?,
            )
        }
    };
    let plan = build_registration_plan_for_ids_sync(session, &request.planning, eligible_ids)?;
    if plan.plan_sha256() != request.expected_plan_sha256 {
        return Err(PreviewCommandError::new(
            "registered_stack_plan_stale",
            "The reviewed registration digest no longer matches native evidence.",
        ));
    }
    if request.artifacts.len() != plan.frames().len() {
        return Err(registered_stack_artifact_set_error());
    }
    let mut by_id = BTreeMap::new();
    for artifact in request.artifacts {
        if !artifact.path.is_absolute() {
            return Err(registered_stack_configuration_error());
        }
        let frame_id =
            FrameId::new(artifact.frame_id).map_err(|_| registered_stack_artifact_set_error())?;
        if by_id.insert(frame_id, artifact.path).is_some() {
            return Err(registered_stack_artifact_set_error());
        }
    }
    let expected_ids = plan
        .frames()
        .iter()
        .map(|frame| frame.frame_id().clone())
        .collect::<BTreeSet<_>>();
    if by_id.keys().cloned().collect::<BTreeSet<_>>() != expected_ids {
        return Err(registered_stack_artifact_set_error());
    }

    let (weight_set, weight_report) = if matches!(
        integration.estimator,
        RegisteredStackEstimatorInput::WeightedMean
    ) {
        let reference_frame_id = request
            .quality_reference_frame_id
            .ok_or_else(registered_stack_configuration_error)?;
        let (reference_frame_id, weights) = validated_registered_weight_set(
            &expected_ids,
            reference_frame_id,
            request.quality_evidence,
        )?;
        let entries = expected_ids
            .iter()
            .map(|frame_id| {
                let weight = weights
                    .weight_for(frame_id)
                    .ok_or_else(registered_stack_configuration_error)?;
                Ok(RegisteredWeightPreflightEntry {
                    frame_id: frame_id.as_str().to_owned(),
                    weight: weight.get(),
                })
            })
            .collect::<Result<Vec<_>, PreviewCommandError>>()?;
        let report = RegisteredStackReportWeights {
            algorithm_id: BALANCED_PSF_WEIGHT_ALGORITHM_ID.to_owned(),
            parameters_sha256: weights.sha256().to_owned(),
            reference_frame_id: reference_frame_id.as_str().to_owned(),
            entries,
        };
        (Some(weights), Some(report))
    } else {
        if !request.quality_evidence.is_empty() || request.quality_reference_frame_id.is_some() {
            return Err(registered_stack_configuration_error());
        }
        (None, None)
    };

    let memory_limit = usize::try_from(request.memory_limit_bytes)
        .map_err(|_| registered_stack_configuration_error())?;
    let memory =
        MemoryBudget::new(memory_limit).map_err(|_| registered_stack_configuration_error())?;
    let mut sources = Vec::new();
    let mut report_sources = Vec::new();
    sources
        .try_reserve_exact(by_id.len())
        .map_err(|_| registered_stack_allocation_error())?;
    report_sources
        .try_reserve_exact(by_id.len())
        .map_err(|_| registered_stack_allocation_error())?;
    for frame in plan.frames() {
        let path = by_id
            .remove(frame.frame_id())
            .ok_or_else(registered_stack_artifact_set_error)?;
        let mut input = File::open(&path).map_err(|_| registered_stack_artifact_error())?;
        let fingerprint =
            fingerprint_reader(&mut input).map_err(|_| registered_stack_artifact_error())?;
        let file_name = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(registered_stack_artifact_error)?
            .to_owned();
        report_sources.push(RegisteredStackReportSource {
            frame_id: frame.frame_id().as_str().to_owned(),
            file_name,
            byte_length: fingerprint.byte_length(),
            sha256: fingerprint.sha256().to_owned(),
        });
        sources.push(RegisteredStackSource::new(
            frame.frame_id().clone(),
            PipelineSource::new(path, fingerprint),
        ));
    }
    let source_count =
        u32::try_from(sources.len()).map_err(|_| registered_stack_configuration_error())?;
    let manifest_sha256 = session
        .manifest
        .canonical_sha256()
        .map_err(|_| registered_stack_artifact_error())?;
    let mut provenance = FitsOutputProvenance::new(
        manifest_sha256.clone(),
        "registered-stack",
        estimator.algorithm_id(),
        source_count,
    )
    .and_then(|value| value.with_plan_sha256(plan.plan_sha256()))
    .map_err(|_| registered_stack_configuration_error())?;
    if let Some(weights) = weight_set.as_ref() {
        provenance = provenance
            .with_parameters_sha256(weights.sha256())
            .map_err(|_| registered_stack_configuration_error())?;
    }
    let output_path = request.output_path;
    let rejection_paths = integration
        .generate_rejection_maps
        .then(|| rejection_map_paths(&output_path))
        .transpose()?;
    let execution = if let Some(weights) = weight_set {
        RegisteredStackRequest::new_weighted(
            plan,
            sources,
            output_path.clone(),
            provenance,
            weights,
        )
    } else {
        RegisteredStackRequest::new_with_estimator(
            plan,
            sources,
            output_path.clone(),
            provenance,
            estimator,
        )
    };
    let mut execution = execution
        .and_then(|value| value.with_band_height(request.band_height))
        .map_err(registered_stack_error)?;
    if let Some((low_path, high_path)) = rejection_paths.as_ref() {
        let low_provenance = FitsOutputProvenance::new(
            manifest_sha256.clone(),
            "registered-rejection-low",
            PERCENTILE_REJECTION_MAP_ALGORITHM_ID,
            source_count,
        )
        .and_then(|value| value.with_plan_sha256(execution.plan().plan_sha256()))
        .map_err(|_| registered_stack_configuration_error())?;
        let high_provenance = FitsOutputProvenance::new(
            manifest_sha256.clone(),
            "registered-rejection-high",
            PERCENTILE_REJECTION_MAP_ALGORITHM_ID,
            source_count,
        )
        .and_then(|value| value.with_plan_sha256(execution.plan().plan_sha256()))
        .map_err(|_| registered_stack_configuration_error())?;
        execution = execution
            .with_rejection_map(RegisteredRejectionMapOutput::new(
                low_path.clone(),
                low_provenance,
                high_path.clone(),
                high_provenance,
            ))
            .map_err(registered_stack_error)?;
    }
    let result = run_registered_stack(&execution, cancellation, &memory, |event| {
        progress(RegisteredStackProgress {
            sequence: event.sequence(),
            stage: event.stage().as_str().to_owned(),
            state: progress_state_name(event.state()),
            completed_units: event.completed_units(),
            total_units: event.total_units(),
            code: event.code().map(str::to_owned),
        });
    })
    .map_err(registered_stack_error)?;
    let output_path_string = output_path.to_str().map(str::to_owned).ok_or_else(|| {
        PreviewCommandError::new(
            "registered_stack_output_path_not_unicode",
            "The integrated output path cannot be represented as Unicode.",
        )
    })?;
    let dimensions = result.dimensions();
    let summary = result.summary();
    let rejection_map_summary = result.rejection_map_summary();
    let low_rejection_map_path = rejection_paths
        .as_ref()
        .map(|(path, _)| unicode_registered_output_path(path))
        .transpose()?;
    let high_rejection_map_path = rejection_paths
        .as_ref()
        .map(|(_, path)| unicode_registered_output_path(path))
        .transpose()?;
    let mut products = vec![registered_stack_report_product(
        "science",
        &output_path,
        summary,
    )?];
    if let (Some((low_path, high_path)), Some(map_summary)) =
        (rejection_paths.as_ref(), rejection_map_summary)
    {
        products.push(registered_stack_report_product(
            "rejection_low",
            low_path,
            map_summary.low(),
        )?);
        products.push(registered_stack_report_product(
            "rejection_high",
            high_path,
            map_summary.high(),
        )?);
    }
    let report = RegisteredStackReport {
        algorithm_id: REGISTERED_STACK_REPORT_ALGORITHM_ID.to_owned(),
        plan_sha256: request.expected_plan_sha256.clone(),
        manifest_sha256,
        integration,
        band_height: request.band_height,
        memory_limit_bytes: memory.limit(),
        peak_reserved_bytes: result.peak_reserved_bytes(),
        dimensions: RegisteredStackReportDimensions {
            width: dimensions.width(),
            height: dimensions.height(),
            planes: dimensions.planes(),
        },
        sources: report_sources,
        weights: weight_report,
        products,
    };
    let (report_sha256, report_bytes) = encode_registered_stack_report(report)?;
    if publish_registered_stack_report(&report_path, &report_bytes).is_err() {
        let mut published = vec![output_path.as_path()];
        if let Some((low_path, high_path)) = rejection_paths.as_ref() {
            published.push(low_path.as_path());
            published.push(high_path.as_path());
        }
        for path in published {
            let _ignored = fs::remove_file(path);
        }
        return Err(registered_stack_report_publication_error());
    }
    let report_path = unicode_registered_output_path(&report_path)?;
    Ok(RegisteredStackResponse {
        plan_sha256: request.expected_plan_sha256,
        output_path: output_path_string,
        width: dimensions.width(),
        height: dimensions.height(),
        planes: dimensions.planes(),
        samples_written: summary.samples_written(),
        substituted_samples: summary.substituted_samples(),
        bytes_written: summary.bytes_written(),
        memory_limit_bytes: memory.limit(),
        peak_reserved_bytes: result.peak_reserved_bytes(),
        estimator: estimator.algorithm_id(),
        low_rejection_map_path,
        high_rejection_map_path,
        rejection_map_samples_written: rejection_map_summary.map(|value| {
            debug_assert_eq!(
                value.low().samples_written(),
                value.high().samples_written()
            );
            value.low().samples_written()
        }),
        report_path,
        report_sha256,
    })
}

fn preview_registered_weights_sync(
    request: RegisteredWeightPreflightRequest,
) -> Result<RegisteredWeightPreflightResponse, PreviewCommandError> {
    if !is_lower_sha256(&request.expected_plan_sha256) {
        return Err(registered_stack_configuration_error());
    }
    let requested_count = request.frame_ids.len();
    let expected_ids = request
        .frame_ids
        .into_iter()
        .map(|value| FrameId::new(value).map_err(|_| registered_stack_configuration_error()))
        .collect::<Result<BTreeSet<_>, _>>()?;
    if expected_ids.len() != requested_count || expected_ids.is_empty() {
        return Err(registered_stack_configuration_error());
    }
    let (reference_frame_id, weight_set) = validated_registered_weight_set(
        &expected_ids,
        request.reference_frame_id,
        request.quality_evidence,
    )?;
    let weights = expected_ids
        .iter()
        .map(|frame_id| {
            let weight = weight_set
                .weight_for(frame_id)
                .ok_or_else(registered_stack_configuration_error)?;
            Ok(RegisteredWeightPreflightEntry {
                frame_id: frame_id.as_str().to_owned(),
                weight: weight.get(),
            })
        })
        .collect::<Result<Vec<_>, PreviewCommandError>>()?;
    Ok(RegisteredWeightPreflightResponse {
        schema_version: 1,
        plan_sha256: request.expected_plan_sha256,
        algorithm_id: BALANCED_PSF_WEIGHT_ALGORITHM_ID,
        parameters_sha256: weight_set.sha256().to_owned(),
        reference_frame_id: reference_frame_id.as_str().to_owned(),
        weights,
    })
}

fn validated_registered_weight_set(
    expected_ids: &BTreeSet<FrameId>,
    reference_frame_id: String,
    quality_evidence: Vec<RegisteredFrameQualityInput>,
) -> Result<(FrameId, RegisteredWeightSet), PreviewCommandError> {
    if quality_evidence.len() != expected_ids.len() {
        return Err(registered_stack_configuration_error());
    }
    let mut quality_by_id = BTreeMap::new();
    for evidence in quality_evidence {
        let frame_id =
            FrameId::new(evidence.frame_id).map_err(|_| registered_stack_configuration_error())?;
        let metrics = QualityWeightMetrics::new(
            evidence.signal_to_noise,
            evidence.fwhm_pixels,
            evidence.eccentricity,
        )
        .map_err(|_| registered_stack_configuration_error())?;
        if quality_by_id.insert(frame_id, metrics).is_some() {
            return Err(registered_stack_configuration_error());
        }
    }
    if quality_by_id.keys().cloned().collect::<BTreeSet<_>>() != *expected_ids {
        return Err(registered_stack_configuration_error());
    }
    let reference_frame_id =
        FrameId::new(reference_frame_id).map_err(|_| registered_stack_configuration_error())?;
    let reference = *quality_by_id
        .get(&reference_frame_id)
        .ok_or_else(registered_stack_configuration_error)?;
    let entries = quality_by_id
        .into_iter()
        .map(|(frame_id, metrics)| RegisteredFrameQuality::new(frame_id, metrics))
        .collect();
    let weight_set = RegisteredWeightSet::from_balanced_psf_metrics(reference, entries)
        .map_err(registered_stack_error)?;
    Ok((reference_frame_id, weight_set))
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn rejection_map_paths(output: &Path) -> Result<(PathBuf, PathBuf), PreviewCommandError> {
    let parent = output
        .parent()
        .ok_or_else(registered_stack_configuration_error)?;
    let stem = output
        .file_stem()
        .ok_or_else(registered_stack_configuration_error)?;
    let extension = output.extension();
    let named = |suffix: &str| {
        let mut name = stem.to_os_string();
        name.push(suffix);
        if let Some(extension) = extension {
            name.push(".");
            name.push(extension);
        }
        parent.join(name)
    };
    Ok((named("-rejection-low"), named("-rejection-high")))
}

fn registered_stack_report_path(output: &Path) -> Result<PathBuf, PreviewCommandError> {
    let parent = output
        .parent()
        .ok_or_else(registered_stack_configuration_error)?;
    let stem = output
        .file_stem()
        .ok_or_else(registered_stack_configuration_error)?;
    let mut name = stem.to_os_string();
    name.push("-integration-report.json");
    Ok(parent.join(name))
}

fn require_absent_registered_report(path: &Path) -> Result<(), PreviewCommandError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(PreviewCommandError::new(
            "registered_stack_report_exists",
            "The integration report destination already exists and was not modified.",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(registered_stack_report_publication_error()),
    }
}

fn registered_stack_report_product(
    role: &'static str,
    path: &Path,
    summary: FitsWriteSummary,
) -> Result<RegisteredStackReportProduct, PreviewCommandError> {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(registered_stack_configuration_error)?
        .to_owned();
    let checksum = summary
        .encoded_checksum()
        .map(|bytes| bytes.into_iter().map(char::from).collect());
    Ok(RegisteredStackReportProduct {
        role: role.to_owned(),
        file_name,
        samples_written: summary.samples_written(),
        substituted_samples: summary.substituted_samples(),
        bytes_written: summary.bytes_written(),
        data_sum: summary.data_checksum(),
        checksum,
    })
}

fn encode_registered_stack_report(
    report: RegisteredStackReport,
) -> Result<(String, Vec<u8>), PreviewCommandError> {
    let canonical = serde_json::to_vec(&report).map_err(|_| registered_stack_report_error())?;
    let report_sha256 = lowercase_hex(&Sha256::digest(&canonical));
    let envelope = RegisteredStackReportEnvelope {
        schema_version: 1,
        report_sha256: report_sha256.clone(),
        report,
    };
    let mut encoded =
        serde_json::to_vec_pretty(&envelope).map_err(|_| registered_stack_report_error())?;
    encoded.push(b'\n');
    Ok((report_sha256, encoded))
}

fn inspect_registered_stack_report_sync(
    path: &Path,
) -> Result<RegisteredStackReportInspectionResponse, PreviewCommandError> {
    inspect_registered_stack_report_sync_with_products(path, true)
}

fn inspect_registered_stack_report_sync_with_products(
    path: &Path,
    verify_products: bool,
) -> Result<RegisteredStackReportInspectionResponse, PreviewCommandError> {
    let file = File::open(path).map_err(|_| registered_stack_report_validation_error())?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(
            usize::try_from(MAX_REGISTERED_STACK_REPORT_BYTES)
                .map_err(|_| registered_stack_report_validation_error())?,
        )
        .map_err(|_| registered_stack_allocation_error())?;
    file.take(MAX_REGISTERED_STACK_REPORT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| registered_stack_report_validation_error())?;
    if u64::try_from(bytes.len()).map_err(|_| registered_stack_report_validation_error())?
        > MAX_REGISTERED_STACK_REPORT_BYTES
    {
        return Err(registered_stack_report_validation_error());
    }
    let envelope: RegisteredStackReportEnvelope =
        serde_json::from_slice(&bytes).map_err(|_| registered_stack_report_validation_error())?;
    if envelope.schema_version != 1
        || !is_lower_sha256(&envelope.report_sha256)
        || envelope.report.algorithm_id != REGISTERED_STACK_REPORT_ALGORITHM_ID
        || !is_lower_sha256(&envelope.report.plan_sha256)
        || !is_lower_sha256(&envelope.report.manifest_sha256)
        || envelope.report.dimensions.width == 0
        || envelope.report.dimensions.height == 0
        || envelope.report.dimensions.planes == 0
        || envelope.report.sources.is_empty()
        || envelope.report.products.is_empty()
    {
        return Err(registered_stack_report_validation_error());
    }
    let canonical =
        serde_json::to_vec(&envelope.report).map_err(|_| registered_stack_report_error())?;
    if lowercase_hex(&Sha256::digest(&canonical)) != envelope.report_sha256 {
        return Err(PreviewCommandError::new(
            "registered_stack_report_digest_mismatch",
            "The integration report payload does not match its SHA-256 seal.",
        ));
    }
    let source_ids = envelope
        .report
        .sources
        .iter()
        .map(|source| {
            if !is_lower_sha256(&source.frame_id)
                || !is_lower_sha256(&source.sha256)
                || !is_safe_report_file_name(&source.file_name)
                || source.byte_length == 0
            {
                return Err(registered_stack_report_validation_error());
            }
            Ok(source.frame_id.as_str())
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    if source_ids.len() != envelope.report.sources.len() {
        return Err(registered_stack_report_validation_error());
    }
    let source_names = envelope
        .report
        .sources
        .iter()
        .map(|source| source.file_name.as_str())
        .collect::<BTreeSet<_>>();
    if source_names.len() != envelope.report.sources.len() {
        return Err(registered_stack_report_validation_error());
    }
    let product_roles = envelope
        .report
        .products
        .iter()
        .map(|product| {
            if !is_safe_report_file_name(&product.file_name)
                || product.samples_written == 0
                || product.bytes_written == 0
                || product
                    .checksum
                    .as_ref()
                    .is_none_or(|checksum| checksum.len() != 16 || !checksum.is_ascii())
            {
                return Err(registered_stack_report_validation_error());
            }
            Ok(product.role.as_str())
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    if product_roles.len() != envelope.report.products.len() || !product_roles.contains("science") {
        return Err(registered_stack_report_validation_error());
    }
    if product_roles
        .iter()
        .any(|role| !matches!(*role, "science" | "rejection_low" | "rejection_high"))
    {
        return Err(registered_stack_report_validation_error());
    }
    let weighted = matches!(
        envelope.report.integration.estimator,
        RegisteredStackEstimatorInput::WeightedMean
    );
    match &envelope.report.weights {
        Some(weights) if weighted => {
            if weights.algorithm_id != BALANCED_PSF_WEIGHT_ALGORITHM_ID
                || !is_lower_sha256(&weights.parameters_sha256)
                || !source_ids.contains(weights.reference_frame_id.as_str())
                || weights.entries.len() != source_ids.len()
            {
                return Err(registered_stack_report_validation_error());
            }
            let weighted_ids = weights
                .entries
                .iter()
                .map(|entry| {
                    if !source_ids.contains(entry.frame_id.as_str())
                        || !entry.weight.is_finite()
                        || entry.weight <= 0.0
                    {
                        return Err(registered_stack_report_validation_error());
                    }
                    Ok(entry.frame_id.as_str())
                })
                .collect::<Result<BTreeSet<_>, _>>()?;
            if weighted_ids.len() != source_ids.len() {
                return Err(registered_stack_report_validation_error());
            }
        }
        None if !weighted => {}
        _ => return Err(registered_stack_report_validation_error()),
    }
    let report_parent = path
        .parent()
        .ok_or_else(registered_stack_report_validation_error)?;
    let products = if verify_products {
        envelope
            .report
            .products
            .iter()
            .map(|product| {
                inspect_registered_stack_product(
                    report_parent,
                    product,
                    &envelope.report,
                    source_ids.len(),
                )
            })
            .collect::<Result<Vec<_>, _>>()?
    } else {
        Vec::new()
    };
    let all_products_verified = verify_products
        && products
            .iter()
            .all(|product| product.status == RegisteredStackReportProductStatus::Verified);
    let sources = envelope
        .report
        .sources
        .iter()
        .map(|source| RegisteredStackReportSourceInspection {
            frame_id: source.frame_id.clone(),
            file_name: source.file_name.clone(),
            byte_length: source.byte_length,
            sha256: source.sha256.clone(),
        })
        .collect();
    Ok(RegisteredStackReportInspectionResponse {
        schema_version: envelope.schema_version,
        report_sha256: envelope.report_sha256,
        plan_sha256: envelope.report.plan_sha256,
        manifest_sha256: envelope.report.manifest_sha256,
        estimator: envelope.report.integration.estimator,
        width: envelope.report.dimensions.width,
        height: envelope.report.dimensions.height,
        planes: envelope.report.dimensions.planes,
        source_count: source_ids.len(),
        product_count: product_roles.len(),
        weighted,
        all_products_verified,
        sources,
        products,
    })
}

fn verify_registered_stack_sources_sync<F>(
    report_path: &Path,
    source_directory: &Path,
    cancellation: &CancellationToken,
    mut on_progress: F,
) -> Result<RegisteredStackSourceVerificationResponse, PreviewCommandError>
where
    F: FnMut(RegisteredStackSourceVerificationProgress),
{
    cancellation
        .checkpoint()
        .map_err(|_| registered_stack_source_verification_cancelled_error())?;
    let directory_metadata = fs::symlink_metadata(source_directory)
        .map_err(|_| registered_stack_source_directory_error())?;
    if !directory_metadata.file_type().is_dir() {
        return Err(registered_stack_source_directory_error());
    }
    let inspection = inspect_registered_stack_report_sync_with_products(report_path, false)?;
    cancellation
        .checkpoint()
        .map_err(|_| registered_stack_source_verification_cancelled_error())?;
    let total_sources = inspection.sources.len();
    let total_bytes = inspection
        .sources
        .iter()
        .try_fold(0_u64, |total, source| total.checked_add(source.byte_length))
        .ok_or_else(registered_stack_report_validation_error)?;
    let mut progress_sequence = 0_u64;
    let mut completed_bytes = 0_u64;
    on_progress(RegisteredStackSourceVerificationProgress {
        sequence: progress_sequence,
        state: "started",
        completed_sources: 0,
        total_sources,
        current_file_name: None,
        completed_bytes,
        total_bytes,
        current_file_bytes: 0,
        current_file_total_bytes: 0,
    });
    let source_directory_text = source_directory
        .to_str()
        .ok_or_else(registered_stack_source_directory_error)?
        .to_owned();
    let mut sources = Vec::new();
    sources
        .try_reserve_exact(total_sources)
        .map_err(|_| registered_stack_allocation_error())?;
    for (index, source) in inspection.sources.iter().enumerate() {
        cancellation
            .checkpoint()
            .map_err(|_| registered_stack_source_verification_cancelled_error())?;
        let completed_before_file = completed_bytes;
        sources.push(verify_registered_stack_source(
            source_directory,
            source,
            cancellation,
            |current_file_bytes| {
                let Some(current_file_bytes) =
                    intermediate_source_progress_bytes(current_file_bytes, source.byte_length)
                else {
                    return;
                };
                progress_sequence = progress_sequence.saturating_add(1);
                on_progress(RegisteredStackSourceVerificationProgress {
                    sequence: progress_sequence,
                    state: "running",
                    completed_sources: index,
                    total_sources,
                    current_file_name: Some(source.file_name.clone()),
                    completed_bytes: completed_before_file.saturating_add(current_file_bytes),
                    total_bytes,
                    current_file_bytes,
                    current_file_total_bytes: source.byte_length,
                });
            },
        )?);
        let completed_sources = index + 1;
        completed_bytes = completed_bytes
            .checked_add(source.byte_length)
            .ok_or_else(registered_stack_report_validation_error)?;
        progress_sequence = progress_sequence
            .checked_add(1)
            .ok_or_else(registered_stack_report_validation_error)?;
        on_progress(RegisteredStackSourceVerificationProgress {
            sequence: progress_sequence,
            state: if completed_sources == total_sources {
                "completed"
            } else {
                "running"
            },
            completed_sources,
            total_sources,
            current_file_name: Some(source.file_name.clone()),
            completed_bytes,
            total_bytes,
            current_file_bytes: source.byte_length,
            current_file_total_bytes: source.byte_length,
        });
    }
    let all_sources_verified = sources
        .iter()
        .all(|source| source.status == RegisteredStackSourceVerificationStatus::Verified);
    Ok(RegisteredStackSourceVerificationResponse {
        report_sha256: inspection.report_sha256,
        source_directory: source_directory_text,
        all_sources_verified,
        sources,
    })
}

/// Keeps in-file updates strictly below the sealed size.
///
/// The caller emits one authoritative file-boundary event after verification.
/// Suppressing equal values avoids duplicate IPC for small sources, while
/// suppressing larger values prevents a concurrently grown file from making
/// aggregate progress exceed its report-bound total before it is rejected.
fn intermediate_source_progress_bytes(current: u64, sealed: u64) -> Option<u64> {
    (current < sealed).then_some(current)
}

fn verify_registered_stack_source<F>(
    source_directory: &Path,
    source: &RegisteredStackReportSourceInspection,
    cancellation: &CancellationToken,
    mut on_bytes: F,
) -> Result<RegisteredStackSourceVerification, PreviewCommandError>
where
    F: FnMut(u64),
{
    let path = source_directory.join(&source.file_name);
    let path_text = path
        .to_str()
        .ok_or_else(registered_stack_source_directory_error)?
        .to_owned();
    let status = match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            RegisteredStackSourceVerificationStatus::Missing
        }
        Err(_) => RegisteredStackSourceVerificationStatus::ReadFailed,
        Ok(metadata) if !metadata.file_type().is_file() => {
            RegisteredStackSourceVerificationStatus::NonRegular
        }
        Ok(metadata) if metadata.len() != source.byte_length => {
            RegisteredStackSourceVerificationStatus::ByteLengthMismatch
        }
        Ok(_) => match File::open(&path).and_then(|mut file| {
            fingerprint_registered_stack_source(&mut file, cancellation, &mut on_bytes)
                .map_err(std::io::Error::other)
        }) {
            Ok((byte_length, sha256))
                if byte_length == source.byte_length
                    && sha256.as_str() == source.sha256.as_str() =>
            {
                RegisteredStackSourceVerificationStatus::Verified
            }
            Ok(_) => RegisteredStackSourceVerificationStatus::FingerprintMismatch,
            Err(_) if cancellation.is_cancelled() => {
                return Err(registered_stack_source_verification_cancelled_error());
            }
            Err(_) => RegisteredStackSourceVerificationStatus::ReadFailed,
        },
    };
    Ok(RegisteredStackSourceVerification {
        frame_id: source.frame_id.clone(),
        file_name: source.file_name.clone(),
        path: path_text,
        byte_length: source.byte_length,
        status,
    })
}

fn fingerprint_registered_stack_source<R, F>(
    reader: &mut R,
    cancellation: &CancellationToken,
    mut on_bytes: F,
) -> Result<(u64, String), Box<dyn Error + Send + Sync>>
where
    R: Read,
    F: FnMut(u64),
{
    let Some(progress_interval) = NonZeroU64::new(SOURCE_VERIFICATION_PROGRESS_BYTES) else {
        return Err(Box::new(std::io::Error::other(
            "source progress interval must be non-zero",
        )));
    };
    let mut reader = CancellationCheckpointReader {
        inner: reader,
        cancellation,
    };
    let fingerprint =
        fingerprint_reader_with_progress(&mut reader, progress_interval, &mut on_bytes)?;
    Ok((fingerprint.byte_length(), fingerprint.sha256().to_owned()))
}

struct CancellationCheckpointReader<'a, R> {
    inner: &'a mut R,
    cancellation: &'a CancellationToken,
}

impl<R: Read> Read for CancellationCheckpointReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.cancellation
            .checkpoint()
            .map_err(std::io::Error::other)?;
        self.inner.read(buffer)
    }
}

fn is_safe_report_file_name(file_name: &str) -> bool {
    let mut components = Path::new(file_name).components();
    matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none()
}

fn inspect_registered_stack_product(
    report_parent: &Path,
    product: &RegisteredStackReportProduct,
    report: &RegisteredStackReport,
    source_count: usize,
) -> Result<RegisteredStackReportProductInspection, PreviewCommandError> {
    let path = report_parent.join(&product.file_name);
    let path_text = path
        .to_str()
        .ok_or_else(registered_stack_report_validation_error)?
        .to_owned();
    let status = match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            RegisteredStackReportProductStatus::Missing
        }
        Err(_) => RegisteredStackReportProductStatus::NonRegular,
        Ok(metadata) if !metadata.file_type().is_file() => {
            RegisteredStackReportProductStatus::NonRegular
        }
        Ok(metadata) if metadata.len() != product.bytes_written => {
            RegisteredStackReportProductStatus::ByteLengthMismatch
        }
        Ok(_) => inspect_registered_stack_product_fits(&path, product, report, source_count),
    };
    Ok(RegisteredStackReportProductInspection {
        role: product.role.clone(),
        file_name: product.file_name.clone(),
        path: path_text,
        bytes_written: product.bytes_written,
        status,
    })
}

fn inspect_registered_stack_product_fits(
    path: &Path,
    product: &RegisteredStackReportProduct,
    report: &RegisteredStackReport,
    source_count: usize,
) -> RegisteredStackReportProductStatus {
    let Ok(file) = File::open(path) else {
        return RegisteredStackReportProductStatus::InvalidFits;
    };
    let Ok(mut reader) = PrimaryImageReader::open(file, HeaderReadOptions::default()) else {
        return RegisteredStackReportProductStatus::InvalidFits;
    };
    if !reader
        .report()
        .is_accepted(aether_fits::ValidationMode::Strict)
    {
        return RegisteredStackReportProductStatus::InvalidFits;
    }
    let expected_axes = if report.dimensions.planes == 1 {
        vec![
            u64::try_from(report.dimensions.width).ok(),
            u64::try_from(report.dimensions.height).ok(),
        ]
    } else {
        vec![
            u64::try_from(report.dimensions.width).ok(),
            u64::try_from(report.dimensions.height).ok(),
            u64::try_from(report.dimensions.planes).ok(),
        ]
    };
    let Some(expected_axes) = expected_axes.into_iter().collect::<Option<Vec<_>>>() else {
        return RegisteredStackReportProductStatus::MetadataMismatch;
    };
    let expected_algorithm = match product.role.as_str() {
        "science" => registered_stack_estimator_algorithm_id(report.integration.estimator),
        "rejection_low" | "rejection_high" => PERCENTILE_REJECTION_MAP_ALGORITHM_ID,
        _ => return RegisteredStackReportProductStatus::MetadataMismatch,
    };
    let header = reader.report().header();
    if reader.descriptor().axes() != expected_axes
        || reader.descriptor().pixel_count() != product.samples_written
        || header.string("AETHMAN") != Some(report.manifest_sha256.as_str())
        || header.string("AETHPLN") != Some(report.plan_sha256.as_str())
        || header.string("AETHALG") != Some(expected_algorithm)
        || header.integer("AETHSRC") != i64::try_from(source_count).ok()
        || header.string("CHECKSUM") != product.checksum.as_deref()
    {
        return RegisteredStackReportProductStatus::MetadataMismatch;
    }
    let Ok(checksums) = reader.verify_checksums() else {
        return RegisteredStackReportProductStatus::InvalidFits;
    };
    match (checksums.datasum(), checksums.checksum(), product.data_sum) {
        (
            DatasumVerification::Valid { checksum },
            HduChecksumVerification::Valid,
            Some(expected),
        ) if checksum == expected => RegisteredStackReportProductStatus::Verified,
        _ => RegisteredStackReportProductStatus::ChecksumMismatch,
    }
}

const fn registered_stack_estimator_algorithm_id(
    estimator: RegisteredStackEstimatorInput,
) -> &'static str {
    match estimator {
        RegisteredStackEstimatorInput::StrictMean => {
            aether_runtime::REGISTERED_CROP_MEAN_ALGORITHM_ID
        }
        RegisteredStackEstimatorInput::WeightedMean => {
            aether_runtime::REGISTERED_WEIGHTED_MEAN_ALGORITHM_ID
        }
        RegisteredStackEstimatorInput::PercentileClipped => {
            aether_runtime::REGISTERED_PERCENTILE_CLIPPED_MEAN_ALGORITHM_ID
        }
    }
}

fn lowercase_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn publish_registered_stack_report(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "report path has no parent",
        )
    })?;
    let file_name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "report path has no file name",
        )
    })?;
    let mut last_error = None;
    for _ in 0..128 {
        let sequence = REGISTERED_STACK_REPORT_TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let mut temporary_name = file_name.to_os_string();
        temporary_name.push(format!(".aetherstack-{sequence}.tmp"));
        let temporary_path = parent.join(temporary_name);
        let mut temporary = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                last_error = Some(error);
                continue;
            }
            Err(error) => return Err(error),
        };
        let mut published = false;
        let staged = (|| {
            temporary.write_all(bytes)?;
            temporary.sync_all()?;
            fs::hard_link(&temporary_path, path)?;
            published = true;
            fs::remove_file(&temporary_path)?;
            File::open(parent)?.sync_all()
        })();
        if staged.is_err() {
            let _ignored = fs::remove_file(&temporary_path);
            if published {
                let _ignored = fs::remove_file(path);
            }
        }
        return staged;
    }
    Err(last_error.unwrap_or_else(|| std::io::Error::other("report staging namespace exhausted")))
}

fn unicode_registered_output_path(path: &Path) -> Result<String, PreviewCommandError> {
    path.to_str().map(str::to_owned).ok_or_else(|| {
        PreviewCommandError::new(
            "registered_stack_output_path_not_unicode",
            "A registered-stack output path cannot be represented as Unicode.",
        )
    })
}

fn registration_native_sources(
    session: &ImportedNativeSession,
) -> Result<BTreeMap<FrameId, RegistrationNativeSource>, PreviewCommandError> {
    let files = session
        .manifest
        .files()
        .iter()
        .map(|file| (file.relative_path(), file))
        .collect::<BTreeMap<_, _>>();
    let mut sources = BTreeMap::new();
    for group in session
        .manifest
        .groups()
        .iter()
        .filter(|group| group.key().frame_type() == &FrameType::Light)
    {
        for relative_path in group.files() {
            let file = files
                .get(relative_path.as_str())
                .ok_or_else(registration_plan_input_error)?;
            let [width, height] = file.axes() else {
                return Err(registration_plan_input_error());
            };
            let width = usize::try_from(*width).map_err(|_| registration_plan_input_error())?;
            let height = usize::try_from(*height).map_err(|_| registration_plan_input_error())?;
            let frame_id = FrameId::derive(
                file.relative_path(),
                file.fingerprint().byte_length(),
                file.fingerprint().sha256(),
            )
            .map_err(|_| registration_plan_input_error())?;
            let source = RegistrationNativeSource {
                path: session.root.join(file.relative_path()),
                width,
                height,
            };
            if sources.insert(frame_id, source).is_some() {
                return Err(registration_plan_input_error());
            }
        }
    }
    if sources.len() < 2 {
        return Err(registration_plan_input_error());
    }
    Ok(sources)
}

fn reviewed_registration_frame_ids(
    review_state: &DesktopReviewState,
    session: &ImportedNativeSession,
) -> Result<BTreeSet<FrameId>, PreviewCommandError> {
    let sources = registration_native_sources(session)?;
    let state = lock_review_state(review_state)?;
    let book = state.as_ref().ok_or_else(review_state_missing_error)?;
    let mut eligible = BTreeSet::new();
    for frame_id in sources.keys() {
        let review = book
            .state(frame_id)
            .ok_or_else(registration_plan_input_error)?;
        if review != ReviewState::Rejected {
            eligible.insert(frame_id.clone());
        }
    }
    if eligible.len() < 2 {
        return Err(registration_plan_input_error());
    }
    Ok(eligible)
}

const fn registration_plan_input_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "registration_plan_input_invalid",
        "The submitted frame identities do not match the imported Light set.",
    )
}

const fn registration_plan_allocation_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "registration_plan_allocation_failed",
        "The registration plan could not reserve its bounded evidence buffer.",
    )
}

const fn registration_plan_diagnostic_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "registration_plan_diagnostic_failed",
        "A Light pair could not reproduce its native geometric diagnostic.",
    )
}

const fn registration_plan_rejected_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "registration_plan_pair_rejected",
        "At least one Light pair no longer passes the native confidence gate.",
    )
}

const fn registration_plan_geometry_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "registration_plan_geometry_invalid",
        "The accepted transforms do not form a valid common registration plan.",
    )
}

const fn registration_execution_configuration_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "registration_execution_configuration_invalid",
        "Registration execution requires absolute paths and positive bounded resources.",
    )
}

const fn registration_artifact_set_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "registration_artifact_set_invalid",
        "The calibrated artifact identities do not match the sealed registration plan.",
    )
}

const fn registration_artifact_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "registration_artifact_invalid",
        "A calibrated registration artifact could not be opened or fingerprinted.",
    )
}

const fn registration_execution_allocation_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "registration_execution_allocation_failed",
        "Registration could not reserve its bounded transaction bookkeeping.",
    )
}

fn registration_execution_error(error: RegistrationPlanExecutionError) -> PreviewCommandError {
    match error {
        RegistrationPlanExecutionError::Cancelled(_) => PreviewCommandError::new(
            "registration_execution_cancelled",
            "Registration was cancelled before the complete frame set was published.",
        ),
        RegistrationPlanExecutionError::DestinationExists(_) => PreviewCommandError::new(
            "registration_destination_exists",
            "A registered destination already exists; no output was replaced.",
        ),
        RegistrationPlanExecutionError::InvalidSourceIdentity
        | RegistrationPlanExecutionError::SourceSetMismatch => registration_artifact_set_error(),
        RegistrationPlanExecutionError::InvalidOutputDirectory => {
            registration_execution_configuration_error()
        }
        RegistrationPlanExecutionError::AllocationFailed => {
            registration_execution_allocation_error()
        }
        RegistrationPlanExecutionError::Provenance(_)
        | RegistrationPlanExecutionError::FramePipeline { .. } => PreviewCommandError::new(
            "registration_execution_failed",
            "A registered frame failed native validation; the complete set remains unpublished.",
        ),
        RegistrationPlanExecutionError::CreateStagingDirectory(_)
        | RegistrationPlanExecutionError::PublishProduct { .. }
        | RegistrationPlanExecutionError::SyncOutputDirectory(_)
        | RegistrationPlanExecutionError::RollbackPublication { .. } => PreviewCommandError::new(
            "registration_publication_failed",
            "The registered frame set could not be published as one atomic transaction.",
        ),
    }
}

const fn registered_stack_configuration_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "registered_stack_configuration_invalid",
        "Registered-stack settings or paths are invalid.",
    )
}

const fn registered_stack_artifact_set_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "registered_stack_artifact_set_invalid",
        "The registered artifacts do not match every identity in the sealed plan.",
    )
}

const fn registered_stack_artifact_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "registered_stack_artifact_invalid",
        "A registered artifact could not be opened or fingerprinted.",
    )
}

const fn registered_stack_allocation_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "registered_stack_allocation_failed",
        "The registered stack could not reserve bounded transaction bookkeeping.",
    )
}

const fn registered_stack_report_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "registered_stack_report_invalid",
        "The deterministic integration report could not be encoded.",
    )
}

const fn registered_stack_report_publication_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "registered_stack_report_publication_failed",
        "The integration report could not be published; stack products were rolled back.",
    )
}

const fn registered_stack_report_validation_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "registered_stack_report_invalid",
        "The integration report is malformed or contains incoherent evidence.",
    )
}

const fn registered_stack_source_directory_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "registered_stack_source_directory_invalid",
        "The selected source directory is unavailable or is not a regular directory.",
    )
}

const fn registered_stack_source_verification_cancelled_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "registered_stack_source_verification_cancelled",
        "Archived source verification was cancelled before all fingerprints were read.",
    )
}

fn registered_stack_error(error: RegisteredStackError) -> PreviewCommandError {
    match error {
        RegisteredStackError::Cancelled(_) => PreviewCommandError::new(
            "registered_stack_cancelled",
            "Integration was cancelled before the output was published.",
        ),
        RegisteredStackError::SourceSetMismatch
        | RegisteredStackError::ArtifactIdentityMismatch { .. }
        | RegisteredStackError::ArtifactPlanMismatch { .. } => {
            registered_stack_artifact_set_error()
        }
        RegisteredStackError::ZeroBandHeight
        | RegisteredStackError::ProvenanceAlgorithmMismatch
        | RegisteredStackError::ProvenanceSourceCountMismatch
        | RegisteredStackError::ProvenancePlanMismatch
        | RegisteredStackError::WeightedEstimatorRequiresWeights
        | RegisteredStackError::InvalidWeightAlgorithmId
        | RegisteredStackError::WeightSetMismatch
        | RegisteredStackError::WeightProvenanceMismatch
        | RegisteredStackError::RejectionMapRequiresPercentileEstimator
        | RegisteredStackError::RejectionMapProvenanceMismatch
        | RegisteredStackError::DuplicateOutputPath => registered_stack_configuration_error(),
        RegisteredStackError::Memory(_) | RegisteredStackError::AllocationFailed => {
            registered_stack_allocation_error()
        }
        RegisteredStackError::Publish(_) | RegisteredStackError::RollbackPublishedOutput(_) => {
            PreviewCommandError::new(
                "registered_stack_publication_failed",
                "The integrated common crop could not be published atomically.",
            )
        }
        RegisteredStackError::DimensionMismatch { .. }
        | RegisteredStackError::PlaneCountMismatch { .. }
        | RegisteredStackError::Input(_)
        | RegisteredStackError::ReadInput(_)
        | RegisteredStackError::Integration(_)
        | RegisteredStackError::InvalidStagedOutput
        | RegisteredStackError::WorkSizeOverflow
        | RegisteredStackError::StageId(_)
        | RegisteredStackError::Progress(_) => PreviewCommandError::new(
            "registered_stack_execution_failed",
            "Registered integration failed strict native validation; no output was published.",
        ),
    }
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

fn inspect_rejection_histogram_reader<R: Read + Seek>(
    input: R,
) -> Result<RejectionHistogramResponse, PreviewCommandError> {
    let mut reader =
        PrimaryImageReader::open(input, HeaderReadOptions::default()).map_err(|_| {
            PreviewCommandError::new(
                "rejection_histogram_header_failed",
                "The rejection-map primary header could not be inspected.",
            )
        })?;
    let total_samples = reader.descriptor().pixel_count();
    let mut bins = BTreeMap::<u32, u64>::new();
    let mut values = vec![0.0; DESKTOP_PREVIEW_IO_CHUNK_SAMPLES];
    let mut statuses = vec![SampleStatus::Valid; DESKTOP_PREVIEW_IO_CHUNK_SAMPLES];
    let mut start = 0_u64;
    while start < total_samples {
        let remaining = total_samples - start;
        let count = usize::try_from(remaining.min(DESKTOP_PREVIEW_IO_CHUNK_SAMPLES as u64))
            .map_err(|_| rejection_histogram_error())?;
        reader
            .read_physical_samples(start, &mut values[..count], &mut statuses[..count])
            .map_err(|_| rejection_histogram_error())?;
        for (&value, &status) in values[..count].iter().zip(&statuses[..count]) {
            if status != SampleStatus::Valid
                || !value.is_finite()
                || value < 0.0
                || value.fract() != 0.0
                || value > f64::from(u32::MAX)
            {
                return Err(PreviewCommandError::new(
                    "rejection_histogram_sample_invalid",
                    "A rejection map contains a missing, non-finite, negative, or fractional count.",
                ));
            }
            let rejected_count = value as u32;
            if !bins.contains_key(&rejected_count) && bins.len() >= MAX_REJECTION_HISTOGRAM_BINS {
                return Err(PreviewCommandError::new(
                    "rejection_histogram_bins_exceeded",
                    "The rejection map contains too many distinct count values.",
                ));
            }
            let samples = bins.entry(rejected_count).or_default();
            *samples = samples
                .checked_add(1)
                .ok_or_else(rejection_histogram_error)?;
        }
        start = start
            .checked_add(u64::try_from(count).map_err(|_| rejection_histogram_error())?)
            .ok_or_else(rejection_histogram_error)?;
    }

    let zero_samples = bins.get(&0).copied().unwrap_or(0);
    let rejected_samples = total_samples
        .checked_sub(zero_samples)
        .ok_or_else(rejection_histogram_error)?;
    let maximum_rejected_count = bins.last_key_value().map_or(0, |(&count, _)| count);
    let bins = bins
        .into_iter()
        .map(|(rejected_count, samples)| RejectionHistogramBin {
            rejected_count,
            samples,
        })
        .collect();
    Ok(RejectionHistogramResponse {
        algorithm_id: REJECTION_HISTOGRAM_ALGORITHM_ID,
        total_samples,
        zero_samples,
        rejected_samples,
        maximum_rejected_count,
        bins,
    })
}

fn rejection_histogram_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "rejection_histogram_failed",
        "The rejection-count histogram could not be calculated exactly.",
    )
}

fn inspect_stack_pixel_sync(
    request: &StackPixelInspectionRequest,
) -> Result<StackPixelInspectionResponse, PreviewCommandError> {
    let science = File::open(&request.science_path).map_err(|_| pixel_inspection_open_error())?;
    let (width, height, science_values) = read_pixel_planes(science, request.x, request.y, None)?;
    let science_planes =
        u64::try_from(science_values.len()).map_err(|_| pixel_inspection_read_error())?;
    let low_rejection_counts = request
        .low_rejection_path
        .as_ref()
        .map(|path| {
            let file = File::open(path).map_err(|_| pixel_inspection_open_error())?;
            let (_, _, values) = read_pixel_planes(
                file,
                request.x,
                request.y,
                Some((width, height, science_planes)),
            )?;
            rejection_counts(values)
        })
        .transpose()?;
    let high_rejection_counts = request
        .high_rejection_path
        .as_ref()
        .map(|path| {
            let file = File::open(path).map_err(|_| pixel_inspection_open_error())?;
            let (_, _, values) = read_pixel_planes(
                file,
                request.x,
                request.y,
                Some((width, height, science_planes)),
            )?;
            rejection_counts(values)
        })
        .transpose()?;
    Ok(StackPixelInspectionResponse {
        x: request.x,
        y: request.y,
        science_values,
        low_rejection_counts,
        high_rejection_counts,
    })
}

fn read_pixel_planes<R: Read + Seek>(
    input: R,
    x: u64,
    y: u64,
    expected_shape: Option<(u64, u64, u64)>,
) -> Result<(u64, u64, Vec<Option<f64>>), PreviewCommandError> {
    let mut reader = PrimaryImageReader::open(input, HeaderReadOptions::default())
        .map_err(|_| pixel_inspection_read_error())?;
    let axes = reader.descriptor().axes();
    if !(2..=3).contains(&axes.len()) {
        return Err(pixel_inspection_read_error());
    }
    let (width, height) = (axes[0], axes[1]);
    let plane_count = axes.get(2).copied().unwrap_or(1);
    if expected_shape.is_some_and(|expected| expected != (width, height, plane_count))
        || x >= width
        || y >= height
    {
        return Err(PreviewCommandError::new(
            "stack_pixel_coordinate_invalid",
            "The requested stack coordinate is outside one or more products.",
        ));
    }
    let plane_size = width
        .checked_mul(height)
        .ok_or_else(pixel_inspection_read_error)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(usize::try_from(plane_count).map_err(|_| pixel_inspection_read_error())?)
        .map_err(|_| pixel_inspection_read_error())?;
    let pixel_offset = y
        .checked_mul(width)
        .and_then(|row| row.checked_add(x))
        .ok_or_else(pixel_inspection_read_error)?;
    for plane in 0..plane_count {
        let start = plane
            .checked_mul(plane_size)
            .and_then(|offset| offset.checked_add(pixel_offset))
            .ok_or_else(pixel_inspection_read_error)?;
        let mut value = [0.0];
        let mut status = [SampleStatus::Valid];
        reader
            .read_physical_samples(start, &mut value, &mut status)
            .map_err(|_| pixel_inspection_read_error())?;
        output.push((status[0] == SampleStatus::Valid && value[0].is_finite()).then_some(value[0]));
    }
    Ok((width, height, output))
}

fn rejection_counts(values: Vec<Option<f64>>) -> Result<Vec<Option<u32>>, PreviewCommandError> {
    values
        .into_iter()
        .map(|value| match value {
            None => Ok(None),
            Some(value) if value >= 0.0 && value.fract() == 0.0 && value <= f64::from(u32::MAX) => {
                Ok(Some(value as u32))
            }
            Some(_) => Err(PreviewCommandError::new(
                "stack_pixel_rejection_invalid",
                "A rejection-map pixel is not an exact non-negative count.",
            )),
        })
        .collect()
}

fn pixel_inspection_open_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "stack_pixel_open_failed",
        "One stack product could not be opened for exact pixel inspection.",
    )
}

fn pixel_inspection_read_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "stack_pixel_read_failed",
        "One stack product could not provide the requested exact pixel.",
    )
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
    let (width, height) =
        quality_source_dimensions(reader.descriptor().axes(), request.interpretation)?;
    let source_samples = width.checked_mul(height).ok_or_else(|| {
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
    let (detection_plane, detection_plane_algorithm_id, interpretation, source_pixel_scale) =
        match request.interpretation {
            QualityInterpretation::Monochrome => {
                let source = read_quality_plane(&mut reader, 0, width, height)?;
                (source, "identity-monochrome-v1", "monochrome", 1.0)
            }
            QualityInterpretation::BayerCellMean { pattern } => {
                let source = read_quality_plane(&mut reader, 0, width, height)?;
                let interpretation = bayer_interpretation_name(pattern);
                let plane = prepare_cfa_cell_mean(source).map_err(|_| {
                    PreviewCommandError::new(
                        "frame_quality_cfa_preparation_failed",
                        "The raw CFA source could not be converted into complete Bayer-cell means.",
                    )
                })?;
                (plane, CFA_CELL_MEAN_ALGORITHM_ID, interpretation, 2.0)
            }
            QualityInterpretation::RgbLuminance => {
                let mut builder = RgbLuminanceBuilder::new(
                    usize::try_from(width).map_err(|_| frame_quality_size_error())?,
                    usize::try_from(height).map_err(|_| frame_quality_size_error())?,
                )
                .map_err(|_| frame_quality_rgb_preparation_error())?;
                for (plane, channel) in [
                    RgbLuminanceChannel::Red,
                    RgbLuminanceChannel::Green,
                    RgbLuminanceChannel::Blue,
                ]
                .into_iter()
                .enumerate()
                {
                    let source = read_quality_plane(
                        &mut reader,
                        u64::try_from(plane).map_err(|_| frame_quality_size_error())?,
                        width,
                        height,
                    )?;
                    builder
                        .push(channel, &source)
                        .map_err(|_| frame_quality_rgb_preparation_error())?;
                }
                let plane = builder
                    .finish()
                    .map_err(|_| frame_quality_rgb_preparation_error())?;
                (
                    plane,
                    RGB_LUMINANCE_ALGORITHM_ID,
                    "calibrated RGB · linear Rec. 709 luminance",
                    1.0,
                )
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
        profile_id: DESKTOP_QUALITY_PROFILE_ID.to_owned(),
        background_algorithm_id: GLOBAL_BACKGROUND_ALGORITHM_ID.to_owned(),
        star_algorithm_id: STAR_MEASUREMENT_ALGORITHM_ID.to_owned(),
        detection_plane_algorithm_id: detection_plane_algorithm_id.to_owned(),
        interpretation: interpretation.to_owned(),
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
        signal_to_noise: quality.median_background_snr(),
        fwhm_pixels: quality
            .median_fwhm_major_pixels()
            .map(|value| value * source_pixel_scale),
        eccentricity: quality.median_eccentricity(),
    })
}

fn quality_source_dimensions(
    axes: &[u64],
    interpretation: QualityInterpretation,
) -> Result<(u64, u64), PreviewCommandError> {
    match (axes, interpretation) {
        (
            [width, height],
            QualityInterpretation::Monochrome | QualityInterpretation::BayerCellMean { .. },
        ) => Ok((*width, *height)),
        ([width, height, 3], QualityInterpretation::RgbLuminance) => Ok((*width, *height)),
        _ => Err(PreviewCommandError::new(
            "frame_quality_axes_unsupported",
            "The FITS axes do not match the requested quality interpretation.",
        )),
    }
}

fn read_quality_plane<R: Read + Seek>(
    reader: &mut PrimaryImageReader<R>,
    plane: u64,
    width: u64,
    height: u64,
) -> Result<aether_core::ScientificImage, PreviewCommandError> {
    reader
        .read_region_image(ImageRegion::new(plane, 0, 0, width, height))
        .map_err(|_| {
            PreviewCommandError::new(
                "frame_quality_decode_failed",
                "A complete linear FITS plane could not be decoded for quality measurement.",
            )
        })
}

const fn frame_quality_size_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "frame_quality_size_overflow",
        "The frame dimensions exceed the supported quality-measurement range.",
    )
}

const fn frame_quality_rgb_preparation_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "frame_quality_rgb_preparation_failed",
        "The planar RGB source could not be converted into linked linear luminance.",
    )
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
    app: tauri::AppHandle,
    review_state: tauri::State<'_, DesktopReviewState>,
    session_state: tauri::State<'_, DesktopSessionState>,
) -> Result<ImportedSession, PreviewCommandError> {
    if !path.is_absolute() {
        return Err(PreviewCommandError::new(
            "session_path_not_absolute",
            "The selected session directory must use an absolute path.",
        ));
    }
    let cache_root = quality_cache_root(&app)?;
    let imported = tauri::async_runtime::spawn_blocking(move || {
        let mut imported = scan_session_directory_sync(&path)?;
        restore_session_quality_evidence(&cache_root, &mut imported.presentation);
        Ok::<_, PreviewCommandError>(imported)
    })
    .await
    .map_err(|_| {
        PreviewCommandError::new(
            "session_import_interrupted",
            "The session import worker stopped before producing a result.",
        )
    })??;
    install_imported_session(&session_state, &review_state, imported)
}

#[tauri::command]
async fn preview_master_plan(
    request: MasterPlanPreviewRequest,
    session_state: tauri::State<'_, DesktopSessionState>,
) -> Result<MasterPlanPreviewResponse, PreviewCommandError> {
    let session = lock_session_state(&session_state)?
        .clone()
        .ok_or_else(session_state_missing_error)?;
    tauri::async_runtime::spawn_blocking(move || preview_master_plan_sync(&session, request))
        .await
        .map_err(|_| {
            PreviewCommandError::new(
                "master_plan_interrupted",
                "The master-planning worker stopped before producing a result.",
            )
        })?
}

#[tauri::command]
async fn execute_master_plan(
    request: MasterPlanExecutionCommandRequest,
    on_progress: tauri::ipc::Channel<MasterExecutionProgress>,
    session_state: tauri::State<'_, DesktopSessionState>,
    execution_state: tauri::State<'_, DesktopCalibrationExecutionState>,
) -> Result<MasterPlanExecutionResponse, PreviewCommandError> {
    if !request.output_directory.is_absolute() {
        return Err(PreviewCommandError::new(
            "master_output_path_not_absolute",
            "The master output directory must use an absolute path.",
        ));
    }
    let session = lock_session_state(&session_state)?
        .clone()
        .ok_or_else(session_state_missing_error)?;
    let cancellation = begin_calibration_execution(&execution_state)?;
    let worker_cancellation = cancellation.clone();
    let execution = tauri::async_runtime::spawn_blocking(move || {
        execute_master_plan_sync(&session, request, &worker_cancellation, |event| {
            // Losing the browser receiver must not compromise or panic the
            // native transaction. The worker remains cancellable through
            // the independently owned command below.
            let _ignored = on_progress.send(event);
        })
    })
    .await;
    finish_calibration_execution(&execution_state)?;
    execution.map_err(|_| {
        PreviewCommandError::new(
            "master_execution_interrupted",
            "The master-build worker stopped before producing a result.",
        )
    })?
}

#[tauri::command]
fn cancel_master_plan(
    execution_state: tauri::State<'_, DesktopCalibrationExecutionState>,
) -> Result<bool, PreviewCommandError> {
    cancel_calibration_execution(&execution_state, "master_execution_missing")
}

#[tauri::command]
async fn execute_light_plan(
    request: LightPlanExecutionCommandRequest,
    on_progress: tauri::ipc::Channel<LightExecutionProgress>,
    session_state: tauri::State<'_, DesktopSessionState>,
    execution_state: tauri::State<'_, DesktopCalibrationExecutionState>,
) -> Result<LightPlanExecutionResponse, PreviewCommandError> {
    if !request.master_directory.is_absolute() || !request.output_directory.is_absolute() {
        return Err(light_execution_configuration_error());
    }
    let session = lock_session_state(&session_state)?
        .clone()
        .ok_or_else(session_state_missing_error)?;
    let cancellation = begin_calibration_execution(&execution_state)?;
    let worker_cancellation = cancellation.clone();
    let execution = tauri::async_runtime::spawn_blocking(move || {
        execute_light_plan_sync(&session, request, &worker_cancellation, |event| {
            let _ignored = on_progress.send(event);
        })
    })
    .await;
    finish_calibration_execution(&execution_state)?;
    execution.map_err(|_| {
        PreviewCommandError::new(
            "light_execution_interrupted",
            "The Light calibration worker stopped before producing a result.",
        )
    })?
}

#[tauri::command]
fn cancel_light_plan(
    execution_state: tauri::State<'_, DesktopCalibrationExecutionState>,
) -> Result<bool, PreviewCommandError> {
    cancel_calibration_execution(&execution_state, "light_execution_missing")
}

fn cancel_calibration_execution(
    execution_state: &DesktopCalibrationExecutionState,
    missing_code: &'static str,
) -> Result<bool, PreviewCommandError> {
    let state = lock_calibration_execution(execution_state)?;
    let cancellation = state.as_ref().ok_or_else(|| {
        PreviewCommandError::new(missing_code, "No calibration task is currently running.")
    })?;
    Ok(cancellation.cancel())
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

#[tauri::command]
fn preview_frame_selection(
    request: FrameSelectionPreviewRequest,
    review_state: tauri::State<'_, DesktopReviewState>,
) -> Result<FrameSelectionPlan, PreviewCommandError> {
    preview_frame_selection_sync(&review_state, request)
}

#[tauri::command]
fn apply_frame_selection(
    request: FrameSelectionApplyRequest,
    review_state: tauri::State<'_, DesktopReviewState>,
) -> Result<ReviewDecisionUpdate, PreviewCommandError> {
    apply_frame_selection_sync(&review_state, request)
}

fn install_imported_session(
    session_state: &DesktopSessionState,
    review_state: &DesktopReviewState,
    imported: ImportedSessionBundle,
) -> Result<ImportedSession, PreviewCommandError> {
    let book = prepare_review_book(&imported.presentation)?;
    let restored_quality = prepare_restored_quality(&imported.presentation)?;
    let mut native_state = lock_session_state(session_state)?;
    let mut native_review = lock_review_state(review_state)?;
    let mut quality_metrics = lock_review_quality_metrics(review_state)?;
    *native_state = Some(Arc::new(imported.native));
    *native_review = book;
    *quality_metrics = restored_quality;
    Ok(imported.presentation)
}

fn prepare_restored_quality(
    imported: &ImportedSession,
) -> Result<NativeQualityMetrics, PreviewCommandError> {
    let mut restored = BTreeMap::new();
    for frame in &imported.frames {
        let Some(response) = &frame.quality else {
            continue;
        };
        let frame_id =
            FrameId::new(frame.id.clone()).map_err(|_| frame_quality_identity_error())?;
        let metrics = validate_quality_response(response)?;
        if restored
            .insert((frame_id, PathBuf::from(&frame.path)), metrics)
            .is_some()
        {
            return Err(frame_quality_result_error());
        }
    }
    Ok(restored)
}

fn prepare_review_book(
    imported: &ImportedSession,
) -> Result<Option<ReviewBook>, PreviewCommandError> {
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
    Ok(book)
}

#[cfg(test)]
fn install_review_book(
    review_state: &DesktopReviewState,
    imported: &ImportedSession,
) -> Result<(), PreviewCommandError> {
    *lock_review_state(review_state)? = prepare_review_book(imported)?;
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

fn preview_frame_selection_sync(
    review_state: &DesktopReviewState,
    request: FrameSelectionPreviewRequest,
) -> Result<FrameSelectionPlan, PreviewCommandError> {
    let native_state = lock_review_state(review_state)?;
    let native_book = native_state
        .as_ref()
        .ok_or_else(review_state_missing_error)?;
    let quality_metrics = lock_review_quality_metrics(review_state)?;
    build_frame_selection_plan(native_book, &quality_metrics, request)
}

fn build_frame_selection_plan(
    native_book: &ReviewBook,
    quality_metrics: &NativeQualityMetrics,
    request: FrameSelectionPreviewRequest,
) -> Result<FrameSelectionPlan, PreviewCommandError> {
    if request.frames.is_empty() || request.frames.len() > MAX_FRAME_SELECTION_PLAN_FRAMES {
        return Err(frame_selection_input_error());
    }

    let mut artifacts_by_id = BTreeMap::new();
    for frame in request.frames {
        let frame_id = FrameId::new(frame.frame_id).map_err(|_| frame_selection_input_error())?;
        validate_runtime_source_path(&frame.source_path)?;
        if artifacts_by_id
            .insert(frame_id, frame.source_path)
            .is_some()
        {
            return Err(frame_selection_input_error());
        }
    }
    let rules = frame_selection_rules(&request.rules)?;

    let mut frames = review_entry_buffer(artifacts_by_id.len())?;
    for frame_id in native_book.processing_order() {
        let Some(artifact_path) = artifacts_by_id.remove(frame_id) else {
            continue;
        };
        let metrics = *quality_metrics
            .get(&(frame_id.clone(), artifact_path))
            .ok_or_else(frame_selection_evidence_missing_error)?;
        let label = native_book
            .label(frame_id)
            .ok_or_else(frame_selection_state_error)?;
        frames.push(
            FrameSpec::new(frame_id.clone(), label, metrics)
                .map_err(|_| frame_selection_state_error())?,
        );
    }
    if !artifacts_by_id.is_empty() || frames.is_empty() {
        return Err(frame_selection_input_error());
    }

    let preview_book = ReviewBook::new(frames, 1).map_err(|_| frame_selection_state_error())?;
    FrameSelectionPlan::build(&preview_book, &rules).map_err(|_| frame_selection_input_error())
}

fn apply_frame_selection_sync(
    review_state: &DesktopReviewState,
    request: FrameSelectionApplyRequest,
) -> Result<ReviewDecisionUpdate, PreviewCommandError> {
    if !is_canonical_sha256(&request.plan_sha256) {
        return Err(frame_selection_input_error());
    }
    let mut native_state = lock_review_state(review_state)?;
    let book = native_state
        .as_mut()
        .ok_or_else(review_state_missing_error)?;
    let quality_metrics = lock_review_quality_metrics(review_state)?;
    let plan = build_frame_selection_plan(
        book,
        &quality_metrics,
        FrameSelectionPreviewRequest {
            frames: request.frames,
            rules: request.rules,
        },
    )?;
    if plan.plan_sha256() != request.plan_sha256 {
        return Err(frame_selection_stale_error());
    }

    let mut requested = review_entry_buffer(plan.frames().len())?;
    for result in plan.frames() {
        // Explicit decisions always win. Applying an automatic plan only fills
        // undecided rows, so a user can safely confirm a batch after reviewing
        // individual exceptions.
        if book.decision(result.frame_id()).is_some() {
            continue;
        }
        let decision = match result.proposal() {
            FrameSelectionProposal::Retain => {
                ManualDecision::accept(None).map_err(|_| review_decision_failed_error())?
            }
            FrameSelectionProposal::Reject => {
                ManualDecision::reject(ManualRejectionReason::QualityRules, None)
                    .map_err(|_| review_decision_failed_error())?
            }
        };
        requested.push(DecisionChange::set(result.frame_id().clone(), decision));
    }
    if requested.is_empty() {
        return Ok(review_decision_update(book, Vec::new()));
    }
    let preview = book
        .preview_changes(&requested)
        .map_err(|_| review_decision_failed_error())?;
    let mut changes = review_entry_buffer(preview.changes().len())?;
    changes.extend(
        preview
            .changes()
            .iter()
            .map(review_decision_entry_from_delta),
    );
    book.apply_preview(preview)
        .map_err(|_| review_decision_failed_error())?;
    Ok(review_decision_update(book, changes))
}

fn frame_selection_rules(
    rules: &[FrameSelectionRuleWire],
) -> Result<Vec<FrameSelectionRule>, PreviewCommandError> {
    let mut output = review_entry_buffer(rules.len())?;
    for rule in rules {
        let metric = match rule.metric {
            FrameSelectionMetricWire::Background => FrameSelectionMetric::Background,
            FrameSelectionMetricWire::Noise => FrameSelectionMetric::Noise,
            FrameSelectionMetricWire::SignalToNoise => FrameSelectionMetric::SignalToNoise,
            FrameSelectionMetricWire::DetectedStars => FrameSelectionMetric::DetectedStars,
            FrameSelectionMetricWire::UsableStars => FrameSelectionMetric::UsableStars,
            FrameSelectionMetricWire::FwhmPixels => FrameSelectionMetric::FwhmMajorPixels,
            FrameSelectionMetricWire::Eccentricity => FrameSelectionMetric::Eccentricity,
        };
        let comparator = match rule.comparator {
            FrameSelectionComparatorWire::LessThan => FrameSelectionComparator::LessThan,
            FrameSelectionComparatorWire::GreaterThan => FrameSelectionComparator::GreaterThan,
        };
        let missing_policy = match rule.missing_policy {
            MissingMetricPolicyWire::Retain => MissingMetricPolicy::Retain,
            MissingMetricPolicyWire::Reject => MissingMetricPolicy::Reject,
        };
        let validated = match rule.threshold {
            FrameSelectionThresholdWire::Scalar(value) => {
                FrameSelectionRule::scalar(metric, comparator, value, missing_policy)
            }
            FrameSelectionThresholdWire::Count(value) => {
                FrameSelectionRule::count(metric, comparator, value, missing_policy)
            }
        }
        .map_err(|_| frame_selection_input_error())?;
        output.push(validated);
    }
    Ok(output)
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

fn lock_review_quality_metrics(
    state: &DesktopReviewState,
) -> Result<MutexGuard<'_, NativeQualityMetrics>, PreviewCommandError> {
    state.quality_metrics.lock().map_err(|_| {
        PreviewCommandError::new(
            "review_quality_state_unavailable",
            "Native quality evidence is unavailable after an internal synchronization failure.",
        )
    })
}

fn ensure_review_frame_exists(
    state: &DesktopReviewState,
    frame_id: &FrameId,
) -> Result<(), PreviewCommandError> {
    let native_review = lock_review_state(state)?;
    let book = native_review
        .as_ref()
        .ok_or_else(review_state_missing_error)?;
    book.metrics(frame_id)
        .map(|_| ())
        .ok_or_else(frame_quality_identity_error)
}

fn imported_quality_source(
    state: &DesktopSessionState,
    frame_id: &FrameId,
    artifact_path: &Path,
) -> Result<Option<(u64, String)>, PreviewCommandError> {
    let native_session = lock_session_state(state)?;
    let session = native_session
        .as_ref()
        .ok_or_else(session_state_missing_error)?;
    for file in session.manifest.files() {
        let fingerprint = file.fingerprint();
        let candidate = FrameId::derive(
            file.relative_path(),
            fingerprint.byte_length(),
            fingerprint.sha256(),
        )
        .map_err(|_| frame_quality_identity_error())?;
        if &candidate != frame_id {
            continue;
        }
        let raw_path = join_portable_path(&session.root, file.relative_path());
        return Ok((raw_path == artifact_path)
            .then(|| (fingerprint.byte_length(), fingerprint.sha256().to_owned())));
    }
    Err(frame_quality_identity_error())
}

fn record_frame_quality(
    state: &DesktopReviewState,
    frame_id: FrameId,
    artifact_path: PathBuf,
    response: &FrameQualityResponse,
) -> Result<(), PreviewCommandError> {
    // Recheck after the worker completes so an import that replaced the review
    // book cannot receive a stale result from the preceding session.
    ensure_review_frame_exists(state, &frame_id)?;
    let metrics = validate_quality_response(response)?;
    lock_review_quality_metrics(state)?.insert((frame_id, artifact_path), metrics);
    Ok(())
}

fn lock_session_state(
    state: &DesktopSessionState,
) -> Result<MutexGuard<'_, Option<Arc<ImportedNativeSession>>>, PreviewCommandError> {
    state.session.lock().map_err(|_| {
        PreviewCommandError::new(
            "session_state_unavailable",
            "The native session is unavailable after an internal synchronization failure.",
        )
    })
}

fn lock_calibration_execution(
    state: &DesktopCalibrationExecutionState,
) -> Result<MutexGuard<'_, Option<CancellationToken>>, PreviewCommandError> {
    state.cancellation.lock().map_err(|_| {
        PreviewCommandError::new(
            "calibration_execution_state_unavailable",
            "Calibration execution is unavailable after an internal synchronization failure.",
        )
    })
}

fn begin_calibration_execution(
    state: &DesktopCalibrationExecutionState,
) -> Result<CancellationToken, PreviewCommandError> {
    let mut active = lock_calibration_execution(state)?;
    if active.is_some() {
        return Err(PreviewCommandError::new(
            "calibration_execution_busy",
            "A calibration task is already running.",
        ));
    }
    let cancellation = CancellationToken::new();
    *active = Some(cancellation.clone());
    Ok(cancellation)
}

fn finish_calibration_execution(
    state: &DesktopCalibrationExecutionState,
) -> Result<(), PreviewCommandError> {
    *lock_calibration_execution(state)? = None;
    Ok(())
}

const fn session_state_missing_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "session_state_missing",
        "Import a FITS session before planning calibration masters.",
    )
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

const fn frame_selection_input_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "frame_selection_input_invalid",
        "The automatic-selection preview contains invalid rules, metrics, or frame identities.",
    )
}

const fn frame_selection_state_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "frame_selection_state_invalid",
        "The native review state cannot produce a consistent automatic-selection preview.",
    )
}

const fn frame_selection_evidence_missing_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "frame_selection_evidence_missing",
        "Measure native quality evidence for every selected artifact before previewing rules.",
    )
}

const fn frame_selection_stale_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "frame_selection_plan_stale",
        "Quality evidence or rules changed after this automatic-selection preview.",
    )
}

fn is_canonical_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

const fn frame_quality_identity_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "frame_quality_identity_invalid",
        "Quality evidence must target a frame identity in the current native review session.",
    )
}

const fn frame_quality_result_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "frame_quality_result_invalid",
        "The native quality result could not be represented as validated review metrics.",
    )
}

const fn frame_quality_cache_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "frame_quality_cache_failed",
        "Validated quality evidence could not be published to the verified cache.",
    )
}

fn quality_cache_root(app: &tauri::AppHandle) -> Result<PathBuf, PreviewCommandError> {
    app.path()
        .app_cache_dir()
        .map(|path| path.join("quality-evidence-v1"))
        .map_err(|_| frame_quality_cache_error())
}

fn preview_master_plan_sync(
    session: &ImportedNativeSession,
    request: MasterPlanPreviewRequest,
) -> Result<MasterPlanPreviewResponse, PreviewCommandError> {
    if !session.root.is_absolute() {
        return Err(PreviewCommandError::new(
            "session_state_invalid",
            "The native session root is not absolute.",
        ));
    }
    let plan = build_master_plan(&session.manifest, request)?;
    master_plan_preview(
        &session.manifest,
        &plan,
        request.maximum_light_dark_temperature_delta_c,
    )
}

fn build_master_plan(
    manifest: &SessionManifest,
    request: MasterPlanPreviewRequest,
) -> Result<MasterPlan, PreviewCommandError> {
    let policy = match request.flat_pedestal_policy {
        FlatPedestalPolicyWire::RequireMatchedDark => FlatPedestalPolicy::RequireMatchedDark,
        FlatPedestalPolicyWire::RequireBias => FlatPedestalPolicy::RequireBias,
        FlatPedestalPolicyWire::PreferMatchedDarkThenBias => {
            FlatPedestalPolicy::PreferMatchedDarkThenBias
        }
    };
    let options = MasterPlanOptions::new(
        policy,
        request.maximum_exposure_delta_seconds,
        request.maximum_temperature_delta_c,
    )
    .map_err(|_| master_plan_options_error())?;
    MasterPlan::from_manifest(manifest, options).map_err(|_| master_plan_generation_error())
}

fn execute_master_plan_sync<F>(
    session: &ImportedNativeSession,
    request: MasterPlanExecutionCommandRequest,
    cancellation: &CancellationToken,
    mut progress: F,
) -> Result<MasterPlanExecutionResponse, PreviewCommandError>
where
    F: FnMut(MasterExecutionProgress),
{
    if !session.root.is_absolute() {
        return Err(PreviewCommandError::new(
            "session_state_invalid",
            "The native session root is not absolute.",
        ));
    }
    // Reject paths that cannot cross the IPC boundary before any scientific
    // product is written. Generated filenames are ASCII-only, so this also
    // guarantees every successful output path can be returned to the UI.
    request.output_directory.to_str().ok_or_else(|| {
        PreviewCommandError::new(
            "master_output_path_not_unicode",
            "The selected master output directory cannot be represented as Unicode.",
        )
    })?;
    let tile_width = request.tile_width;
    let tile_height = request.tile_height;
    let memory_limit = usize::try_from(request.memory_limit_bytes)
        .map_err(|_| master_execution_configuration_error())?;
    let memory =
        MemoryBudget::new(memory_limit).map_err(|_| master_execution_configuration_error())?;
    let normalization = FlatNormalizationParameters::new(
        request.minimum_flat_normalization_samples,
        request.minimum_positive_flat_median,
    )
    .map_err(|_| master_execution_configuration_error())?;
    let plan = build_master_plan(&session.manifest, request.planning)?;
    let manifest_sha256 = session
        .manifest
        .canonical_sha256()
        .map_err(|_| master_plan_generation_error())?;
    let plan_sha256 = plan
        .canonical_sha256()
        .map_err(|_| master_plan_generation_error())?;
    if manifest_sha256 != request.expected_manifest_sha256
        || plan_sha256 != request.expected_plan_sha256
    {
        return Err(master_plan_stale_error());
    }
    let execution_request = MasterPlanExecutionRequest::new_shared(
        session.root.clone(),
        request.output_directory,
        Arc::clone(&session.manifest),
        plan,
        normalization,
    )
    .and_then(|request| request.with_tile_shape(tile_width, tile_height))
    .map_err(master_execution_error)?;
    let result = run_master_plan(&execution_request, cancellation, &memory, |event| {
        let stage = event.stage();
        progress(MasterExecutionProgress {
            product_index: event.product_index(),
            product_count: event.product_count(),
            group_id: event.group_id().to_owned(),
            kind: master_product_kind_name(event.kind()),
            sequence: stage.sequence(),
            stage: stage.stage().as_str().to_owned(),
            state: progress_state_name(stage.state()),
            completed_units: stage.completed_units(),
            total_units: stage.total_units(),
            code: stage.code().map(str::to_owned),
        });
    })
    .map_err(master_execution_error)?;
    let mut products = Vec::new();
    products
        .try_reserve_exact(result.products().len())
        .map_err(|_| master_plan_allocation_error())?;
    for product in result.products() {
        let output_path = product.output().to_str().ok_or_else(|| {
            PreviewCommandError::new(
                "master_output_path_not_unicode",
                "A generated master path cannot be represented as Unicode.",
            )
        })?;
        let statistics = product.statistics();
        let write = product.write_summary();
        products.push(ExecutedMasterProduct {
            group_id: product.group_id().to_owned(),
            kind: master_product_kind_name(product.kind()),
            output_path: output_path.to_owned(),
            total_samples: statistics.total_samples(),
            usable_samples: statistics.usable_samples(),
            masked_samples: statistics.masked_samples(),
            non_finite_samples: statistics.non_finite_samples(),
            minimum: statistics.minimum(),
            maximum: statistics.maximum(),
            mean: statistics.mean(),
            population_standard_deviation: statistics.population_standard_deviation(),
            samples_written: write.samples_written(),
            substituted_samples: write.substituted_samples(),
            bytes_written: write.bytes_written(),
            normalization: product.normalization(),
        });
    }
    Ok(MasterPlanExecutionResponse {
        manifest_sha256: result.manifest_sha256().to_owned(),
        plan_sha256: result.plan_sha256().to_owned(),
        memory_limit_bytes: memory.limit(),
        peak_reserved_bytes: memory.peak(),
        products,
    })
}

fn execute_light_plan_sync<F>(
    session: &ImportedNativeSession,
    request: LightPlanExecutionCommandRequest,
    cancellation: &CancellationToken,
    mut progress: F,
) -> Result<LightPlanExecutionResponse, PreviewCommandError>
where
    F: FnMut(LightExecutionProgress),
{
    if !session.root.is_absolute() {
        return Err(PreviewCommandError::new(
            "session_state_invalid",
            "The native session root is not absolute.",
        ));
    }
    request.master_directory.to_str().ok_or_else(|| {
        PreviewCommandError::new(
            "light_master_path_not_unicode",
            "The selected master directory cannot be represented as Unicode.",
        )
    })?;
    request.output_directory.to_str().ok_or_else(|| {
        PreviewCommandError::new(
            "light_output_path_not_unicode",
            "The selected Light output directory cannot be represented as Unicode.",
        )
    })?;
    let memory_limit = usize::try_from(request.memory_limit_bytes)
        .map_err(|_| light_execution_configuration_error())?;
    let memory =
        MemoryBudget::new(memory_limit).map_err(|_| light_execution_configuration_error())?;
    let calibration = CalibrationParameters::new(request.minimum_absolute_flat)
        .map_err(|_| light_execution_configuration_error())?;
    let master_plan = build_master_plan(&session.manifest, request.planning)?;
    let light_plan = build_light_plan(
        &session.manifest,
        &master_plan,
        request.planning.maximum_light_dark_temperature_delta_c,
    )?;
    let manifest_sha256 = session
        .manifest
        .canonical_sha256()
        .map_err(|_| light_plan_generation_error())?;
    let master_plan_sha256 = master_plan
        .canonical_sha256()
        .map_err(|_| light_plan_generation_error())?;
    let light_plan_sha256 = light_plan
        .canonical_sha256()
        .map_err(|_| light_plan_generation_error())?;
    if manifest_sha256 != request.expected_manifest_sha256
        || master_plan_sha256 != request.expected_master_plan_sha256
        || light_plan_sha256 != request.expected_light_plan_sha256
    {
        return Err(light_plan_stale_error());
    }
    let should_demosaic = light_plan_has_only_standard_cfa(&session.manifest, &light_plan);
    let execution_request = LightPlanExecutionRequest::new_shared(
        session.root.clone(),
        request.master_directory,
        request.output_directory,
        Arc::clone(&session.manifest),
        master_plan,
        light_plan,
        calibration,
    )
    .and_then(|execution| execution.with_tile_shape(request.tile_width, request.tile_height))
    .map_err(light_execution_error)?;
    match request.output_mode {
        LightOutputMode::Integrated => {
            let result = run_light_plan(&execution_request, cancellation, &memory, |event| {
                let stage = event.stage();
                progress(LightExecutionProgress {
                    product_index: event.product_index(),
                    product_count: event.product_count(),
                    group_id: event.group_id().to_owned(),
                    sequence: stage.sequence(),
                    stage: stage.stage().as_str().to_owned(),
                    state: progress_state_name(stage.state()),
                    completed_units: stage.completed_units(),
                    total_units: stage.total_units(),
                    code: stage.code().map(str::to_owned),
                    source_index: None,
                    source_count: None,
                });
            })
            .map_err(light_execution_error)?;
            let mut products = Vec::new();
            products
                .try_reserve_exact(result.products().len())
                .map_err(|_| light_execution_allocation_error())?;
            for product in result.products() {
                let output_path = light_output_path(product.output())?;
                let statistics = product.statistics();
                let write = product.write_summary();
                products.push(ExecutedLightProduct {
                    group_id: product.group_id().to_owned(),
                    dark_group_id: product.dark_group_id().to_owned(),
                    flat_group_id: product.flat_group_id().to_owned(),
                    output_path,
                    total_samples: statistics.total_samples(),
                    usable_samples: statistics.usable_samples(),
                    masked_samples: statistics.masked_samples(),
                    non_finite_samples: statistics.non_finite_samples(),
                    minimum: statistics.minimum(),
                    maximum: statistics.maximum(),
                    mean: statistics.mean(),
                    population_standard_deviation: statistics.population_standard_deviation(),
                    samples_written: write.samples_written(),
                    substituted_samples: write.substituted_samples(),
                    bytes_written: write.bytes_written(),
                    tiles_processed: product.tiles_processed(),
                    tiles_reused: product.tiles_reused(),
                });
            }
            Ok(LightPlanExecutionResponse {
                manifest_sha256: result.manifest_sha256().to_owned(),
                master_plan_sha256: result.master_plan_sha256().to_owned(),
                light_plan_sha256: result.light_plan_sha256().to_owned(),
                memory_limit_bytes: memory.limit(),
                peak_reserved_bytes: memory.peak(),
                output_mode: request.output_mode.as_str(),
                products,
                calibrated_frames: Vec::new(),
            })
        }
        LightOutputMode::CalibratedFrames => {
            let result =
                run_calibrated_light_plan(&execution_request, cancellation, &memory, |event| {
                    let stage = event.stage();
                    progress(LightExecutionProgress {
                        product_index: event.product_index(),
                        product_count: event.product_count(),
                        group_id: event.group_id().to_owned(),
                        sequence: stage.sequence(),
                        stage: stage.stage().as_str().to_owned(),
                        state: progress_state_name(stage.state()),
                        completed_units: stage.completed_units(),
                        total_units: stage.total_units(),
                        code: stage.code().map(str::to_owned),
                        source_index: Some(event.source_index()),
                        source_count: Some(event.source_count()),
                    });
                })
                .map_err(light_execution_error)?;
            // A reviewed all-CFA Light set is promoted to planar RGB as a
            // second all-or-nothing transaction. Mono and mixed sessions keep
            // their calibrated scalar products until a mixed-plan contract is
            // introduced explicitly.
            let demosaiced = should_demosaic
                .then(|| {
                    run_demosaiced_light_plan(
                        &execution_request,
                        &result,
                        cancellation,
                        &memory,
                        |event| {
                            let stage = event.stage();
                            progress(LightExecutionProgress {
                                product_index: event.product_index(),
                                product_count: event.product_count(),
                                group_id: event.group_id().to_owned(),
                                sequence: stage.sequence(),
                                stage: stage.stage().as_str().to_owned(),
                                state: progress_state_name(stage.state()),
                                completed_units: stage.completed_units(),
                                total_units: stage.total_units(),
                                code: stage.code().map(str::to_owned),
                                source_index: Some(event.source_index()),
                                source_count: Some(event.source_count()),
                            });
                        },
                    )
                })
                .transpose()
                .map_err(light_execution_error)?;
            let mut calibrated_frames = Vec::new();
            calibrated_frames
                .try_reserve_exact(result.frames().len())
                .map_err(|_| light_execution_allocation_error())?;
            for frame in result.frames() {
                let (source_frame_id, source_label) = calibrated_source_identity(
                    &session.manifest,
                    frame.group_id(),
                    frame.source_index(),
                    frame.source_sha256(),
                )?;
                let statistics = frame.statistics();
                let write = frame.write_summary();
                let rgb_output_path = demosaiced
                    .as_ref()
                    .and_then(|rgb| {
                        rgb.frames().iter().find(|candidate| {
                            candidate.group_id() == frame.group_id()
                                && candidate.source_index() == frame.source_index()
                        })
                    })
                    .map(|rgb| light_output_path(rgb.output()))
                    .transpose()?;
                calibrated_frames.push(ExecutedCalibratedLightFrame {
                    group_id: frame.group_id().to_owned(),
                    source_index: frame.source_index(),
                    source_frame_id,
                    source_label,
                    source_sha256: frame.source_sha256().to_owned(),
                    output_path: light_output_path(frame.output())?,
                    rgb_output_path,
                    total_samples: statistics.total_samples(),
                    usable_samples: statistics.usable_samples(),
                    masked_samples: statistics.masked_samples(),
                    non_finite_samples: statistics.non_finite_samples(),
                    minimum: statistics.minimum(),
                    maximum: statistics.maximum(),
                    mean: statistics.mean(),
                    population_standard_deviation: statistics.population_standard_deviation(),
                    samples_written: write.samples_written(),
                    substituted_samples: write.substituted_samples(),
                    bytes_written: write.bytes_written(),
                    tiles_processed: frame.tiles_processed(),
                    tiles_reused: frame.tiles_reused(),
                });
            }
            Ok(LightPlanExecutionResponse {
                manifest_sha256: result.manifest_sha256().to_owned(),
                master_plan_sha256: result.master_plan_sha256().to_owned(),
                light_plan_sha256: result.light_plan_sha256().to_owned(),
                memory_limit_bytes: memory.limit(),
                peak_reserved_bytes: memory.peak(),
                output_mode: request.output_mode.as_str(),
                products: Vec::new(),
                calibrated_frames,
            })
        }
    }
}

fn light_plan_has_only_standard_cfa(
    manifest: &SessionManifest,
    light_plan: &LightCalibrationPlan,
) -> bool {
    !light_plan.products().is_empty()
        && light_plan.products().iter().all(|product| {
            manifest
                .groups()
                .iter()
                .find(|group| group.id() == product.source_group_id())
                .and_then(|group| group.key().bayer_pattern())
                .is_some_and(|pattern| !matches!(pattern, BayerPattern::Other(_)))
        })
}

fn light_output_path(path: &Path) -> Result<String, PreviewCommandError> {
    path.to_str().map(str::to_owned).ok_or_else(|| {
        PreviewCommandError::new(
            "light_output_path_not_unicode",
            "A generated Light path cannot be represented as Unicode.",
        )
    })
}

/// Resolves a published calibrated artifact back to the stable review identity
/// of its exact raw source. The browser can therefore show calibrated pixels
/// without inventing a second identity or bypassing the native decision book.
fn calibrated_source_identity(
    manifest: &SessionManifest,
    group_id: &str,
    source_index: usize,
    expected_sha256: &str,
) -> Result<(String, String), PreviewCommandError> {
    let group = manifest
        .groups()
        .iter()
        .find(|group| group.id() == group_id)
        .ok_or_else(calibrated_source_identity_error)?;
    let relative_path = group
        .files()
        .get(source_index)
        .ok_or_else(calibrated_source_identity_error)?;
    let file = manifest
        .files()
        .iter()
        .find(|file| file.relative_path() == relative_path)
        .ok_or_else(calibrated_source_identity_error)?;
    if file.fingerprint().sha256() != expected_sha256 {
        return Err(calibrated_source_identity_error());
    }
    let frame_id = FrameId::derive(
        file.relative_path(),
        file.fingerprint().byte_length(),
        file.fingerprint().sha256(),
    )
    .map_err(|_| calibrated_source_identity_error())?;
    let label = Path::new(relative_path)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(relative_path)
        .to_owned();
    Ok((frame_id.as_str().to_owned(), label))
}

const fn calibrated_source_identity_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "calibrated_source_identity_invalid",
        "A calibrated Light output cannot be bound to its exact imported source.",
    )
}

fn build_light_plan(
    manifest: &SessionManifest,
    master_plan: &MasterPlan,
    maximum_dark_temperature_delta_c: f64,
) -> Result<LightCalibrationPlan, PreviewCommandError> {
    let options = LightCalibrationPlanOptions::new(maximum_dark_temperature_delta_c)
        .map_err(|_| light_plan_options_error())?;
    LightCalibrationPlan::from_manifest_and_master_plan(manifest, master_plan, options)
        .map_err(|_| light_plan_generation_error())
}

fn master_plan_preview(
    manifest: &SessionManifest,
    plan: &MasterPlan,
    maximum_light_dark_temperature_delta_c: f64,
) -> Result<MasterPlanPreviewResponse, PreviewCommandError> {
    let mut products = Vec::new();
    products
        .try_reserve_exact(plan.products().len())
        .map_err(|_| master_plan_allocation_error())?;
    for product in plan.products() {
        let group = manifest
            .groups()
            .iter()
            .find(|group| group.id() == product.source_group_id())
            .ok_or_else(master_plan_generation_error)?;
        products.push(master_product_preview(group, product)?);
    }
    let light_plan = if plan.is_ready() {
        let light_plan = build_light_plan(manifest, plan, maximum_light_dark_temperature_delta_c)?;
        Some(light_calibration_plan_preview(&light_plan)?)
    } else {
        None
    };
    Ok(MasterPlanPreviewResponse {
        schema_version: plan.schema_version(),
        manifest_sha256: manifest
            .canonical_sha256()
            .map_err(|_| master_plan_generation_error())?,
        plan_sha256: plan
            .canonical_sha256()
            .map_err(|_| master_plan_generation_error())?,
        ready: plan.is_ready(),
        products,
        light_plan,
    })
}

fn light_calibration_plan_preview(
    plan: &LightCalibrationPlan,
) -> Result<LightCalibrationPlanPreview, PreviewCommandError> {
    let mut products = Vec::new();
    products
        .try_reserve_exact(plan.products().len())
        .map_err(|_| master_plan_allocation_error())?;
    for product in plan.products() {
        let mut candidates = Vec::new();
        candidates
            .try_reserve_exact(product.candidates().len())
            .map_err(|_| master_plan_allocation_error())?;
        for candidate in product.candidates() {
            candidates.push(light_master_candidate_preview(candidate)?);
        }
        products.push(LightCalibrationProductPreview {
            group_id: product.source_group_id().to_owned(),
            dark: light_master_association_preview(product.dark())?,
            flat: light_master_association_preview(product.flat())?,
            candidates,
        });
    }
    Ok(LightCalibrationPlanPreview {
        schema_version: plan.schema_version(),
        plan_sha256: plan
            .canonical_sha256()
            .map_err(|_| light_plan_generation_error())?,
        ready: plan.is_ready(),
        products,
    })
}

fn light_master_association_preview(
    association: &LightMasterAssociation,
) -> Result<LightMasterAssociationPreview, PreviewCommandError> {
    match association {
        LightMasterAssociation::Matched {
            group_id,
            kind,
            temperature_basis,
            temperature_delta_c,
        } => Ok(LightMasterAssociationPreview {
            kind: light_master_kind_name(*kind),
            status: "matched",
            selected_group_id: Some(group_id.clone()),
            temperature_basis: temperature_basis.map(temperature_basis_name),
            temperature_delta_celsius: *temperature_delta_c,
            blocking_reason: None,
            ambiguous_group_ids: Vec::new(),
            missing_fields: Vec::new(),
        }),
        LightMasterAssociation::Unresolved {
            kind,
            blocking_reason,
        } => {
            let mut preview = LightMasterAssociationPreview {
                kind: light_master_kind_name(*kind),
                status: "unresolved",
                selected_group_id: None,
                temperature_basis: None,
                temperature_delta_celsius: None,
                blocking_reason: Some(light_master_blocking_reason_name(blocking_reason)),
                ambiguous_group_ids: Vec::new(),
                missing_fields: Vec::new(),
            };
            match blocking_reason {
                LightMasterBlockingReason::MissingLightMetadata { fields } => {
                    preview
                        .missing_fields
                        .try_reserve_exact(fields.len())
                        .map_err(|_| master_plan_allocation_error())?;
                    preview
                        .missing_fields
                        .extend(fields.iter().copied().map(light_master_match_field_name));
                }
                LightMasterBlockingReason::AmbiguousCandidates { group_ids } => {
                    preview
                        .ambiguous_group_ids
                        .try_reserve_exact(group_ids.len())
                        .map_err(|_| master_plan_allocation_error())?;
                    preview
                        .ambiguous_group_ids
                        .extend(group_ids.iter().cloned());
                }
                LightMasterBlockingReason::NoCompatibleCandidate => {}
            }
            Ok(preview)
        }
    }
}

fn light_master_candidate_preview(
    candidate: &aether_session::LightMasterCandidateEvaluation,
) -> Result<LightMasterCandidatePreview, PreviewCommandError> {
    match candidate.compatibility() {
        LightMasterCandidateCompatibility::Compatible {
            temperature_basis,
            temperature_delta_c,
        } => Ok(LightMasterCandidatePreview {
            group_id: candidate.group_id().to_owned(),
            kind: light_master_kind_name(candidate.kind()),
            status: "compatible",
            temperature_basis: temperature_basis.map(temperature_basis_name),
            temperature_delta_celsius: *temperature_delta_c,
            mismatches: Vec::new(),
        }),
        LightMasterCandidateCompatibility::Rejected { mismatches } => {
            let mut preview = Vec::new();
            preview
                .try_reserve_exact(mismatches.len())
                .map_err(|_| master_plan_allocation_error())?;
            preview.extend(
                mismatches
                    .iter()
                    .map(|mismatch| LightMasterMismatchPreview {
                        field: light_master_match_field_name(mismatch.field()),
                        reason: light_master_mismatch_reason_name(mismatch.reason()),
                    }),
            );
            Ok(LightMasterCandidatePreview {
                group_id: candidate.group_id().to_owned(),
                kind: light_master_kind_name(candidate.kind()),
                status: "rejected",
                temperature_basis: None,
                temperature_delta_celsius: None,
                mismatches: preview,
            })
        }
    }
}

fn master_product_preview(
    group: &ManifestGroup,
    product: &aether_session::MasterProductPlan,
) -> Result<MasterProductPreview, PreviewCommandError> {
    let key = group.key();
    let mut candidates = Vec::new();
    candidates
        .try_reserve_exact(product.pedestal_candidates().len())
        .map_err(|_| master_plan_allocation_error())?;
    for candidate in product.pedestal_candidates() {
        candidates.push(master_candidate_preview(candidate)?);
    }
    Ok(MasterProductPreview {
        group_id: group.id().to_owned(),
        kind: master_product_kind_name(product.kind()),
        frame_count: group.files().len(),
        camera: key
            .camera()
            .map(|camera| camera.canonical_name().to_owned()),
        axes: key.axes().to_vec(),
        exposure_seconds: key.exposure_seconds(),
        sensor_temperature_celsius: key.sensor_temperature_c(),
        set_temperature_celsius: key.set_temperature_c(),
        gain: key.gain(),
        offset: key.offset(),
        binning: key.binning().map(|binning| MasterBinningPreview {
            x: binning.x,
            y: binning.y,
        }),
        filter: key.filter().map(str::to_owned),
        bayer_pattern: key.bayer_pattern().map(bayer_pattern_name),
        pedestal: master_pedestal_preview(product.flat_pedestal()),
        candidates,
    })
}

fn master_candidate_preview(
    candidate: &aether_session::PedestalCandidateEvaluation,
) -> Result<MasterPedestalCandidatePreview, PreviewCommandError> {
    let source_kind = match candidate.source_kind() {
        PedestalSourceKind::Dark => "dark",
        PedestalSourceKind::Bias => "bias",
    };
    match candidate.compatibility() {
        PedestalCandidateCompatibility::Compatible {
            exposure_delta_seconds,
            temperature_basis,
            temperature_delta_c,
        } => Ok(MasterPedestalCandidatePreview {
            group_id: candidate.group_id().to_owned(),
            source_kind,
            status: "compatible",
            exposure_delta_seconds: *exposure_delta_seconds,
            temperature_basis: Some(temperature_basis_name(*temperature_basis)),
            temperature_delta_celsius: Some(*temperature_delta_c),
            mismatches: Vec::new(),
        }),
        PedestalCandidateCompatibility::Rejected { mismatches } => {
            let mut preview = Vec::new();
            preview
                .try_reserve_exact(mismatches.len())
                .map_err(|_| master_plan_allocation_error())?;
            preview.extend(
                mismatches
                    .iter()
                    .map(|mismatch| MasterPedestalMismatchPreview {
                        field: pedestal_match_field_name(mismatch.field()),
                        reason: pedestal_mismatch_reason_name(mismatch.reason()),
                    }),
            );
            Ok(MasterPedestalCandidatePreview {
                group_id: candidate.group_id().to_owned(),
                source_kind,
                status: "rejected",
                exposure_delta_seconds: None,
                temperature_basis: None,
                temperature_delta_celsius: None,
                mismatches: preview,
            })
        }
    }
}

fn master_pedestal_preview(association: Option<&FlatPedestalAssociation>) -> MasterPedestalPreview {
    match association {
        None => MasterPedestalPreview {
            status: "not_applicable",
            selected_group_id: None,
            exposure_delta_seconds: None,
            temperature_basis: None,
            temperature_delta_celsius: None,
            blocking_reason: None,
            ambiguous_group_ids: Vec::new(),
        },
        Some(FlatPedestalAssociation::MatchedDark {
            group_id,
            exposure_delta_seconds,
            temperature_basis,
            temperature_delta_c,
        }) => MasterPedestalPreview {
            status: "matched_dark",
            selected_group_id: Some(group_id.clone()),
            exposure_delta_seconds: Some(*exposure_delta_seconds),
            temperature_basis: Some(temperature_basis_name(*temperature_basis)),
            temperature_delta_celsius: Some(*temperature_delta_c),
            blocking_reason: None,
            ambiguous_group_ids: Vec::new(),
        },
        Some(FlatPedestalAssociation::Bias {
            group_id,
            temperature_basis,
            temperature_delta_c,
        }) => MasterPedestalPreview {
            status: "bias",
            selected_group_id: Some(group_id.clone()),
            exposure_delta_seconds: None,
            temperature_basis: Some(temperature_basis_name(*temperature_basis)),
            temperature_delta_celsius: Some(*temperature_delta_c),
            blocking_reason: None,
            ambiguous_group_ids: Vec::new(),
        },
        Some(FlatPedestalAssociation::Unresolved { blocking_reason }) => {
            let ambiguous_group_ids = match blocking_reason {
                FlatPedestalBlockingReason::AmbiguousDark { group_ids }
                | FlatPedestalBlockingReason::AmbiguousBias { group_ids } => group_ids.clone(),
                FlatPedestalBlockingReason::MissingFlatMetadata { .. }
                | FlatPedestalBlockingReason::NoCompatibleDark
                | FlatPedestalBlockingReason::NoCompatibleBias
                | FlatPedestalBlockingReason::NoCompatibleDarkOrBias => Vec::new(),
            };
            MasterPedestalPreview {
                status: "unresolved",
                selected_group_id: None,
                exposure_delta_seconds: None,
                temperature_basis: None,
                temperature_delta_celsius: None,
                blocking_reason: Some(flat_blocking_reason_name(blocking_reason)),
                ambiguous_group_ids,
            }
        }
    }
}

const fn master_product_kind_name(kind: MasterProductKind) -> &'static str {
    match kind {
        MasterProductKind::Bias => "bias",
        MasterProductKind::Dark => "dark",
        MasterProductKind::Flat => "flat",
    }
}

const fn progress_state_name(state: ProgressState) -> &'static str {
    match state {
        ProgressState::Started => "started",
        ProgressState::Running => "running",
        ProgressState::Completed => "completed",
        ProgressState::Cancelled => "cancelled",
        ProgressState::Failed => "failed",
    }
}

fn bayer_pattern_name(pattern: &BayerPattern) -> String {
    match pattern {
        BayerPattern::Rggb => "RGGB".to_owned(),
        BayerPattern::Bggr => "BGGR".to_owned(),
        BayerPattern::Grbg => "GRBG".to_owned(),
        BayerPattern::Gbrg => "GBRG".to_owned(),
        BayerPattern::Other(name) => name.clone(),
    }
}

const fn temperature_basis_name(basis: TemperatureBasis) -> &'static str {
    match basis {
        TemperatureBasis::Sensor => "sensor",
        TemperatureBasis::SetPoint => "set_point",
    }
}

const fn pedestal_match_field_name(field: PedestalMatchField) -> &'static str {
    match field {
        PedestalMatchField::Camera => "camera",
        PedestalMatchField::Axes => "axes",
        PedestalMatchField::Exposure => "exposure",
        PedestalMatchField::SensorTemperature => "sensor_temperature",
        PedestalMatchField::SetTemperature => "set_temperature",
        PedestalMatchField::Gain => "gain",
        PedestalMatchField::Offset => "offset",
        PedestalMatchField::Binning => "binning",
        PedestalMatchField::BayerPattern => "bayer_pattern",
    }
}

const fn pedestal_mismatch_reason_name(reason: PedestalMismatchReason) -> &'static str {
    match reason {
        PedestalMismatchReason::Missing => "missing",
        PedestalMismatchReason::Different => "different",
        PedestalMismatchReason::OutsideTolerance => "outside_tolerance",
    }
}

const fn flat_blocking_reason_name(reason: &FlatPedestalBlockingReason) -> &'static str {
    match reason {
        FlatPedestalBlockingReason::MissingFlatMetadata { .. } => "missing_flat_metadata",
        FlatPedestalBlockingReason::NoCompatibleDark => "no_compatible_dark",
        FlatPedestalBlockingReason::NoCompatibleBias => "no_compatible_bias",
        FlatPedestalBlockingReason::NoCompatibleDarkOrBias => "no_compatible_dark_or_bias",
        FlatPedestalBlockingReason::AmbiguousDark { .. } => "ambiguous_dark",
        FlatPedestalBlockingReason::AmbiguousBias { .. } => "ambiguous_bias",
    }
}

const fn light_master_kind_name(kind: LightMasterKind) -> &'static str {
    match kind {
        LightMasterKind::Dark => "dark",
        LightMasterKind::Flat => "flat",
    }
}

const fn light_master_match_field_name(field: LightMasterMatchField) -> &'static str {
    match field {
        LightMasterMatchField::Camera => "camera",
        LightMasterMatchField::Axes => "axes",
        LightMasterMatchField::Exposure => "exposure",
        LightMasterMatchField::SensorTemperature => "sensor_temperature",
        LightMasterMatchField::SetTemperature => "set_temperature",
        LightMasterMatchField::Gain => "gain",
        LightMasterMatchField::Offset => "offset",
        LightMasterMatchField::Binning => "binning",
        LightMasterMatchField::Filter => "filter",
        LightMasterMatchField::BayerPattern => "bayer_pattern",
    }
}

const fn light_master_mismatch_reason_name(reason: LightMasterMismatchReason) -> &'static str {
    match reason {
        LightMasterMismatchReason::Missing => "missing",
        LightMasterMismatchReason::Different => "different",
        LightMasterMismatchReason::OutsideTolerance => "outside_tolerance",
    }
}

const fn light_master_blocking_reason_name(reason: &LightMasterBlockingReason) -> &'static str {
    match reason {
        LightMasterBlockingReason::MissingLightMetadata { .. } => "missing_light_metadata",
        LightMasterBlockingReason::NoCompatibleCandidate => "no_compatible_candidate",
        LightMasterBlockingReason::AmbiguousCandidates { .. } => "ambiguous_candidates",
    }
}

const fn master_plan_options_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "master_plan_options_invalid",
        "Master-calibration tolerances must be finite non-negative values.",
    )
}

const fn master_plan_generation_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "master_plan_generation_failed",
        "The imported manifest could not produce a coherent master plan.",
    )
}

const fn light_plan_options_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "light_plan_options_invalid",
        "Light-to-Dark temperature tolerance must be finite and non-negative.",
    )
}

const fn light_plan_generation_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "light_plan_generation_failed",
        "The reviewed master plan could not produce coherent Light associations.",
    )
}

const fn master_plan_allocation_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "master_plan_allocation_failed",
        "The master-plan preview could not reserve its bounded response buffers.",
    )
}

const fn master_execution_configuration_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "master_execution_configuration_invalid",
        "Master execution requires positive tile, memory, and flat-normalization limits.",
    )
}

const fn master_plan_stale_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "master_plan_stale",
        "The reviewed calibration plan changed before execution; review the refreshed plan first.",
    )
}

fn master_execution_error(error: MasterPlanExecutionError) -> PreviewCommandError {
    match error {
        MasterPlanExecutionError::Cancelled(_) => PreviewCommandError::new(
            "master_execution_cancelled",
            "Master execution was cancelled without publishing a partial product set.",
        ),
        MasterPlanExecutionError::DestinationExists { .. } => PreviewCommandError::new(
            "master_destination_exists",
            "A planned master already exists; existing files were not modified.",
        ),
        MasterPlanExecutionError::UnresolvedFlatPedestal { .. }
        | MasterPlanExecutionError::InvalidPedestalProduct { .. } => PreviewCommandError::new(
            "master_dependency_unresolved",
            "Every flat must have one exclusive resolved pedestal before execution.",
        ),
        MasterPlanExecutionError::SessionRootNotAbsolute
        | MasterPlanExecutionError::OutputDirectoryNotAbsolute
        | MasterPlanExecutionError::ZeroTileExtent { .. } => master_execution_configuration_error(),
        MasterPlanExecutionError::SessionRootNotDirectory
        | MasterPlanExecutionError::OutputDirectoryNotDirectory
        | MasterPlanExecutionError::InspectSessionRoot(_)
        | MasterPlanExecutionError::InspectOutputDirectory(_)
        | MasterPlanExecutionError::CreateStagingDirectory(_)
        | MasterPlanExecutionError::PublishProduct { .. }
        | MasterPlanExecutionError::SyncOutputDirectory(_)
        | MasterPlanExecutionError::RollbackPublication { .. } => PreviewCommandError::new(
            "master_execution_filesystem_failed",
            "The master transaction could not safely use or publish to the selected directories.",
        ),
        MasterPlanExecutionError::Manifest(_)
        | MasterPlanExecutionError::Plan(_)
        | MasterPlanExecutionError::ManifestDigestMismatch
        | MasterPlanExecutionError::MissingGroup { .. }
        | MasterPlanExecutionError::ProductKindMismatch { .. }
        | MasterPlanExecutionError::MissingManifestFile { .. }
        | MasterPlanExecutionError::DestinationNameCollision { .. }
        | MasterPlanExecutionError::Provenance(_) => master_plan_generation_error(),
        MasterPlanExecutionError::ProductPipeline { .. }
        | MasterPlanExecutionError::OpenGeneratedPedestal { .. }
        | MasterPlanExecutionError::FingerprintGeneratedPedestal { .. } => {
            PreviewCommandError::new(
                "master_product_failed",
                "A master product failed validation or calculation; no product set was published.",
            )
        }
        MasterPlanExecutionError::AllocationFailed => PreviewCommandError::new(
            "master_execution_allocation_failed",
            "Master execution could not reserve its bounded native state.",
        ),
    }
}

const fn light_execution_configuration_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "light_execution_configuration_invalid",
        "Light execution requires absolute directories and positive tile and memory limits.",
    )
}

const fn light_plan_stale_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "light_plan_stale",
        "The reviewed manifest, master plan, or Light plan changed before execution.",
    )
}

const fn light_execution_allocation_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "light_execution_allocation_failed",
        "Light execution could not reserve its bounded native state.",
    )
}

fn light_execution_error(error: LightPlanExecutionError) -> PreviewCommandError {
    match error {
        LightPlanExecutionError::Cancelled(_) => PreviewCommandError::new(
            "light_execution_cancelled",
            "Light execution was cancelled without publishing a partial product set.",
        ),
        LightPlanExecutionError::DestinationExists { .. } => PreviewCommandError::new(
            "light_destination_exists",
            "A planned Light product already exists; existing files were not modified.",
        ),
        LightPlanExecutionError::LightPlanNotReady
        | LightPlanExecutionError::NoLightProducts
        | LightPlanExecutionError::MissingBayerPattern { .. }
        | LightPlanExecutionError::InvalidMasterAssociation { .. }
        | LightPlanExecutionError::MasterMissing { .. }
        | LightPlanExecutionError::MasterProvenanceMismatch { .. } => PreviewCommandError::new(
            "light_dependency_unresolved",
            "Every Light requires verified Dark and Flat products from the reviewed master plan.",
        ),
        LightPlanExecutionError::SessionRootNotAbsolute
        | LightPlanExecutionError::MasterDirectoryNotAbsolute
        | LightPlanExecutionError::OutputDirectoryNotAbsolute
        | LightPlanExecutionError::ZeroTileExtent { .. } => light_execution_configuration_error(),
        LightPlanExecutionError::InspectDirectory(_)
        | LightPlanExecutionError::DirectoryNotPhysical { .. }
        | LightPlanExecutionError::CreateStagingDirectory(_)
        | LightPlanExecutionError::PublishProduct { .. }
        | LightPlanExecutionError::SyncOutputDirectory(_)
        | LightPlanExecutionError::RollbackPublication { .. } => PreviewCommandError::new(
            "light_execution_filesystem_failed",
            "The Light transaction could not safely use or publish to the selected directories.",
        ),
        LightPlanExecutionError::Manifest(_)
        | LightPlanExecutionError::MasterPlan(_)
        | LightPlanExecutionError::LightPlan(_)
        | LightPlanExecutionError::MasterPlanMismatch
        | LightPlanExecutionError::LightPlanMismatch
        | LightPlanExecutionError::CalibratedPlanMismatch
        | LightPlanExecutionError::CalibratedFrameSetMismatch
        | LightPlanExecutionError::InvalidGroup { .. }
        | LightPlanExecutionError::MissingManifestFile { .. }
        | LightPlanExecutionError::DestinationNameCollision { .. }
        | LightPlanExecutionError::Provenance(_) => light_plan_generation_error(),
        LightPlanExecutionError::OpenMaster { .. }
        | LightPlanExecutionError::FingerprintMaster { .. }
        | LightPlanExecutionError::OpenCalibrated { .. }
        | LightPlanExecutionError::CalibratedProvenanceMismatch { .. }
        | LightPlanExecutionError::FingerprintCalibrated { .. }
        | LightPlanExecutionError::SourceChanged { .. }
        | LightPlanExecutionError::ProductPipeline { .. }
        | LightPlanExecutionError::FramePipeline { .. }
        | LightPlanExecutionError::DemosaicFramePipeline { .. } => PreviewCommandError::new(
            "light_product_failed",
            "A Light product failed validation or calculation; no product set was published.",
        ),
        LightPlanExecutionError::AllocationFailed => light_execution_allocation_error(),
    }
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
        .and_then(|metrics| metrics.with_signal_to_noise(frame.signal_to_noise))
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

fn scan_session_directory_sync(root: &Path) -> Result<ImportedSessionBundle, PreviewCommandError> {
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
    let presentation = imported_session_from_report(root, root_path, &report)?;
    let manifest = Arc::new(report.into_manifest());
    Ok(ImportedSessionBundle {
        presentation,
        native: ImportedNativeSession {
            root: root.to_owned(),
            manifest,
        },
    })
}

#[cfg(test)]
fn import_session_directory_sync(root: &Path) -> Result<ImportedSession, PreviewCommandError> {
    Ok(scan_session_directory_sync(root)?.presentation)
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
        quality_evidence_restored: 0,
        quality_evidence_missing: 0,
        quality_evidence_rejected: 0,
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
        source_byte_length: fingerprint.byte_length(),
        source_sha256: fingerprint.sha256().to_owned(),
        quality: None,
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
    mut input: R,
    request: &FitsPreviewRequest,
) -> Result<Vec<u8>, PreviewCommandError> {
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
    let rgba = match request.content {
        FitsPreviewContent::Scalar { plane } => {
            let scalar = build_scalar_preview(
                &mut input,
                plane,
                request.maximum_width,
                request.maximum_height,
            )?;
            match request.palette {
                PreviewPalette::Grayscale => {
                    render_grayscale_rgba8(&scalar, transform, MissingPixelStyle::Checkerboard)
                }
                PreviewPalette::LowRejection => render_false_color_rgba8(
                    &scalar,
                    transform,
                    MissingPixelStyle::Checkerboard,
                    ScalarPalette::LowRejection,
                ),
                PreviewPalette::HighRejection => render_false_color_rgba8(
                    &scalar,
                    transform,
                    MissingPixelStyle::Checkerboard,
                    ScalarPalette::HighRejection,
                ),
            }
        }
        FitsPreviewContent::Rgb => {
            if !matches!(request.palette, PreviewPalette::Grayscale) {
                return Err(PreviewCommandError::new(
                    "preview_palette_invalid",
                    "Diagnostic palettes require one scalar FITS plane.",
                ));
            }
            let [red, green, blue] =
                build_rgb_preview(&mut input, request.maximum_width, request.maximum_height)?;
            render_rgb_rgba8(
                &red,
                &green,
                &blue,
                transform,
                MissingPixelStyle::Checkerboard,
            )
        }
    }
    .map_err(|_| {
        PreviewCommandError::new(
            "preview_mapping_failed",
            "The FITS preview could not be mapped for display.",
        )
    })?;
    encode_png(&rgba)
}

fn estimate_fits_preview_transform_from_reader<R: Read + Seek>(
    mut input: R,
    request: &FitsPreviewEstimateRequest,
) -> Result<EstimatedDisplayTransform, PreviewCommandError> {
    let estimate = match request.content {
        FitsPreviewContent::Scalar { plane } => {
            let scalar = build_scalar_preview(
                &mut input,
                plane,
                request.maximum_width,
                request.maximum_height,
            )?;
            estimate_display_transform(&scalar)
        }
        FitsPreviewContent::Rgb => {
            let [red, green, blue] =
                build_rgb_preview(&mut input, request.maximum_width, request.maximum_height)?;
            estimate_rgb_display_transform(&red, &green, &blue)
        }
    }
    .map_err(|_| {
        PreviewCommandError::new(
            "preview_stretch_failed",
            "A robust display stretch could not be estimated from this frame.",
        )
    })?;
    Ok(estimated_transform(estimate))
}

/// Reduces the three canonical planar RGB channels with identical bounds.
///
/// The reader is rewound before every channel because each independent FITS
/// decoder owns a complete header pass. This keeps memory bounded without
/// relying on the prior decoder's final cursor position.
fn build_rgb_preview<R: Read + Seek>(
    input: &mut R,
    maximum_width: usize,
    maximum_height: usize,
) -> Result<[ScalarPreview; 3], PreviewCommandError> {
    rewind_preview_input(input)?;
    let red = build_scalar_preview(&mut *input, 0, maximum_width, maximum_height)?;
    rewind_preview_input(input)?;
    let green = build_scalar_preview(&mut *input, 1, maximum_width, maximum_height)?;
    rewind_preview_input(input)?;
    let blue = build_scalar_preview(&mut *input, 2, maximum_width, maximum_height)?;
    Ok([red, green, blue])
}

fn rewind_preview_input<R: Seek>(input: &mut R) -> Result<(), PreviewCommandError> {
    input.seek(SeekFrom::Start(0)).map(|_| ()).map_err(|_| {
        PreviewCommandError::new(
            "fits_seek_failed",
            "The FITS preview source could not be rewound between RGB channels.",
        )
    })
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
        .manage(DesktopCalibrationExecutionState::default())
        .manage(DesktopReviewState::default())
        .manage(DesktopSessionState::default())
        .invoke_handler(tauri::generate_handler![
            apply_frame_selection,
            apply_review_decision,
            cancel_light_plan,
            cancel_master_plan,
            cancel_registration_plan,
            cancel_registered_stack,
            cancel_registered_stack_source_verification,
            diagnose_fits_registration,
            estimate_fits_preview_transform,
            execute_light_plan,
            execute_master_plan,
            execute_registration_plan,
            execute_registered_stack,
            import_session_directory,
            inspect_frame_quality,
            inspect_fits_statistics,
            inspect_rejection_histogram,
            inspect_registered_stack_report,
            inspect_stack_pixel,
            preview_master_plan,
            preview_frame_selection,
            preview_registration_plan,
            preview_registered_weights,
            render_fits_preview,
            sort_review_frames,
            undo_review_decision,
            verify_registered_stack_sources
        ])
        .run(tauri::generate_context!())
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File};
    use std::io::Cursor;
    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};

    use aether_core::{Dimensions, ScientificImage};
    use aether_fits::{
        FitsOutputProvenance, write_f64_primary, write_f64_primary_atomic_new_with_provenance,
    };
    use aether_metadata::{Binning, CameraModel, CanonicalMetadata, CanonicalValue, Confidence};
    use aether_runtime::{
        REGISTERED_CROP_MEAN_ALGORITHM_ID, REGISTERED_PERCENTILE_CLIPPED_MEAN_ALGORITHM_ID,
        REGISTERED_WEIGHTED_MEAN_ALGORITHM_ID,
    };
    use aether_session::{ManifestGroup, StrictGroupingKey, classify_frame, fingerprint_reader};

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

    #[test]
    fn source_fingerprint_progress_is_monotone_and_throttled() {
        let input = vec![0x5a_u8; 8 * 1_024 * 1_024 + 17];
        let expected = lowercase_hex(&Sha256::digest(&input));
        let mut progress = Vec::new();
        let result = fingerprint_registered_stack_source(
            &mut Cursor::new(input),
            &CancellationToken::new(),
            |bytes| progress.push(bytes),
        );

        assert!(matches!(result, Ok((bytes, ref digest))
            if bytes == SOURCE_VERIFICATION_PROGRESS_BYTES + 17 && digest == &expected));
        assert_eq!(
            progress.first().copied(),
            Some(SOURCE_VERIFICATION_PROGRESS_BYTES)
        );
        assert_eq!(
            progress.last().copied(),
            Some(SOURCE_VERIFICATION_PROGRESS_BYTES + 17)
        );
        assert!(progress.windows(2).all(|values| values[0] < values[1]));
    }

    #[test]
    fn source_progress_reserves_the_exact_boundary_for_one_completion_event() {
        assert_eq!(intermediate_source_progress_bytes(7, 8), Some(7));
        assert_eq!(intermediate_source_progress_bytes(8, 8), None);
        assert_eq!(intermediate_source_progress_bytes(9, 8), None);
    }

    #[test]
    fn source_fingerprint_cancels_between_fixed_reads() {
        struct CancelAfterFirstRead {
            inner: Cursor<Vec<u8>>,
            cancellation: CancellationToken,
            reads: usize,
        }

        impl Read for CancelAfterFirstRead {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                let read = self.inner.read(buffer)?;
                self.reads += 1;
                if self.reads == 1 {
                    assert!(self.cancellation.cancel());
                }
                Ok(read)
            }
        }

        let cancellation = CancellationToken::new();
        let mut reader = CancelAfterFirstRead {
            inner: Cursor::new(vec![0x5a_u8; aether_session::FINGERPRINT_BUFFER_BYTES * 2]),
            cancellation: cancellation.clone(),
            reads: 0,
        };
        let result = fingerprint_registered_stack_source(&mut reader, &cancellation, |_| {});

        assert!(result.is_err());
        assert_eq!(reader.reads, 1);
    }

    fn request(transfer: PreviewTransfer) -> FitsPreviewRequest {
        FitsPreviewRequest {
            path: PathBuf::from("unused-in-memory-test.fits"),
            content: FitsPreviewContent::Scalar { plane: 0 },
            maximum_width: 4,
            maximum_height: 2,
            black_point: 0.0,
            white_point: 8.0,
            midtone: 0.5,
            transfer,
            palette: PreviewPalette::Grayscale,
        }
    }

    fn estimate_request() -> FitsPreviewEstimateRequest {
        FitsPreviewEstimateRequest {
            path: PathBuf::from("unused-in-memory-test.fits"),
            content: FitsPreviewContent::Scalar { plane: 0 },
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

    fn rgb_fits_bytes() -> TestResult<Vec<u8>> {
        let mut pixels = Vec::new();
        pixels.extend((1..=8).map(f64::from));
        pixels.extend((2..=9).map(f64::from));
        pixels.extend((3..=10).map(f64::from));
        let image = ScientificImage::from_pixels(Dimensions::new(4, 2, 3)?, pixels)?;
        let mut bytes = Vec::new();
        write_f64_primary(&mut bytes, &image)?;
        Ok(bytes)
    }

    fn exact<T>(value: T, keyword: &str) -> CanonicalValue<T> {
        CanonicalValue::new(value, keyword, Confidence::Exact)
    }

    fn planning_metadata(frame_type: FrameType) -> CanonicalMetadata {
        CanonicalMetadata {
            camera: Some(exact(CameraModel::ZwoAsi294McPro, "INSTRUME")),
            frame_type: Some(exact(frame_type, "IMAGETYP")),
            exposure_seconds: Some(exact(2.0, "EXPTIME")),
            sensor_temperature_c: Some(exact(-10.0, "CCD-TEMP")),
            set_temperature_c: Some(exact(-10.0, "SET-TEMP")),
            gain: Some(exact(120.0, "GAIN")),
            offset: Some(exact(30.0, "OFFSET")),
            binning: Some(exact(Binning { x: 1, y: 1 }, "XBINNING")),
            filter: Some(exact("UVIR".to_owned(), "FILTER")),
            bayer_pattern: Some(exact(BayerPattern::Rggb, "BAYERPAT")),
            issues: Vec::new(),
        }
    }

    fn planning_file(
        root: &Path,
        relative_path: &str,
        frame_type: FrameType,
    ) -> TestResult<ManifestFile> {
        let path = root.join(relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let sample = if frame_type == FrameType::Dark {
            1.0
        } else {
            2.0
        };
        let image = ScientificImage::from_pixels(Dimensions::new(4, 2, 1)?, vec![sample; 8])?;
        let mut output = File::create(&path)?;
        write_f64_primary(&mut output, &image)?;
        drop(output);
        let mut source = File::open(path)?;
        let fingerprint = fingerprint_reader(&mut source)?;
        let metadata = planning_metadata(frame_type);
        let classification = classify_frame(Path::new(relative_path), &metadata);
        Ok(ManifestFile::from_analysis(
            relative_path,
            fingerprint,
            vec![4, 2],
            metadata,
            Vec::new(),
            classification,
            ClassificationPolicy::RequireAgreement,
        )?)
    }

    fn planning_session(root: &Path) -> TestResult<ImportedNativeSession> {
        let dark = planning_file(root, "DARKS/dark.fits", FrameType::Dark)?;
        let flat = planning_file(root, "FLATS/flat.fits", FrameType::Flat)?;
        let light = planning_file(root, "LIGHTS/light.fits", FrameType::Light)?;
        let dark_key =
            StrictGroupingKey::from_metadata(FrameType::Dark, dark.metadata(), dark.axes())?;
        let flat_key =
            StrictGroupingKey::from_metadata(FrameType::Flat, flat.metadata(), flat.axes())?;
        let light_key =
            StrictGroupingKey::from_metadata(FrameType::Light, light.metadata(), light.axes())?;
        let groups = vec![
            ManifestGroup::new(
                "dark-2s",
                dark_key,
                vec![dark.relative_path().to_owned()],
                Vec::new(),
                None,
            )?,
            ManifestGroup::new(
                "flat-uvir",
                flat_key,
                vec![flat.relative_path().to_owned()],
                Vec::new(),
                None,
            )?,
            ManifestGroup::new(
                "light-uvir",
                light_key,
                vec![light.relative_path().to_owned()],
                Vec::new(),
                None,
            )?,
        ];
        Ok(ImportedNativeSession {
            root: root.to_owned(),
            manifest: Arc::new(SessionManifest::new(
                ClassificationPolicy::RequireAgreement,
                vec![dark, flat, light],
                groups,
            )?),
        })
    }

    fn registration_planning_file(
        root: &Path,
        relative_path: &str,
        translation: (f64, f64),
    ) -> TestResult<ManifestFile> {
        const WIDTH: usize = 256;
        const HEIGHT: usize = 256;
        const STARS: [(f64, f64); 24] = [
            (20.0, 20.0),
            (50.0, 25.0),
            (83.0, 18.0),
            (125.0, 29.0),
            (170.0, 22.0),
            (215.0, 35.0),
            (30.0, 65.0),
            (72.0, 78.0),
            (110.0, 60.0),
            (152.0, 82.0),
            (205.0, 69.0),
            (18.0, 115.0),
            (58.0, 128.0),
            (98.0, 110.0),
            (142.0, 132.0),
            (190.0, 118.0),
            (225.0, 145.0),
            (35.0, 175.0),
            (80.0, 160.0),
            (120.0, 190.0),
            (165.0, 170.0),
            (210.0, 205.0),
            (65.0, 220.0),
            (145.0, 225.0),
        ];
        let path = root.join(relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut pixels = Vec::new();
        pixels.try_reserve_exact(WIDTH * HEIGHT)?;
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let mut sample = 1_000.0 + f64::from(((x + 3 * y) % 5) as u8) - 2.0;
                for (index, (star_x, star_y)) in STARS.iter().copied().enumerate() {
                    let dx = x as f64 - (star_x + translation.0);
                    let dy = y as f64 - (star_y + translation.1);
                    let amplitude = 500.0 + index as f64 * 17.0;
                    sample += amplitude * (-(dx * dx + dy * dy) / (2.0 * 3.5 * 3.5)).exp();
                }
                pixels.push(sample);
            }
        }
        let image = ScientificImage::from_pixels(Dimensions::new(WIDTH, HEIGHT, 1)?, pixels)?;
        let mut output = File::create(&path)?;
        write_f64_primary(&mut output, &image)?;
        drop(output);
        let mut source = File::open(path)?;
        let fingerprint = fingerprint_reader(&mut source)?;
        let metadata = planning_metadata(FrameType::Light);
        let classification = classify_frame(Path::new(relative_path), &metadata);
        Ok(ManifestFile::from_analysis(
            relative_path,
            fingerprint,
            vec![WIDTH as u64, HEIGHT as u64],
            metadata,
            Vec::new(),
            classification,
            ClassificationPolicy::RequireAgreement,
        )?)
    }

    fn registration_planning_session(root: &Path) -> TestResult<ImportedNativeSession> {
        let reference = registration_planning_file(root, "LIGHTS/reference.fits", (0.0, 0.0))?;
        let source = registration_planning_file(root, "LIGHTS/source.fits", (4.0, 6.0))?;
        let key = StrictGroupingKey::from_metadata(
            FrameType::Light,
            reference.metadata(),
            reference.axes(),
        )?;
        let group = ManifestGroup::new(
            "light-uvir",
            key,
            vec![
                reference.relative_path().to_owned(),
                source.relative_path().to_owned(),
            ],
            Vec::new(),
            None,
        )?;
        Ok(ImportedNativeSession {
            root: root.to_owned(),
            manifest: Arc::new(SessionManifest::new(
                ClassificationPolicy::RequireAgreement,
                vec![reference, source],
                vec![group],
            )?),
        })
    }

    fn registration_execution_request(
        session: &ImportedNativeSession,
        artifact_directory: &Path,
        output_directory: PathBuf,
    ) -> TestResult<RegistrationPlanExecutionCommandRequest> {
        fs::create_dir(artifact_directory)?;
        fs::create_dir(&output_directory)?;
        let sources = registration_native_sources(session)?;
        let mut frame_ids = sources.keys();
        let reference = frame_ids.next().ok_or("reference Light missing")?.clone();
        let source = frame_ids.next().ok_or("source Light missing")?.clone();
        let planning = RegistrationPlanPreviewRequest {
            reference_frame_id: reference.as_str().to_owned(),
            source_frame_ids: vec![source.as_str().to_owned()],
        };
        let plan = build_registration_plan_sync(session, &planning)?;
        let image = ScientificImage::from_pixels(
            Dimensions::new(256, 256, 3)?,
            vec![1_000.0; 256 * 256 * 3],
        )?;
        let mut artifacts = Vec::new();
        for (index, frame_id) in [reference, source].into_iter().enumerate() {
            let path = artifact_directory.join(format!("linear-{index}.fits"));
            let provenance =
                FitsOutputProvenance::new("a".repeat(64), "light-uvir", "linear-rgb-v1", 1)?
                    .with_frame_id_sha256(frame_id.as_str())?;
            write_f64_primary_atomic_new_with_provenance(&path, &image, &provenance)?;
            artifacts.push(RegistrationArtifactInput {
                frame_id: frame_id.as_str().to_owned(),
                path,
            });
        }
        Ok(RegistrationPlanExecutionCommandRequest {
            planning,
            expected_plan_sha256: plan.plan_sha256().to_owned(),
            artifacts,
            output_directory,
            band_height: 32,
            memory_limit_bytes: 16 * 1_024 * 1_024,
        })
    }

    fn master_execution_request(
        session: &ImportedNativeSession,
        output_directory: PathBuf,
        policy: FlatPedestalPolicyWire,
    ) -> TestResult<MasterPlanExecutionCommandRequest> {
        let planning = MasterPlanPreviewRequest {
            flat_pedestal_policy: policy,
            maximum_exposure_delta_seconds: 0.01,
            maximum_temperature_delta_c: 1.0,
            maximum_light_dark_temperature_delta_c: 2.0,
        };
        let preview = preview_master_plan_sync(session, planning)?;
        Ok(MasterPlanExecutionCommandRequest {
            output_directory,
            planning,
            expected_manifest_sha256: preview.manifest_sha256,
            expected_plan_sha256: preview.plan_sha256,
            minimum_flat_normalization_samples: 3,
            minimum_positive_flat_median: 1.0e-12,
            tile_width: 2,
            tile_height: 2,
            memory_limit_bytes: 1_048_576,
        })
    }

    fn light_execution_request(
        session: &ImportedNativeSession,
        master_directory: PathBuf,
        output_directory: PathBuf,
    ) -> TestResult<LightPlanExecutionCommandRequest> {
        let planning = MasterPlanPreviewRequest {
            flat_pedestal_policy: FlatPedestalPolicyWire::RequireMatchedDark,
            maximum_exposure_delta_seconds: 0.01,
            maximum_temperature_delta_c: 1.0,
            maximum_light_dark_temperature_delta_c: 2.0,
        };
        let preview = preview_master_plan_sync(session, planning)?;
        let light_plan = preview.light_plan.ok_or("light plan missing")?;
        Ok(LightPlanExecutionCommandRequest {
            master_directory,
            output_directory,
            planning,
            expected_manifest_sha256: preview.manifest_sha256,
            expected_master_plan_sha256: preview.plan_sha256,
            expected_light_plan_sha256: light_plan.plan_sha256,
            minimum_absolute_flat: 1.0e-12,
            tile_width: 2,
            tile_height: 2,
            memory_limit_bytes: 1_048_576,
            output_mode: LightOutputMode::Integrated,
        })
    }

    fn quality_pixels() -> TestResult<Vec<f64>> {
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
        Ok(pixels)
    }

    fn quality_fits_bytes() -> TestResult<Vec<u8>> {
        const WIDTH: usize = 128;
        const HEIGHT: usize = 128;
        let image =
            ScientificImage::from_pixels(Dimensions::new(WIDTH, HEIGHT, 1)?, quality_pixels()?)?;
        let mut bytes = Vec::new();
        write_f64_primary(&mut bytes, &image)?;
        Ok(bytes)
    }

    fn rgb_quality_fits_bytes() -> TestResult<Vec<u8>> {
        const WIDTH: usize = 128;
        const HEIGHT: usize = 128;
        let plane = quality_pixels()?;
        let mut pixels = Vec::new();
        pixels.try_reserve_exact(plane.len() * 3)?;
        pixels.extend_from_slice(&plane);
        pixels.extend_from_slice(&plane);
        pixels.extend_from_slice(&plane);
        let image = ScientificImage::from_pixels(Dimensions::new(WIDTH, HEIGHT, 3)?, pixels)?;
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
    fn renders_distinct_false_color_rejection_previews() -> TestResult {
        let mut low_request = request(PreviewTransfer::Linear);
        low_request.palette = PreviewPalette::LowRejection;
        let low = render_fits_preview_png(Cursor::new(fits_bytes()?), &low_request)?;
        let mut high_request = request(PreviewTransfer::Linear);
        high_request.palette = PreviewPalette::HighRejection;
        let high = render_fits_preview_png(Cursor::new(fits_bytes()?), &high_request)?;

        assert_eq!(&low[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&high[..8], b"\x89PNG\r\n\x1a\n");
        assert_ne!(low, high);
        Ok(())
    }

    #[test]
    fn accepts_frontend_rejection_palette_names() -> TestResult {
        let decoded: FitsPreviewRequest = serde_json::from_value(serde_json::json!({
            "path": "rejection.fits",
            "content": { "kind": "scalar", "plane": 0 },
            "maximumWidth": 800,
            "maximumHeight": 600,
            "blackPoint": 0.0,
            "whitePoint": 4.0,
            "midtone": 0.5,
            "transfer": { "kind": "linear" },
            "palette": "rejection_low"
        }))?;

        assert!(matches!(decoded.palette, PreviewPalette::LowRejection));
        Ok(())
    }

    #[test]
    fn rejection_histogram_counts_every_exact_integer_sample() -> TestResult {
        let image = ScientificImage::from_pixels(
            Dimensions::new(8, 1, 1)?,
            vec![0.0, 0.0, 1.0, 1.0, 1.0, 3.0, 3.0, 5.0],
        )?;
        let mut bytes = Vec::new();
        write_f64_primary(&mut bytes, &image)?;

        let histogram = inspect_rejection_histogram_reader(Cursor::new(bytes))?;

        assert_eq!(histogram.algorithm_id, REJECTION_HISTOGRAM_ALGORITHM_ID);
        assert_eq!(histogram.total_samples, 8);
        assert_eq!(histogram.zero_samples, 2);
        assert_eq!(histogram.rejected_samples, 6);
        assert_eq!(histogram.maximum_rejected_count, 5);
        assert_eq!(
            histogram
                .bins
                .iter()
                .map(|bin| (bin.rejected_count, bin.samples))
                .collect::<Vec<_>>(),
            vec![(0, 2), (1, 3), (3, 2), (5, 1)]
        );
        Ok(())
    }

    #[test]
    fn rejection_histogram_refuses_fractional_counts() -> TestResult {
        let image = ScientificImage::from_pixels(Dimensions::new(2, 1, 1)?, vec![0.0, 1.5])?;
        let mut bytes = Vec::new();
        write_f64_primary(&mut bytes, &image)?;

        let Err(error) = inspect_rejection_histogram_reader(Cursor::new(bytes)) else {
            return Err("fractional rejection count was accepted".into());
        };

        assert_eq!(error.code, "rejection_histogram_sample_invalid");
        Ok(())
    }

    #[test]
    fn exact_pixel_inspection_preserves_plane_order_and_counts() -> TestResult {
        let image = ScientificImage::from_pixels(
            Dimensions::new(2, 2, 3)?,
            vec![
                1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
            ],
        )?;
        let mut bytes = Vec::new();
        write_f64_primary(&mut bytes, &image)?;

        let (width, height, values) = read_pixel_planes(Cursor::new(bytes), 1, 0, None)?;

        assert_eq!((width, height), (2, 2));
        assert_eq!(values, vec![Some(2.0), Some(6.0), Some(10.0)]);
        assert_eq!(
            rejection_counts(vec![Some(0.0), Some(2.0), None])?,
            vec![Some(0), Some(2), None]
        );
        Ok(())
    }

    #[test]
    fn exact_pixel_inspection_rejects_out_of_bounds_coordinates() -> TestResult {
        let Err(error) = read_pixel_planes(Cursor::new(fits_bytes()?), 4, 0, None) else {
            return Err("out-of-bounds stack coordinate was accepted".into());
        };

        assert_eq!(error.code, "stack_pixel_coordinate_invalid");
        let Err(error) = read_pixel_planes(Cursor::new(fits_bytes()?), 0, 0, Some((4, 2, 3)))
        else {
            return Err("a mismatched rejection-map plane count was accepted".into());
        };
        assert_eq!(error.code, "stack_pixel_coordinate_invalid");
        Ok(())
    }

    #[test]
    fn renders_planar_rgb_in_canonical_channel_order() -> TestResult {
        let mut rgb_request = request(PreviewTransfer::Linear);
        rgb_request.content = FitsPreviewContent::Rgb;
        rgb_request.white_point = 12.0;
        let encoded = render_fits_preview_png(Cursor::new(rgb_fits_bytes()?), &rgb_request)?;
        let decoder = png::Decoder::new(Cursor::new(encoded));
        let mut reader = decoder.read_info()?;
        let output_size = reader
            .output_buffer_size()
            .ok_or("decoded PNG output size overflowed")?;
        let mut pixels = vec![0; output_size];
        let info = reader.next_frame(&mut pixels)?;

        assert_eq!(info.color_type, png::ColorType::Rgba);
        assert_eq!(&pixels[..4], &[21, 43, 64, 255]);
        Ok(())
    }

    #[test]
    fn rejects_a_diagnostic_palette_for_rgb_content() -> TestResult {
        let mut invalid = request(PreviewTransfer::Linear);
        invalid.content = FitsPreviewContent::Rgb;
        invalid.palette = PreviewPalette::LowRejection;

        let Err(error) = render_fits_preview_png(Cursor::new(rgb_fits_bytes()?), &invalid) else {
            return Err("an RGB diagnostic palette was accepted".into());
        };
        assert_eq!(error.code, "preview_palette_invalid");
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
    fn seals_registration_geometry_reconstructed_from_the_imported_lights() -> TestResult {
        let directory = TestDirectory::new()?;
        let session = registration_planning_session(directory.path())?;
        let sources = registration_native_sources(&session)?;
        let mut frame_ids = sources.keys();
        let reference = frame_ids.next().ok_or("reference Light missing")?.clone();
        let source = frame_ids.next().ok_or("source Light missing")?.clone();
        let accepted = diagnose_paths(&sources[&source].path, &sources[&reference].path)?;
        if accepted.accepted_plan().is_none() {
            return Err("synthetic registration evidence did not pass confidence".into());
        }

        let plan = preview_registration_plan_sync(
            &session,
            RegistrationPlanPreviewRequest {
                reference_frame_id: reference.as_str().to_owned(),
                source_frame_ids: vec![source.as_str().to_owned()],
            },
        )?;

        assert_eq!(plan.schema_version, 1);
        assert_eq!(plan.reference_frame_id, reference.as_str());
        assert_eq!(plan.reference_width, 256);
        assert_eq!(plan.reference_height, 256);
        assert_eq!(plan.frames.len(), 2);
        assert_eq!(
            plan.frames.iter().filter(|frame| frame.reference).count(),
            1
        );
        assert_eq!(plan.plan_sha256.len(), 64);
        assert!(plan.covered_pixels > 0);
        assert!(plan.autocrop.width > 0);
        assert!(plan.autocrop.height > 0);
        Ok(())
    }

    #[test]
    fn rejects_registration_requests_that_do_not_name_each_light_once() -> TestResult {
        let directory = TestDirectory::new()?;
        let session = registration_planning_session(directory.path())?;
        let sources = registration_native_sources(&session)?;
        let mut frame_ids = sources.keys();
        let reference = frame_ids.next().ok_or("reference Light missing")?.clone();
        let source = frame_ids.next().ok_or("source Light missing")?.clone();

        for source_frame_ids in [
            Vec::new(),
            vec![source.as_str().to_owned(), source.as_str().to_owned()],
            vec![reference.as_str().to_owned()],
        ] {
            let error = preview_registration_plan_sync(
                &session,
                RegistrationPlanPreviewRequest {
                    reference_frame_id: reference.as_str().to_owned(),
                    source_frame_ids,
                },
            )
            .err()
            .ok_or("an incomplete or duplicate Light identity set was accepted")?;
            assert_eq!(error.code, "registration_plan_input_invalid");
        }
        Ok(())
    }

    #[test]
    fn native_review_rejection_removes_a_light_from_registration_membership() -> TestResult {
        let directory = TestDirectory::new()?;
        let session = registration_planning_session(directory.path())?;
        let sources = registration_native_sources(&session)?;
        let review_state = DesktopReviewState::default();
        let frames = sources
            .keys()
            .enumerate()
            .map(|(index, frame_id)| {
                FrameSpec::new(
                    frame_id.clone(),
                    format!("light-{index}.fits"),
                    FrameMetrics::default(),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        *lock_review_state(&review_state)? = Some(ReviewBook::new(frames, MAX_UNDO_DEPTH)?);
        assert_eq!(
            reviewed_registration_frame_ids(&review_state, &session)?.len(),
            2
        );
        let rejected = sources.keys().next().ok_or("Light missing")?.clone();

        apply_review_decision_sync(
            &review_state,
            ReviewDecisionRequest {
                frame_id: rejected.as_str().to_owned(),
                action: ReviewDecisionAction::Reject {
                    reason: ReviewRejectionReasonWire::Blur,
                },
            },
        )?;

        let error = reviewed_registration_frame_ids(&review_state, &session)
            .err()
            .ok_or("registration remained eligible with only one non-rejected Light")?;
        assert_eq!(error.code, "registration_plan_input_invalid");
        Ok(())
    }

    #[test]
    fn executes_the_sealed_registration_plan_as_one_artifact_transaction() -> TestResult {
        let directory = TestDirectory::new()?;
        let session_root = directory.path().join("session");
        let artifact_root = directory.path().join("linear-rgb");
        let output_root = directory.path().join("registered");
        fs::create_dir(&session_root)?;
        let session = registration_planning_session(&session_root)?;
        let request =
            registration_execution_request(&session, &artifact_root, output_root.clone())?;
        let expected_digest = request.expected_plan_sha256.clone();
        let mut progress = Vec::new();

        let result = execute_registration_plan_sync(
            &session,
            request,
            &CancellationToken::new(),
            |event| progress.push(event),
        )?;

        assert_eq!(result.plan_sha256, expected_digest);
        assert_eq!(result.frames.len(), 2);
        assert!(result.peak_reserved_bytes > 0);
        assert!(result.peak_reserved_bytes <= result.memory_limit_bytes);
        assert!(result.frames.iter().all(|frame| {
            Path::new(&frame.output_path).is_file()
                && frame.samples_written == 256 * 256 * 3
                && frame.bytes_written > 0
        }));
        assert!(progress.iter().all(|event| event.frame_count == 2));
        assert_eq!(fs::read_dir(output_root)?.count(), 2);
        Ok(())
    }

    #[test]
    fn integrates_the_published_registered_set_on_the_sealed_common_crop() -> TestResult {
        let directory = TestDirectory::new()?;
        let session_root = directory.path().join("session");
        let artifact_root = directory.path().join("linear-rgb");
        let registered_root = directory.path().join("registered");
        let stack_path = directory.path().join("integrated-common-crop.fits");
        fs::create_dir(&session_root)?;
        let session = registration_planning_session(&session_root)?;
        let registration_request =
            registration_execution_request(&session, &artifact_root, registered_root.clone())?;
        let planning = registration_request.planning.clone();
        let expected_plan_sha256 = registration_request.expected_plan_sha256.clone();
        let registered = execute_registration_plan_sync(
            &session,
            registration_request,
            &CancellationToken::new(),
            |_| {},
        )?;
        let artifacts = registered
            .frames
            .into_iter()
            .map(|frame| RegistrationArtifactInput {
                frame_id: frame.frame_id,
                path: PathBuf::from(frame.output_path),
            })
            .collect();
        let mut progress = Vec::new();

        let result = execute_registered_stack_sync(
            &session,
            RegisteredStackCommandRequest {
                planning,
                expected_plan_sha256: expected_plan_sha256.clone(),
                artifacts,
                quality_evidence: Vec::new(),
                quality_reference_frame_id: None,
                output_path: stack_path.clone(),
                band_height: 32,
                memory_limit_bytes: 16 * 1_024 * 1_024,
                integration: RegisteredStackIntegrationSettings {
                    estimator: RegisteredStackEstimatorInput::StrictMean,
                    low_fraction: 0.1,
                    high_fraction: 0.1,
                    minimum_retained_samples: 3,
                    generate_rejection_maps: false,
                },
            },
            &CancellationToken::new(),
            |event| progress.push(event),
        )?;

        assert_eq!(result.plan_sha256, expected_plan_sha256);
        assert_eq!(result.output_path, stack_path.to_string_lossy());
        assert_eq!(result.planes, 3);
        assert!(result.width > 0 && result.width < 256);
        assert!(result.height > 0 && result.height < 256);
        assert_eq!(
            result.samples_written,
            u64::try_from(result.width * result.height * result.planes)?
        );
        assert!(result.bytes_written > 0);
        assert!(result.peak_reserved_bytes <= result.memory_limit_bytes);
        assert_eq!(result.estimator, REGISTERED_CROP_MEAN_ALGORITHM_ID);
        assert!(result.low_rejection_map_path.is_none());
        assert!(result.high_rejection_map_path.is_none());
        assert!(result.rejection_map_samples_written.is_none());
        assert!(stack_path.is_file());
        let report_path = directory
            .path()
            .join("integrated-common-crop-integration-report.json");
        assert_eq!(result.report_path, report_path.to_string_lossy());
        assert_eq!(result.report_sha256.len(), 64);
        let mut report: serde_json::Value = serde_json::from_slice(&fs::read(&report_path)?)?;
        assert_eq!(report["schemaVersion"], 1);
        assert_eq!(report["reportSha256"], result.report_sha256);
        assert_eq!(
            report["report"]["algorithmId"],
            REGISTERED_STACK_REPORT_ALGORITHM_ID
        );
        assert_eq!(report["report"]["planSha256"], expected_plan_sha256);
        assert_eq!(report["report"]["integration"]["estimator"], "strict_mean");
        assert_eq!(report["report"]["products"][0]["role"], "science");
        assert_eq!(
            report["report"]["sources"].as_array().map(Vec::len),
            Some(2)
        );
        let inspection = inspect_registered_stack_report_sync(&report_path)?;
        assert_eq!(inspection.report_sha256, result.report_sha256);
        assert_eq!(inspection.source_count, 2);
        assert_eq!(inspection.product_count, 1);
        assert_eq!(inspection.width, result.width);
        assert_eq!(inspection.height, result.height);
        assert_eq!(inspection.planes, result.planes);
        assert!(!inspection.weighted);
        assert_eq!(inspection.sources.len(), 2);
        assert_eq!(inspection.sources[0].frame_id.len(), 64);
        assert_eq!(inspection.sources[0].sha256.len(), 64);
        assert!(inspection.sources[0].byte_length > 0);
        let mut source_progress = Vec::new();
        let verified_sources = verify_registered_stack_sources_sync(
            &report_path,
            &registered_root,
            &CancellationToken::new(),
            |progress| source_progress.push(progress),
        )?;
        assert!(verified_sources.all_sources_verified);
        assert_eq!(verified_sources.report_sha256, result.report_sha256);
        assert_eq!(verified_sources.sources.len(), 2);
        assert_eq!(
            source_progress.first().map(|event| event.state),
            Some("started")
        );
        assert_eq!(
            source_progress.last().map(|event| event.state),
            Some("completed")
        );
        assert_eq!(
            source_progress.last().map(|event| event.completed_sources),
            Some(2)
        );
        assert_eq!(
            source_progress.last().map(|event| event.completed_bytes),
            source_progress.last().map(|event| event.total_bytes)
        );
        assert!(source_progress.windows(2).all(|events| {
            events[0].sequence < events[1].sequence
                && events[0].completed_bytes <= events[1].completed_bytes
        }));
        assert!(verified_sources.sources.iter().all(|source| {
            source.status == RegisteredStackSourceVerificationStatus::Verified
                && source
                    .path
                    .starts_with(registered_root.to_string_lossy().as_ref())
                && source.byte_length > 0
        }));
        let first_source_path = registered_root.join(&inspection.sources[0].file_name);
        let parked_source_path = registered_root.join("parked-source.fits");
        fs::rename(&first_source_path, &parked_source_path)?;
        let missing_source = verify_registered_stack_sources_sync(
            &report_path,
            &registered_root,
            &CancellationToken::new(),
            |_| {},
        )?;
        assert!(!missing_source.all_sources_verified);
        assert_eq!(
            missing_source.sources[0].status,
            RegisteredStackSourceVerificationStatus::Missing
        );
        fs::rename(&parked_source_path, &first_source_path)?;
        let mut altered_source = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&first_source_path)?;
        let mut source_byte = [0_u8; 1];
        altered_source.read_exact(&mut source_byte)?;
        altered_source.seek(SeekFrom::Start(0))?;
        altered_source.write_all(&[source_byte[0] ^ 0x01])?;
        altered_source.flush()?;
        let changed_source = verify_registered_stack_sources_sync(
            &report_path,
            &registered_root,
            &CancellationToken::new(),
            |_| {},
        )?;
        assert!(!changed_source.all_sources_verified);
        assert_eq!(
            changed_source.sources[0].status,
            RegisteredStackSourceVerificationStatus::FingerprintMismatch
        );
        altered_source.seek(SeekFrom::Start(0))?;
        altered_source.write_all(&source_byte)?;
        altered_source.flush()?;
        drop(altered_source);
        assert!(
            verify_registered_stack_sources_sync(
                &report_path,
                &registered_root,
                &CancellationToken::new(),
                |_| {},
            )?
            .all_sources_verified
        );
        let cancelled = CancellationToken::new();
        assert!(cancelled.cancel());
        let Err(cancelled_error) = verify_registered_stack_sources_sync(
            &report_path,
            &registered_root,
            &cancelled,
            |_| {},
        ) else {
            return Err("cancelled source verification unexpectedly completed".into());
        };
        assert_eq!(
            cancelled_error.code,
            "registered_stack_source_verification_cancelled"
        );
        assert!(inspection.all_products_verified);
        assert_eq!(inspection.products.len(), 1);
        assert_eq!(inspection.products[0].role, "science");
        assert_eq!(
            inspection.products[0].status,
            RegisteredStackReportProductStatus::Verified
        );
        assert_eq!(inspection.products[0].bytes_written, result.bytes_written);
        let parked_stack_path = directory.path().join("parked-stack.fits");
        fs::rename(&stack_path, &parked_stack_path)?;
        let missing_product = inspect_registered_stack_report_sync(&report_path)?;
        assert!(!missing_product.all_products_verified);
        assert_eq!(
            missing_product.products[0].status,
            RegisteredStackReportProductStatus::Missing
        );
        fs::rename(&parked_stack_path, &stack_path)?;
        let mut altered_stack = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&stack_path)?;
        altered_stack.seek(SeekFrom::Start(2_888))?;
        let mut stored_byte = [0_u8; 1];
        altered_stack.read_exact(&mut stored_byte)?;
        altered_stack.seek(SeekFrom::Start(2_888))?;
        altered_stack.write_all(&[stored_byte[0] ^ 0x01])?;
        altered_stack.flush()?;
        let changed_product = inspect_registered_stack_report_sync(&report_path)?;
        assert!(!changed_product.all_products_verified);
        assert_eq!(
            changed_product.products[0].status,
            RegisteredStackReportProductStatus::ChecksumMismatch
        );
        altered_stack.seek(SeekFrom::Start(2_888))?;
        altered_stack.write_all(&stored_byte)?;
        altered_stack.flush()?;
        drop(altered_stack);
        assert!(inspect_registered_stack_report_sync(&report_path)?.all_products_verified);
        assert!(is_safe_report_file_name("integrated.fits"));
        assert!(!is_safe_report_file_name("../integrated.fits"));
        assert!(!is_safe_report_file_name("nested/integrated.fits"));
        assert_eq!(progress.first().map(|event| event.state), Some("started"));
        assert_eq!(progress.last().map(|event| event.state), Some("completed"));
        report["report"]["dimensions"]["width"] = serde_json::json!(1);
        fs::write(&report_path, serde_json::to_vec_pretty(&report)?)?;
        let Err(error) = inspect_registered_stack_report_sync(&report_path) else {
            return Err("tampered integration report passed native inspection".into());
        };
        assert_eq!(error.code, "registered_stack_report_digest_mismatch");
        Ok(())
    }

    #[test]
    fn publishes_advanced_stack_and_both_rejection_maps() -> TestResult {
        let directory = TestDirectory::new()?;
        let session_root = directory.path().join("session");
        let artifact_root = directory.path().join("linear-rgb");
        let registered_root = directory.path().join("registered");
        let stack_path = directory.path().join("advanced-stack.fits");
        fs::create_dir(&session_root)?;
        let session = registration_planning_session(&session_root)?;
        let registration_request =
            registration_execution_request(&session, &artifact_root, registered_root)?;
        let planning = registration_request.planning.clone();
        let expected_plan_sha256 = registration_request.expected_plan_sha256.clone();
        let registered = execute_registration_plan_sync(
            &session,
            registration_request,
            &CancellationToken::new(),
            |_| {},
        )?;

        let result = execute_registered_stack_sync(
            &session,
            RegisteredStackCommandRequest {
                planning,
                expected_plan_sha256,
                artifacts: registered
                    .frames
                    .into_iter()
                    .map(|frame| RegistrationArtifactInput {
                        frame_id: frame.frame_id,
                        path: PathBuf::from(frame.output_path),
                    })
                    .collect(),
                quality_evidence: Vec::new(),
                quality_reference_frame_id: None,
                output_path: stack_path.clone(),
                band_height: 32,
                memory_limit_bytes: 16 * 1_024 * 1_024,
                integration: RegisteredStackIntegrationSettings {
                    estimator: RegisteredStackEstimatorInput::PercentileClipped,
                    low_fraction: 0.1,
                    high_fraction: 0.1,
                    minimum_retained_samples: 2,
                    generate_rejection_maps: true,
                },
            },
            &CancellationToken::new(),
            |_| {},
        )?;

        let low_path = directory.path().join("advanced-stack-rejection-low.fits");
        let high_path = directory.path().join("advanced-stack-rejection-high.fits");
        assert_eq!(
            result.estimator,
            REGISTERED_PERCENTILE_CLIPPED_MEAN_ALGORITHM_ID
        );
        assert_eq!(result.low_rejection_map_path.as_deref(), low_path.to_str());
        assert_eq!(
            result.high_rejection_map_path.as_deref(),
            high_path.to_str()
        );
        assert_eq!(
            result.rejection_map_samples_written,
            Some(result.samples_written)
        );
        assert!(stack_path.is_file());
        assert!(low_path.is_file());
        assert!(high_path.is_file());
        Ok(())
    }

    #[test]
    fn integrates_with_native_identity_bound_quality_weights() -> TestResult {
        let directory = TestDirectory::new()?;
        let session_root = directory.path().join("session");
        let artifact_root = directory.path().join("linear-rgb");
        let registered_root = directory.path().join("registered");
        let stack_path = directory.path().join("weighted-stack.fits");
        fs::create_dir(&session_root)?;
        let session = registration_planning_session(&session_root)?;
        let registration_request =
            registration_execution_request(&session, &artifact_root, registered_root)?;
        let planning = registration_request.planning.clone();
        let expected_plan_sha256 = registration_request.expected_plan_sha256.clone();
        let registered = execute_registration_plan_sync(
            &session,
            registration_request,
            &CancellationToken::new(),
            |_| {},
        )?;
        let quality_evidence: Vec<RegisteredFrameQualityInput> = registered
            .frames
            .iter()
            .enumerate()
            .map(|(index, frame)| RegisteredFrameQualityInput {
                frame_id: frame.frame_id.clone(),
                signal_to_noise: 20.0 + index as f64,
                fwhm_pixels: 2.0 + index as f64 * 0.1,
                eccentricity: 0.25 + index as f64 * 0.01,
            })
            .collect();
        let report_reference_frame_id = planning.reference_frame_id.clone();
        let quality_reference_frame_id = Some(report_reference_frame_id.clone());
        let weight_preflight = preview_registered_weights_sync(RegisteredWeightPreflightRequest {
            expected_plan_sha256: expected_plan_sha256.clone(),
            frame_ids: registered
                .frames
                .iter()
                .map(|frame| frame.frame_id.clone())
                .collect(),
            reference_frame_id: planning.reference_frame_id.clone(),
            quality_evidence: quality_evidence.clone(),
        })?;

        let result = execute_registered_stack_sync(
            &session,
            RegisteredStackCommandRequest {
                planning,
                expected_plan_sha256,
                artifacts: registered
                    .frames
                    .into_iter()
                    .map(|frame| RegistrationArtifactInput {
                        frame_id: frame.frame_id,
                        path: PathBuf::from(frame.output_path),
                    })
                    .collect(),
                quality_evidence,
                quality_reference_frame_id,
                output_path: stack_path.clone(),
                band_height: 32,
                memory_limit_bytes: 16 * 1_024 * 1_024,
                integration: RegisteredStackIntegrationSettings {
                    estimator: RegisteredStackEstimatorInput::WeightedMean,
                    low_fraction: 0.1,
                    high_fraction: 0.1,
                    minimum_retained_samples: 2,
                    generate_rejection_maps: false,
                },
            },
            &CancellationToken::new(),
            |_| {},
        )?;

        assert_eq!(result.estimator, REGISTERED_WEIGHTED_MEAN_ALGORITHM_ID);
        assert!(result.samples_written > 0);
        assert!(stack_path.is_file());
        let bytes = fs::read(&stack_path)?;
        let header = String::from_utf8_lossy(&bytes[..bytes.len().min(2_880)]);
        assert!(header.contains(&weight_preflight.parameters_sha256));
        let report: serde_json::Value = serde_json::from_slice(&fs::read(
            directory
                .path()
                .join("weighted-stack-integration-report.json"),
        )?)?;
        assert_eq!(
            report["report"]["weights"]["parametersSha256"],
            weight_preflight.parameters_sha256
        );
        assert_eq!(
            report["report"]["weights"]["referenceFrameId"],
            report_reference_frame_id
        );
        Ok(())
    }

    #[test]
    fn previews_canonical_registered_weights_before_execution() -> TestResult {
        let first = "1".repeat(64);
        let second = "2".repeat(64);
        let response = preview_registered_weights_sync(RegisteredWeightPreflightRequest {
            expected_plan_sha256: "a".repeat(64),
            frame_ids: vec![second.clone(), first.clone()],
            reference_frame_id: second.clone(),
            quality_evidence: vec![
                RegisteredFrameQualityInput {
                    frame_id: first.clone(),
                    signal_to_noise: 10.0,
                    fwhm_pixels: 2.0,
                    eccentricity: 0.0,
                },
                RegisteredFrameQualityInput {
                    frame_id: second.clone(),
                    signal_to_noise: 20.0,
                    fwhm_pixels: 1.0,
                    eccentricity: 0.0,
                },
            ],
        })?;

        assert_eq!(response.schema_version, 1);
        assert_eq!(response.plan_sha256, "a".repeat(64));
        assert_eq!(response.algorithm_id, BALANCED_PSF_WEIGHT_ALGORITHM_ID);
        assert_eq!(response.parameters_sha256.len(), 64);
        assert_eq!(response.reference_frame_id, second);
        assert_eq!(response.weights.len(), 2);
        assert_eq!(response.weights[0].frame_id, first);
        assert!((response.weights[0].weight - 0.0625).abs() < 1.0e-14);
        assert_eq!(response.weights[1].weight.to_bits(), 1.0_f64.to_bits());
        Ok(())
    }

    #[test]
    fn integration_report_publication_never_overwrites_an_existing_file() -> TestResult {
        let directory = TestDirectory::new()?;
        let report_path = directory.path().join("stack-integration-report.json");
        fs::write(&report_path, b"existing evidence\n")?;

        assert!(publish_registered_stack_report(&report_path, b"replacement\n").is_err());
        assert_eq!(fs::read(&report_path)?, b"existing evidence\n");
        Ok(())
    }

    #[test]
    fn registered_weight_preview_rejects_duplicate_or_foreign_identity() -> TestResult {
        let first = "1".repeat(64);
        let error = preview_registered_weights_sync(RegisteredWeightPreflightRequest {
            expected_plan_sha256: "a".repeat(64),
            frame_ids: vec![first.clone(), first.clone()],
            reference_frame_id: first.clone(),
            quality_evidence: vec![RegisteredFrameQualityInput {
                frame_id: first,
                signal_to_noise: 10.0,
                fwhm_pixels: 2.0,
                eccentricity: 0.0,
            }],
        })
        .err()
        .ok_or("duplicate registered weight identity was accepted")?;

        assert_eq!(error.code, "registered_stack_configuration_invalid");
        Ok(())
    }

    #[test]
    fn weighted_stack_rejects_incomplete_quality_evidence() -> TestResult {
        let directory = TestDirectory::new()?;
        let session_root = directory.path().join("session");
        let artifact_root = directory.path().join("linear-rgb");
        let registered_root = directory.path().join("registered");
        let stack_path = directory.path().join("invalid-weighted-stack.fits");
        fs::create_dir(&session_root)?;
        let session = registration_planning_session(&session_root)?;
        let registration_request =
            registration_execution_request(&session, &artifact_root, registered_root)?;
        let planning = registration_request.planning.clone();
        let registered = execute_registration_plan_sync(
            &session,
            registration_request,
            &CancellationToken::new(),
            |_| {},
        )?;
        let quality_reference_frame_id = Some(planning.reference_frame_id.clone());

        let error = execute_registered_stack_sync(
            &session,
            RegisteredStackCommandRequest {
                planning,
                expected_plan_sha256: registered.plan_sha256.clone(),
                artifacts: registered
                    .frames
                    .into_iter()
                    .map(|frame| RegistrationArtifactInput {
                        frame_id: frame.frame_id,
                        path: PathBuf::from(frame.output_path),
                    })
                    .collect(),
                quality_evidence: Vec::new(),
                quality_reference_frame_id,
                output_path: stack_path.clone(),
                band_height: 32,
                memory_limit_bytes: 16 * 1_024 * 1_024,
                integration: RegisteredStackIntegrationSettings {
                    estimator: RegisteredStackEstimatorInput::WeightedMean,
                    low_fraction: 0.1,
                    high_fraction: 0.1,
                    minimum_retained_samples: 2,
                    generate_rejection_maps: false,
                },
            },
            &CancellationToken::new(),
            |_| {},
        )
        .err()
        .ok_or("weighted stack accepted incomplete quality evidence")?;

        assert_eq!(error.code, "registered_stack_configuration_invalid");
        assert!(!stack_path.exists());
        Ok(())
    }

    #[test]
    fn cancelled_desktop_registered_stack_publishes_nothing() -> TestResult {
        let directory = TestDirectory::new()?;
        let session_root = directory.path().join("session");
        let artifact_root = directory.path().join("linear-rgb");
        let registered_root = directory.path().join("registered");
        let stack_path = directory.path().join("cancelled-stack.fits");
        fs::create_dir(&session_root)?;
        let session = registration_planning_session(&session_root)?;
        let registration_request =
            registration_execution_request(&session, &artifact_root, registered_root)?;
        let planning = registration_request.planning.clone();
        let expected_plan_sha256 = registration_request.expected_plan_sha256.clone();
        let registered = execute_registration_plan_sync(
            &session,
            registration_request,
            &CancellationToken::new(),
            |_| {},
        )?;
        let cancellation = CancellationToken::new();
        assert!(cancellation.cancel());

        let error = execute_registered_stack_sync(
            &session,
            RegisteredStackCommandRequest {
                planning,
                expected_plan_sha256,
                artifacts: registered
                    .frames
                    .into_iter()
                    .map(|frame| RegistrationArtifactInput {
                        frame_id: frame.frame_id,
                        path: PathBuf::from(frame.output_path),
                    })
                    .collect(),
                quality_evidence: Vec::new(),
                quality_reference_frame_id: None,
                output_path: stack_path.clone(),
                band_height: 32,
                memory_limit_bytes: 16 * 1_024 * 1_024,
                integration: RegisteredStackIntegrationSettings {
                    estimator: RegisteredStackEstimatorInput::StrictMean,
                    low_fraction: 0.1,
                    high_fraction: 0.1,
                    minimum_retained_samples: 3,
                    generate_rejection_maps: false,
                },
            },
            &cancellation,
            |_| {},
        )
        .err()
        .ok_or("cancelled desktop registered stack succeeded")?;

        assert_eq!(error.code, "registered_stack_cancelled");
        assert!(!stack_path.exists());
        Ok(())
    }

    #[test]
    fn stale_registration_digest_publishes_nothing() -> TestResult {
        let directory = TestDirectory::new()?;
        let session_root = directory.path().join("session");
        let artifact_root = directory.path().join("linear-rgb");
        let output_root = directory.path().join("registered");
        fs::create_dir(&session_root)?;
        let session = registration_planning_session(&session_root)?;
        let mut request =
            registration_execution_request(&session, &artifact_root, output_root.clone())?;
        request.expected_plan_sha256 = "0".repeat(64);

        let error =
            execute_registration_plan_sync(&session, request, &CancellationToken::new(), |_| {})
                .err()
                .ok_or("a stale registration digest was executed")?;

        assert_eq!(error.code, "registration_plan_stale");
        assert_eq!(fs::read_dir(output_root)?.count(), 0);
        Ok(())
    }

    #[test]
    fn cancelled_desktop_registration_publishes_nothing() -> TestResult {
        let directory = TestDirectory::new()?;
        let session_root = directory.path().join("session");
        let artifact_root = directory.path().join("linear-rgb");
        let output_root = directory.path().join("registered");
        fs::create_dir(&session_root)?;
        let session = registration_planning_session(&session_root)?;
        let request =
            registration_execution_request(&session, &artifact_root, output_root.clone())?;
        let cancellation = CancellationToken::new();
        assert!(cancellation.cancel());

        let error = execute_registration_plan_sync(&session, request, &cancellation, |_| {})
            .err()
            .ok_or("cancelled desktop registration succeeded")?;

        assert_eq!(error.code, "registration_execution_cancelled");
        assert_eq!(fs::read_dir(output_root)?.count(), 0);
        Ok(())
    }

    #[test]
    fn previews_native_master_groups_and_exclusive_flat_dependencies() -> TestResult {
        let directory = TestDirectory::new()?;
        let session = planning_session(directory.path())?;
        let preview = preview_master_plan_sync(
            &session,
            MasterPlanPreviewRequest {
                flat_pedestal_policy: FlatPedestalPolicyWire::PreferMatchedDarkThenBias,
                maximum_exposure_delta_seconds: 0.01,
                maximum_temperature_delta_c: 1.0,
                maximum_light_dark_temperature_delta_c: 2.0,
            },
        )?;

        assert!(preview.ready);
        assert_eq!(preview.schema_version, 1);
        assert_eq!(preview.manifest_sha256.len(), 64);
        assert_eq!(preview.plan_sha256.len(), 64);
        assert_eq!(preview.products.len(), 2);
        assert_eq!(preview.products[0].group_id, "dark-2s");
        assert_eq!(preview.products[0].kind, "dark");
        assert_eq!(preview.products[0].pedestal.status, "not_applicable");
        assert_eq!(preview.products[1].group_id, "flat-uvir");
        assert_eq!(preview.products[1].kind, "flat");
        assert_eq!(preview.products[1].frame_count, 1);
        assert_eq!(
            preview.products[1].camera.as_deref(),
            Some("ZWO ASI294MC Pro")
        );
        assert_eq!(preview.products[1].axes, [4, 2]);
        assert_eq!(preview.products[1].pedestal.status, "matched_dark");
        assert_eq!(
            preview.products[1].pedestal.selected_group_id.as_deref(),
            Some("dark-2s")
        );
        assert_eq!(preview.products[1].candidates.len(), 1);
        assert_eq!(preview.products[1].candidates[0].status, "compatible");
        assert!(preview.products[1].candidates[0].mismatches.is_empty());
        let light_plan = preview.light_plan.ok_or("light plan missing")?;
        assert!(light_plan.ready);
        assert_eq!(light_plan.schema_version, 1);
        assert_eq!(light_plan.plan_sha256.len(), 64);
        assert_eq!(light_plan.products.len(), 1);
        assert_eq!(light_plan.products[0].group_id, "light-uvir");
        assert_eq!(light_plan.products[0].dark.status, "matched");
        assert_eq!(
            light_plan.products[0].dark.selected_group_id.as_deref(),
            Some("dark-2s")
        );
        assert_eq!(light_plan.products[0].flat.status, "matched");
        assert_eq!(
            light_plan.products[0].flat.selected_group_id.as_deref(),
            Some("flat-uvir")
        );
        Ok(())
    }

    #[test]
    fn executes_native_master_plan_with_progress_and_bounded_memory() -> TestResult {
        let directory = TestDirectory::new()?;
        let session_root = directory.path().join("session");
        let output = directory.path().join("masters");
        fs::create_dir(&session_root)?;
        fs::create_dir(&output)?;
        let session = planning_session(&session_root)?;
        let mut progress = Vec::new();

        let result = execute_master_plan_sync(
            &session,
            master_execution_request(
                &session,
                output.clone(),
                FlatPedestalPolicyWire::PreferMatchedDarkThenBias,
            )?,
            &CancellationToken::new(),
            |event| progress.push(event),
        )?;

        assert_eq!(result.products.len(), 2);
        assert!(result.peak_reserved_bytes > 0);
        assert!(result.peak_reserved_bytes <= result.memory_limit_bytes);
        assert_eq!(result.products[0].kind, "dark");
        assert_eq!(result.products[0].mean.to_bits(), 1.0_f64.to_bits());
        assert_eq!(result.products[1].kind, "flat");
        assert_eq!(result.products[1].normalization, Some(1.0));
        assert_eq!(result.products[1].mean.to_bits(), 1.0_f64.to_bits());
        assert!(
            result
                .products
                .iter()
                .all(|product| Path::new(&product.output_path).is_file())
        );
        assert!(progress.iter().any(|event| event.group_id == "dark-2s"));
        assert!(progress.iter().any(|event| event.group_id == "flat-uvir"));
        assert!(progress.iter().all(|event| event.product_count == 2));
        Ok(())
    }

    #[test]
    fn executes_the_reviewed_light_plan_with_shared_bounded_progress() -> TestResult {
        let directory = TestDirectory::new()?;
        let session_root = directory.path().join("session");
        let masters = directory.path().join("masters");
        let output = directory.path().join("lights");
        fs::create_dir(&session_root)?;
        fs::create_dir(&masters)?;
        fs::create_dir(&output)?;
        let session = planning_session(&session_root)?;
        execute_master_plan_sync(
            &session,
            master_execution_request(
                &session,
                masters.clone(),
                FlatPedestalPolicyWire::RequireMatchedDark,
            )?,
            &CancellationToken::new(),
            |_| {},
        )?;
        let mut progress = Vec::new();

        let result = execute_light_plan_sync(
            &session,
            light_execution_request(&session, masters.clone(), output)?,
            &CancellationToken::new(),
            |event| progress.push(event),
        )?;

        assert_eq!(result.products.len(), 1);
        assert_eq!(result.products[0].group_id, "light-uvir");
        assert_eq!(result.products[0].dark_group_id, "dark-2s");
        assert_eq!(result.products[0].flat_group_id, "flat-uvir");
        assert_eq!(result.products[0].mean.to_bits(), 1.0_f64.to_bits());
        assert!(Path::new(&result.products[0].output_path).is_file());
        assert!(result.peak_reserved_bytes > 0);
        assert!(result.peak_reserved_bytes <= result.memory_limit_bytes);
        assert!(progress.iter().all(|event| {
            event.product_index == 0 && event.product_count == 1 && event.group_id == "light-uvir"
        }));

        let calibrated_output = directory.path().join("calibrated");
        fs::create_dir(&calibrated_output)?;
        let mut calibrated_request = light_execution_request(&session, masters, calibrated_output)?;
        calibrated_request.output_mode = LightOutputMode::CalibratedFrames;
        let mut calibrated_progress = Vec::new();
        let calibrated = execute_light_plan_sync(
            &session,
            calibrated_request,
            &CancellationToken::new(),
            |event| calibrated_progress.push(event),
        )?;
        assert_eq!(calibrated.output_mode, "calibrated_frames");
        assert!(calibrated.products.is_empty());
        assert_eq!(calibrated.calibrated_frames.len(), 1);
        assert_eq!(calibrated.calibrated_frames[0].group_id, "light-uvir");
        assert_eq!(calibrated.calibrated_frames[0].source_index, 0);
        let source = session
            .manifest
            .files()
            .iter()
            .find(|file| file.relative_path() == "LIGHTS/light.fits")
            .ok_or("test Light source missing")?;
        let expected_frame_id = FrameId::derive(
            source.relative_path(),
            source.fingerprint().byte_length(),
            source.fingerprint().sha256(),
        )?;
        assert_eq!(
            calibrated.calibrated_frames[0].source_frame_id,
            expected_frame_id.as_str()
        );
        assert_eq!(calibrated.calibrated_frames[0].source_label, "light.fits");
        assert_eq!(calibrated.calibrated_frames[0].source_sha256.len(), 64);
        assert!(Path::new(&calibrated.calibrated_frames[0].output_path).is_file());
        let rgb_path = calibrated.calibrated_frames[0]
            .rgb_output_path
            .as_deref()
            .ok_or("standard CFA Light did not publish an RGB artifact")?;
        let rgb_reader =
            PrimaryImageReader::open(File::open(rgb_path)?, HeaderReadOptions::default())?;
        assert_eq!(rgb_reader.descriptor().axes(), [4, 2, 3]);
        assert!(
            calibrated_progress
                .iter()
                .all(|event| { event.source_index == Some(0) && event.source_count == Some(1) })
        );
        Ok(())
    }

    #[test]
    fn refuses_to_execute_a_plan_other_than_the_reviewed_digest() -> TestResult {
        let directory = TestDirectory::new()?;
        let session_root = directory.path().join("session");
        let output = directory.path().join("masters");
        fs::create_dir(&session_root)?;
        fs::create_dir(&output)?;
        let session = planning_session(&session_root)?;
        let mut request = master_execution_request(
            &session,
            output.clone(),
            FlatPedestalPolicyWire::PreferMatchedDarkThenBias,
        )?;
        request.expected_plan_sha256 = "0".repeat(64);

        let error = execute_master_plan_sync(&session, request, &CancellationToken::new(), |_| {})
            .err()
            .ok_or("a plan with an unreviewed digest was executed")?;

        assert_eq!(error.code, "master_plan_stale");
        assert_eq!(fs::read_dir(output)?.count(), 0);
        Ok(())
    }

    #[test]
    fn cancellation_publishes_no_desktop_master_products() -> TestResult {
        let directory = TestDirectory::new()?;
        let session_root = directory.path().join("session");
        let output = directory.path().join("masters");
        fs::create_dir(&session_root)?;
        fs::create_dir(&output)?;
        let session = planning_session(&session_root)?;
        let cancellation = CancellationToken::new();
        assert!(cancellation.cancel());

        let error = execute_master_plan_sync(
            &session,
            master_execution_request(
                &session,
                output.clone(),
                FlatPedestalPolicyWire::RequireMatchedDark,
            )?,
            &cancellation,
            |_| {},
        )
        .err()
        .ok_or("cancelled master execution succeeded")?;

        assert_eq!(error.code, "master_execution_cancelled");
        assert_eq!(fs::read_dir(output)?.count(), 0);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn non_unicode_master_destination_is_rejected_before_writing() -> TestResult {
        let directory = TestDirectory::new()?;
        let session_root = directory.path().join("session");
        fs::create_dir(&session_root)?;
        let session = planning_session(&session_root)?;
        let output = directory
            .path()
            .join(std::ffi::OsString::from_vec(vec![b'm', 0xff, b's']));

        let error = execute_master_plan_sync(
            &session,
            master_execution_request(
                &session,
                output.clone(),
                FlatPedestalPolicyWire::RequireMatchedDark,
            )?,
            &CancellationToken::new(),
            |_| {},
        )
        .err()
        .ok_or("non-Unicode master destination was accepted")?;

        assert_eq!(error.code, "master_output_path_not_unicode");
        assert!(!output.exists());
        Ok(())
    }

    #[test]
    fn desktop_calibration_execution_slot_is_exclusive_and_reusable() -> TestResult {
        let state = DesktopCalibrationExecutionState::default();
        let first = begin_calibration_execution(&state)?;
        let busy = begin_calibration_execution(&state)
            .err()
            .ok_or("concurrent calibration execution was accepted")?;
        assert_eq!(busy.code, "calibration_execution_busy");
        assert!(first.cancel());
        assert!(
            lock_calibration_execution(&state)?
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled)
        );
        finish_calibration_execution(&state)?;
        let second = begin_calibration_execution(&state)?;
        assert!(!second.is_cancelled());
        finish_calibration_execution(&state)?;
        Ok(())
    }

    #[test]
    fn rejects_invalid_master_tolerances_before_planning() -> TestResult {
        let directory = TestDirectory::new()?;
        let session = planning_session(directory.path())?;
        let error = preview_master_plan_sync(
            &session,
            MasterPlanPreviewRequest {
                flat_pedestal_policy: FlatPedestalPolicyWire::RequireMatchedDark,
                maximum_exposure_delta_seconds: -0.01,
                maximum_temperature_delta_c: 1.0,
                maximum_light_dark_temperature_delta_c: 2.0,
            },
        )
        .err()
        .ok_or("negative plan tolerance was accepted")?;

        assert_eq!(error.code, "master_plan_options_invalid");

        let error = preview_master_plan_sync(
            &session,
            MasterPlanPreviewRequest {
                flat_pedestal_policy: FlatPedestalPolicyWire::RequireMatchedDark,
                maximum_exposure_delta_seconds: 0.01,
                maximum_temperature_delta_c: 1.0,
                maximum_light_dark_temperature_delta_c: f64::NAN,
            },
        )
        .err()
        .ok_or("non-finite Light-to-Dark tolerance was accepted")?;
        assert_eq!(error.code, "light_plan_options_invalid");
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
    fn estimates_one_linked_transform_for_planar_rgb() -> TestResult {
        let mut request = estimate_request();
        request.content = FitsPreviewContent::Rgb;
        let estimate =
            estimate_fits_preview_transform_from_reader(Cursor::new(rgb_fits_bytes()?), &request)?;

        assert_eq!(estimate.algorithm_id, AUTO_STRETCH_ALGORITHM_ID);
        assert_eq!(estimate.finite_samples, 8);
        assert!(estimate.black_point < estimate.median);
        assert!(estimate.white_point > estimate.median);
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
            frame_id: "a".repeat(64),
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
        assert!(quality.signal_to_noise.is_some_and(|value| value > 0.0));
        assert!(quality.fwhm_pixels.is_some_and(|value| value > 10.0));
        assert!(quality.eccentricity.is_some_and(|value| value < 0.2));
        Ok(())
    }

    #[test]
    fn measures_planar_rgb_on_linked_linear_luminance() -> TestResult {
        let directory = TestDirectory::new()?;
        let path = directory.path().join("quality-rgb.fits");
        fs::write(&path, rgb_quality_fits_bytes()?)?;

        let quality = inspect_frame_quality_sync(&FrameQualityRequest {
            frame_id: "a".repeat(64),
            path,
            interpretation: QualityInterpretation::RgbLuminance,
        })?;

        assert_eq!(
            quality.detection_plane_algorithm_id,
            RGB_LUMINANCE_ALGORITHM_ID
        );
        assert_eq!(
            quality.interpretation,
            "calibrated RGB · linear Rec. 709 luminance"
        );
        assert_eq!(quality.source_pixel_scale.to_bits(), 1.0_f64.to_bits());
        assert!(quality.detected_stars > 0);
        assert_eq!(quality.usable_stars, quality.detected_stars);
        assert!(quality.signal_to_noise.is_some_and(|value| value > 0.0));
        assert!(quality.fwhm_pixels.is_some_and(|value| value > 0.0));
        Ok(())
    }

    #[test]
    fn sorts_review_rows_in_rust_with_missing_metrics_last() -> TestResult {
        let frame = |digit: char, label: &str, fwhm_pixels: Option<f64>| ReviewSortFrame {
            id: digit.to_string().repeat(64),
            label: label.to_owned(),
            signal_to_noise: None,
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
            quality_evidence_restored: 0,
            quality_evidence_missing: 0,
            quality_evidence_rejected: 0,
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

    #[test]
    fn previews_selection_in_native_order_without_mutating_manual_state() -> TestResult {
        let imported = ImportedSession {
            name: "selection test".to_owned(),
            root_path: "/runtime-only".to_owned(),
            frames: vec![
                imported_review_test_frame('a', "first.fits"),
                imported_review_test_frame('b', "second.fits"),
            ],
            files_considered: 2,
            classification_conflicts: 0,
            recoverable_failures: Vec::new(),
            unassigned_sources: Vec::new(),
            quality_evidence_restored: 0,
            quality_evidence_missing: 0,
            quality_evidence_rejected: 0,
        };
        let state = DesktopReviewState::default();
        install_review_book(&state, &imported)?;
        install_selection_test_quality(&state, 'a', 3.0, 700)?;
        install_selection_test_quality(&state, 'b', 5.0, 300)?;
        apply_review_decision_sync(
            &state,
            ReviewDecisionRequest {
                frame_id: "a".repeat(64),
                action: ReviewDecisionAction::Accept,
            },
        )?;

        let plan = preview_frame_selection_sync(
            &state,
            FrameSelectionPreviewRequest {
                // Deliberately reversed: the native review book owns processing order.
                frames: vec![selection_test_frame('b'), selection_test_frame('a')],
                rules: vec![FrameSelectionRuleWire {
                    metric: FrameSelectionMetricWire::FwhmPixels,
                    comparator: FrameSelectionComparatorWire::LessThan,
                    threshold: FrameSelectionThresholdWire::Scalar(4.0),
                    missing_policy: MissingMetricPolicyWire::Reject,
                }],
            },
        )?;

        assert_eq!(plan.frames()[0].frame_id().as_str(), "a".repeat(64));
        assert_eq!(plan.frames()[1].frame_id().as_str(), "b".repeat(64));
        assert_eq!(
            plan.frames()[0].proposal(),
            aether_review::FrameSelectionProposal::Retain
        );
        assert_eq!(
            plan.frames()[1].proposal(),
            aether_review::FrameSelectionProposal::Reject
        );
        let native = lock_review_state(&state)?;
        let book = native.as_ref().ok_or("missing review book")?;
        assert_eq!(book.generation(), 1);
        assert_eq!(
            book.state(&FrameId::new("a".repeat(64))?),
            Some(ReviewState::Accepted)
        );

        let json = serde_json::to_value(&plan)?;
        assert_eq!(json["schemaVersion"], 1);
        assert_eq!(json["frames"][0]["frameId"], "a".repeat(64));
        assert!(json.get("schema_version").is_none());
        Ok(())
    }

    #[test]
    fn selection_preview_rejects_foreign_identity_and_wrong_threshold_kind() -> TestResult {
        let imported = ImportedSession {
            name: "selection validation".to_owned(),
            root_path: "/runtime-only".to_owned(),
            frames: vec![imported_review_test_frame('a', "first.fits")],
            files_considered: 1,
            classification_conflicts: 0,
            recoverable_failures: Vec::new(),
            unassigned_sources: Vec::new(),
            quality_evidence_restored: 0,
            quality_evidence_missing: 0,
            quality_evidence_rejected: 0,
        };
        let state = DesktopReviewState::default();
        install_review_book(&state, &imported)?;

        let Err(foreign) = preview_frame_selection_sync(
            &state,
            FrameSelectionPreviewRequest {
                frames: vec![selection_test_frame('b')],
                rules: vec![selection_test_fwhm_rule()],
            },
        ) else {
            return Err("a foreign frame identity was accepted".into());
        };
        assert_eq!(foreign.code, "frame_selection_input_invalid");

        let Err(missing) = preview_frame_selection_sync(
            &state,
            FrameSelectionPreviewRequest {
                frames: vec![selection_test_frame('a')],
                rules: vec![selection_test_fwhm_rule()],
            },
        ) else {
            return Err("selection without native quality evidence was accepted".into());
        };
        assert_eq!(missing.code, "frame_selection_evidence_missing");

        let Err(wrong_kind) = preview_frame_selection_sync(
            &state,
            FrameSelectionPreviewRequest {
                frames: vec![selection_test_frame('a')],
                rules: vec![FrameSelectionRuleWire {
                    metric: FrameSelectionMetricWire::DetectedStars,
                    comparator: FrameSelectionComparatorWire::GreaterThan,
                    threshold: FrameSelectionThresholdWire::Scalar(500.0),
                    missing_policy: MissingMetricPolicyWire::Reject,
                }],
            },
        ) else {
            return Err("a scalar star-count threshold was accepted".into());
        };
        assert_eq!(wrong_kind.code, "frame_selection_input_invalid");
        Ok(())
    }

    #[test]
    fn confirmed_selection_applies_once_preserves_manual_decisions_and_undoes_atomically()
    -> TestResult {
        let imported = ImportedSession {
            name: "selection apply".to_owned(),
            root_path: "/runtime-only".to_owned(),
            frames: vec![
                imported_review_test_frame('a', "first.fits"),
                imported_review_test_frame('b', "second.fits"),
                imported_review_test_frame('c', "third.fits"),
            ],
            files_considered: 3,
            classification_conflicts: 0,
            recoverable_failures: Vec::new(),
            unassigned_sources: Vec::new(),
            quality_evidence_restored: 0,
            quality_evidence_missing: 0,
            quality_evidence_rejected: 0,
        };
        let state = DesktopReviewState::default();
        install_review_book(&state, &imported)?;
        install_selection_test_quality(&state, 'a', 3.0, 700)?;
        install_selection_test_quality(&state, 'b', 5.0, 300)?;
        install_selection_test_quality(&state, 'c', 2.5, 800)?;
        apply_review_decision_sync(
            &state,
            ReviewDecisionRequest {
                frame_id: "c".repeat(64),
                action: ReviewDecisionAction::Reject {
                    reason: ReviewRejectionReasonWire::Trailing,
                },
            },
        )?;

        let plan = preview_frame_selection_sync(
            &state,
            FrameSelectionPreviewRequest {
                frames: vec![
                    selection_test_frame('a'),
                    selection_test_frame('b'),
                    selection_test_frame('c'),
                ],
                rules: vec![selection_test_fwhm_rule()],
            },
        )?;
        let update = apply_frame_selection_sync(
            &state,
            FrameSelectionApplyRequest {
                frames: vec![
                    selection_test_frame('c'),
                    selection_test_frame('b'),
                    selection_test_frame('a'),
                ],
                rules: vec![selection_test_fwhm_rule()],
                plan_sha256: plan.plan_sha256().to_owned(),
            },
        )?;

        assert_eq!(update.generation, 2);
        assert!(update.can_undo);
        assert_eq!(update.changes.len(), 2);
        let native = lock_review_state(&state)?;
        let book = native.as_ref().ok_or("missing review book")?;
        assert_eq!(
            book.state(&FrameId::new("a".repeat(64))?),
            Some(ReviewState::Accepted)
        );
        assert_eq!(
            book.decision(&FrameId::new("b".repeat(64))?)
                .and_then(ManualDecision::rejection_reason),
            Some(ManualRejectionReason::QualityRules)
        );
        assert_eq!(
            book.decision(&FrameId::new("c".repeat(64))?)
                .and_then(ManualDecision::rejection_reason),
            Some(ManualRejectionReason::Trailing)
        );
        drop(native);

        let undone = undo_review_decision_sync(&state)?;
        assert_eq!(undone.changes.len(), 2);
        let native = lock_review_state(&state)?;
        let book = native.as_ref().ok_or("missing review book")?;
        assert_eq!(
            book.state(&FrameId::new("a".repeat(64))?),
            Some(ReviewState::Undecided)
        );
        assert_eq!(
            book.state(&FrameId::new("b".repeat(64))?),
            Some(ReviewState::Undecided)
        );
        assert_eq!(
            book.state(&FrameId::new("c".repeat(64))?),
            Some(ReviewState::Rejected)
        );
        Ok(())
    }

    #[test]
    fn confirmed_selection_rejects_a_stale_digest_without_mutation() -> TestResult {
        let imported = ImportedSession {
            name: "stale selection".to_owned(),
            root_path: "/runtime-only".to_owned(),
            frames: vec![imported_review_test_frame('a', "first.fits")],
            files_considered: 1,
            classification_conflicts: 0,
            recoverable_failures: Vec::new(),
            unassigned_sources: Vec::new(),
            quality_evidence_restored: 0,
            quality_evidence_missing: 0,
            quality_evidence_rejected: 0,
        };
        let state = DesktopReviewState::default();
        install_review_book(&state, &imported)?;
        install_selection_test_quality(&state, 'a', 3.0, 700)?;

        let Err(error) = apply_frame_selection_sync(
            &state,
            FrameSelectionApplyRequest {
                frames: vec![selection_test_frame('a')],
                rules: vec![selection_test_fwhm_rule()],
                plan_sha256: "f".repeat(64),
            },
        ) else {
            return Err("a stale selection plan was applied".into());
        };
        assert_eq!(error.code, "frame_selection_plan_stale");
        let native = lock_review_state(&state)?;
        let book = native.as_ref().ok_or("missing review book")?;
        assert_eq!(book.generation(), 0);
        assert_eq!(
            book.state(&FrameId::new("a".repeat(64))?),
            Some(ReviewState::Undecided)
        );
        assert!(!book.can_undo());
        Ok(())
    }

    fn selection_test_frame(digit: char) -> FrameSelectionFrameWire {
        FrameSelectionFrameWire {
            frame_id: digit.to_string().repeat(64),
            source_path: PathBuf::from(format!("/runtime-only/LIGHTS/{digit}.fits")),
        }
    }

    fn install_selection_test_quality(
        state: &DesktopReviewState,
        digit: char,
        fwhm_pixels: f64,
        usable_stars: usize,
    ) -> TestResult {
        let response = selection_test_quality_response(fwhm_pixels, usable_stars);
        record_frame_quality(
            state,
            FrameId::new(digit.to_string().repeat(64))?,
            PathBuf::from(format!("/runtime-only/LIGHTS/{digit}.fits")),
            &response,
        )?;
        Ok(())
    }

    fn selection_test_quality_response(
        fwhm_pixels: f64,
        usable_stars: usize,
    ) -> FrameQualityResponse {
        FrameQualityResponse {
            profile_id: DESKTOP_QUALITY_PROFILE_ID.to_owned(),
            background_algorithm_id: GLOBAL_BACKGROUND_ALGORITHM_ID.to_owned(),
            star_algorithm_id: STAR_MEASUREMENT_ALGORITHM_ID.to_owned(),
            detection_plane_algorithm_id: CFA_CELL_MEAN_ALGORITHM_ID.to_owned(),
            interpretation: "raw CFA · RGGB".to_owned(),
            source_pixel_scale: 2.0,
            diagnostic_only: true,
            background: 1_000.0,
            noise: 12.0,
            initial_usable_samples: 10_000,
            retained_background_samples: 9_000,
            masked_samples: 0,
            non_finite_samples: 0,
            detected_stars: usable_stars + 20,
            usable_stars,
            saturation_level: None,
            saturated_stars: None,
            raw_candidates: usable_stars + 50,
            suppressed_candidates: 10,
            rejected_measurements: 20,
            signal_to_noise: Some(30.0),
            fwhm_pixels: Some(fwhm_pixels),
            eccentricity: Some(0.4),
        }
    }

    const fn selection_test_fwhm_rule() -> FrameSelectionRuleWire {
        FrameSelectionRuleWire {
            metric: FrameSelectionMetricWire::FwhmPixels,
            comparator: FrameSelectionComparatorWire::LessThan,
            threshold: FrameSelectionThresholdWire::Scalar(4.0),
            missing_policy: MissingMetricPolicyWire::Reject,
        }
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
            source_byte_length: 1,
            source_sha256: digit.to_string().repeat(64),
            quality: None,
        }
    }

    fn cached_quality_test_frame(digit: char, label: &str) -> TestResult<ImportedFrame> {
        let mut frame = imported_review_test_frame(digit, label);
        frame.id = FrameId::derive(
            &frame.relative_path,
            frame.source_byte_length,
            &frame.source_sha256,
        )?
        .as_str()
        .to_owned();
        Ok(frame)
    }

    #[test]
    fn quality_evidence_cache_round_trips_and_rejects_a_changed_source() -> TestResult {
        let directory = TestDirectory::new()?;
        let frame = cached_quality_test_frame('a', "first.fits")?;
        let frame_id = FrameId::new(frame.id.clone())?;
        let response = selection_test_quality_response(3.25, 720);

        publish_quality_evidence(
            directory.path(),
            &frame_id,
            frame.source_byte_length,
            &frame.source_sha256,
            &response,
        )?;
        assert_eq!(
            restore_quality_evidence(directory.path(), &frame),
            QualityEvidenceRestore::Restored(Box::new(response))
        );

        let mut changed = ImportedFrame {
            source_byte_length: 2,
            ..cached_quality_test_frame('a', "first.fits")?
        };
        changed.id = FrameId::derive(
            &changed.relative_path,
            changed.source_byte_length,
            &changed.source_sha256,
        )?
        .as_str()
        .to_owned();
        assert_eq!(
            restore_quality_evidence(directory.path(), &changed),
            QualityEvidenceRestore::Missing
        );
        Ok(())
    }

    #[test]
    fn quality_evidence_cache_fails_closed_on_corruption_and_algorithm_drift() -> TestResult {
        let directory = TestDirectory::new()?;
        let frame = cached_quality_test_frame('b', "second.fits")?;
        let frame_id = FrameId::new(frame.id.clone())?;
        let response = selection_test_quality_response(4.0, 600);
        publish_quality_evidence(
            directory.path(),
            &frame_id,
            frame.source_byte_length,
            &frame.source_sha256,
            &response,
        )?;
        let key = quality_evidence_key(&frame_id, frame.source_byte_length, &frame.source_sha256)?;
        let artifact_path = directory
            .path()
            .join(&key.as_str()[..2])
            .join(format!("{}.artifact", key.as_str()));
        let mut artifact = OpenOptions::new().write(true).open(artifact_path)?;
        artifact.seek(SeekFrom::Start(0))?;
        artifact.write_all(b"X")?;
        artifact.sync_all()?;
        assert_eq!(
            restore_quality_evidence(directory.path(), &frame),
            QualityEvidenceRestore::Rejected
        );

        let mut drifted = selection_test_quality_response(4.0, 600);
        drifted.star_algorithm_id = "future-star-model-v2".to_owned();
        let Err(error) = validate_quality_response(&drifted) else {
            return Err("algorithm drift restored persisted quality evidence".into());
        };
        assert_eq!(error.code, "frame_quality_result_invalid");
        Ok(())
    }

    #[test]
    fn session_quality_restore_counts_only_eligible_cache_outcomes() -> TestResult {
        let directory = TestDirectory::new()?;
        let restored = cached_quality_test_frame('a', "restored.fits")?;
        let missing = cached_quality_test_frame('b', "missing.fits")?;
        let rejected = cached_quality_test_frame('c', "rejected.fits")?;
        let response = selection_test_quality_response(3.25, 720);

        for frame in [&restored, &rejected] {
            publish_quality_evidence(
                directory.path(),
                &FrameId::new(frame.id.clone())?,
                frame.source_byte_length,
                &frame.source_sha256,
                &response,
            )?;
        }
        let rejected_key = quality_evidence_key(
            &FrameId::new(rejected.id.clone())?,
            rejected.source_byte_length,
            &rejected.source_sha256,
        )?;
        let rejected_path = directory
            .path()
            .join(&rejected_key.as_str()[..2])
            .join(format!("{}.artifact", rejected_key.as_str()));
        let mut artifact = OpenOptions::new().write(true).open(rejected_path)?;
        artifact.seek(SeekFrom::Start(0))?;
        artifact.write_all(b"X")?;
        artifact.sync_all()?;

        let mut ineligible = cached_quality_test_frame('d', "master-dark.fits")?;
        ineligible.role = "dark";
        ineligible.bayer_pattern = None;
        let mut session = ImportedSession {
            name: "cache diagnostics".to_owned(),
            root_path: "/runtime-only".to_owned(),
            frames: vec![restored, missing, rejected, ineligible],
            files_considered: 4,
            classification_conflicts: 0,
            recoverable_failures: Vec::new(),
            unassigned_sources: Vec::new(),
            quality_evidence_restored: usize::MAX,
            quality_evidence_missing: usize::MAX,
            quality_evidence_rejected: usize::MAX,
        };

        restore_session_quality_evidence(directory.path(), &mut session);

        assert_eq!(session.quality_evidence_restored, 1);
        assert_eq!(session.quality_evidence_missing, 1);
        assert_eq!(session.quality_evidence_rejected, 1);
        assert!(session.frames[0].quality.is_some());
        assert!(
            session.frames[1..]
                .iter()
                .all(|frame| frame.quality.is_none())
        );
        Ok(())
    }

    #[test]
    fn restored_quality_populates_native_selection_evidence_by_exact_artifact() -> TestResult {
        let mut frame = imported_review_test_frame('c', "third.fits");
        frame.quality = Some(selection_test_quality_response(3.5, 640));
        let expected_path = PathBuf::from(&frame.path);
        let expected_id = FrameId::new(frame.id.clone())?;
        let session = ImportedSession {
            name: "restored quality".to_owned(),
            root_path: "/runtime-only".to_owned(),
            frames: vec![frame],
            files_considered: 1,
            classification_conflicts: 0,
            recoverable_failures: Vec::new(),
            unassigned_sources: Vec::new(),
            quality_evidence_restored: 1,
            quality_evidence_missing: 0,
            quality_evidence_rejected: 0,
        };

        let restored = prepare_restored_quality(&session)?;
        let metrics = restored
            .get(&(expected_id, expected_path))
            .ok_or("restored evidence was not indexed by its exact artifact")?;
        assert_eq!(metrics.fwhm_major_pixels(), Some(3.5));
        assert_eq!(metrics.usable_stars(), Some(640));
        Ok(())
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
            content: FitsPreviewContent::Scalar { plane: 0 },
            maximum_width: 800,
            maximum_height: 600,
        };
        let estimate = estimate_fits_preview_transform_from_reader(
            File::open(&estimate_request.path)?,
            &estimate_request,
        )?;
        let transform = FitsPreviewRequest {
            path: estimate_request.path.clone(),
            content: FitsPreviewContent::Scalar { plane: 0 },
            maximum_width: 800,
            maximum_height: 600,
            black_point: estimate.black_point,
            white_point: estimate.white_point,
            midtone: estimate.midtone,
            transfer: PreviewTransfer::Midtones,
            palette: PreviewPalette::Grayscale,
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
            frame_id: "a".repeat(64),
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
