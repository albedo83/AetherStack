//! Explainable classification of files in an astronomical session.
//!
//! The crate combines header declarations with evidence found in the directory
//! tree. Contradictions remain visible and are never resolved by an implicit
//! assumption.

mod classification;
mod directory;
mod fingerprint;
mod generator;
mod grouping;
mod manifest;
mod source;

pub use classification::{
    ClassificationEvidence, ClassificationPolicy, ClassificationSource, ClassificationSourceKind,
    FrameClassification, FrameResolution, ResolutionBasis, classify_frame,
};
pub use directory::{
    DEFAULT_MAX_DEPTH, DEFAULT_MAX_ENTRIES_PER_DIRECTORY, DEFAULT_MAX_FAILURES,
    DEFAULT_MAX_FITS_FILES, DEFAULT_MAX_SOURCE_BYTES, DEFAULT_MAX_TOTAL_ENTRIES,
    DEFAULT_MAX_TOTAL_SOURCE_BYTES, DirectoryFailureCode, DirectoryFailureReason,
    DirectoryManifestError, DirectoryManifestOptions, DirectoryManifestReport,
    DirectoryScanFailure, DirectoryScanLimits, generate_manifest_from_directory,
};
pub use fingerprint::{FINGERPRINT_BUFFER_BYTES, FingerprintError, fingerprint_reader};
pub use generator::{ManifestGenerationError, generate_manifest};
pub use grouping::{GroupingField, GroupingKeyError, StrictGroupingKey};
pub use manifest::{
    MAX_SESSION_MANIFEST_BYTES, ManifestError, ManifestFile, ManifestGroup, ManifestValidationCode,
    ManifestValidationError, SESSION_MANIFEST_SCHEMA_VERSION, SessionManifest, SourceFingerprint,
};
pub use source::{SourceAnalysisError, analyze_fits_source};
