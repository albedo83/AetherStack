//! Deterministic coordinate, transform, residual, and reference-selection core.
//!
//! It fixes coordinate conventions, matching, similarity consensus, and an
//! inspectable reference policy before later resampling stages, preventing the
//! desktop shell from inventing scientific geometry.

mod confidence;
mod consensus;
mod features;
mod footprint;
mod geometry;
mod matching;
mod model_comparison;
mod model_selection;
mod model_validation;
mod plan;
mod projective;
mod projective_fit;
mod reference;
mod resampling;
mod triangles;

pub use confidence::{
    REGISTRATION_CONFIDENCE_ALGORITHM_ID, RegistrationConfidenceError,
    RegistrationConfidenceParameters, RegistrationConfidenceRejection,
    RegistrationConfidenceReport, assess_registration_confidence,
};
pub use consensus::{
    CompetingSimilarity, MAX_CONSENSUS_MODELS, MAX_CONSENSUS_RESIDUAL_EVALUATIONS,
    SIMILARITY_CONSENSUS_ALGORITHM_ID, SimilarityConsensus, SimilarityConsensusError,
    SimilarityConsensusParameters, SimilarityConsensusStatistics, estimate_similarity_consensus,
};
pub use features::{
    FEATURE_CATALOG_ALGORITHM_ID, FeatureCatalog, FeatureCatalogError, FeatureExclusions,
    FeatureSelectionParameters, MAX_REGISTRATION_FEATURES, MAX_REGISTRATION_MEASUREMENTS,
    RegistrationFeature, build_feature_catalog,
};
pub use footprint::{
    COMMON_LANCZOS3_FOOTPRINT_ALGORITHM_ID, CommonFootprintError, CommonFootprintReport,
    MAX_COMMON_FOOTPRINT_EVALUATIONS, MAX_COMMON_FOOTPRINT_FRAMES, ReferenceRectangle,
    RegistrationFootprint, derive_common_lanczos3_footprint,
};
pub use geometry::{
    AffineTransform, CoordinateError, ImagePoint, RegistrationMatch, ResidualError,
    ResidualStatistics, evaluate_residuals,
};
pub use matching::{
    DESCRIPTOR_MATCH_ALGORITHM_ID, DescriptorMatchCatalog, DescriptorMatchError,
    DescriptorMatchHypothesis, DescriptorMatchParameters, DescriptorMatchStatistics, FeaturePair,
    MAX_DESCRIPTOR_COMPARISONS, MAX_MATCH_CANDIDATES_PER_DESCRIPTOR, MAX_MATCH_HYPOTHESES,
    ReflectionPolicy, match_triangle_descriptors,
};
pub use model_comparison::{
    PROJECTIVE_ADEQUACY_ALGORITHM_ID, ProjectiveAdequacyError, ProjectiveAdequacyEvidence,
    compare_similarity_with_projective,
};
pub use model_selection::{
    PROJECTIVE_SELECTION_ALGORITHM_ID, ProjectiveSelectionDecision, ProjectiveSelectionError,
    ProjectiveSelectionPolicy, evaluate_projective_selection,
};
pub use model_validation::{
    MAX_PROJECTIVE_VALIDATION_FOLDS, PROJECTIVE_CROSS_VALIDATION_ALGORITHM_ID,
    ProjectiveCrossValidationError, ProjectiveCrossValidationEvidence,
    cross_validate_similarity_with_projective,
};
pub use plan::{
    PlannedRegistrationFrame, REGISTRATION_PLAN_ALGORITHM_ID, RegistrationPlan,
    RegistrationPlanError,
};
pub use projective::ProjectiveTransform;
pub use projective_fit::{
    MAX_PROJECTIVE_FIT_MATCHES, PROJECTIVE_FIT_ALGORITHM_ID, ProjectiveFit, ProjectiveFitError,
    fit_projective,
};
pub use reference::{
    CandidateRanks, ReferenceCandidate, ReferenceMetrics, ReferenceSelection,
    ReferenceSelectionError, ReferenceSelectionEvidence, select_reference,
};
pub use resampling::{
    LANCZOS3_RESAMPLING_ALGORITHM_ID, Lanczos3BandExecutor, Lanczos3BandPlan, Lanczos3SourceWindow,
    ProjectiveLanczos3BandExecutor, ProjectiveLanczos3BandPlan, ProjectivelyResampledImage,
    ResampledBand, ResampledImage, ResamplingError, ResamplingStatistics,
    plan_lanczos3_projective_source_window, plan_lanczos3_source_window, resample_lanczos3,
    resample_lanczos3_projective,
};
pub use triangles::{
    MAX_DESCRIPTOR_ANCHORS, MAX_DESCRIPTOR_NEIGHBORS, MAX_TRIANGLE_DESCRIPTORS,
    TRIANGLE_DESCRIPTOR_ALGORITHM_ID, TriangleDescriptor, TriangleDescriptorCatalog,
    TriangleDescriptorError, TriangleDescriptorParameters, TriangleDescriptorStatistics,
    TriangleOrientation, build_triangle_descriptors,
};
