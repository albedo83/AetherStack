//! End-to-end, inspectable registration diagnostics for raw astronomical FITS frames.
//!
//! This crate deliberately keeps file-system locations out of its serializable
//! reports. Inputs are identified by their content digests, so a diagnostic can
//! be shared without publishing an observer name, target name, or directory
//! layout. Every scientific stage is deterministic and bounded by the versioned
//! [`PRECISION_DIAGNOSTIC_PROFILE_ID`] profile.

use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use aether_core::ScientificImage;
use aether_fits::{HeaderReadOptions, ImageRegion, PrimaryImageReader, ValidationMode};
use aether_quality::{
    BackgroundParameters, CFA_CELL_MEAN_ALGORITHM_ID, GLOBAL_BACKGROUND_ALGORITHM_ID,
    STAR_MEASUREMENT_ALGORITHM_ID, StarMeasurementParameters, measure_frame_quality,
    prepare_cfa_cell_mean,
};
use aether_registration::{
    DESCRIPTOR_MATCH_ALGORITHM_ID, DescriptorMatchParameters, FEATURE_CATALOG_ALGORITHM_ID,
    FeatureCatalog, FeatureSelectionParameters, REGISTRATION_CONFIDENCE_ALGORITHM_ID,
    ReflectionPolicy, RegistrationConfidenceParameters, RegistrationConfidenceRejection,
    SIMILARITY_CONSENSUS_ALGORITHM_ID, SimilarityConsensusParameters,
    TRIANGLE_DESCRIPTOR_ALGORITHM_ID, TriangleDescriptorParameters, assess_registration_confidence,
    build_feature_catalog, build_triangle_descriptors, estimate_similarity_consensus,
    match_triangle_descriptors,
};
use aether_review::FrameId;
use aether_session::fingerprint_reader;
use serde::Serialize;

/// Versioned parameters used by this first real-corpus precision probe.
///
/// This is intentionally named a diagnostic profile: its measurements establish
/// evidence for later production defaults, rather than silently declaring them.
pub const PRECISION_DIAGNOSTIC_PROFILE_ID: &str = "raw-cfa-registration-precision-v1";

/// Maximum decoded source samples accepted by one diagnostic input.
pub const MAX_DIAGNOSTIC_SOURCE_SAMPLES: usize = 100_000_000;

/// Complete path-free report emitted for one source/reference comparison.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RegistrationDiagnostic {
    schema_version: u32,
    profile_id: &'static str,
    diagnostic_only: bool,
    algorithms: AlgorithmSummary,
    source: FrameSummary,
    reference: FrameSummary,
    matching: MatchingSummary,
    consensus: ConsensusSummary,
    confidence: ConfidenceSummary,
}

impl RegistrationDiagnostic {
    /// Whether the result passed every conservative confidence check.
    #[must_use]
    pub const fn accepted(&self) -> bool {
        self.confidence.accepted
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct AlgorithmSummary {
    detection_plane: &'static str,
    background: &'static str,
    stars: &'static str,
    features: &'static str,
    triangles: &'static str,
    descriptor_matching: &'static str,
    consensus: &'static str,
    confidence: &'static str,
}

/// Aggregate measurements for one input. No source path or FITS metadata is retained.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FrameSummary {
    content_sha256: String,
    source_width: usize,
    source_height: usize,
    detection_width: usize,
    detection_height: usize,
    header_conformant: bool,
    header_diagnostics: usize,
    background: f64,
    noise_sigma: f64,
    detected_stars: usize,
    median_fwhm_source_pixels: Option<f64>,
    median_eccentricity: Option<f64>,
    registration_features: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct MatchingSummary {
    source_descriptors: usize,
    reference_descriptors: usize,
    comparisons: usize,
    geometric_candidates: usize,
    retained_hypotheses: usize,
    ambiguous_source_descriptors: usize,
    truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct ConsensusSummary {
    transform_coefficients: [f64; 6],
    scale: f64,
    rotation_radians: f64,
    reflected: bool,
    inlier_hypotheses: usize,
    inlier_feature_pairs: usize,
    rms_residual_detection_pixels: f64,
    maximum_residual_detection_pixels: f64,
    competing_model_support: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct ConfidenceSummary {
    accepted: bool,
    inlier_ratio: f64,
    winner_support_margin: Option<usize>,
    source_axis_span_fraction: [f64; 2],
    reference_axis_span_fraction: [f64; 2],
    rejections: Vec<&'static str>,
}

/// Failure from a named, path-free diagnostic stage.
#[derive(Debug)]
pub struct RegistrationDiagnosticError {
    stage: &'static str,
    detail: String,
}

impl RegistrationDiagnosticError {
    fn new(stage: &'static str, error: impl Display) -> Self {
        Self {
            stage,
            detail: error.to_string(),
        }
    }

    fn message(stage: &'static str, detail: impl Into<String>) -> Self {
        Self {
            stage,
            detail: detail.into(),
        }
    }
}

impl Display for RegistrationDiagnosticError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.stage, self.detail)
    }
}

impl Error for RegistrationDiagnosticError {}

struct PreparedFrame {
    features: FeatureCatalog,
    summary: FrameSummary,
}

struct DiagnosticProfile {
    quality: StarMeasurementParameters,
    features: FeatureSelectionParameters,
    triangles: TriangleDescriptorParameters,
    matching: DescriptorMatchParameters,
    consensus: SimilarityConsensusParameters,
    confidence: RegistrationConfidenceParameters,
}

impl DiagnosticProfile {
    fn precision() -> Result<Self, RegistrationDiagnosticError> {
        let background = BackgroundParameters::new(3.0, 8, 100)
            .map_err(|error| RegistrationDiagnosticError::new("profile", error))?;
        let quality = StarMeasurementParameters::new(background, 6.0, 2.0, 8, 4, 6, 100_000, None)
            .map_err(|error| RegistrationDiagnosticError::new("profile", error))?;
        let features = FeatureSelectionParameters::new(10.0, 0.95, 12.0, 4_096)
            .map_err(|error| RegistrationDiagnosticError::new("profile", error))?;
        // Local neighborhoods limit repeated asterisms in dense fields while
        // retaining broad spatial coverage through many independent anchors.
        let triangles = TriangleDescriptorParameters::new(256, 8, 4.0, 0.005, 8_000)
            .map_err(|error| RegistrationDiagnosticError::new("profile", error))?;
        let matching = DescriptorMatchParameters::new(
            0.003,
            0.003,
            0.95,
            1.05,
            ReflectionPolicy::Forbid,
            16,
            5_000,
            50_000_000,
        )
        .map_err(|error| RegistrationDiagnosticError::new("profile", error))?;
        // Matching and model limits are equal so normal evidence is never
        // silently truncated; their product remains below the evaluation bound.
        let consensus = SimilarityConsensusParameters::new(1.5, 3, 6, 3.0, 5_000, 100_000_000)
            .map_err(|error| RegistrationDiagnosticError::new("profile", error))?;
        let confidence = RegistrationConfidenceParameters::new(
            5, 0.05, 12, 2, 0.75, 1.5, 0.15, 0.15, false, false,
        )
        .map_err(|error| RegistrationDiagnosticError::new("profile", error))?;
        Ok(Self {
            quality,
            features,
            triangles,
            matching,
            consensus,
            confidence,
        })
    }
}

/// Runs the complete diagnostic on two raw single-plane CFA FITS files.
///
/// Neither path is copied into the returned report or into errors produced after
/// opening the files. The source transform maps source detection-plane coordinates
/// into the reference detection-plane coordinate system.
pub fn diagnose_paths(
    source_path: &Path,
    reference_path: &Path,
) -> Result<RegistrationDiagnostic, RegistrationDiagnosticError> {
    let source = File::open(source_path)
        .map_err(|error| RegistrationDiagnosticError::new("open source input", error))?;
    let reference = File::open(reference_path)
        .map_err(|error| RegistrationDiagnosticError::new("open reference input", error))?;
    diagnose_readers(source, reference)
}

/// Reader-based form used by deterministic tests and non-filesystem front ends.
pub fn diagnose_readers<S, R>(
    source: S,
    reference: R,
) -> Result<RegistrationDiagnostic, RegistrationDiagnosticError>
where
    S: Read + Seek,
    R: Read + Seek,
{
    let profile = DiagnosticProfile::precision()?;
    let source = prepare_frame(source, "source", &profile)?;
    let reference = prepare_frame(reference, "reference", &profile)?;

    let source_descriptors = build_triangle_descriptors(&source.features, profile.triangles)
        .map_err(|error| RegistrationDiagnosticError::new("source descriptors", error))?;
    let reference_descriptors = build_triangle_descriptors(&reference.features, profile.triangles)
        .map_err(|error| RegistrationDiagnosticError::new("reference descriptors", error))?;
    let matches = match_triangle_descriptors(
        &source_descriptors,
        &reference_descriptors,
        profile.matching,
    )
    .map_err(|error| RegistrationDiagnosticError::new("descriptor matching", error))?;
    let consensus = estimate_similarity_consensus(
        &source.features,
        &reference.features,
        &matches,
        profile.consensus,
    )
    .map_err(|error| RegistrationDiagnosticError::new("similarity consensus", error))?;
    let confidence = assess_registration_confidence(
        &source.features,
        &reference.features,
        &consensus,
        profile.confidence,
    )
    .map_err(|error| RegistrationDiagnosticError::new("confidence", error))?;

    let match_statistics = matches.statistics();
    let consensus_statistics = consensus.statistics();
    let residuals = consensus.residual_statistics();
    let truncated = matches.source_descriptor_catalog_truncated()
        || matches.reference_descriptor_catalog_truncated()
        || match_statistics.discarded_by_global_limit() > 0
        || consensus_statistics.models_discarded_by_limit() > 0;
    let rejections = confidence
        .rejections()
        .iter()
        .copied()
        .map(rejection_name)
        .collect();

    Ok(RegistrationDiagnostic {
        schema_version: 1,
        profile_id: PRECISION_DIAGNOSTIC_PROFILE_ID,
        diagnostic_only: true,
        algorithms: AlgorithmSummary {
            detection_plane: CFA_CELL_MEAN_ALGORITHM_ID,
            background: GLOBAL_BACKGROUND_ALGORITHM_ID,
            stars: STAR_MEASUREMENT_ALGORITHM_ID,
            features: FEATURE_CATALOG_ALGORITHM_ID,
            triangles: TRIANGLE_DESCRIPTOR_ALGORITHM_ID,
            descriptor_matching: DESCRIPTOR_MATCH_ALGORITHM_ID,
            consensus: SIMILARITY_CONSENSUS_ALGORITHM_ID,
            confidence: REGISTRATION_CONFIDENCE_ALGORITHM_ID,
        },
        source: source.summary,
        reference: reference.summary,
        matching: MatchingSummary {
            source_descriptors: source_descriptors.descriptors().len(),
            reference_descriptors: reference_descriptors.descriptors().len(),
            comparisons: match_statistics.comparisons(),
            geometric_candidates: match_statistics.geometric_candidates(),
            retained_hypotheses: matches.hypotheses().len(),
            ambiguous_source_descriptors: match_statistics.ambiguous_source_descriptors(),
            truncated,
        },
        consensus: ConsensusSummary {
            transform_coefficients: consensus.transform().coefficients(),
            scale: consensus.scale(),
            rotation_radians: consensus.rotation_radians(),
            reflected: consensus.reflected(),
            inlier_hypotheses: consensus_statistics.refined_inlier_hypotheses(),
            inlier_feature_pairs: consensus.inlier_feature_pairs().len(),
            rms_residual_detection_pixels: residuals.root_mean_square_pixels(),
            maximum_residual_detection_pixels: residuals.maximum_pixels(),
            competing_model_support: consensus
                .competing_similarity()
                .map(|candidate| candidate.inlier_hypotheses()),
        },
        confidence: ConfidenceSummary {
            accepted: confidence.accepted(),
            inlier_ratio: confidence.inlier_ratio(),
            winner_support_margin: confidence.winner_support_margin(),
            source_axis_span_fraction: [
                confidence.source_horizontal_span_fraction(),
                confidence.source_vertical_span_fraction(),
            ],
            reference_axis_span_fraction: [
                confidence.reference_horizontal_span_fraction(),
                confidence.reference_vertical_span_fraction(),
            ],
            rejections,
        },
    })
}

fn prepare_frame<R: Read + Seek>(
    mut input: R,
    role: &'static str,
    profile: &DiagnosticProfile,
) -> Result<PreparedFrame, RegistrationDiagnosticError> {
    input
        .seek(SeekFrom::Start(0))
        .map_err(|error| RegistrationDiagnosticError::new(role, error))?;
    let fingerprint = fingerprint_reader(&mut input)
        .map_err(|error| RegistrationDiagnosticError::new(role, error))?;
    input
        .seek(SeekFrom::Start(0))
        .map_err(|error| RegistrationDiagnosticError::new(role, error))?;
    let mut reader = PrimaryImageReader::open(input, HeaderReadOptions::default())
        .map_err(|error| RegistrationDiagnosticError::new(role, error))?;
    if !reader.report().is_accepted(ValidationMode::Tolerant) {
        return Err(RegistrationDiagnosticError::message(
            role,
            "primary FITS header is not accepted in tolerant mode",
        ));
    }
    let (source_width_u64, source_height_u64) = match reader.descriptor().axes() {
        [width, height] => (*width, *height),
        _ => {
            return Err(RegistrationDiagnosticError::message(
                role,
                "raw CFA diagnostics require exactly two FITS axes",
            ));
        }
    };
    let source_width = usize::try_from(source_width_u64)
        .map_err(|error| RegistrationDiagnosticError::new(role, error))?;
    let source_height = usize::try_from(source_height_u64)
        .map_err(|error| RegistrationDiagnosticError::new(role, error))?;
    let source_samples = source_width.checked_mul(source_height).ok_or_else(|| {
        RegistrationDiagnosticError::message(role, "source dimensions overflow address space")
    })?;
    if source_samples > MAX_DIAGNOSTIC_SOURCE_SAMPLES {
        return Err(RegistrationDiagnosticError::message(
            role,
            format!(
                "source contains {source_samples} samples; diagnostic limit is {MAX_DIAGNOSTIC_SOURCE_SAMPLES}"
            ),
        ));
    }
    let header_conformant = reader.report().is_conformant();
    let header_diagnostics = reader.report().diagnostics().len();
    let source_image = reader
        .read_region_image(ImageRegion::new(
            0,
            0,
            0,
            source_width_u64,
            source_height_u64,
        ))
        .map_err(|error| RegistrationDiagnosticError::new(role, error))?;
    let detection_plane = prepare_cfa_cell_mean(source_image)
        .map_err(|error| RegistrationDiagnosticError::new(role, error))?;
    prepare_detection_plane(
        fingerprint.sha256(),
        source_width,
        source_height,
        header_conformant,
        header_diagnostics,
        detection_plane,
        profile,
        role,
    )
}

#[allow(clippy::too_many_arguments)]
fn prepare_detection_plane(
    content_sha256: &str,
    source_width: usize,
    source_height: usize,
    header_conformant: bool,
    header_diagnostics: usize,
    detection_plane: ScientificImage,
    profile: &DiagnosticProfile,
    role: &'static str,
) -> Result<PreparedFrame, RegistrationDiagnosticError> {
    let dimensions = detection_plane.dimensions();
    let detection_width = dimensions.width();
    let detection_height = dimensions.height();
    let quality = measure_frame_quality(&detection_plane, 0, profile.quality)
        .map_err(|error| RegistrationDiagnosticError::new(role, error))?;
    let id = FrameId::new(content_sha256.to_owned())
        .map_err(|error| RegistrationDiagnosticError::new(role, error))?;
    let features = build_feature_catalog(
        id.clone(),
        detection_width,
        detection_height,
        &quality,
        profile.features,
    )
    .map_err(|error| RegistrationDiagnosticError::new(role, error))?;
    let background = quality.background();
    let summary = FrameSummary {
        content_sha256: content_sha256.to_owned(),
        source_width,
        source_height,
        detection_width,
        detection_height,
        header_conformant,
        header_diagnostics,
        background: background.location(),
        noise_sigma: background.noise_sigma(),
        detected_stars: quality.stars().len(),
        median_fwhm_source_pixels: quality.median_fwhm_major_pixels().map(|value| value * 2.0),
        median_eccentricity: quality.median_eccentricity(),
        registration_features: features.features().len(),
    };
    Ok(PreparedFrame { features, summary })
}

const fn rejection_name(rejection: RegistrationConfidenceRejection) -> &'static str {
    match rejection {
        RegistrationConfidenceRejection::InsufficientInlierHypotheses => {
            "insufficient_inlier_hypotheses"
        }
        RegistrationConfidenceRejection::InsufficientInlierRatio => "insufficient_inlier_ratio",
        RegistrationConfidenceRejection::InsufficientFeaturePairs => "insufficient_feature_pairs",
        RegistrationConfidenceRejection::InsufficientWinnerSupportMargin => {
            "insufficient_winner_support_margin"
        }
        RegistrationConfidenceRejection::ExcessiveRmsResidual => "excessive_rms_residual",
        RegistrationConfidenceRejection::ExcessiveMaximumResidual => "excessive_maximum_residual",
        RegistrationConfidenceRejection::InsufficientSourceHorizontalSpan => {
            "insufficient_source_horizontal_span"
        }
        RegistrationConfidenceRejection::InsufficientSourceVerticalSpan => {
            "insufficient_source_vertical_span"
        }
        RegistrationConfidenceRejection::InsufficientReferenceHorizontalSpan => {
            "insufficient_reference_horizontal_span"
        }
        RegistrationConfidenceRejection::InsufficientReferenceVerticalSpan => {
            "insufficient_reference_vertical_span"
        }
        RegistrationConfidenceRejection::ReflectionForbidden => "reflection_forbidden",
        RegistrationConfidenceRejection::TruncatedEvidence => "truncated_evidence",
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn image_fits(width: usize, height: usize) -> Vec<u8> {
        let cards = [
            "SIMPLE  =                    T",
            "BITPIX  =                   16",
            "NAXIS   =                    2",
            &format!("NAXIS1  = {width:>20}"),
            &format!("NAXIS2  = {height:>20}"),
            "END",
        ];
        let mut bytes = Vec::new();
        for card in cards {
            let mut encoded = card.as_bytes().to_vec();
            encoded.resize(80, b' ');
            bytes.extend_from_slice(&encoded);
        }
        let header_padding = (2_880 - bytes.len() % 2_880) % 2_880;
        bytes.resize(bytes.len() + header_padding, b' ');
        let samples = width.checked_mul(height).unwrap_or_default();
        bytes.resize(bytes.len() + samples.saturating_mul(2), 0);
        let data_padding = (2_880 - bytes.len() % 2_880) % 2_880;
        bytes.resize(bytes.len() + data_padding, 0);
        bytes
    }

    #[test]
    fn every_confidence_rejection_has_a_stable_machine_name() {
        let cases = [
            RegistrationConfidenceRejection::InsufficientInlierHypotheses,
            RegistrationConfidenceRejection::InsufficientInlierRatio,
            RegistrationConfidenceRejection::InsufficientFeaturePairs,
            RegistrationConfidenceRejection::InsufficientWinnerSupportMargin,
            RegistrationConfidenceRejection::ExcessiveRmsResidual,
            RegistrationConfidenceRejection::ExcessiveMaximumResidual,
            RegistrationConfidenceRejection::InsufficientSourceHorizontalSpan,
            RegistrationConfidenceRejection::InsufficientSourceVerticalSpan,
            RegistrationConfidenceRejection::InsufficientReferenceHorizontalSpan,
            RegistrationConfidenceRejection::InsufficientReferenceVerticalSpan,
            RegistrationConfidenceRejection::ReflectionForbidden,
            RegistrationConfidenceRejection::TruncatedEvidence,
        ];
        for rejection in cases {
            let name = rejection_name(rejection);
            assert!(!name.is_empty());
            assert!(
                name.bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
            );
        }
    }

    #[test]
    fn profile_is_internally_valid_and_versioned() {
        assert!(DiagnosticProfile::precision().is_ok());
        assert!(PRECISION_DIAGNOSTIC_PROFILE_ID.ends_with("-v1"));
    }

    #[test]
    fn opening_errors_do_not_disclose_input_paths() {
        let source = Path::new("/private/observer/secret-target/source.fits");
        let reference = Path::new("/private/observer/secret-target/reference.fits");
        let result = diagnose_paths(source, reference);
        assert!(result.is_err());
        let Some(error) = result.err() else {
            return;
        };
        let message = error.to_string();
        assert!(!message.contains("secret-target"));
        assert!(!message.contains("source.fits"));
    }

    #[test]
    fn reader_pipeline_rejects_partial_bayer_cells() {
        let source = Cursor::new(image_fits(3, 4));
        let reference = Cursor::new(image_fits(4, 4));
        let result = diagnose_readers(source, reference);
        assert!(result.is_err());
        let Some(error) = result.err() else {
            return;
        };
        assert!(error.to_string().contains("complete 2x2 cells"));
    }
}
