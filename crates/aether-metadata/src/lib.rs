//! Traceable normalization of astronomical metadata.
//!
//! Canonical values never replace the source FITS header. Each value retains its
//! source keyword and a confidence level; contradictions are exposed as
//! structured diagnostics.

mod model;
mod normalize;

pub use model::{
    BayerPattern, Binning, CameraModel, CanonicalMetadata, CanonicalValue, Confidence, FrameType,
    MetadataIssue, MetadataIssueCode, SensorKind,
};
pub use normalize::normalize_header;
