//! Deterministic coordinate, transform, residual, and reference-selection core.
//!
//! It fixes coordinate conventions, matching, similarity consensus, and an
//! inspectable reference policy before later resampling stages, preventing the
//! desktop shell from inventing scientific geometry.

mod confidence;
mod consensus;
mod features;
mod geometry;
mod matching;
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
pub use reference::{
    CandidateRanks, ReferenceCandidate, ReferenceMetrics, ReferenceSelection,
    ReferenceSelectionError, ReferenceSelectionEvidence, select_reference,
};
pub use resampling::{
    LANCZOS3_RESAMPLING_ALGORITHM_ID, ResampledImage, ResamplingError, ResamplingStatistics,
    resample_lanczos3,
};
pub use triangles::{
    MAX_DESCRIPTOR_ANCHORS, MAX_DESCRIPTOR_NEIGHBORS, MAX_TRIANGLE_DESCRIPTORS,
    TRIANGLE_DESCRIPTOR_ALGORITHM_ID, TriangleDescriptor, TriangleDescriptorCatalog,
    TriangleDescriptorError, TriangleDescriptorParameters, TriangleDescriptorStatistics,
    TriangleOrientation, build_triangle_descriptors,
};
