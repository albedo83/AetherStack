//! Explainable classification of files in an astronomical session.
//!
//! The crate combines header declarations with evidence found in the directory
//! tree. Contradictions remain visible and are never resolved by an implicit
//! assumption.

mod classification;
mod grouping;

pub use classification::{
    ClassificationEvidence, ClassificationPolicy, ClassificationSource, ClassificationSourceKind,
    FrameClassification, FrameResolution, ResolutionBasis, classify_frame,
};
pub use grouping::{GroupingField, GroupingKeyError, StrictGroupingKey};
