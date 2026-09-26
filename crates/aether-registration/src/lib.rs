//! Deterministic coordinate, transform, residual, and reference-selection core.
//!
//! This crate deliberately stops before star matching and resampling. It fixes
//! the coordinate conventions and inspectable reference policy those later
//! stages must obey, preventing the desktop shell from inventing geometry.

mod features;
mod geometry;
mod reference;

pub use features::{
    FEATURE_CATALOG_ALGORITHM_ID, FeatureCatalog, FeatureCatalogError, FeatureExclusions,
    FeatureSelectionParameters, MAX_REGISTRATION_FEATURES, MAX_REGISTRATION_MEASUREMENTS,
    RegistrationFeature, build_feature_catalog,
};
pub use geometry::{
    AffineTransform, CoordinateError, ImagePoint, RegistrationMatch, ResidualError,
    ResidualStatistics, evaluate_residuals,
};
pub use reference::{
    CandidateRanks, ReferenceCandidate, ReferenceMetrics, ReferenceSelection,
    ReferenceSelectionError, ReferenceSelectionEvidence, select_reference,
};
