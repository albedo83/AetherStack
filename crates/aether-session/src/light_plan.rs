use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_metadata::{FrameType, SensorKind};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    ManifestError, ManifestGroup, MasterPlan, MasterPlanError, MasterProductKind, SessionManifest,
    StrictGroupingKey, TemperatureBasis,
};

/// Schema version emitted for light-to-master calibration plans.
pub const LIGHT_CALIBRATION_PLAN_SCHEMA_VERSION: u32 = 1;
/// Largest light-calibration plan accepted by the bounded JSON decoder.
pub const MAX_LIGHT_CALIBRATION_PLAN_BYTES: usize = 16 * 1_024 * 1_024;
/// Largest inspectable light-to-master candidate matrix retained in one plan.
pub const MAX_LIGHT_CALIBRATION_CANDIDATE_EVALUATIONS: usize = 100_000;

/// Explicit matching controls for calibrating light groups.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LightCalibrationPlanOptions {
    maximum_dark_temperature_delta_c: f64,
}

impl LightCalibrationPlanOptions {
    /// Creates conservative matching controls.
    ///
    /// Dark exposure remains an exact match. Exposure scaling is intentionally
    /// absent until it has a separate versioned algorithm and provenance model.
    /// The temperature tolerance is inclusive.
    ///
    /// # Errors
    ///
    /// Returns a typed error for a negative or non-finite tolerance.
    pub fn new(maximum_dark_temperature_delta_c: f64) -> Result<Self, LightCalibrationPlanError> {
        let options = Self {
            maximum_dark_temperature_delta_c: canonical_non_negative(
                maximum_dark_temperature_delta_c,
            ),
        };
        options.validate()?;
        Ok(options)
    }

    /// Inclusive light-to-dark temperature tolerance in degrees Celsius.
    #[must_use]
    pub const fn maximum_dark_temperature_delta_c(self) -> f64 {
        self.maximum_dark_temperature_delta_c
    }

    fn validate(self) -> Result<(), LightCalibrationPlanError> {
        if !self.maximum_dark_temperature_delta_c.is_finite()
            || self.maximum_dark_temperature_delta_c < 0.0
        {
            return Err(LightCalibrationPlanError::InvalidDarkTemperatureTolerance {
                value: self.maximum_dark_temperature_delta_c,
            });
        }
        Ok(())
    }
}

/// Master role considered while calibrating a light group.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LightMasterKind {
    /// Dark master subtracted from the light.
    Dark,
    /// Normalized flat master used as the divisor.
    Flat,
}

/// Metadata field that can reject a Dark or Flat candidate.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LightMasterMatchField {
    /// Canonical camera model.
    Camera,
    /// Image dimensions in FITS axis order.
    Axes,
    /// Exposure duration; applicable only to the Dark.
    Exposure,
    /// Measured sensor temperature; applicable only to the Dark.
    SensorTemperature,
    /// Requested sensor temperature used when the light has no measurement.
    SetTemperature,
    /// Camera gain.
    Gain,
    /// Camera digital offset.
    Offset,
    /// Horizontal and vertical binning.
    Binning,
    /// Optical filter; applicable only to the Flat.
    Filter,
    /// CFA pattern and phase at the stored image origin.
    BayerPattern,
}

/// Stable reason one required field did not match.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LightMasterMismatchReason {
    /// A required value is absent.
    Missing,
    /// Both values exist but differ.
    Different,
    /// The absolute temperature delta exceeds the configured tolerance.
    OutsideTolerance,
}

/// One machine-readable candidate rejection reason.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LightMasterMismatch {
    field: LightMasterMatchField,
    reason: LightMasterMismatchReason,
}

impl LightMasterMismatch {
    /// Field whose comparison failed.
    #[must_use]
    pub const fn field(self) -> LightMasterMatchField {
        self.field
    }

    /// Nature of the mismatch.
    #[must_use]
    pub const fn reason(self) -> LightMasterMismatchReason {
        self.reason
    }
}

/// Complete result of comparing one master product with one light group.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum LightMasterCandidateCompatibility {
    /// Every role-specific field matches.
    Compatible {
        /// Temperature field used for Dark matching; absent for a Flat.
        temperature_basis: Option<TemperatureBasis>,
        /// Absolute temperature delta; absent for a Flat.
        temperature_delta_c: Option<f64>,
    },
    /// At least one required field is missing, different, or outside tolerance.
    Rejected {
        /// Stable reasons in field declaration order.
        mismatches: Vec<LightMasterMismatch>,
    },
}

impl LightMasterCandidateCompatibility {
    const fn is_compatible(&self) -> bool {
        matches!(self, Self::Compatible { .. })
    }
}

/// Inspectable evaluation of one Dark or Flat master candidate.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LightMasterCandidateEvaluation {
    group_id: String,
    kind: LightMasterKind,
    compatibility: LightMasterCandidateCompatibility,
}

impl LightMasterCandidateEvaluation {
    /// Source group used to construct the candidate master.
    #[must_use]
    pub fn group_id(&self) -> &str {
        &self.group_id
    }

    /// Candidate master role.
    #[must_use]
    pub const fn kind(&self) -> LightMasterKind {
        self.kind
    }

    /// Full compatibility result retained for diagnostics.
    #[must_use]
    pub const fn compatibility(&self) -> &LightMasterCandidateCompatibility {
        &self.compatibility
    }
}

/// Why a Dark or Flat could not be selected without guessing.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "reason")]
pub enum LightMasterBlockingReason {
    /// The light lacks fields required for this master role.
    MissingLightMetadata {
        /// Missing fields in stable declaration order.
        fields: Vec<LightMasterMatchField>,
    },
    /// No master of the required role is compatible.
    NoCompatibleCandidate,
    /// Several candidates have the same best role-specific score.
    AmbiguousCandidates {
        /// Equally ranked candidate group identifiers in lexical order.
        group_ids: Vec<String>,
    },
}

/// One role-specific master association for a light group.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "selection")]
pub enum LightMasterAssociation {
    /// Exactly one compatible master was selected.
    Matched {
        /// Selected source group used to construct the master.
        group_id: String,
        /// Selected master role.
        kind: LightMasterKind,
        /// Temperature field used for a Dark; absent for a Flat.
        temperature_basis: Option<TemperatureBasis>,
        /// Absolute temperature delta for a Dark; absent for a Flat.
        temperature_delta_c: Option<f64>,
    },
    /// No unambiguous automatic association exists.
    Unresolved {
        /// Master role that remains unresolved.
        kind: LightMasterKind,
        /// Explicit blocking reason.
        blocking_reason: LightMasterBlockingReason,
    },
}

impl LightMasterAssociation {
    /// Returns true only when exactly one master is selected.
    #[must_use]
    pub const fn is_resolved(&self) -> bool {
        matches!(self, Self::Matched { .. })
    }

    const fn kind(&self) -> LightMasterKind {
        match self {
            Self::Matched { kind, .. } | Self::Unresolved { kind, .. } => *kind,
        }
    }
}

/// Calibration associations and evidence for one exact light group.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LightCalibrationProductPlan {
    source_group_id: String,
    dark: LightMasterAssociation,
    flat: LightMasterAssociation,
    candidates: Vec<LightMasterCandidateEvaluation>,
}

impl LightCalibrationProductPlan {
    /// Exact light group calibrated by this product.
    #[must_use]
    pub fn source_group_id(&self) -> &str {
        &self.source_group_id
    }

    /// Selected or blocked Dark association.
    #[must_use]
    pub const fn dark(&self) -> &LightMasterAssociation {
        &self.dark
    }

    /// Selected or blocked Flat association.
    #[must_use]
    pub const fn flat(&self) -> &LightMasterAssociation {
        &self.flat
    }

    /// Every candidate master considered for this light group.
    #[must_use]
    pub fn candidates(&self) -> &[LightMasterCandidateEvaluation] {
        &self.candidates
    }

    /// True only when both a Dark and a Flat are resolved.
    #[must_use]
    pub const fn is_ready(&self) -> bool {
        self.dark.is_resolved() && self.flat.is_resolved()
    }
}

/// Versioned deterministic plan for assigning masters to light groups.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LightCalibrationPlan {
    schema_version: u32,
    manifest_sha256: String,
    master_plan_sha256: String,
    options: LightCalibrationPlanOptions,
    products: Vec<LightCalibrationProductPlan>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LightCalibrationPlanWire {
    schema_version: u32,
    manifest_sha256: String,
    master_plan_sha256: String,
    options: LightCalibrationPlanOptions,
    products: Vec<LightCalibrationProductPlan>,
}

#[derive(Deserialize)]
struct SchemaVersionProbe {
    schema_version: u32,
}

impl LightCalibrationPlan {
    /// Builds light associations from one exact manifest and master plan.
    ///
    /// Only Dark and fully normalized Flat products present in the supplied
    /// master plan are considered. Dark exposure is exact; Dark temperature is
    /// matched within the explicit tolerance. Flat exposure and temperature are
    /// irrelevant after normalization, while its filter must match exactly.
    ///
    /// # Errors
    ///
    /// Returns a typed error for invalid options, mismatched input digests, an
    /// unresolved master plan, an oversized candidate matrix, or allocation.
    pub fn from_manifest_and_master_plan(
        manifest: &SessionManifest,
        master_plan: &MasterPlan,
        options: LightCalibrationPlanOptions,
    ) -> Result<Self, LightCalibrationPlanError> {
        options.validate()?;
        let manifest_sha256 = manifest
            .canonical_sha256()
            .map_err(LightCalibrationPlanError::Manifest)?;
        if master_plan.manifest_sha256() != manifest_sha256 {
            return Err(LightCalibrationPlanError::MasterPlanManifestMismatch);
        }
        let supplied_master_plan_sha256 = master_plan
            .canonical_sha256()
            .map_err(LightCalibrationPlanError::MasterPlan)?;
        let canonical_master_plan = MasterPlan::from_manifest(manifest, master_plan.options())
            .map_err(LightCalibrationPlanError::MasterPlan)?;
        let canonical_master_plan_sha256 = canonical_master_plan
            .canonical_sha256()
            .map_err(LightCalibrationPlanError::MasterPlan)?;
        if supplied_master_plan_sha256 != canonical_master_plan_sha256 {
            return Err(LightCalibrationPlanError::MasterPlanContentMismatch);
        }
        if !master_plan.is_ready() {
            return Err(LightCalibrationPlanError::MasterPlanNotReady);
        }
        let master_plan_sha256 = supplied_master_plan_sha256;

        let light_count = manifest
            .groups()
            .iter()
            .filter(|group| group.key().frame_type() == &FrameType::Light)
            .count();
        let candidate_count = master_plan
            .products()
            .iter()
            .filter(|product| {
                matches!(
                    product.kind(),
                    MasterProductKind::Dark | MasterProductKind::Flat
                )
            })
            .count();
        validate_candidate_evaluation_count(light_count, candidate_count)?;

        let mut candidates = Vec::new();
        candidates
            .try_reserve_exact(candidate_count)
            .map_err(|_| LightCalibrationPlanError::AllocationFailed)?;
        for product in master_plan.products() {
            let kind = match product.kind() {
                MasterProductKind::Dark => LightMasterKind::Dark,
                MasterProductKind::Flat => LightMasterKind::Flat,
                MasterProductKind::Bias => continue,
            };
            let group = manifest
                .groups()
                .iter()
                .find(|group| group.id() == product.source_group_id())
                .ok_or_else(|| LightCalibrationPlanError::MissingMasterGroup {
                    group_id: product.source_group_id().to_owned(),
                })?;
            candidates.push((kind, group));
        }

        let mut products = Vec::new();
        products
            .try_reserve_exact(light_count)
            .map_err(|_| LightCalibrationPlanError::AllocationFailed)?;
        for light in manifest
            .groups()
            .iter()
            .filter(|group| group.key().frame_type() == &FrameType::Light)
        {
            products.push(plan_light_product(light, &candidates, options)?);
        }

        let mut plan = Self {
            schema_version: LIGHT_CALIBRATION_PLAN_SCHEMA_VERSION,
            manifest_sha256,
            master_plan_sha256,
            options,
            products,
        };
        plan.canonicalize();
        plan.validate()?;
        Ok(plan)
    }

    /// Decodes and semantically validates one bounded JSON document.
    ///
    /// # Errors
    ///
    /// Returns a typed size, JSON, version, option, or invariant failure.
    pub fn from_json_slice(input: &[u8]) -> Result<Self, LightCalibrationPlanError> {
        if input.len() > MAX_LIGHT_CALIBRATION_PLAN_BYTES {
            return Err(LightCalibrationPlanError::PlanTooLarge { bytes: input.len() });
        }
        let version: SchemaVersionProbe =
            serde_json::from_slice(input).map_err(LightCalibrationPlanError::Json)?;
        if version.schema_version != LIGHT_CALIBRATION_PLAN_SCHEMA_VERSION {
            return Err(LightCalibrationPlanError::UnsupportedSchemaVersion {
                found: version.schema_version,
            });
        }
        let wire: LightCalibrationPlanWire =
            serde_json::from_slice(input).map_err(LightCalibrationPlanError::Json)?;
        let mut plan = Self {
            schema_version: wire.schema_version,
            manifest_sha256: wire.manifest_sha256,
            master_plan_sha256: wire.master_plan_sha256,
            options: wire.options,
            products: wire.products,
        };
        plan.canonicalize();
        plan.validate()?;
        Ok(plan)
    }

    /// Encodes deterministic pretty JSON terminated by one newline.
    ///
    /// # Errors
    ///
    /// Returns an invariant or JSON encoding failure.
    pub fn to_json_pretty(&self) -> Result<Vec<u8>, LightCalibrationPlanError> {
        self.validate()?;
        let mut output =
            serde_json::to_vec_pretty(self).map_err(LightCalibrationPlanError::Json)?;
        output.push(b'\n');
        Ok(output)
    }

    /// Computes SHA-256 over the exact canonical JSON bytes.
    ///
    /// # Errors
    ///
    /// Returns the same validation or encoding failure as [`Self::to_json_pretty`].
    pub fn canonical_sha256(&self) -> Result<String, LightCalibrationPlanError> {
        let encoded = self.to_json_pretty()?;
        Ok(crate::fingerprint::encode_lower_hex(&Sha256::digest(
            encoded,
        )))
    }

    /// Plan schema version.
    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Digest of the exact source manifest.
    #[must_use]
    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }

    /// Digest of the exact master-construction plan.
    #[must_use]
    pub fn master_plan_sha256(&self) -> &str {
        &self.master_plan_sha256
    }

    /// Explicit Dark matching controls.
    #[must_use]
    pub const fn options(&self) -> LightCalibrationPlanOptions {
        self.options
    }

    /// Light products in canonical source-group order.
    #[must_use]
    pub fn products(&self) -> &[LightCalibrationProductPlan] {
        &self.products
    }

    /// True only when every light has one Dark and one Flat.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.products
            .iter()
            .all(LightCalibrationProductPlan::is_ready)
    }

    fn canonicalize(&mut self) {
        self.options.maximum_dark_temperature_delta_c =
            canonical_non_negative(self.options.maximum_dark_temperature_delta_c);
        self.products
            .sort_by(|left, right| left.source_group_id.cmp(&right.source_group_id));
        for product in &mut self.products {
            product.candidates.sort_by(|left, right| {
                left.kind
                    .cmp(&right.kind)
                    .then_with(|| left.group_id.cmp(&right.group_id))
            });
            for candidate in &mut product.candidates {
                match &mut candidate.compatibility {
                    LightMasterCandidateCompatibility::Compatible {
                        temperature_delta_c,
                        ..
                    } => {
                        *temperature_delta_c = temperature_delta_c.map(canonical_non_negative);
                    }
                    LightMasterCandidateCompatibility::Rejected { mismatches } => {
                        mismatches.sort();
                    }
                }
            }
            canonicalize_association(&mut product.dark);
            canonicalize_association(&mut product.flat);
        }
    }

    fn validate(&self) -> Result<(), LightCalibrationPlanError> {
        if self.schema_version != LIGHT_CALIBRATION_PLAN_SCHEMA_VERSION {
            return Err(LightCalibrationPlanError::UnsupportedSchemaVersion {
                found: self.schema_version,
            });
        }
        if !is_lower_sha256(&self.manifest_sha256) {
            return Err(LightCalibrationPlanError::InvalidManifestSha256);
        }
        if !is_lower_sha256(&self.master_plan_sha256) {
            return Err(LightCalibrationPlanError::InvalidMasterPlanSha256);
        }
        self.options.validate()?;

        let evaluations = self.products.iter().try_fold(0_usize, |total, product| {
            total.checked_add(product.candidates.len()).ok_or(
                LightCalibrationPlanError::TooManyCandidateEvaluations {
                    attempted: usize::MAX,
                    maximum: MAX_LIGHT_CALIBRATION_CANDIDATE_EVALUATIONS,
                },
            )
        })?;
        validate_candidate_evaluation_count(1, evaluations)?;

        let mut product_ids = BTreeSet::new();
        for product in &self.products {
            if product.source_group_id.is_empty()
                || !product_ids.insert(product.source_group_id.as_str())
                || product.dark.kind() != LightMasterKind::Dark
                || product.flat.kind() != LightMasterKind::Flat
            {
                return Err(LightCalibrationPlanError::InvalidProduct {
                    group_id: product.source_group_id.clone(),
                });
            }
            validate_product(product)?;
        }
        Ok(())
    }
}

/// Failure while constructing, encoding, or decoding light associations.
#[derive(Debug)]
pub enum LightCalibrationPlanError {
    /// Dark temperature tolerance is negative or non-finite.
    InvalidDarkTemperatureTolerance {
        /// Rejected value in degrees Celsius.
        value: f64,
    },
    /// The source manifest could not be encoded or fingerprinted.
    Manifest(ManifestError),
    /// The master plan could not be encoded or fingerprinted.
    MasterPlan(MasterPlanError),
    /// The master plan was built from another manifest.
    MasterPlanManifestMismatch,
    /// The supplied master plan is not the canonical plan for its own options.
    MasterPlanContentMismatch,
    /// At least one flat master still lacks an exclusive pedestal.
    MasterPlanNotReady,
    /// A product in the master plan does not exist in the manifest.
    MissingMasterGroup {
        /// Missing source-group identifier.
        group_id: String,
    },
    /// JSON is malformed or contains an unknown field.
    Json(serde_json::Error),
    /// The document uses an unsupported schema version.
    UnsupportedSchemaVersion {
        /// Version found in the document.
        found: u32,
    },
    /// Input exceeds the bounded decoder limit.
    PlanTooLarge {
        /// Received byte count.
        bytes: usize,
    },
    /// Manifest digest is not canonical lowercase SHA-256.
    InvalidManifestSha256,
    /// Master-plan digest is not canonical lowercase SHA-256.
    InvalidMasterPlanSha256,
    /// A product is empty, duplicated, or role-inconsistent.
    InvalidProduct {
        /// Offending light group identifier.
        group_id: String,
    },
    /// A candidate or selected association contradicts retained evidence.
    InvalidCandidate {
        /// Light group containing the invalid candidate.
        light_group_id: String,
        /// Candidate group identifier when available.
        candidate_group_id: String,
    },
    /// Candidate matrix exceeds its documented bound.
    TooManyCandidateEvaluations {
        /// Comparisons required by the input.
        attempted: usize,
        /// Maximum comparisons retained by one plan.
        maximum: usize,
    },
    /// A bounded plan vector could not be allocated.
    AllocationFailed,
}

impl Display for LightCalibrationPlanError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidDarkTemperatureTolerance { value } => write!(
                formatter,
                "light dark temperature tolerance must be finite and non-negative, received {value}"
            ),
            Self::Manifest(error) => Display::fmt(error, formatter),
            Self::MasterPlan(error) => Display::fmt(error, formatter),
            Self::MasterPlanManifestMismatch => {
                formatter.write_str("master plan was built from another session manifest")
            }
            Self::MasterPlanContentMismatch => formatter
                .write_str("master plan content is not canonical for its manifest and options"),
            Self::MasterPlanNotReady => {
                formatter.write_str("master plan contains an unresolved flat pedestal")
            }
            Self::MissingMasterGroup { group_id } => {
                write!(
                    formatter,
                    "master group `{group_id}` is absent from the manifest"
                )
            }
            Self::Json(error) => write!(formatter, "invalid light-calibration plan JSON: {error}"),
            Self::UnsupportedSchemaVersion { found } => {
                write!(
                    formatter,
                    "unsupported light-calibration plan schema version {found}"
                )
            }
            Self::PlanTooLarge { bytes } => write!(
                formatter,
                "light-calibration plan JSON exceeds the limit at {bytes} bytes"
            ),
            Self::InvalidManifestSha256 => {
                formatter.write_str("light-calibration plan has an invalid manifest SHA-256")
            }
            Self::InvalidMasterPlanSha256 => {
                formatter.write_str("light-calibration plan has an invalid master-plan SHA-256")
            }
            Self::InvalidProduct { group_id } => {
                write!(formatter, "invalid light calibration product `{group_id}`")
            }
            Self::InvalidCandidate {
                light_group_id,
                candidate_group_id,
            } => write!(
                formatter,
                "invalid master candidate `{candidate_group_id}` for light `{light_group_id}`"
            ),
            Self::TooManyCandidateEvaluations { attempted, maximum } => write!(
                formatter,
                "light calibration plan requires {attempted} candidate comparisons; limit is {maximum}"
            ),
            Self::AllocationFailed => formatter.write_str("cannot allocate light calibration plan"),
        }
    }
}

impl Error for LightCalibrationPlanError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Manifest(error) => Some(error),
            Self::MasterPlan(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::InvalidDarkTemperatureTolerance { .. }
            | Self::MasterPlanManifestMismatch
            | Self::MasterPlanContentMismatch
            | Self::MasterPlanNotReady
            | Self::MissingMasterGroup { .. }
            | Self::UnsupportedSchemaVersion { .. }
            | Self::PlanTooLarge { .. }
            | Self::InvalidManifestSha256
            | Self::InvalidMasterPlanSha256
            | Self::InvalidProduct { .. }
            | Self::InvalidCandidate { .. }
            | Self::TooManyCandidateEvaluations { .. }
            | Self::AllocationFailed => None,
        }
    }
}

#[derive(Clone, Copy)]
struct CandidateScore<'a> {
    group_id: &'a str,
    kind: LightMasterKind,
    temperature_basis: Option<TemperatureBasis>,
    temperature_delta_c: Option<f64>,
}

fn plan_light_product(
    light: &ManifestGroup,
    candidates: &[(LightMasterKind, &ManifestGroup)],
    options: LightCalibrationPlanOptions,
) -> Result<LightCalibrationProductPlan, LightCalibrationPlanError> {
    let mut evaluations = Vec::new();
    let mut compatible = Vec::new();
    evaluations
        .try_reserve_exact(candidates.len())
        .map_err(|_| LightCalibrationPlanError::AllocationFailed)?;
    compatible
        .try_reserve_exact(candidates.len())
        .map_err(|_| LightCalibrationPlanError::AllocationFailed)?;

    for (kind, candidate) in candidates {
        let (compatibility, score) =
            compare_candidate(light.key(), candidate.key(), *kind, options);
        if let Some((temperature_basis, temperature_delta_c)) = score {
            compatible.push(CandidateScore {
                group_id: candidate.id(),
                kind: *kind,
                temperature_basis,
                temperature_delta_c,
            });
        }
        evaluations.push(LightMasterCandidateEvaluation {
            group_id: candidate.id().to_owned(),
            kind: *kind,
            compatibility,
        });
    }

    let dark_missing = missing_light_fields(light.key(), LightMasterKind::Dark);
    let flat_missing = missing_light_fields(light.key(), LightMasterKind::Flat);
    let dark = select_candidate(LightMasterKind::Dark, &compatible, dark_missing)?;
    let flat = select_candidate(LightMasterKind::Flat, &compatible, flat_missing)?;
    Ok(LightCalibrationProductPlan {
        source_group_id: light.id().to_owned(),
        dark,
        flat,
        candidates: evaluations,
    })
}

type CompatibilityScore = (Option<TemperatureBasis>, Option<f64>);

fn compare_candidate(
    light: &StrictGroupingKey,
    candidate: &StrictGroupingKey,
    kind: LightMasterKind,
    options: LightCalibrationPlanOptions,
) -> (
    LightMasterCandidateCompatibility,
    Option<CompatibilityScore>,
) {
    let mut mismatches = Vec::new();
    compare_exact_option(
        light.camera(),
        candidate.camera(),
        LightMasterMatchField::Camera,
        &mut mismatches,
    );
    if light.axes() != candidate.axes() {
        mismatches.push(LightMasterMismatch {
            field: LightMasterMatchField::Axes,
            reason: LightMasterMismatchReason::Different,
        });
    }
    compare_f64_option(
        light.gain(),
        candidate.gain(),
        LightMasterMatchField::Gain,
        &mut mismatches,
    );
    compare_f64_option(
        light.offset(),
        candidate.offset(),
        LightMasterMatchField::Offset,
        &mut mismatches,
    );
    compare_exact_option(
        light.binning(),
        candidate.binning(),
        LightMasterMatchField::Binning,
        &mut mismatches,
    );
    compare_bayer(light, candidate, &mut mismatches);

    let (temperature_basis, temperature_delta_c) = match kind {
        LightMasterKind::Dark => {
            compare_f64_option(
                light.exposure_seconds(),
                candidate.exposure_seconds(),
                LightMasterMatchField::Exposure,
                &mut mismatches,
            );
            let (basis, delta) = compare_temperature(
                light,
                candidate,
                options.maximum_dark_temperature_delta_c,
                &mut mismatches,
            );
            (basis, delta)
        }
        LightMasterKind::Flat => {
            compare_exact_option(
                light.filter(),
                candidate.filter(),
                LightMasterMatchField::Filter,
                &mut mismatches,
            );
            (None, None)
        }
    };

    if mismatches.is_empty() {
        (
            LightMasterCandidateCompatibility::Compatible {
                temperature_basis,
                temperature_delta_c,
            },
            Some((temperature_basis, temperature_delta_c)),
        )
    } else {
        (
            LightMasterCandidateCompatibility::Rejected { mismatches },
            None,
        )
    }
}

fn missing_light_fields(
    light: &StrictGroupingKey,
    kind: LightMasterKind,
) -> Vec<LightMasterMatchField> {
    let mut fields = Vec::new();
    if light.camera().is_none() {
        fields.push(LightMasterMatchField::Camera);
    }
    if kind == LightMasterKind::Dark && light.exposure_seconds().is_none() {
        fields.push(LightMasterMatchField::Exposure);
    }
    if kind == LightMasterKind::Dark
        && light.sensor_temperature_c().is_none()
        && light.set_temperature_c().is_none()
    {
        fields.push(LightMasterMatchField::SensorTemperature);
        fields.push(LightMasterMatchField::SetTemperature);
    }
    if light.gain().is_none() {
        fields.push(LightMasterMatchField::Gain);
    }
    if light.offset().is_none() {
        fields.push(LightMasterMatchField::Offset);
    }
    if light.binning().is_none() {
        fields.push(LightMasterMatchField::Binning);
    }
    if kind == LightMasterKind::Flat && light.filter().is_none() {
        fields.push(LightMasterMatchField::Filter);
    }
    if bayer_is_required(light) && light.bayer_pattern().is_none() {
        fields.push(LightMasterMatchField::BayerPattern);
    }
    fields
}

fn bayer_is_required(key: &StrictGroupingKey) -> bool {
    key.camera()
        .is_none_or(|camera| camera.sensor_kind() != SensorKind::Monochrome)
}

fn compare_bayer(
    light: &StrictGroupingKey,
    candidate: &StrictGroupingKey,
    mismatches: &mut Vec<LightMasterMismatch>,
) {
    if bayer_is_required(light) {
        compare_exact_option(
            light.bayer_pattern(),
            candidate.bayer_pattern(),
            LightMasterMatchField::BayerPattern,
            mismatches,
        );
    } else if candidate.bayer_pattern().is_some() {
        mismatches.push(LightMasterMismatch {
            field: LightMasterMatchField::BayerPattern,
            reason: LightMasterMismatchReason::Different,
        });
    }
}

fn compare_temperature(
    light: &StrictGroupingKey,
    candidate: &StrictGroupingKey,
    tolerance: f64,
    mismatches: &mut Vec<LightMasterMismatch>,
) -> (Option<TemperatureBasis>, Option<f64>) {
    if light.sensor_temperature_c().is_some() {
        let delta = compare_tolerated(
            light.sensor_temperature_c(),
            candidate.sensor_temperature_c(),
            tolerance,
            LightMasterMatchField::SensorTemperature,
            mismatches,
        );
        (Some(TemperatureBasis::Sensor), delta)
    } else {
        let delta = compare_tolerated(
            light.set_temperature_c(),
            candidate.set_temperature_c(),
            tolerance,
            LightMasterMatchField::SetTemperature,
            mismatches,
        );
        (Some(TemperatureBasis::SetPoint), delta)
    }
}

fn compare_tolerated(
    left: Option<f64>,
    right: Option<f64>,
    tolerance: f64,
    field: LightMasterMatchField,
    mismatches: &mut Vec<LightMasterMismatch>,
) -> Option<f64> {
    let (Some(left), Some(right)) = (left, right) else {
        mismatches.push(LightMasterMismatch {
            field,
            reason: LightMasterMismatchReason::Missing,
        });
        return None;
    };
    let delta = (left - right).abs();
    if delta > tolerance {
        mismatches.push(LightMasterMismatch {
            field,
            reason: LightMasterMismatchReason::OutsideTolerance,
        });
        return None;
    }
    Some(canonical_non_negative(delta))
}

fn compare_exact_option<T: PartialEq>(
    left: Option<T>,
    right: Option<T>,
    field: LightMasterMatchField,
    mismatches: &mut Vec<LightMasterMismatch>,
) {
    let reason = match (left, right) {
        (Some(left), Some(right)) if left == right => return,
        (Some(_), Some(_)) => LightMasterMismatchReason::Different,
        (None, _) | (_, None) => LightMasterMismatchReason::Missing,
    };
    mismatches.push(LightMasterMismatch { field, reason });
}

fn compare_f64_option(
    left: Option<f64>,
    right: Option<f64>,
    field: LightMasterMatchField,
    mismatches: &mut Vec<LightMasterMismatch>,
) {
    let reason = match (left, right) {
        (Some(left), Some(right)) if canonical_f64_bits(left) == canonical_f64_bits(right) => {
            return;
        }
        (Some(_), Some(_)) => LightMasterMismatchReason::Different,
        (None, _) | (_, None) => LightMasterMismatchReason::Missing,
    };
    mismatches.push(LightMasterMismatch { field, reason });
}

fn select_candidate(
    kind: LightMasterKind,
    scores: &[CandidateScore<'_>],
    missing_fields: Vec<LightMasterMatchField>,
) -> Result<LightMasterAssociation, LightCalibrationPlanError> {
    if !missing_fields.is_empty() {
        return Ok(LightMasterAssociation::Unresolved {
            kind,
            blocking_reason: LightMasterBlockingReason::MissingLightMetadata {
                fields: missing_fields,
            },
        });
    }
    let compatible_count = scores.iter().filter(|score| score.kind == kind).count();
    if compatible_count == 0 {
        return Ok(LightMasterAssociation::Unresolved {
            kind,
            blocking_reason: LightMasterBlockingReason::NoCompatibleCandidate,
        });
    }

    let best_temperature = scores
        .iter()
        .filter(|score| score.kind == kind)
        .filter_map(|score| score.temperature_delta_c)
        .min_by(|left, right| left.total_cmp(right));
    let tied_count = scores
        .iter()
        .filter(|score| {
            score.kind == kind
                && (kind == LightMasterKind::Flat
                    || score.temperature_delta_c.map(canonical_f64_bits)
                        == best_temperature.map(canonical_f64_bits))
        })
        .count();
    if tied_count != 1 {
        let mut group_ids = Vec::new();
        group_ids
            .try_reserve_exact(tied_count)
            .map_err(|_| LightCalibrationPlanError::AllocationFailed)?;
        group_ids.extend(
            scores
                .iter()
                .filter(|score| {
                    score.kind == kind
                        && (kind == LightMasterKind::Flat
                            || score.temperature_delta_c.map(canonical_f64_bits)
                                == best_temperature.map(canonical_f64_bits))
                })
                .map(|score| score.group_id.to_owned()),
        );
        group_ids.sort();
        return Ok(LightMasterAssociation::Unresolved {
            kind,
            blocking_reason: LightMasterBlockingReason::AmbiguousCandidates { group_ids },
        });
    }
    let selected = scores
        .iter()
        .find(|score| {
            score.kind == kind
                && (kind == LightMasterKind::Flat
                    || score.temperature_delta_c.map(canonical_f64_bits)
                        == best_temperature.map(canonical_f64_bits))
        })
        .ok_or(LightCalibrationPlanError::AllocationFailed)?;
    Ok(LightMasterAssociation::Matched {
        group_id: selected.group_id.to_owned(),
        kind,
        temperature_basis: selected.temperature_basis,
        temperature_delta_c: selected.temperature_delta_c,
    })
}

fn validate_product(
    product: &LightCalibrationProductPlan,
) -> Result<(), LightCalibrationPlanError> {
    let mut candidate_ids = BTreeSet::new();
    for candidate in &product.candidates {
        if candidate.group_id.is_empty()
            || !candidate_ids.insert((candidate.kind, candidate.group_id.as_str()))
            || !valid_compatibility(candidate.kind, &candidate.compatibility)
        {
            return Err(LightCalibrationPlanError::InvalidCandidate {
                light_group_id: product.source_group_id.clone(),
                candidate_group_id: candidate.group_id.clone(),
            });
        }
    }
    validate_association(product, &product.dark)?;
    validate_association(product, &product.flat)?;
    Ok(())
}

fn valid_compatibility(
    kind: LightMasterKind,
    compatibility: &LightMasterCandidateCompatibility,
) -> bool {
    match compatibility {
        LightMasterCandidateCompatibility::Compatible {
            temperature_basis,
            temperature_delta_c,
        } => match kind {
            LightMasterKind::Dark => {
                temperature_basis.is_some()
                    && temperature_delta_c.is_some_and(|delta| delta.is_finite() && delta >= 0.0)
            }
            LightMasterKind::Flat => temperature_basis.is_none() && temperature_delta_c.is_none(),
        },
        LightMasterCandidateCompatibility::Rejected { mismatches } => {
            !mismatches.is_empty()
                && mismatches.windows(2).all(|pair| pair[0] < pair[1])
                && mismatches.iter().all(|mismatch| match kind {
                    LightMasterKind::Dark => mismatch.field != LightMasterMatchField::Filter,
                    LightMasterKind::Flat => !matches!(
                        mismatch.field,
                        LightMasterMatchField::Exposure
                            | LightMasterMatchField::SensorTemperature
                            | LightMasterMatchField::SetTemperature
                    ),
                })
        }
    }
}

fn validate_association(
    product: &LightCalibrationProductPlan,
    association: &LightMasterAssociation,
) -> Result<(), LightCalibrationPlanError> {
    match association {
        LightMasterAssociation::Matched {
            group_id,
            kind,
            temperature_basis,
            temperature_delta_c,
        } => {
            let valid_shape = match kind {
                LightMasterKind::Dark => {
                    temperature_basis.is_some()
                        && temperature_delta_c
                            .is_some_and(|delta| delta.is_finite() && delta >= 0.0)
                }
                LightMasterKind::Flat => {
                    temperature_basis.is_none() && temperature_delta_c.is_none()
                }
            };
            let candidate_matches = product.candidates.iter().any(|candidate| {
                candidate.group_id == *group_id
                    && candidate.kind == *kind
                    && matches!(
                        &candidate.compatibility,
                        LightMasterCandidateCompatibility::Compatible {
                            temperature_basis: candidate_basis,
                            temperature_delta_c: candidate_delta,
                        } if candidate_basis == temperature_basis
                            && candidate_delta.map(canonical_f64_bits)
                                == temperature_delta_c.map(canonical_f64_bits)
                    )
            });
            if group_id.is_empty()
                || !valid_shape
                || !candidate_matches
                || !matched_is_unique_best(product, *kind, group_id)
            {
                return Err(LightCalibrationPlanError::InvalidCandidate {
                    light_group_id: product.source_group_id.clone(),
                    candidate_group_id: group_id.clone(),
                });
            }
        }
        LightMasterAssociation::Unresolved {
            kind,
            blocking_reason,
        } => match blocking_reason {
            LightMasterBlockingReason::MissingLightMetadata { fields } => {
                if fields.is_empty()
                    || !fields.windows(2).all(|pair| pair[0] < pair[1])
                    || product.candidates.iter().any(|candidate| {
                        candidate.kind == *kind && candidate.compatibility.is_compatible()
                    })
                    || fields.iter().any(|field| match kind {
                        LightMasterKind::Dark => *field == LightMasterMatchField::Filter,
                        LightMasterKind::Flat => matches!(
                            field,
                            LightMasterMatchField::Exposure
                                | LightMasterMatchField::SensorTemperature
                                | LightMasterMatchField::SetTemperature
                        ),
                    })
                {
                    return Err(LightCalibrationPlanError::InvalidProduct {
                        group_id: product.source_group_id.clone(),
                    });
                }
            }
            LightMasterBlockingReason::NoCompatibleCandidate => {
                if product.candidates.iter().any(|candidate| {
                    candidate.kind == *kind && candidate.compatibility.is_compatible()
                }) {
                    return Err(LightCalibrationPlanError::InvalidProduct {
                        group_id: product.source_group_id.clone(),
                    });
                }
            }
            LightMasterBlockingReason::AmbiguousCandidates { group_ids } => {
                let expected = best_compatible_group_ids(product, *kind)?;
                if group_ids.len() < 2
                    || !group_ids.windows(2).all(|pair| pair[0] < pair[1])
                    || *group_ids != expected
                {
                    return Err(LightCalibrationPlanError::InvalidProduct {
                        group_id: product.source_group_id.clone(),
                    });
                }
            }
        },
    }
    Ok(())
}

fn matched_is_unique_best(
    product: &LightCalibrationProductPlan,
    kind: LightMasterKind,
    selected_group_id: &str,
) -> bool {
    let Ok(group_ids) = best_compatible_group_ids(product, kind) else {
        return false;
    };
    group_ids.len() == 1 && group_ids[0] == selected_group_id
}

fn best_compatible_group_ids(
    product: &LightCalibrationProductPlan,
    kind: LightMasterKind,
) -> Result<Vec<String>, LightCalibrationPlanError> {
    let best_temperature = product
        .candidates
        .iter()
        .filter(|candidate| candidate.kind == kind)
        .filter_map(|candidate| match &candidate.compatibility {
            LightMasterCandidateCompatibility::Compatible {
                temperature_delta_c,
                ..
            } => *temperature_delta_c,
            LightMasterCandidateCompatibility::Rejected { .. } => None,
        })
        .min_by(|left, right| left.total_cmp(right));
    let is_best = |candidate: &LightMasterCandidateEvaluation| {
        candidate.kind == kind
            && match &candidate.compatibility {
                LightMasterCandidateCompatibility::Compatible {
                    temperature_delta_c,
                    ..
                } => {
                    kind == LightMasterKind::Flat
                        || temperature_delta_c.map(canonical_f64_bits)
                            == best_temperature.map(canonical_f64_bits)
                }
                LightMasterCandidateCompatibility::Rejected { .. } => false,
            }
    };
    let count = product
        .candidates
        .iter()
        .filter(|candidate| is_best(candidate))
        .count();
    let mut group_ids = Vec::new();
    group_ids
        .try_reserve_exact(count)
        .map_err(|_| LightCalibrationPlanError::AllocationFailed)?;
    group_ids.extend(
        product
            .candidates
            .iter()
            .filter(|candidate| is_best(candidate))
            .map(|candidate| candidate.group_id.clone()),
    );
    group_ids.sort();
    Ok(group_ids)
}

fn canonicalize_association(association: &mut LightMasterAssociation) {
    match association {
        LightMasterAssociation::Matched {
            temperature_delta_c,
            ..
        } => *temperature_delta_c = temperature_delta_c.map(canonical_non_negative),
        LightMasterAssociation::Unresolved {
            blocking_reason, ..
        } => match blocking_reason {
            LightMasterBlockingReason::MissingLightMetadata { fields } => fields.sort(),
            LightMasterBlockingReason::AmbiguousCandidates { group_ids } => group_ids.sort(),
            LightMasterBlockingReason::NoCompatibleCandidate => {}
        },
    }
}

fn validate_candidate_evaluation_count(
    left: usize,
    right: usize,
) -> Result<(), LightCalibrationPlanError> {
    let attempted =
        left.checked_mul(right)
            .ok_or(LightCalibrationPlanError::TooManyCandidateEvaluations {
                attempted: usize::MAX,
                maximum: MAX_LIGHT_CALIBRATION_CANDIDATE_EVALUATIONS,
            })?;
    if attempted > MAX_LIGHT_CALIBRATION_CANDIDATE_EVALUATIONS {
        return Err(LightCalibrationPlanError::TooManyCandidateEvaluations {
            attempted,
            maximum: MAX_LIGHT_CALIBRATION_CANDIDATE_EVALUATIONS,
        });
    }
    Ok(())
}

fn canonical_non_negative(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}

fn canonical_f64_bits(value: f64) -> u64 {
    canonical_non_negative(value).to_bits()
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use aether_metadata::{
        BayerPattern, Binning, CameraModel, CanonicalMetadata, CanonicalValue, Confidence,
    };

    use super::*;
    use crate::{
        ClassificationPolicy, FlatPedestalPolicy, ManifestFile, ManifestGroup,
        ManifestValidationError, MasterPlanOptions, SourceFingerprint, classify_frame,
    };

    type TestResult<T = ()> = Result<T, Box<dyn Error>>;

    #[derive(Clone)]
    struct GroupSpec {
        id: &'static str,
        frame_type: FrameType,
        exposure_seconds: Option<f64>,
        sensor_temperature_c: Option<f64>,
        set_temperature_c: Option<f64>,
        gain: Option<f64>,
        offset: Option<f64>,
        camera: Option<CameraModel>,
        binning: Option<Binning>,
        filter: Option<&'static str>,
        bayer_pattern: Option<BayerPattern>,
        axes: Vec<u64>,
    }

    impl GroupSpec {
        fn new(id: &'static str, frame_type: FrameType, exposure_seconds: f64) -> Self {
            Self {
                id,
                frame_type,
                exposure_seconds: Some(exposure_seconds),
                sensor_temperature_c: Some(-10.0),
                set_temperature_c: Some(-10.0),
                gain: Some(120.0),
                offset: Some(30.0),
                camera: Some(CameraModel::ZwoAsi294McPro),
                binning: Some(Binning { x: 1, y: 1 }),
                filter: Some("UVIR"),
                bayer_pattern: Some(BayerPattern::Rggb),
                axes: vec![4_144, 2_822],
            }
        }

        fn metadata(&self) -> CanonicalMetadata {
            CanonicalMetadata {
                camera: self.camera.clone().map(|value| exact(value, "INSTRUME")),
                frame_type: Some(exact(self.frame_type.clone(), "IMAGETYP")),
                exposure_seconds: self.exposure_seconds.map(|value| exact(value, "EXPTIME")),
                sensor_temperature_c: self
                    .sensor_temperature_c
                    .map(|value| exact(value, "CCD-TEMP")),
                set_temperature_c: self.set_temperature_c.map(|value| exact(value, "SET-TEMP")),
                gain: self.gain.map(|value| exact(value, "GAIN")),
                offset: self.offset.map(|value| exact(value, "OFFSET")),
                binning: self.binning.map(|value| exact(value, "XBINNING")),
                filter: self.filter.map(|value| exact(value.to_owned(), "FILTER")),
                bayer_pattern: self
                    .bayer_pattern
                    .clone()
                    .map(|value| exact(value, "BAYERPAT")),
                issues: Vec::new(),
            }
        }
    }

    fn exact<T>(value: T, keyword: &str) -> CanonicalValue<T> {
        CanonicalValue::new(value, keyword, Confidence::Exact)
    }

    fn fingerprint(index: usize) -> Result<SourceFingerprint, ManifestValidationError> {
        let digit = b"0123456789abcdef"[index % 16] as char;
        SourceFingerprint::new(5_760, digit.to_string().repeat(64))
    }

    fn manifest(specs: &[GroupSpec]) -> TestResult<SessionManifest> {
        let mut files = Vec::new();
        let mut groups = Vec::new();
        for (index, spec) in specs.iter().enumerate() {
            let path = format!("frames/{}-{index}.fits", spec.id);
            let metadata = spec.metadata();
            let classification = classify_frame(Path::new(&path), &metadata);
            let file = ManifestFile::from_analysis(
                &path,
                fingerprint(index + 1)?,
                spec.axes.clone(),
                metadata,
                Vec::new(),
                classification,
                ClassificationPolicy::RequireAgreement,
            )?;
            let key = StrictGroupingKey::from_metadata(
                spec.frame_type.clone(),
                file.metadata(),
                file.axes(),
            )?;
            let missing = key.missing_fields();
            let rationale = (!missing.is_empty()).then(|| "explicit synthetic omission".to_owned());
            groups.push(ManifestGroup::new(
                spec.id,
                key,
                vec![path],
                missing,
                rationale,
            )?);
            files.push(file);
        }
        Ok(SessionManifest::new(
            ClassificationPolicy::RequireAgreement,
            files,
            groups,
        )?)
    }

    fn master_plan(session: &SessionManifest) -> TestResult<MasterPlan> {
        Ok(MasterPlan::from_manifest(
            session,
            MasterPlanOptions::new(FlatPedestalPolicy::RequireMatchedDark, 0.01, 1.0)?,
        )?)
    }

    fn options() -> TestResult<LightCalibrationPlanOptions> {
        Ok(LightCalibrationPlanOptions::new(2.0)?)
    }

    fn complete_specs() -> Vec<GroupSpec> {
        let light = GroupSpec::new("light-60s", FrameType::Light, 60.0);
        let mut light_dark = GroupSpec::new("dark-60s", FrameType::Dark, 60.0);
        light_dark.filter = Some("DARK");
        let flat = GroupSpec::new("flat-uvir", FrameType::Flat, 2.0);
        let mut flat_dark = GroupSpec::new("dark-2s", FrameType::Dark, 2.0);
        flat_dark.filter = Some("DARK");
        vec![light, light_dark, flat, flat_dark]
    }

    fn product(plan: &LightCalibrationPlan) -> TestResult<&LightCalibrationProductPlan> {
        plan.products()
            .first()
            .ok_or_else(|| std::io::Error::other("light product missing").into())
    }

    fn matched_group(association: &LightMasterAssociation) -> Option<&str> {
        match association {
            LightMasterAssociation::Matched { group_id, .. } => Some(group_id),
            LightMasterAssociation::Unresolved { .. } => None,
        }
    }

    #[test]
    fn selects_exact_dark_and_filter_flat_without_matching_dark_filter() -> TestResult {
        let session = manifest(&complete_specs())?;
        let masters = master_plan(&session)?;
        let plan =
            LightCalibrationPlan::from_manifest_and_master_plan(&session, &masters, options()?)?;
        let light = product(&plan)?;

        assert!(plan.is_ready());
        assert_eq!(matched_group(light.dark()), Some("dark-60s"));
        assert_eq!(matched_group(light.flat()), Some("flat-uvir"));
        assert_eq!(light.candidates().len(), 3);
        assert_eq!(plan.manifest_sha256(), masters.manifest_sha256());
        assert_eq!(plan.master_plan_sha256(), masters.canonical_sha256()?);
        Ok(())
    }

    #[test]
    fn refuses_nearby_dark_exposure_without_a_scaling_algorithm() -> TestResult {
        let mut specs = complete_specs();
        specs[1].exposure_seconds = Some(59.999);
        let session = manifest(&specs)?;
        let plan = LightCalibrationPlan::from_manifest_and_master_plan(
            &session,
            &master_plan(&session)?,
            options()?,
        )?;
        let light = product(&plan)?;

        assert!(!plan.is_ready());
        assert!(matches!(
            light.dark(),
            LightMasterAssociation::Unresolved {
                blocking_reason: LightMasterBlockingReason::NoCompatibleCandidate,
                ..
            }
        ));
        let dark = light
            .candidates()
            .iter()
            .find(|candidate| candidate.group_id() == "dark-60s")
            .ok_or("dark candidate missing")?;
        assert!(matches!(
            dark.compatibility(),
            LightMasterCandidateCompatibility::Rejected { mismatches }
                if mismatches.contains(&LightMasterMismatch {
                    field: LightMasterMatchField::Exposure,
                    reason: LightMasterMismatchReason::Different,
                })
        ));
        Ok(())
    }

    #[test]
    fn dark_temperature_tolerance_is_inclusive_and_prefers_closest() -> TestResult {
        let mut specs = complete_specs();
        specs[1].sensor_temperature_c = Some(-8.0);
        let mut closer = specs[1].clone();
        closer.id = "dark-60s-closer";
        closer.sensor_temperature_c = Some(-9.5);
        specs.push(closer);
        let session = manifest(&specs)?;
        let plan = LightCalibrationPlan::from_manifest_and_master_plan(
            &session,
            &master_plan(&session)?,
            options()?,
        )?;

        assert_eq!(
            matched_group(product(&plan)?.dark()),
            Some("dark-60s-closer")
        );
        Ok(())
    }

    #[test]
    fn flat_filter_mismatch_is_explicit_and_dark_filter_is_irrelevant() -> TestResult {
        let mut specs = complete_specs();
        specs[2].filter = Some("HA");
        let session = manifest(&specs)?;
        let plan = LightCalibrationPlan::from_manifest_and_master_plan(
            &session,
            &master_plan(&session)?,
            options()?,
        )?;
        let light = product(&plan)?;

        assert!(light.dark().is_resolved());
        assert!(!light.flat().is_resolved());
        let flat = light
            .candidates()
            .iter()
            .find(|candidate| candidate.group_id() == "flat-uvir")
            .ok_or("flat candidate missing")?;
        assert!(matches!(
            flat.compatibility(),
            LightMasterCandidateCompatibility::Rejected { mismatches }
                if mismatches.contains(&LightMasterMismatch {
                    field: LightMasterMatchField::Filter,
                    reason: LightMasterMismatchReason::Different,
                })
        ));
        Ok(())
    }

    #[test]
    fn equally_applicable_normalized_flats_remain_ambiguous() -> TestResult {
        let mut specs = complete_specs();
        let mut second_flat = specs[2].clone();
        second_flat.id = "flat-uvir-long";
        second_flat.exposure_seconds = Some(3.0);
        let mut second_flat_dark = specs[3].clone();
        second_flat_dark.id = "dark-3s";
        second_flat_dark.exposure_seconds = Some(3.0);
        specs.extend([second_flat, second_flat_dark]);
        let session = manifest(&specs)?;
        let plan = LightCalibrationPlan::from_manifest_and_master_plan(
            &session,
            &master_plan(&session)?,
            options()?,
        )?;

        assert!(matches!(
            product(&plan)?.flat(),
            LightMasterAssociation::Unresolved {
                blocking_reason: LightMasterBlockingReason::AmbiguousCandidates { group_ids },
                ..
            } if group_ids == &["flat-uvir".to_owned(), "flat-uvir-long".to_owned()]
        ));
        Ok(())
    }

    #[test]
    fn missing_light_filter_blocks_flat_selection_with_visible_fields() -> TestResult {
        let mut specs = complete_specs();
        specs[0].filter = None;
        let session = manifest(&specs)?;
        let plan = LightCalibrationPlan::from_manifest_and_master_plan(
            &session,
            &master_plan(&session)?,
            options()?,
        )?;

        assert!(matches!(
            product(&plan)?.flat(),
            LightMasterAssociation::Unresolved {
                blocking_reason: LightMasterBlockingReason::MissingLightMetadata { fields },
                ..
            } if fields == &[LightMasterMatchField::Filter]
        ));
        Ok(())
    }

    #[test]
    fn json_round_trip_is_canonical_and_rejects_a_tampered_selection() -> TestResult {
        let session = manifest(&complete_specs())?;
        let masters = master_plan(&session)?;
        let plan =
            LightCalibrationPlan::from_manifest_and_master_plan(&session, &masters, options()?)?;
        let encoded = plan.to_json_pretty()?;
        let decoded = LightCalibrationPlan::from_json_slice(&encoded)?;

        assert_eq!(decoded, plan);
        assert_eq!(decoded.to_json_pretty()?, encoded);
        assert_eq!(decoded.canonical_sha256()?.len(), 64);

        let mut value: serde_json::Value = serde_json::from_slice(&encoded)?;
        value["products"][0]["dark"]["group_id"] = serde_json::json!("dark-2s");
        let tampered = serde_json::to_vec(&value)?;
        assert!(matches!(
            LightCalibrationPlan::from_json_slice(&tampered),
            Err(LightCalibrationPlanError::InvalidCandidate { .. })
        ));

        let mut changed_delta: serde_json::Value = serde_json::from_slice(&encoded)?;
        changed_delta["products"][0]["dark"]["temperature_delta_c"] = serde_json::json!(0.5);
        assert!(matches!(
            LightCalibrationPlan::from_json_slice(&serde_json::to_vec(&changed_delta)?),
            Err(LightCalibrationPlanError::InvalidCandidate { .. })
        ));
        Ok(())
    }

    #[test]
    fn decoder_rejects_a_compatible_but_non_best_dark() -> TestResult {
        let mut specs = complete_specs();
        specs[1].sensor_temperature_c = Some(-8.0);
        let mut closer = specs[1].clone();
        closer.id = "dark-60s-closer";
        closer.sensor_temperature_c = Some(-9.5);
        specs.push(closer);
        let session = manifest(&specs)?;
        let plan = LightCalibrationPlan::from_manifest_and_master_plan(
            &session,
            &master_plan(&session)?,
            options()?,
        )?;
        let mut value: serde_json::Value = serde_json::from_slice(&plan.to_json_pretty()?)?;
        value["products"][0]["dark"]["group_id"] = serde_json::json!("dark-60s");
        value["products"][0]["dark"]["temperature_delta_c"] = serde_json::json!(2.0);

        assert!(matches!(
            LightCalibrationPlan::from_json_slice(&serde_json::to_vec(&value)?),
            Err(LightCalibrationPlanError::InvalidCandidate { .. })
        ));
        Ok(())
    }

    #[test]
    fn decoder_rejects_unknown_versions_fields_and_oversized_input() -> TestResult {
        let session = manifest(&complete_specs())?;
        let plan = LightCalibrationPlan::from_manifest_and_master_plan(
            &session,
            &master_plan(&session)?,
            options()?,
        )?;
        let mut value: serde_json::Value = serde_json::from_slice(&plan.to_json_pretty()?)?;
        value["schema_version"] = serde_json::json!(99);
        assert!(matches!(
            LightCalibrationPlan::from_json_slice(&serde_json::to_vec(&value)?),
            Err(LightCalibrationPlanError::UnsupportedSchemaVersion { found: 99 })
        ));

        value["schema_version"] = serde_json::json!(LIGHT_CALIBRATION_PLAN_SCHEMA_VERSION);
        value["unexpected"] = serde_json::json!(true);
        assert!(matches!(
            LightCalibrationPlan::from_json_slice(&serde_json::to_vec(&value)?),
            Err(LightCalibrationPlanError::Json(_))
        ));

        let oversized = vec![b' '; MAX_LIGHT_CALIBRATION_PLAN_BYTES + 1];
        assert!(matches!(
            LightCalibrationPlan::from_json_slice(&oversized),
            Err(LightCalibrationPlanError::PlanTooLarge { .. })
        ));
        Ok(())
    }

    #[test]
    fn rejects_a_master_plan_from_another_manifest() -> TestResult {
        let first = manifest(&complete_specs())?;
        let masters = master_plan(&first)?;
        let mut changed_specs = complete_specs();
        changed_specs[0].gain = Some(121.0);
        let changed = manifest(&changed_specs)?;

        assert!(matches!(
            LightCalibrationPlan::from_manifest_and_master_plan(&changed, &masters, options()?),
            Err(LightCalibrationPlanError::MasterPlanManifestMismatch)
        ));
        Ok(())
    }

    #[test]
    fn rejects_an_incomplete_but_structurally_valid_master_plan() -> TestResult {
        let session = manifest(&complete_specs())?;
        let masters = master_plan(&session)?;
        let mut value: serde_json::Value = serde_json::from_slice(&masters.to_json_pretty()?)?;
        let products = value["products"]
            .as_array_mut()
            .ok_or("master product list missing")?;
        products.retain(|product| product["source_group_id"] != "dark-60s");
        let incomplete = MasterPlan::from_json_slice(&serde_json::to_vec(&value)?)?;

        assert!(matches!(
            LightCalibrationPlan::from_manifest_and_master_plan(&session, &incomplete, options()?),
            Err(LightCalibrationPlanError::MasterPlanContentMismatch)
        ));
        Ok(())
    }

    #[test]
    fn validates_options_and_candidate_matrix_bound() {
        assert!(matches!(
            LightCalibrationPlanOptions::new(f64::NAN),
            Err(LightCalibrationPlanError::InvalidDarkTemperatureTolerance { .. })
        ));
        assert!(matches!(
            validate_candidate_evaluation_count(501, 200),
            Err(LightCalibrationPlanError::TooManyCandidateEvaluations {
                attempted: 100_200,
                maximum: MAX_LIGHT_CALIBRATION_CANDIDATE_EVALUATIONS,
            })
        ));
    }

    #[test]
    fn role_specific_missing_fields_do_not_cross_contaminate() -> TestResult {
        let mut specs = complete_specs();
        specs[0].exposure_seconds = None;
        specs[0].filter = None;
        let session = manifest(&specs)?;
        let plan = LightCalibrationPlan::from_manifest_and_master_plan(
            &session,
            &master_plan(&session)?,
            options()?,
        )?;
        let light = product(&plan)?;

        assert!(matches!(
            light.dark(),
            LightMasterAssociation::Unresolved {
                blocking_reason: LightMasterBlockingReason::MissingLightMetadata { fields },
                ..
            } if fields == &[LightMasterMatchField::Exposure]
        ));
        assert!(matches!(
            light.flat(),
            LightMasterAssociation::Unresolved {
                blocking_reason: LightMasterBlockingReason::MissingLightMetadata { fields },
                ..
            } if fields == &[LightMasterMatchField::Filter]
        ));
        assert!(
            light
                .candidates()
                .iter()
                .filter(|candidate| candidate.kind() == LightMasterKind::Dark)
                .all(|candidate| match candidate.compatibility() {
                    LightMasterCandidateCompatibility::Rejected { mismatches } => mismatches
                        .iter()
                        .all(|mismatch| mismatch.field() != LightMasterMatchField::Filter),
                    LightMasterCandidateCompatibility::Compatible { .. } => true,
                })
        );
        assert!(
            light
                .candidates()
                .iter()
                .filter(|candidate| candidate.kind() == LightMasterKind::Flat)
                .all(|candidate| match candidate.compatibility() {
                    LightMasterCandidateCompatibility::Rejected { mismatches } => mismatches
                        .iter()
                        .all(|mismatch| mismatch.field() != LightMasterMatchField::Exposure),
                    LightMasterCandidateCompatibility::Compatible { .. } => true,
                })
        );
        Ok(())
    }
}
