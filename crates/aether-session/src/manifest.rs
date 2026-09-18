use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_fits::{Diagnostic, Severity, ValidationMode};
use aether_metadata::{Binning, CanonicalMetadata};
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use sha2::{Digest, Sha256};

use crate::{
    ClassificationPolicy, FrameClassification, FrameResolution, GroupingField, StrictGroupingKey,
};

/// Schema version written by this release.
pub const SESSION_MANIFEST_SCHEMA_VERSION: u32 = 1;
/// Largest JSON manifest accepted by the in-memory decoder (128 MiB).
pub const MAX_SESSION_MANIFEST_BYTES: usize = 128 * 1_024 * 1_024;

/// Stable category of a session-manifest validation failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManifestValidationCode {
    /// The manifest uses a schema version this release cannot interpret.
    UnsupportedSchemaVersion,
    /// The encoded manifest exceeds the documented parser memory boundary.
    ManifestTooLarge,
    /// A source path is not a portable, normalized relative path.
    InvalidRelativePath,
    /// A source fingerprint is empty or is not canonical SHA-256.
    InvalidFingerprint,
    /// Two source records use the same relative path.
    DuplicateFilePath,
    /// Image axes are not a supported non-empty 2D or 3D shape.
    InvalidImageAxes,
    /// Canonical numeric metadata contains NaN or infinity.
    NonFiniteMetadata,
    /// Error-level FITS diagnostics conflict with strict manifest policy.
    FitsDiagnosticsRejected,
    /// Stored classification fields contradict their retained evidence.
    InconsistentClassification,
    /// A group identifier is empty or contains non-portable characters.
    InvalidGroupId,
    /// Two groups use the same identifier.
    DuplicateGroupId,
    /// A group contains no source file.
    EmptyGroup,
    /// Two groups contain the same exact grouping key.
    DuplicateGroupingKey,
    /// A group refers to a source path absent from the manifest.
    UnknownGroupFile,
    /// A source path is assigned to more than one group.
    FileInMultipleGroups,
    /// A group contains a source whose classification was not resolved.
    UnresolvedGroupFile,
    /// A source's canonical metadata does not reproduce its group's exact key.
    GroupingKeyMismatch,
    /// Missing grouping fields are not explicitly and coherently acknowledged.
    InvalidMissingMetadataPolicy,
}

impl Display for ManifestValidationCode {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::UnsupportedSchemaVersion => "unsupported_schema_version",
            Self::ManifestTooLarge => "manifest_too_large",
            Self::InvalidRelativePath => "invalid_relative_path",
            Self::InvalidFingerprint => "invalid_fingerprint",
            Self::DuplicateFilePath => "duplicate_file_path",
            Self::InvalidImageAxes => "invalid_image_axes",
            Self::NonFiniteMetadata => "non_finite_metadata",
            Self::FitsDiagnosticsRejected => "fits_diagnostics_rejected",
            Self::InconsistentClassification => "inconsistent_classification",
            Self::InvalidGroupId => "invalid_group_id",
            Self::DuplicateGroupId => "duplicate_group_id",
            Self::EmptyGroup => "empty_group",
            Self::DuplicateGroupingKey => "duplicate_grouping_key",
            Self::UnknownGroupFile => "unknown_group_file",
            Self::FileInMultipleGroups => "file_in_multiple_groups",
            Self::UnresolvedGroupFile => "unresolved_group_file",
            Self::GroupingKeyMismatch => "grouping_key_mismatch",
            Self::InvalidMissingMetadataPolicy => "invalid_missing_metadata_policy",
        };
        formatter.write_str(value)
    }
}

/// Structured validation failure with a stable code and offending subject.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestValidationError {
    code: ManifestValidationCode,
    subject: String,
}

impl ManifestValidationError {
    fn new(code: ManifestValidationCode, subject: impl Into<String>) -> Self {
        Self {
            code,
            subject: subject.into(),
        }
    }

    /// Stable machine-readable error category.
    #[must_use]
    pub const fn code(&self) -> ManifestValidationCode {
        self.code
    }

    /// Path, identifier, or field associated with the failure.
    #[must_use]
    pub fn subject(&self) -> &str {
        &self.subject
    }
}

impl Display for ManifestValidationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "session manifest validation failed with {} at `{}`",
            self.code, self.subject
        )
    }
}

impl Error for ManifestValidationError {}

/// Error raised while encoding or decoding a session manifest.
#[derive(Debug)]
pub enum ManifestError {
    /// JSON syntax or representation failure.
    Json(serde_json::Error),
    /// Parsed content violates a session invariant.
    Validation(ManifestValidationError),
}

impl Display for ManifestError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "invalid session manifest JSON: {error}"),
            Self::Validation(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for ManifestError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::Validation(error) => Some(error),
        }
    }
}

/// Immutable source-file fingerprint recorded before processing.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceFingerprint {
    byte_length: u64,
    sha256: String,
}

impl SourceFingerprint {
    /// Builds a fingerprint from the exact byte length and lowercase SHA-256.
    ///
    /// # Errors
    ///
    /// Returns [`ManifestValidationCode::InvalidFingerprint`] for an empty file
    /// or a digest that is not exactly 64 lowercase hexadecimal characters.
    pub fn new(
        byte_length: u64,
        sha256: impl Into<String>,
    ) -> Result<Self, ManifestValidationError> {
        let fingerprint = Self {
            byte_length,
            sha256: sha256.into(),
        };
        fingerprint.validate("fingerprint")?;
        Ok(fingerprint)
    }

    /// Exact source size in bytes.
    #[must_use]
    pub const fn byte_length(&self) -> u64 {
        self.byte_length
    }

    /// Canonical lowercase SHA-256 digest.
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    fn validate(&self, subject: &str) -> Result<(), ManifestValidationError> {
        if self.byte_length == 0
            || self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ManifestValidationError::new(
                ManifestValidationCode::InvalidFingerprint,
                subject,
            ));
        }
        Ok(())
    }
}

/// One immutable FITS source and all decisions made during session ingestion.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestFile {
    relative_path: String,
    fingerprint: SourceFingerprint,
    axes: Vec<u64>,
    metadata: CanonicalMetadata,
    fits_diagnostics: Vec<Diagnostic>,
    classification: FrameClassification,
    resolution: Option<FrameResolution>,
}

impl ManifestFile {
    /// Captures one analyzed source using the selected classification policy.
    ///
    /// The resolution is computed here rather than accepted from the caller, so
    /// the retained evidence and policy cannot silently diverge.
    ///
    /// # Errors
    ///
    /// Returns a validation error for a non-portable path, invalid axes,
    /// fingerprint, metadata, or inconsistent classification.
    pub fn from_analysis(
        relative_path: impl Into<String>,
        fingerprint: SourceFingerprint,
        axes: Vec<u64>,
        metadata: CanonicalMetadata,
        fits_diagnostics: Vec<Diagnostic>,
        classification: FrameClassification,
        policy: ClassificationPolicy,
    ) -> Result<Self, ManifestValidationError> {
        let resolution = classification.resolve_with(policy);
        let file = Self {
            relative_path: relative_path.into(),
            fingerprint,
            axes,
            metadata,
            fits_diagnostics,
            classification,
            resolution,
        };
        file.validate(policy)?;
        Ok(file)
    }

    /// Portable path relative to the session root.
    #[must_use]
    pub fn relative_path(&self) -> &str {
        &self.relative_path
    }

    /// Immutable source fingerprint.
    #[must_use]
    pub const fn fingerprint(&self) -> &SourceFingerprint {
        &self.fingerprint
    }

    /// Image axes in FITS order.
    #[must_use]
    pub fn axes(&self) -> &[u64] {
        &self.axes
    }

    /// Canonical metadata with source-keyword provenance.
    #[must_use]
    pub const fn metadata(&self) -> &CanonicalMetadata {
        &self.metadata
    }

    /// FITS conformance diagnostics retained in detection order.
    #[must_use]
    pub fn fits_diagnostics(&self) -> &[Diagnostic] {
        &self.fits_diagnostics
    }

    /// Retained classification evidence and conflict state.
    #[must_use]
    pub const fn classification(&self) -> &FrameClassification {
        &self.classification
    }

    /// Policy-derived resolution, absent when evidence is insufficient.
    #[must_use]
    pub const fn resolution(&self) -> Option<&FrameResolution> {
        self.resolution.as_ref()
    }

    fn validate(&self, policy: ClassificationPolicy) -> Result<(), ManifestValidationError> {
        if !valid_relative_path(&self.relative_path) {
            return Err(ManifestValidationError::new(
                ManifestValidationCode::InvalidRelativePath,
                &self.relative_path,
            ));
        }
        self.fingerprint.validate(&self.relative_path)?;
        if !valid_image_axes(&self.axes) {
            return Err(ManifestValidationError::new(
                ManifestValidationCode::InvalidImageAxes,
                &self.relative_path,
            ));
        }
        if !metadata_numbers_are_finite(&self.metadata) {
            return Err(ManifestValidationError::new(
                ManifestValidationCode::NonFiniteMetadata,
                &self.relative_path,
            ));
        }
        if !self.classification.is_consistent()
            || self.classification.resolve_with(policy) != self.resolution
        {
            return Err(ManifestValidationError::new(
                ManifestValidationCode::InconsistentClassification,
                &self.relative_path,
            ));
        }
        Ok(())
    }
}

/// Exact group membership and any explicit missing-metadata acknowledgement.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestGroup {
    id: String,
    key: StrictGroupingKey,
    files: Vec<String>,
    accepted_missing_fields: Vec<GroupingField>,
    missing_metadata_rationale: Option<String>,
}

impl ManifestGroup {
    /// Builds one exact group.
    ///
    /// `accepted_missing_fields` must contain each field absent from `key`
    /// exactly once. A non-empty rationale is required whenever that list is not
    /// empty, making a relaxed grouping decision visible in the manifest.
    ///
    /// # Errors
    ///
    /// Returns a validation error for malformed identifiers, empty membership,
    /// invalid member paths, or an incoherent missing-metadata policy.
    pub fn new(
        id: impl Into<String>,
        key: StrictGroupingKey,
        files: Vec<String>,
        accepted_missing_fields: Vec<GroupingField>,
        missing_metadata_rationale: Option<String>,
    ) -> Result<Self, ManifestValidationError> {
        let group = Self {
            id: id.into(),
            key,
            files,
            accepted_missing_fields,
            missing_metadata_rationale,
        };
        group.validate_standalone()?;
        Ok(group)
    }

    /// Stable group identifier used by later pipeline stages.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Exact grouping key shared by every member.
    #[must_use]
    pub const fn key(&self) -> &StrictGroupingKey {
        &self.key
    }

    /// Portable source paths assigned to this group.
    #[must_use]
    pub fn files(&self) -> &[String] {
        &self.files
    }

    /// Missing key fields explicitly accepted by the caller.
    #[must_use]
    pub fn accepted_missing_fields(&self) -> &[GroupingField] {
        &self.accepted_missing_fields
    }

    /// Human-readable justification for grouping with missing metadata.
    #[must_use]
    pub fn missing_metadata_rationale(&self) -> Option<&str> {
        self.missing_metadata_rationale.as_deref()
    }

    fn validate_standalone(&self) -> Result<(), ManifestValidationError> {
        if !valid_group_id(&self.id) {
            return Err(ManifestValidationError::new(
                ManifestValidationCode::InvalidGroupId,
                &self.id,
            ));
        }
        if self.files.is_empty() {
            return Err(ManifestValidationError::new(
                ManifestValidationCode::EmptyGroup,
                &self.id,
            ));
        }
        if self.files.iter().any(|path| !valid_relative_path(path)) {
            return Err(ManifestValidationError::new(
                ManifestValidationCode::InvalidRelativePath,
                &self.id,
            ));
        }
        if !valid_image_axes(self.key.axes()) {
            return Err(ManifestValidationError::new(
                ManifestValidationCode::InvalidImageAxes,
                &self.id,
            ));
        }

        let missing = self.key.missing_fields();
        let accepted: BTreeSet<_> = self.accepted_missing_fields.iter().copied().collect();
        let rationale = self
            .missing_metadata_rationale
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if accepted.len() != self.accepted_missing_fields.len()
            || accepted.len() != missing.len()
            || missing.iter().any(|field| !accepted.contains(field))
            || (missing.is_empty() && self.missing_metadata_rationale.is_some())
            || (!missing.is_empty() && rationale.is_none())
        {
            return Err(ManifestValidationError::new(
                ManifestValidationCode::InvalidMissingMetadataPolicy,
                &self.id,
            ));
        }
        Ok(())
    }
}

/// Versioned, deterministic record of session ingestion and exact grouping.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SessionManifest {
    schema_version: u32,
    fits_validation_mode: ValidationMode,
    classification_policy: ClassificationPolicy,
    files: Vec<ManifestFile>,
    groups: Vec<ManifestGroup>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionManifestWire {
    schema_version: u32,
    fits_validation_mode: ValidationMode,
    classification_policy: ClassificationPolicy,
    files: Vec<ManifestFile>,
    groups: Vec<ManifestGroup>,
}

#[derive(Deserialize)]
struct SchemaVersionProbe {
    schema_version: u32,
}

impl SessionManifest {
    /// Builds and canonicalizes a current-version manifest.
    ///
    /// Files are ordered by relative path, groups by identifier, and members by
    /// path. This makes byte-for-byte JSON output independent of discovery order.
    ///
    /// # Errors
    ///
    /// Returns a structured error when any file, classification, group, or
    /// cross-reference violates the manifest contract.
    pub fn new(
        classification_policy: ClassificationPolicy,
        files: Vec<ManifestFile>,
        groups: Vec<ManifestGroup>,
    ) -> Result<Self, ManifestValidationError> {
        Self::with_validation_mode(ValidationMode::Strict, classification_policy, files, groups)
    }

    /// Builds a manifest with an explicit FITS conformance policy.
    ///
    /// Use this constructor for tolerant imports so the relaxed acceptance
    /// policy is persisted beside every retained FITS diagnostic.
    ///
    /// # Errors
    ///
    /// Returns the same structured validation failures as [`Self::new`].
    pub fn with_validation_mode(
        fits_validation_mode: ValidationMode,
        classification_policy: ClassificationPolicy,
        files: Vec<ManifestFile>,
        groups: Vec<ManifestGroup>,
    ) -> Result<Self, ManifestValidationError> {
        let mut manifest = Self {
            schema_version: SESSION_MANIFEST_SCHEMA_VERSION,
            fits_validation_mode,
            classification_policy,
            files,
            groups,
        };
        manifest.validate()?;
        manifest.canonicalize();
        Ok(manifest)
    }

    /// Decodes and validates a UTF-8 JSON manifest.
    ///
    /// Unknown fields and unsupported schema versions are rejected. Accepted
    /// input is canonicalized before it is returned.
    ///
    /// # Errors
    ///
    /// Returns [`ManifestError::Json`] for malformed JSON or unknown fields and
    /// [`ManifestError::Validation`] for a semantic invariant violation.
    pub fn from_json_slice(input: &[u8]) -> Result<Self, ManifestError> {
        validate_input_length(input.len()).map_err(ManifestError::Validation)?;
        let version: SchemaVersionProbe =
            serde_json::from_slice(input).map_err(ManifestError::Json)?;
        if version.schema_version != SESSION_MANIFEST_SCHEMA_VERSION {
            return Err(ManifestError::Validation(ManifestValidationError::new(
                ManifestValidationCode::UnsupportedSchemaVersion,
                version.schema_version.to_string(),
            )));
        }
        let wire: SessionManifestWire =
            serde_json::from_slice(input).map_err(ManifestError::Json)?;
        Self::from_wire(wire).map_err(ManifestError::Validation)
    }

    /// Encodes deterministic, human-readable JSON terminated by one newline.
    ///
    /// # Errors
    ///
    /// Returns a validation error if internal content is invalid, or a JSON
    /// error if encoding fails.
    pub fn to_json_pretty(&self) -> Result<Vec<u8>, ManifestError> {
        self.validate().map_err(ManifestError::Validation)?;
        let mut output = serde_json::to_vec_pretty(self).map_err(ManifestError::Json)?;
        output.push(b'\n');
        Ok(output)
    }

    /// Computes SHA-256 over the exact canonical JSON bytes.
    ///
    /// The hashed representation is exactly [`Self::to_json_pretty`], including
    /// its terminating newline. This digest is suitable for the `AETHMAN` FITS
    /// provenance card and changes whenever any serialized manifest content
    /// changes.
    ///
    /// # Errors
    ///
    /// Returns the same validation or encoding failure as
    /// [`Self::to_json_pretty`].
    pub fn canonical_sha256(&self) -> Result<String, ManifestError> {
        let encoded = self.to_json_pretty()?;
        let digest = Sha256::digest(encoded);
        Ok(crate::fingerprint::encode_lower_hex(&digest))
    }

    /// Manifest schema version.
    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// FITS conformance policy applied to every source.
    #[must_use]
    pub const fn fits_validation_mode(&self) -> ValidationMode {
        self.fits_validation_mode
    }

    /// Policy used to resolve every retained classification.
    #[must_use]
    pub const fn classification_policy(&self) -> ClassificationPolicy {
        self.classification_policy
    }

    /// Canonically ordered source records.
    #[must_use]
    pub fn files(&self) -> &[ManifestFile] {
        &self.files
    }

    /// Canonically ordered exact groups.
    #[must_use]
    pub fn groups(&self) -> &[ManifestGroup] {
        &self.groups
    }

    fn from_wire(wire: SessionManifestWire) -> Result<Self, ManifestValidationError> {
        let mut manifest = Self {
            schema_version: wire.schema_version,
            fits_validation_mode: wire.fits_validation_mode,
            classification_policy: wire.classification_policy,
            files: wire.files,
            groups: wire.groups,
        };
        manifest.validate()?;
        manifest.canonicalize();
        Ok(manifest)
    }

    fn canonicalize(&mut self) {
        self.files
            .sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        self.groups.sort_by(|left, right| left.id.cmp(&right.id));
        for group in &mut self.groups {
            group.files.sort();
            group.accepted_missing_fields.sort();
            if let Some(rationale) = &mut group.missing_metadata_rationale {
                *rationale = rationale.trim().to_owned();
            }
        }
    }

    fn validate(&self) -> Result<(), ManifestValidationError> {
        if self.schema_version != SESSION_MANIFEST_SCHEMA_VERSION {
            return Err(ManifestValidationError::new(
                ManifestValidationCode::UnsupportedSchemaVersion,
                self.schema_version.to_string(),
            ));
        }

        let mut files_by_path = BTreeMap::new();
        let mut portable_paths = BTreeSet::new();
        for file in &self.files {
            file.validate(self.classification_policy)?;
            if self.fits_validation_mode == ValidationMode::Strict
                && file
                    .fits_diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.severity() == Severity::Error)
            {
                return Err(ManifestValidationError::new(
                    ManifestValidationCode::FitsDiagnosticsRejected,
                    &file.relative_path,
                ));
            }
            if !portable_paths.insert(file.relative_path.to_ascii_lowercase()) {
                return Err(ManifestValidationError::new(
                    ManifestValidationCode::DuplicateFilePath,
                    &file.relative_path,
                ));
            }
            if files_by_path
                .insert(file.relative_path.as_str(), file)
                .is_some()
            {
                return Err(ManifestValidationError::new(
                    ManifestValidationCode::DuplicateFilePath,
                    &file.relative_path,
                ));
            }
        }

        let mut group_ids = BTreeSet::new();
        let mut grouping_keys = HashMap::new();
        let mut assigned_files: BTreeMap<&str, &str> = BTreeMap::new();
        for group in &self.groups {
            group.validate_standalone()?;
            if !group_ids.insert(group.id.as_str()) {
                return Err(ManifestValidationError::new(
                    ManifestValidationCode::DuplicateGroupId,
                    &group.id,
                ));
            }
            if let Some(previous) = grouping_keys.insert(&group.key, group.id.as_str()) {
                return Err(ManifestValidationError::new(
                    ManifestValidationCode::DuplicateGroupingKey,
                    format!("{previous},{}", group.id),
                ));
            }

            let mut members = BTreeSet::new();
            for path in &group.files {
                if !members.insert(path.as_str()) {
                    return Err(ManifestValidationError::new(
                        ManifestValidationCode::FileInMultipleGroups,
                        path,
                    ));
                }
                let Some(file) = files_by_path.get(path.as_str()).copied() else {
                    return Err(ManifestValidationError::new(
                        ManifestValidationCode::UnknownGroupFile,
                        path,
                    ));
                };
                if let Some(previous) = assigned_files.insert(path, group.id.as_str()) {
                    return Err(ManifestValidationError::new(
                        ManifestValidationCode::FileInMultipleGroups,
                        format!("{path} ({previous},{})", group.id),
                    ));
                }
                let Some(resolution) = file.resolution.as_ref() else {
                    return Err(ManifestValidationError::new(
                        ManifestValidationCode::UnresolvedGroupFile,
                        path,
                    ));
                };
                let expected = StrictGroupingKey::from_metadata(
                    resolution.frame_type().clone(),
                    &file.metadata,
                    &file.axes,
                )
                .map_err(|_| {
                    ManifestValidationError::new(ManifestValidationCode::NonFiniteMetadata, path)
                })?;
                if expected != group.key {
                    return Err(ManifestValidationError::new(
                        ManifestValidationCode::GroupingKeyMismatch,
                        path,
                    ));
                }
            }
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for SessionManifest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = SessionManifestWire::deserialize(deserializer)?;
        Self::from_wire(wire).map_err(D::Error::custom)
    }
}

fn valid_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.contains(':')
        && !path.chars().any(char::is_control)
        && path.split('/').all(valid_path_component)
}

fn validate_input_length(length: usize) -> Result<(), ManifestValidationError> {
    if length > MAX_SESSION_MANIFEST_BYTES {
        return Err(ManifestValidationError::new(
            ManifestValidationCode::ManifestTooLarge,
            length.to_string(),
        ));
    }
    Ok(())
}

fn valid_path_component(component: &str) -> bool {
    if component.is_empty()
        || component == "."
        || component == ".."
        || component.ends_with(' ')
        || component.ends_with('.')
    {
        return false;
    }

    let basename = component
        .split_once('.')
        .map_or(component, |(basename, _)| basename)
        .to_ascii_uppercase();
    !matches!(basename.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !windows_numbered_device(&basename, "COM")
        && !windows_numbered_device(&basename, "LPT")
}

fn windows_numbered_device(value: &str, prefix: &str) -> bool {
    value
        .strip_prefix(prefix)
        .is_some_and(|suffix| matches!(suffix.as_bytes(), [b'1'..=b'9']))
}

fn valid_group_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn valid_image_axes(axes: &[u64]) -> bool {
    matches!(axes.len(), 2 | 3) && axes.iter().all(|axis| *axis > 0)
}

fn metadata_numbers_are_finite(metadata: &CanonicalMetadata) -> bool {
    let numeric_values = [
        metadata.exposure_seconds.as_ref(),
        metadata.sensor_temperature_c.as_ref(),
        metadata.set_temperature_c.as_ref(),
        metadata.gain.as_ref(),
        metadata.offset.as_ref(),
    ];
    numeric_values
        .into_iter()
        .flatten()
        .all(|value| value.value().is_finite())
        && metadata
            .binning
            .as_ref()
            .is_none_or(|value| valid_binning(value.value()))
}

const fn valid_binning(binning: &Binning) -> bool {
    binning.x > 0 && binning.y > 0
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::path::Path;

    use aether_metadata::{
        BayerPattern, CameraModel, CanonicalValue, Confidence, FrameType, MetadataIssue,
    };

    use super::*;
    use crate::classify_frame;

    type TestResult<T = ()> = Result<T, Box<dyn StdError>>;

    fn value<T>(value: T, keyword: &str) -> CanonicalValue<T> {
        CanonicalValue::new(value, keyword, Confidence::Exact)
    }

    fn complete_metadata(frame_type: FrameType) -> CanonicalMetadata {
        CanonicalMetadata {
            camera: Some(value(CameraModel::TouptekAtr585C, "INSTRUME")),
            frame_type: Some(value(frame_type, "IMAGETYP")),
            exposure_seconds: Some(value(60.0, "EXPTIME")),
            sensor_temperature_c: Some(value(-10.0, "CCD-TEMP")),
            set_temperature_c: Some(value(-10.0, "SET-TEMP")),
            gain: Some(value(120.0, "GAIN")),
            offset: Some(value(512.0, "OFFSET")),
            binning: Some(value(Binning { x: 1, y: 1 }, "XBINNING+YBINNING")),
            filter: Some(value("UV/IR Cut".to_owned(), "FILTER")),
            bayer_pattern: Some(value(BayerPattern::Rggb, "BAYERPAT")),
            issues: Vec::<MetadataIssue>::new(),
        }
    }

    fn fingerprint(digit: char) -> Result<SourceFingerprint, ManifestValidationError> {
        SourceFingerprint::new(5_760, digit.to_string().repeat(64))
    }

    fn typed_file(
        path: &str,
        digest_digit: char,
        frame_type: FrameType,
    ) -> TestResult<ManifestFile> {
        let metadata = complete_metadata(frame_type);
        let classification = classify_frame(Path::new(path), &metadata);
        Ok(ManifestFile::from_analysis(
            path,
            fingerprint(digest_digit)?,
            vec![4, 3],
            metadata,
            Vec::new(),
            classification,
            ClassificationPolicy::RequireAgreement,
        )?)
    }

    fn file(path: &str, digest_digit: char) -> TestResult<ManifestFile> {
        typed_file(path, digest_digit, FrameType::Light)
    }

    fn group_for_file(id: &str, file: &ManifestFile) -> TestResult<ManifestGroup> {
        let resolution = file
            .resolution()
            .ok_or_else(|| std::io::Error::other("test file must have a resolution"))?;
        let key = StrictGroupingKey::from_metadata(
            resolution.frame_type().clone(),
            file.metadata(),
            file.axes(),
        )?;
        Ok(ManifestGroup::new(
            id,
            key,
            vec![file.relative_path().to_owned()],
            Vec::new(),
            None,
        )?)
    }

    fn manifest() -> TestResult<SessionManifest> {
        let first = file("lights/frame_0002.fits", 'b')?;
        let second = file("lights/frame_0001.fits", 'a')?;
        let key =
            StrictGroupingKey::from_metadata(FrameType::Light, first.metadata(), first.axes())?;
        let group = ManifestGroup::new(
            "light-001",
            key,
            vec![
                first.relative_path().to_owned(),
                second.relative_path().to_owned(),
            ],
            Vec::new(),
            None,
        )?;
        Ok(SessionManifest::new(
            ClassificationPolicy::RequireAgreement,
            vec![first, second],
            vec![group],
        )?)
    }

    #[test]
    fn round_trip_is_exact_and_canonically_ordered() -> TestResult {
        let manifest = manifest()?;
        assert_eq!(
            manifest.files()[0].relative_path(),
            "lights/frame_0001.fits"
        );
        assert_eq!(manifest.groups()[0].files()[0], "lights/frame_0001.fits");

        let encoded = manifest.to_json_pretty()?;
        assert_eq!(encoded.last(), Some(&b'\n'));

        let decoded = SessionManifest::from_json_slice(&encoded)?;
        assert_eq!(decoded, manifest);
        Ok(())
    }

    #[test]
    fn canonical_digest_hashes_the_exact_portable_bytes() -> TestResult {
        let manifest = manifest()?;
        let encoded = manifest.to_json_pretty()?;
        let expected = crate::fingerprint::encode_lower_hex(&Sha256::digest(&encoded));

        assert_eq!(manifest.canonical_sha256()?, expected);
        assert_eq!(
            SessionManifest::from_json_slice(&encoded)?.canonical_sha256()?,
            expected
        );

        let changed =
            String::from_utf8(encoded)?.replace("lights/frame_0001.fits", "lights/frame_0003.fits");
        let changed = SessionManifest::from_json_slice(changed.as_bytes())?;
        assert_ne!(changed.canonical_sha256()?, expected);
        Ok(())
    }

    #[test]
    fn rejects_unknown_schema_version() -> TestResult {
        let encoded = manifest()?.to_json_pretty()?;
        let text =
            String::from_utf8(encoded)?.replace("\"schema_version\": 1", "\"schema_version\": 2");
        let text = text.replacen('{', "{\"future_field\":true,", 1);

        assert!(matches!(
            SessionManifest::from_json_slice(text.as_bytes()),
            Err(ManifestError::Validation(error))
                if error.code() == ManifestValidationCode::UnsupportedSchemaVersion
        ));
        Ok(())
    }

    #[test]
    fn rejects_manifest_length_above_the_parser_boundary() {
        assert_eq!(validate_input_length(MAX_SESSION_MANIFEST_BYTES), Ok(()));
        assert!(matches!(
            validate_input_length(MAX_SESSION_MANIFEST_BYTES + 1),
            Err(error) if error.code() == ManifestValidationCode::ManifestTooLarge
        ));
    }

    #[test]
    fn rejects_unknown_json_field() -> TestResult {
        let mut document = serde_json::to_value(manifest()?)?;
        let object = document
            .as_object_mut()
            .ok_or_else(|| std::io::Error::other("manifest must serialize as an object"))?;
        object.insert("unexpected".to_owned(), serde_json::Value::Bool(true));
        let encoded = serde_json::to_vec(&document)?;

        assert!(matches!(
            SessionManifest::from_json_slice(&encoded),
            Err(ManifestError::Json(_))
        ));
        Ok(())
    }

    #[test]
    fn rejects_non_portable_paths() -> TestResult {
        for path in [
            "/absolute/frame.fits",
            "lights\\frame.fits",
            "C:/lights/frame.fits",
            "lights/CON.fits",
            "lights/frame.fits.",
            "lights//frame.fits",
        ] {
            assert!(!valid_relative_path(path), "path was accepted: {path}");
        }

        let metadata = complete_metadata(FrameType::Light);
        let classification = classify_frame(Path::new("lights/frame.fits"), &metadata);

        assert!(matches!(
            ManifestFile::from_analysis(
                "../private/frame.fits",
                fingerprint('a')?,
                vec![4, 3],
                metadata,
                Vec::new(),
                classification,
                ClassificationPolicy::RequireAgreement,
            ),
            Err(error) if error.code() == ManifestValidationCode::InvalidRelativePath
        ));
        Ok(())
    }

    #[test]
    fn rejects_non_canonical_fingerprint() {
        assert!(matches!(
            SourceFingerprint::new(5_760, "A".repeat(64)),
            Err(error) if error.code() == ManifestValidationCode::InvalidFingerprint
        ));
        assert!(matches!(
            SourceFingerprint::new(0, "a".repeat(64)),
            Err(error) if error.code() == ManifestValidationCode::InvalidFingerprint
        ));
    }

    #[test]
    fn rejects_invalid_axes_and_non_finite_metadata() -> TestResult {
        let metadata = complete_metadata(FrameType::Light);
        let classification = classify_frame(Path::new("lights/frame.fits"), &metadata);
        assert!(matches!(
            ManifestFile::from_analysis(
                "lights/frame.fits",
                fingerprint('a')?,
                vec![4],
                metadata,
                Vec::new(),
                classification,
                ClassificationPolicy::RequireAgreement,
            ),
            Err(error) if error.code() == ManifestValidationCode::InvalidImageAxes
        ));

        let mut metadata = complete_metadata(FrameType::Light);
        metadata.gain = Some(value(f64::NAN, "GAIN"));
        let classification = classify_frame(Path::new("lights/frame.fits"), &metadata);
        assert!(matches!(
            ManifestFile::from_analysis(
                "lights/frame.fits",
                fingerprint('a')?,
                vec![4, 3],
                metadata,
                Vec::new(),
                classification,
                ClassificationPolicy::RequireAgreement,
            ),
            Err(error) if error.code() == ManifestValidationCode::NonFiniteMetadata
        ));
        Ok(())
    }

    #[test]
    fn rejects_classification_state_tampered_in_json() -> TestResult {
        let mut document = serde_json::to_value(manifest()?)?;
        let classification = document
            .get_mut("files")
            .and_then(serde_json::Value::as_array_mut)
            .and_then(|files| files.first_mut())
            .and_then(|file| file.get_mut("classification"))
            .and_then(serde_json::Value::as_object_mut)
            .ok_or_else(|| std::io::Error::other("classification object is missing"))?;
        classification.insert("has_conflict".to_owned(), serde_json::Value::Bool(true));
        let encoded = serde_json::to_vec(&document)?;

        assert!(matches!(
            SessionManifest::from_json_slice(&encoded),
            Err(ManifestError::Json(_))
        ));
        Ok(())
    }

    #[test]
    fn rejects_duplicate_source_paths() -> TestResult {
        let source = file("lights/frame.fits", 'a')?;
        assert!(matches!(
            SessionManifest::new(
                ClassificationPolicy::RequireAgreement,
                vec![source.clone(), source],
                Vec::new(),
            ),
            Err(error) if error.code() == ManifestValidationCode::DuplicateFilePath
        ));

        let source = file("lights/frame.fits", 'a')?;
        let mut case_variant = source.clone();
        case_variant.relative_path = "LIGHTS/FRAME.FITS".to_owned();
        assert!(matches!(
            SessionManifest::new(
                ClassificationPolicy::RequireAgreement,
                vec![source, case_variant],
                Vec::new(),
            ),
            Err(error) if error.code() == ManifestValidationCode::DuplicateFilePath
        ));
        Ok(())
    }

    #[test]
    fn rejects_invalid_or_empty_groups() -> TestResult {
        let source = file("lights/frame.fits", 'a')?;
        let resolution = source
            .resolution()
            .ok_or_else(|| std::io::Error::other("test file must have a resolution"))?;
        let key = StrictGroupingKey::from_metadata(
            resolution.frame_type().clone(),
            source.metadata(),
            source.axes(),
        )?;

        assert!(matches!(
            ManifestGroup::new(
                "bad/id",
                key.clone(),
                vec![source.relative_path().to_owned()],
                Vec::new(),
                None,
            ),
            Err(error) if error.code() == ManifestValidationCode::InvalidGroupId
        ));
        assert!(matches!(
            ManifestGroup::new("light-001", key, Vec::new(), Vec::new(), None),
            Err(error) if error.code() == ManifestValidationCode::EmptyGroup
        ));
        Ok(())
    }

    #[test]
    fn rejects_duplicate_group_ids_and_keys() -> TestResult {
        let light = file("lights/light.fits", 'a')?;
        let dark = typed_file("darks/dark.fits", 'b', FrameType::Dark)?;
        let light_group = group_for_file("group-001", &light)?;
        let dark_group = group_for_file("group-001", &dark)?;
        assert!(matches!(
            SessionManifest::new(
                ClassificationPolicy::RequireAgreement,
                vec![light.clone(), dark],
                vec![light_group.clone(), dark_group],
            ),
            Err(error) if error.code() == ManifestValidationCode::DuplicateGroupId
        ));

        let duplicate_key_group = ManifestGroup::new(
            "group-002",
            light_group.key().clone(),
            vec![light.relative_path().to_owned()],
            Vec::new(),
            None,
        )?;
        assert!(matches!(
            SessionManifest::new(
                ClassificationPolicy::RequireAgreement,
                vec![light],
                vec![light_group, duplicate_key_group],
            ),
            Err(error) if error.code() == ManifestValidationCode::DuplicateGroupingKey
        ));
        Ok(())
    }

    #[test]
    fn rejects_unknown_and_multiply_assigned_group_files() -> TestResult {
        let light = file("lights/light.fits", 'a')?;
        let valid_group = group_for_file("light-001", &light)?;
        let unknown_group = ManifestGroup::new(
            "light-002",
            valid_group.key().clone(),
            vec!["lights/missing.fits".to_owned()],
            Vec::new(),
            None,
        )?;
        assert!(matches!(
            SessionManifest::new(
                ClassificationPolicy::RequireAgreement,
                vec![light.clone()],
                vec![unknown_group],
            ),
            Err(error) if error.code() == ManifestValidationCode::UnknownGroupFile
        ));

        let dark_metadata = complete_metadata(FrameType::Dark);
        let dark_key = StrictGroupingKey::from_metadata(FrameType::Dark, &dark_metadata, &[4, 3])?;
        let second_group = ManifestGroup::new(
            "dark-001",
            dark_key,
            vec![light.relative_path().to_owned()],
            Vec::new(),
            None,
        )?;
        assert!(matches!(
            SessionManifest::new(
                ClassificationPolicy::RequireAgreement,
                vec![light],
                vec![valid_group, second_group],
            ),
            Err(error) if error.code() == ManifestValidationCode::FileInMultipleGroups
        ));
        Ok(())
    }

    #[test]
    fn rejects_unresolved_file_in_a_group() -> TestResult {
        let metadata = CanonicalMetadata::default();
        let classification = classify_frame(Path::new("session/frame.fits"), &metadata);
        let source = ManifestFile::from_analysis(
            "session/frame.fits",
            fingerprint('a')?,
            vec![4, 3],
            metadata.clone(),
            Vec::new(),
            classification,
            ClassificationPolicy::RequireAgreement,
        )?;
        let key = StrictGroupingKey::from_metadata(FrameType::Light, &metadata, &[4, 3])?;
        let missing = key.missing_fields();
        let group = ManifestGroup::new(
            "light-001",
            key,
            vec![source.relative_path().to_owned()],
            missing,
            Some("Operator accepted absent acquisition metadata".to_owned()),
        )?;

        assert!(matches!(
            SessionManifest::new(
                ClassificationPolicy::RequireAgreement,
                vec![source],
                vec![group],
            ),
            Err(error) if error.code() == ManifestValidationCode::UnresolvedGroupFile
        ));
        Ok(())
    }

    #[test]
    fn rejects_grouping_key_that_does_not_match_a_member() -> TestResult {
        let source = file("lights/frame.fits", 'a')?;
        let dark_metadata = complete_metadata(FrameType::Dark);
        let dark_key = StrictGroupingKey::from_metadata(FrameType::Dark, &dark_metadata, &[4, 3])?;
        let group = ManifestGroup::new(
            "dark-001",
            dark_key,
            vec![source.relative_path().to_owned()],
            Vec::new(),
            None,
        )?;

        assert!(matches!(
            SessionManifest::new(
                ClassificationPolicy::RequireAgreement,
                vec![source],
                vec![group]
            ),
            Err(error) if error.code() == ManifestValidationCode::GroupingKeyMismatch
        ));
        Ok(())
    }

    #[test]
    fn requires_rationale_for_every_missing_grouping_field() -> TestResult {
        let metadata = CanonicalMetadata {
            frame_type: Some(value(FrameType::Light, "IMAGETYP")),
            ..CanonicalMetadata::default()
        };
        let key = StrictGroupingKey::from_metadata(FrameType::Light, &metadata, &[4, 3])?;
        let missing = key.missing_fields();

        assert!(matches!(
            ManifestGroup::new(
                "light-001",
                key.clone(),
                vec!["lights/frame.fits".to_owned()],
                missing.clone(),
                None,
            ),
            Err(error)
                if error.code() == ManifestValidationCode::InvalidMissingMetadataPolicy
        ));

        let classification = classify_frame(Path::new("lights/frame.fits"), &metadata);
        let source = ManifestFile::from_analysis(
            "lights/frame.fits",
            fingerprint('a')?,
            vec![4, 3],
            metadata,
            Vec::new(),
            classification,
            ClassificationPolicy::RequireAgreement,
        )?;
        let group = ManifestGroup::new(
            "light-001",
            key,
            vec![source.relative_path().to_owned()],
            missing,
            Some("  Accepted for a controlled import  ".to_owned()),
        )?;
        let manifest = SessionManifest::new(
            ClassificationPolicy::RequireAgreement,
            vec![source],
            vec![group],
        )?;
        assert_eq!(
            manifest.groups()[0].missing_metadata_rationale(),
            Some("Accepted for a controlled import")
        );
        Ok(())
    }

    #[test]
    fn metadata_issue_type_remains_serializable() -> TestResult {
        let issue = MetadataIssue::new(
            aether_metadata::MetadataIssueCode::InvalidValue,
            ["GAIN"],
            "GAIN is invalid",
        );
        let encoded = serde_json::to_string(&issue)?;
        assert!(encoded.contains("invalid_value"));
        Ok(())
    }
}
