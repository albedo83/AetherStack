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

use aether_fits::{
    DEFAULT_STATISTICS_CHUNK_SAMPLES, FITS_STATISTICS_ALGORITHM_ID, HeaderReadOptions,
    PrimaryImageReader, StoredSampleFormat, primary_image_statistics,
};
use aether_metadata::FrameType;
use aether_preview::{
    AUTO_STRETCH_ALGORITHM_ID, AutomaticDisplayTransform, FitsPreviewParameters, MissingPixelStyle,
    PreviewLimits, RgbaPreview, ScalarPreview, build_fits_preview, choose_reduction_level,
    estimate_display_transform, render_grayscale_rgba8,
};
use aether_review::{
    DisplayTransform, FrameId, FrameMetrics, FrameSpec, MissingPlacement, ReviewBook,
    SortDirection as ReviewSortDirection, SortField as ReviewSortField, SortSpec, TransferFunction,
};
use aether_session::{
    ClassificationPolicy, DirectoryManifestOptions, DirectoryManifestReport, ManifestFile,
    generate_manifest_from_directory,
};
use serde::{Deserialize, Serialize};
use tauri::ipc::Response;

const MAX_DESKTOP_PREVIEW_PIXELS: usize = 2 * 1_024 * 1_024;
const DESKTOP_PREVIEW_IO_CHUNK_SAMPLES: usize = 256 * 1_024;

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
async fn import_session_directory(path: PathBuf) -> Result<ImportedSession, PreviewCommandError> {
    if !path.is_absolute() {
        return Err(PreviewCommandError::new(
            "session_path_not_absolute",
            "The selected session directory must use an absolute path.",
        ));
    }
    tauri::async_runtime::spawn_blocking(move || import_session_directory_sync(&path))
        .await
        .map_err(|_| {
            PreviewCommandError::new(
                "session_import_interrupted",
                "The session import worker stopped before producing a result.",
            )
        })?
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
        axes: file.axes().to_vec(),
        fits_diagnostic_count: file.fits_diagnostics().len(),
        classification_conflict: file.classification().has_conflict(),
    }))
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
        .invoke_handler(tauri::generate_handler![
            estimate_fits_preview_transform,
            import_session_directory,
            inspect_fits_statistics,
            render_fits_preview,
            sort_review_frames
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
        if let Some(output) = std::env::var_os("AETHERSTACK_TEST_PREVIEW_OUTPUT") {
            fs::write(output, encoded)?;
        }
        Ok(())
    }
}
