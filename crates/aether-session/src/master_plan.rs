use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_metadata::FrameType;
use serde::{Deserialize, Serialize};

use crate::{ManifestError, ManifestGroup, SessionManifest, StrictGroupingKey};

/// Schema version emitted for master-calibration plans.
pub const MASTER_PLAN_SCHEMA_VERSION: u32 = 1;
/// Largest master-plan JSON document accepted by the decoder (16 MiB).
pub const MAX_MASTER_PLAN_BYTES: usize = 16 * 1_024 * 1_024;
/// Maximum number of flat-to-pedestal comparisons retained in one plan.
pub const MAX_MASTER_PLAN_CANDIDATE_EVALUATIONS: usize = 100_000;

/// Explicit policy used to select the pedestal correction for a flat group.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FlatPedestalPolicy {
    /// Require a compatible short-exposure dark and never fall back to a bias.
    RequireMatchedDark,
    /// Require a compatible true bias and ignore compatible dark candidates.
    RequireBias,
    /// Prefer a compatible short dark, then use a bias only when no dark matches.
    PreferMatchedDarkThenBias,
}

/// Validated, unit-bearing matching controls for flat pedestal calibration.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MasterPlanOptions {
    flat_pedestal_policy: FlatPedestalPolicy,
    maximum_exposure_delta_seconds: f64,
    maximum_temperature_delta_c: f64,
}

impl MasterPlanOptions {
    /// Builds explicit flat-calibrator matching controls.
    ///
    /// Both tolerances are inclusive. No default is supplied because changing a
    /// tolerance can select a different calibration source and must therefore be
    /// a visible scientific decision.
    ///
    /// # Errors
    ///
    /// Returns a typed error if either tolerance is negative, NaN, or infinite.
    pub fn new(
        flat_pedestal_policy: FlatPedestalPolicy,
        maximum_exposure_delta_seconds: f64,
        maximum_temperature_delta_c: f64,
    ) -> Result<Self, MasterPlanError> {
        let options = Self {
            flat_pedestal_policy,
            maximum_exposure_delta_seconds: canonical_non_negative(maximum_exposure_delta_seconds),
            maximum_temperature_delta_c: canonical_non_negative(maximum_temperature_delta_c),
        };
        options.validate()?;
        Ok(options)
    }

    /// Flat pedestal-selection policy.
    #[must_use]
    pub const fn flat_pedestal_policy(self) -> FlatPedestalPolicy {
        self.flat_pedestal_policy
    }

    /// Inclusive dark-to-flat exposure tolerance in seconds.
    #[must_use]
    pub const fn maximum_exposure_delta_seconds(self) -> f64 {
        self.maximum_exposure_delta_seconds
    }

    /// Inclusive calibrator-to-flat temperature tolerance in degrees Celsius.
    #[must_use]
    pub const fn maximum_temperature_delta_c(self) -> f64 {
        self.maximum_temperature_delta_c
    }

    fn validate(self) -> Result<(), MasterPlanError> {
        if !valid_tolerance(self.maximum_exposure_delta_seconds) {
            return Err(MasterPlanError::InvalidExposureTolerance {
                value: self.maximum_exposure_delta_seconds,
            });
        }
        if !valid_tolerance(self.maximum_temperature_delta_c) {
            return Err(MasterPlanError::InvalidTemperatureTolerance {
                value: self.maximum_temperature_delta_c,
            });
        }
        Ok(())
    }
}

/// Scientific role of one planned master product.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MasterProductKind {
    /// Master generated from a true-bias group.
    Bias,
    /// Master generated from a dark group, including short-exposure darks.
    Dark,
    /// Normalized master generated from a flat group.
    Flat,
}

/// Candidate role considered for flat pedestal correction.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PedestalSourceKind {
    /// Short-exposure dark candidate.
    Dark,
    /// True-bias candidate.
    Bias,
}

/// Temperature metadata used to compare a flat with a pedestal candidate.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TemperatureBasis {
    /// Measured sensor temperatures were available for both groups.
    Sensor,
    /// Set-point temperatures were used because the flat lacks a measurement.
    SetPoint,
}

/// Field that can prevent a pedestal candidate from calibrating a flat.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PedestalMatchField {
    /// Canonical camera model.
    Camera,
    /// Image dimensions in FITS axis order.
    Axes,
    /// Exposure duration, applicable to dark candidates only.
    Exposure,
    /// Measured sensor temperature.
    SensorTemperature,
    /// Requested sensor temperature.
    SetTemperature,
    /// Camera gain.
    Gain,
    /// Camera digital offset.
    Offset,
    /// Horizontal and vertical binning.
    Binning,
    /// CFA pattern and phase at the stored image origin.
    BayerPattern,
}

/// Reason a matching field rejected one candidate.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PedestalMismatchReason {
    /// A required value is absent from the flat or candidate grouping key.
    Missing,
    /// Both exact values exist but differ.
    Different,
    /// The absolute numerical delta exceeds the explicit inclusive tolerance.
    OutsideTolerance,
}

/// One stable, machine-readable reason for rejecting a pedestal candidate.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PedestalMismatch {
    field: PedestalMatchField,
    reason: PedestalMismatchReason,
}

impl PedestalMismatch {
    /// Field whose comparison failed.
    #[must_use]
    pub const fn field(self) -> PedestalMatchField {
        self.field
    }

    /// Nature of the comparison failure.
    #[must_use]
    pub const fn reason(self) -> PedestalMismatchReason {
        self.reason
    }
}

/// Result of comparing one bias or dark group with one flat group.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum PedestalCandidateCompatibility {
    /// All required fields match under the explicit tolerances.
    Compatible {
        /// Absolute exposure difference in seconds; absent for a bias.
        exposure_delta_seconds: Option<f64>,
        /// Temperature field used for matching.
        temperature_basis: TemperatureBasis,
        /// Absolute temperature difference in degrees Celsius.
        temperature_delta_c: f64,
    },
    /// At least one required field is missing, different, or out of tolerance.
    Rejected {
        /// Stable reasons in field declaration order.
        mismatches: Vec<PedestalMismatch>,
    },
}

/// Inspectable evaluation of one possible flat pedestal source.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PedestalCandidateEvaluation {
    group_id: String,
    source_kind: PedestalSourceKind,
    compatibility: PedestalCandidateCompatibility,
}

impl PedestalCandidateEvaluation {
    /// Session group containing the candidate frames.
    #[must_use]
    pub fn group_id(&self) -> &str {
        &self.group_id
    }

    /// Whether the candidate is a dark or a true bias.
    #[must_use]
    pub const fn source_kind(&self) -> PedestalSourceKind {
        self.source_kind
    }

    /// Complete compatibility result retained for diagnostics and UI display.
    #[must_use]
    pub const fn compatibility(&self) -> &PedestalCandidateCompatibility {
        &self.compatibility
    }
}

/// Blocking state when the planner cannot select exactly one pedestal source.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "reason")]
pub enum FlatPedestalBlockingReason {
    /// The flat lacks metadata required for conservative automatic matching.
    MissingFlatMetadata {
        /// Missing fields in stable declaration order.
        fields: Vec<PedestalMatchField>,
    },
    /// No dark satisfies the selected policy and matching tolerances.
    NoCompatibleDark,
    /// No bias satisfies the selected policy and matching tolerances.
    NoCompatibleBias,
    /// Neither a dark nor a bias satisfies the fallback policy.
    NoCompatibleDarkOrBias,
    /// Multiple darks have exactly the same best exposure and temperature score.
    AmbiguousDark {
        /// Equally ranked candidate group identifiers in lexical order.
        group_ids: Vec<String>,
    },
    /// Multiple biases have exactly the same best temperature score.
    AmbiguousBias {
        /// Equally ranked candidate group identifiers in lexical order.
        group_ids: Vec<String>,
    },
}

/// Exclusive flat pedestal choice; the type cannot represent double subtraction.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "selection")]
pub enum FlatPedestalAssociation {
    /// A unique compatible short-exposure dark was selected.
    MatchedDark {
        /// Selected dark session group.
        group_id: String,
        /// Absolute exposure difference in seconds.
        exposure_delta_seconds: f64,
        /// Temperature field used for matching.
        temperature_basis: TemperatureBasis,
        /// Absolute temperature difference in degrees Celsius.
        temperature_delta_c: f64,
    },
    /// A unique compatible true bias was selected.
    Bias {
        /// Selected bias session group.
        group_id: String,
        /// Temperature field used for matching.
        temperature_basis: TemperatureBasis,
        /// Absolute temperature difference in degrees Celsius.
        temperature_delta_c: f64,
    },
    /// No scientifically unambiguous automatic association exists.
    Unresolved {
        /// Explicit reason processing must remain blocked.
        blocking_reason: FlatPedestalBlockingReason,
    },
}

impl FlatPedestalAssociation {
    /// Returns true when exactly one dark or bias has been selected.
    #[must_use]
    pub const fn is_resolved(&self) -> bool {
        matches!(self, Self::MatchedDark { .. } | Self::Bias { .. })
    }
}

/// One bias, dark, or flat master to construct from an exact session group.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MasterProductPlan {
    source_group_id: String,
    kind: MasterProductKind,
    flat_pedestal: Option<FlatPedestalAssociation>,
    pedestal_candidates: Vec<PedestalCandidateEvaluation>,
}

impl MasterProductPlan {
    /// Exact source group integrated into the master.
    #[must_use]
    pub fn source_group_id(&self) -> &str {
        &self.source_group_id
    }

    /// Scientific role of the output master.
    #[must_use]
    pub const fn kind(&self) -> MasterProductKind {
        self.kind
    }

    /// Exclusive flat pedestal choice; absent for bias and dark masters.
    #[must_use]
    pub const fn flat_pedestal(&self) -> Option<&FlatPedestalAssociation> {
        self.flat_pedestal.as_ref()
    }

    /// Every bias and dark candidate considered for this flat.
    #[must_use]
    pub fn pedestal_candidates(&self) -> &[PedestalCandidateEvaluation] {
        &self.pedestal_candidates
    }
}

/// Versioned and deterministic plan for constructing calibration masters.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MasterPlan {
    schema_version: u32,
    manifest_sha256: String,
    options: MasterPlanOptions,
    products: Vec<MasterProductPlan>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MasterPlanWire {
    schema_version: u32,
    manifest_sha256: String,
    options: MasterPlanOptions,
    products: Vec<MasterProductPlan>,
}

#[derive(Deserialize)]
struct SchemaVersionProbe {
    schema_version: u32,
}

impl MasterPlan {
    /// Builds one plan from exact session groups and explicit matching controls.
    ///
    /// Bias and dark groups always produce independent master products. Each flat
    /// produces a product plus a single exclusive pedestal association. Lights
    /// and unrecognized frame types do not produce master products.
    ///
    /// # Errors
    ///
    /// Returns a typed error if the options are invalid, the session manifest
    /// cannot be fingerprinted, or the resulting plan violates an invariant.
    pub fn from_manifest(
        manifest: &SessionManifest,
        options: MasterPlanOptions,
    ) -> Result<Self, MasterPlanError> {
        options.validate()?;
        let manifest_sha256 = manifest
            .canonical_sha256()
            .map_err(MasterPlanError::Manifest)?;
        let candidate_count = manifest
            .groups()
            .iter()
            .filter(|group| matches!(group.key().frame_type(), FrameType::Bias | FrameType::Dark))
            .count();
        let flat_count = manifest
            .groups()
            .iter()
            .filter(|group| matches!(group.key().frame_type(), FrameType::Flat))
            .count();
        validate_candidate_evaluation_count(flat_count, candidate_count)?;
        let mut candidates = Vec::new();
        candidates
            .try_reserve_exact(candidate_count)
            .map_err(|_| MasterPlanError::AllocationFailed)?;
        candidates.extend(
            manifest.groups().iter().filter(|group| {
                matches!(group.key().frame_type(), FrameType::Bias | FrameType::Dark)
            }),
        );

        let mut products = Vec::new();
        products
            .try_reserve_exact(
                manifest
                    .groups()
                    .iter()
                    .filter(|group| {
                        matches!(
                            group.key().frame_type(),
                            FrameType::Bias | FrameType::Dark | FrameType::Flat
                        )
                    })
                    .count(),
            )
            .map_err(|_| MasterPlanError::AllocationFailed)?;

        for group in manifest.groups() {
            let (kind, flat_pedestal, pedestal_candidates) = match group.key().frame_type() {
                FrameType::Bias => (MasterProductKind::Bias, None, Vec::new()),
                FrameType::Dark => (MasterProductKind::Dark, None, Vec::new()),
                FrameType::Flat => {
                    let (association, evaluations) =
                        plan_flat_pedestal(group, &candidates, options)?;
                    (MasterProductKind::Flat, Some(association), evaluations)
                }
                FrameType::Light | FrameType::Other(_) => continue,
            };
            products.push(MasterProductPlan {
                source_group_id: group.id().to_owned(),
                kind,
                flat_pedestal,
                pedestal_candidates,
            });
        }

        let mut plan = Self {
            schema_version: MASTER_PLAN_SCHEMA_VERSION,
            manifest_sha256,
            options,
            products,
        };
        plan.canonicalize();
        plan.validate()?;
        Ok(plan)
    }

    /// Decodes and validates a master plan from UTF-8 JSON.
    ///
    /// # Errors
    ///
    /// Returns a size, JSON, version, option, or semantic validation error.
    pub fn from_json_slice(input: &[u8]) -> Result<Self, MasterPlanError> {
        if input.len() > MAX_MASTER_PLAN_BYTES {
            return Err(MasterPlanError::PlanTooLarge { bytes: input.len() });
        }
        let version: SchemaVersionProbe =
            serde_json::from_slice(input).map_err(MasterPlanError::Json)?;
        if version.schema_version != MASTER_PLAN_SCHEMA_VERSION {
            return Err(MasterPlanError::UnsupportedSchemaVersion {
                found: version.schema_version,
            });
        }
        let wire: MasterPlanWire = serde_json::from_slice(input).map_err(MasterPlanError::Json)?;
        let mut plan = Self {
            schema_version: wire.schema_version,
            manifest_sha256: wire.manifest_sha256,
            options: wire.options,
            products: wire.products,
        };
        plan.canonicalize();
        plan.validate()?;
        Ok(plan)
    }

    /// Encodes deterministic, human-readable JSON terminated by one newline.
    ///
    /// # Errors
    ///
    /// Returns an invariant or JSON encoding error.
    pub fn to_json_pretty(&self) -> Result<Vec<u8>, MasterPlanError> {
        self.validate()?;
        let mut output = serde_json::to_vec_pretty(self).map_err(MasterPlanError::Json)?;
        output.push(b'\n');
        Ok(output)
    }

    /// Master-plan schema version.
    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// SHA-256 of the exact canonical session manifest used to build this plan.
    #[must_use]
    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }

    /// Matching controls embedded in this plan.
    #[must_use]
    pub const fn options(&self) -> MasterPlanOptions {
        self.options
    }

    /// Canonically ordered master products.
    #[must_use]
    pub fn products(&self) -> &[MasterProductPlan] {
        &self.products
    }

    /// Returns true only when every flat has exactly one pedestal source.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.products.iter().all(|product| {
            product
                .flat_pedestal
                .as_ref()
                .is_none_or(FlatPedestalAssociation::is_resolved)
        })
    }

    fn canonicalize(&mut self) {
        self.options.maximum_exposure_delta_seconds =
            canonical_non_negative(self.options.maximum_exposure_delta_seconds);
        self.options.maximum_temperature_delta_c =
            canonical_non_negative(self.options.maximum_temperature_delta_c);
        self.products
            .sort_by(|left, right| left.source_group_id.cmp(&right.source_group_id));
        for product in &mut self.products {
            product
                .pedestal_candidates
                .sort_by(|left, right| left.group_id.cmp(&right.group_id));
            for candidate in &mut product.pedestal_candidates {
                match &mut candidate.compatibility {
                    PedestalCandidateCompatibility::Compatible {
                        exposure_delta_seconds,
                        temperature_delta_c,
                        ..
                    } => {
                        *exposure_delta_seconds =
                            exposure_delta_seconds.map(canonical_non_negative);
                        *temperature_delta_c = canonical_non_negative(*temperature_delta_c);
                    }
                    PedestalCandidateCompatibility::Rejected { mismatches } => mismatches.sort(),
                }
            }
            if let Some(association) = &mut product.flat_pedestal {
                match association {
                    FlatPedestalAssociation::MatchedDark {
                        exposure_delta_seconds,
                        temperature_delta_c,
                        ..
                    } => {
                        *exposure_delta_seconds = canonical_non_negative(*exposure_delta_seconds);
                        *temperature_delta_c = canonical_non_negative(*temperature_delta_c);
                    }
                    FlatPedestalAssociation::Bias {
                        temperature_delta_c,
                        ..
                    } => *temperature_delta_c = canonical_non_negative(*temperature_delta_c),
                    FlatPedestalAssociation::Unresolved { blocking_reason } => {
                        match blocking_reason {
                            FlatPedestalBlockingReason::MissingFlatMetadata { fields } => {
                                fields.sort();
                            }
                            FlatPedestalBlockingReason::AmbiguousDark { group_ids }
                            | FlatPedestalBlockingReason::AmbiguousBias { group_ids } => {
                                group_ids.sort();
                            }
                            FlatPedestalBlockingReason::NoCompatibleDark
                            | FlatPedestalBlockingReason::NoCompatibleBias
                            | FlatPedestalBlockingReason::NoCompatibleDarkOrBias => {}
                        }
                    }
                }
            }
        }
    }

    fn validate(&self) -> Result<(), MasterPlanError> {
        if self.schema_version != MASTER_PLAN_SCHEMA_VERSION {
            return Err(MasterPlanError::UnsupportedSchemaVersion {
                found: self.schema_version,
            });
        }
        if !is_lower_sha256(&self.manifest_sha256) {
            return Err(MasterPlanError::InvalidManifestSha256);
        }
        self.options.validate()?;
        let candidate_evaluations = self.products.iter().try_fold(0_usize, |total, product| {
            total.checked_add(product.pedestal_candidates.len()).ok_or(
                MasterPlanError::TooManyCandidateEvaluations {
                    attempted: usize::MAX,
                    maximum: MAX_MASTER_PLAN_CANDIDATE_EVALUATIONS,
                },
            )
        })?;
        validate_candidate_evaluation_count(1, candidate_evaluations)?;

        let mut product_ids = BTreeSet::new();
        for product in &self.products {
            if product.source_group_id.is_empty()
                || !product_ids.insert(product.source_group_id.as_str())
            {
                return Err(MasterPlanError::InvalidProduct {
                    group_id: product.source_group_id.clone(),
                });
            }
            match product.kind {
                MasterProductKind::Bias | MasterProductKind::Dark => {
                    if product.flat_pedestal.is_some() || !product.pedestal_candidates.is_empty() {
                        return Err(MasterPlanError::InvalidProduct {
                            group_id: product.source_group_id.clone(),
                        });
                    }
                }
                MasterProductKind::Flat => {
                    let Some(association) = &product.flat_pedestal else {
                        return Err(MasterPlanError::InvalidProduct {
                            group_id: product.source_group_id.clone(),
                        });
                    };
                    validate_flat_product(product, association, self.options)?;
                }
            }
        }
        Ok(())
    }
}

/// Failure while constructing, encoding, or decoding a master plan.
#[derive(Debug)]
pub enum MasterPlanError {
    /// Dark-to-flat exposure tolerance is negative or non-finite.
    InvalidExposureTolerance {
        /// Rejected value in seconds.
        value: f64,
    },
    /// Calibrator-to-flat temperature tolerance is negative or non-finite.
    InvalidTemperatureTolerance {
        /// Rejected value in degrees Celsius.
        value: f64,
    },
    /// The source manifest could not be encoded and fingerprinted.
    Manifest(ManifestError),
    /// The JSON representation is malformed or contains unknown fields.
    Json(serde_json::Error),
    /// The document uses a schema version this release cannot interpret.
    UnsupportedSchemaVersion {
        /// Version found in the document.
        found: u32,
    },
    /// Input exceeds the bounded in-memory JSON parser limit.
    PlanTooLarge {
        /// Received byte count.
        bytes: usize,
    },
    /// The retained manifest fingerprint is not canonical lowercase SHA-256.
    InvalidManifestSha256,
    /// A product has a duplicate identifier or role-inconsistent fields.
    InvalidProduct {
        /// Offending source group identifier.
        group_id: String,
    },
    /// A candidate result is duplicated, empty, non-finite, or inconsistent.
    InvalidCandidate {
        /// Flat group containing the invalid candidate.
        flat_group_id: String,
        /// Candidate group identifier, when available.
        candidate_group_id: String,
    },
    /// The inspectable candidate matrix exceeds the documented bound.
    TooManyCandidateEvaluations {
        /// Comparisons required by the supplied groups.
        attempted: usize,
        /// Maximum comparisons retained by one plan.
        maximum: usize,
    },
    /// Memory reservation for the bounded plan structure failed.
    AllocationFailed,
}

impl Display for MasterPlanError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidExposureTolerance { value } => write!(
                formatter,
                "flat dark exposure tolerance must be finite and non-negative, received {value}"
            ),
            Self::InvalidTemperatureTolerance { value } => write!(
                formatter,
                "flat calibrator temperature tolerance must be finite and non-negative, received {value}"
            ),
            Self::Manifest(error) => Display::fmt(error, formatter),
            Self::Json(error) => write!(formatter, "invalid master-plan JSON: {error}"),
            Self::UnsupportedSchemaVersion { found } => {
                write!(formatter, "unsupported master-plan schema version {found}")
            }
            Self::PlanTooLarge { bytes } => {
                write!(
                    formatter,
                    "master-plan JSON exceeds the limit at {bytes} bytes"
                )
            }
            Self::InvalidManifestSha256 => {
                formatter.write_str("master plan contains an invalid manifest SHA-256")
            }
            Self::InvalidProduct { group_id } => {
                write!(formatter, "invalid master product for group `{group_id}`")
            }
            Self::InvalidCandidate {
                flat_group_id,
                candidate_group_id,
            } => write!(
                formatter,
                "invalid pedestal candidate `{candidate_group_id}` for flat `{flat_group_id}`"
            ),
            Self::TooManyCandidateEvaluations { attempted, maximum } => write!(
                formatter,
                "master plan requires {attempted} flat pedestal comparisons; limit is {maximum}"
            ),
            Self::AllocationFailed => formatter.write_str("cannot allocate master plan"),
        }
    }
}

impl Error for MasterPlanError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Manifest(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::InvalidExposureTolerance { .. }
            | Self::InvalidTemperatureTolerance { .. }
            | Self::UnsupportedSchemaVersion { .. }
            | Self::PlanTooLarge { .. }
            | Self::InvalidManifestSha256
            | Self::InvalidProduct { .. }
            | Self::InvalidCandidate { .. }
            | Self::TooManyCandidateEvaluations { .. }
            | Self::AllocationFailed => None,
        }
    }
}

#[derive(Clone, Copy)]
struct CompatibleScore<'a> {
    group_id: &'a str,
    source_kind: PedestalSourceKind,
    exposure_delta_seconds: Option<f64>,
    temperature_basis: TemperatureBasis,
    temperature_delta_c: f64,
}

fn plan_flat_pedestal(
    flat: &ManifestGroup,
    candidates: &[&ManifestGroup],
    options: MasterPlanOptions,
) -> Result<(FlatPedestalAssociation, Vec<PedestalCandidateEvaluation>), MasterPlanError> {
    let missing_flat_fields = missing_flat_match_fields(flat.key());
    let mut evaluations = Vec::new();
    let mut compatible = Vec::new();
    evaluations
        .try_reserve_exact(candidates.len())
        .map_err(|_| MasterPlanError::AllocationFailed)?;
    compatible
        .try_reserve_exact(candidates.len())
        .map_err(|_| MasterPlanError::AllocationFailed)?;

    for candidate in candidates {
        let source_kind = match candidate.key().frame_type() {
            FrameType::Dark => PedestalSourceKind::Dark,
            FrameType::Bias => PedestalSourceKind::Bias,
            FrameType::Flat | FrameType::Light | FrameType::Other(_) => continue,
        };
        let (compatibility, score) =
            compare_pedestal(flat.key(), candidate.key(), source_kind, options);
        if let Some(score) = score {
            compatible.push(CompatibleScore {
                group_id: candidate.id(),
                source_kind,
                exposure_delta_seconds: score.exposure_delta_seconds,
                temperature_basis: score.temperature_basis,
                temperature_delta_c: score.temperature_delta_c,
            });
        }
        evaluations.push(PedestalCandidateEvaluation {
            group_id: candidate.id().to_owned(),
            source_kind,
            compatibility,
        });
    }

    if !missing_flat_fields.is_empty() {
        return Ok((
            FlatPedestalAssociation::Unresolved {
                blocking_reason: FlatPedestalBlockingReason::MissingFlatMetadata {
                    fields: missing_flat_fields,
                },
            },
            evaluations,
        ));
    }

    let association = select_compatible(&compatible, options.flat_pedestal_policy)?;
    Ok((association, evaluations))
}

#[derive(Clone, Copy)]
struct ComparisonScore {
    exposure_delta_seconds: Option<f64>,
    temperature_basis: TemperatureBasis,
    temperature_delta_c: f64,
}

fn compare_pedestal(
    flat: &StrictGroupingKey,
    candidate: &StrictGroupingKey,
    source_kind: PedestalSourceKind,
    options: MasterPlanOptions,
) -> (PedestalCandidateCompatibility, Option<ComparisonScore>) {
    let mut mismatches = Vec::new();
    compare_exact_option(
        flat.camera(),
        candidate.camera(),
        PedestalMatchField::Camera,
        &mut mismatches,
    );
    if flat.axes() != candidate.axes() {
        mismatches.push(PedestalMismatch {
            field: PedestalMatchField::Axes,
            reason: PedestalMismatchReason::Different,
        });
    }
    compare_f64_option(
        flat.gain(),
        candidate.gain(),
        PedestalMatchField::Gain,
        &mut mismatches,
    );
    compare_f64_option(
        flat.offset(),
        candidate.offset(),
        PedestalMatchField::Offset,
        &mut mismatches,
    );
    compare_exact_option(
        flat.binning(),
        candidate.binning(),
        PedestalMatchField::Binning,
        &mut mismatches,
    );
    compare_exact_option(
        flat.bayer_pattern(),
        candidate.bayer_pattern(),
        PedestalMatchField::BayerPattern,
        &mut mismatches,
    );

    let exposure_delta_seconds = if source_kind == PedestalSourceKind::Dark {
        compare_tolerated(
            flat.exposure_seconds(),
            candidate.exposure_seconds(),
            options.maximum_exposure_delta_seconds,
            PedestalMatchField::Exposure,
            &mut mismatches,
        )
    } else {
        None
    };

    let (temperature_basis, temperature_delta_c) = compare_temperature(
        flat,
        candidate,
        options.maximum_temperature_delta_c,
        &mut mismatches,
    );

    if mismatches.is_empty() {
        let Some(temperature_basis) = temperature_basis else {
            return rejected_missing_temperature(source_kind);
        };
        let Some(temperature_delta_c) = temperature_delta_c else {
            return rejected_missing_temperature(source_kind);
        };
        let score = ComparisonScore {
            exposure_delta_seconds,
            temperature_basis,
            temperature_delta_c,
        };
        (
            PedestalCandidateCompatibility::Compatible {
                exposure_delta_seconds,
                temperature_basis,
                temperature_delta_c,
            },
            Some(score),
        )
    } else {
        (
            PedestalCandidateCompatibility::Rejected { mismatches },
            None,
        )
    }
}

fn rejected_missing_temperature(
    _source_kind: PedestalSourceKind,
) -> (PedestalCandidateCompatibility, Option<ComparisonScore>) {
    (
        PedestalCandidateCompatibility::Rejected {
            mismatches: vec![PedestalMismatch {
                field: PedestalMatchField::SensorTemperature,
                reason: PedestalMismatchReason::Missing,
            }],
        },
        None,
    )
}

fn compare_temperature(
    flat: &StrictGroupingKey,
    candidate: &StrictGroupingKey,
    tolerance: f64,
    mismatches: &mut Vec<PedestalMismatch>,
) -> (Option<TemperatureBasis>, Option<f64>) {
    if flat.sensor_temperature_c().is_some() {
        let delta = compare_tolerated(
            flat.sensor_temperature_c(),
            candidate.sensor_temperature_c(),
            tolerance,
            PedestalMatchField::SensorTemperature,
            mismatches,
        );
        (Some(TemperatureBasis::Sensor), delta)
    } else {
        let delta = compare_tolerated(
            flat.set_temperature_c(),
            candidate.set_temperature_c(),
            tolerance,
            PedestalMatchField::SetTemperature,
            mismatches,
        );
        (Some(TemperatureBasis::SetPoint), delta)
    }
}

fn compare_tolerated(
    left: Option<f64>,
    right: Option<f64>,
    tolerance: f64,
    field: PedestalMatchField,
    mismatches: &mut Vec<PedestalMismatch>,
) -> Option<f64> {
    let (Some(left), Some(right)) = (left, right) else {
        mismatches.push(PedestalMismatch {
            field,
            reason: PedestalMismatchReason::Missing,
        });
        return None;
    };
    let delta = (left - right).abs();
    if delta > tolerance {
        mismatches.push(PedestalMismatch {
            field,
            reason: PedestalMismatchReason::OutsideTolerance,
        });
        return None;
    }
    Some(canonical_non_negative(delta))
}

fn compare_exact_option<T: PartialEq>(
    left: Option<T>,
    right: Option<T>,
    field: PedestalMatchField,
    mismatches: &mut Vec<PedestalMismatch>,
) {
    let reason = match (left, right) {
        (Some(left), Some(right)) if left == right => return,
        (Some(_), Some(_)) => PedestalMismatchReason::Different,
        (None, _) | (_, None) => PedestalMismatchReason::Missing,
    };
    mismatches.push(PedestalMismatch { field, reason });
}

fn compare_f64_option(
    left: Option<f64>,
    right: Option<f64>,
    field: PedestalMatchField,
    mismatches: &mut Vec<PedestalMismatch>,
) {
    let reason = match (left, right) {
        (Some(left), Some(right)) if canonical_f64_bits(left) == canonical_f64_bits(right) => {
            return;
        }
        (Some(_), Some(_)) => PedestalMismatchReason::Different,
        (None, _) | (_, None) => PedestalMismatchReason::Missing,
    };
    mismatches.push(PedestalMismatch { field, reason });
}

fn select_dark(scores: &[CompatibleScore<'_>]) -> Result<FlatPedestalAssociation, MasterPlanError> {
    let Some(best) = scores
        .iter()
        .filter(|score| score.source_kind == PedestalSourceKind::Dark)
        .min_by(|left, right| compare_dark_score(left, right))
    else {
        return Ok(FlatPedestalAssociation::Unresolved {
            blocking_reason: FlatPedestalBlockingReason::NoCompatibleDark,
        });
    };
    let tied_count = scores
        .iter()
        .filter(|candidate| {
            candidate.source_kind == PedestalSourceKind::Dark && equal_dark_score(candidate, best)
        })
        .count();
    let mut tied = Vec::new();
    tied.try_reserve_exact(tied_count)
        .map_err(|_| MasterPlanError::AllocationFailed)?;
    tied.extend(
        scores
            .iter()
            .filter(|candidate| {
                candidate.source_kind == PedestalSourceKind::Dark
                    && equal_dark_score(candidate, best)
            })
            .map(|candidate| candidate.group_id.to_owned()),
    );
    if tied.len() > 1 {
        return Ok(FlatPedestalAssociation::Unresolved {
            blocking_reason: FlatPedestalBlockingReason::AmbiguousDark { group_ids: tied },
        });
    }
    let Some(exposure_delta_seconds) = best.exposure_delta_seconds else {
        return Ok(FlatPedestalAssociation::Unresolved {
            blocking_reason: FlatPedestalBlockingReason::NoCompatibleDark,
        });
    };
    Ok(FlatPedestalAssociation::MatchedDark {
        group_id: best.group_id.to_owned(),
        exposure_delta_seconds,
        temperature_basis: best.temperature_basis,
        temperature_delta_c: best.temperature_delta_c,
    })
}

fn select_bias(scores: &[CompatibleScore<'_>]) -> Result<FlatPedestalAssociation, MasterPlanError> {
    let Some(best) = scores
        .iter()
        .filter(|score| score.source_kind == PedestalSourceKind::Bias)
        .min_by(|left, right| compare_bias_score(left, right))
    else {
        return Ok(FlatPedestalAssociation::Unresolved {
            blocking_reason: FlatPedestalBlockingReason::NoCompatibleBias,
        });
    };
    let tied_count = scores
        .iter()
        .filter(|candidate| {
            candidate.source_kind == PedestalSourceKind::Bias && equal_bias_score(candidate, best)
        })
        .count();
    let mut tied = Vec::new();
    tied.try_reserve_exact(tied_count)
        .map_err(|_| MasterPlanError::AllocationFailed)?;
    tied.extend(
        scores
            .iter()
            .filter(|candidate| {
                candidate.source_kind == PedestalSourceKind::Bias
                    && equal_bias_score(candidate, best)
            })
            .map(|candidate| candidate.group_id.to_owned()),
    );
    if tied.len() > 1 {
        return Ok(FlatPedestalAssociation::Unresolved {
            blocking_reason: FlatPedestalBlockingReason::AmbiguousBias { group_ids: tied },
        });
    }
    Ok(FlatPedestalAssociation::Bias {
        group_id: best.group_id.to_owned(),
        temperature_basis: best.temperature_basis,
        temperature_delta_c: best.temperature_delta_c,
    })
}

fn select_compatible(
    scores: &[CompatibleScore<'_>],
    policy: FlatPedestalPolicy,
) -> Result<FlatPedestalAssociation, MasterPlanError> {
    match policy {
        FlatPedestalPolicy::RequireMatchedDark => select_dark(scores),
        FlatPedestalPolicy::RequireBias => select_bias(scores),
        FlatPedestalPolicy::PreferMatchedDarkThenBias => {
            if scores
                .iter()
                .any(|score| score.source_kind == PedestalSourceKind::Dark)
            {
                select_dark(scores)
            } else if scores
                .iter()
                .any(|score| score.source_kind == PedestalSourceKind::Bias)
            {
                select_bias(scores)
            } else {
                Ok(FlatPedestalAssociation::Unresolved {
                    blocking_reason: FlatPedestalBlockingReason::NoCompatibleDarkOrBias,
                })
            }
        }
    }
}

fn compare_dark_score(
    left: &CompatibleScore<'_>,
    right: &CompatibleScore<'_>,
) -> std::cmp::Ordering {
    left.exposure_delta_seconds
        .unwrap_or(f64::INFINITY)
        .total_cmp(&right.exposure_delta_seconds.unwrap_or(f64::INFINITY))
        .then_with(|| {
            left.temperature_delta_c
                .total_cmp(&right.temperature_delta_c)
        })
        .then_with(|| left.group_id.cmp(right.group_id))
}

fn compare_bias_score(
    left: &CompatibleScore<'_>,
    right: &CompatibleScore<'_>,
) -> std::cmp::Ordering {
    left.temperature_delta_c
        .total_cmp(&right.temperature_delta_c)
        .then_with(|| left.group_id.cmp(right.group_id))
}

fn equal_dark_score(left: &CompatibleScore<'_>, right: &CompatibleScore<'_>) -> bool {
    left.exposure_delta_seconds.map(canonical_f64_bits)
        == right.exposure_delta_seconds.map(canonical_f64_bits)
        && canonical_f64_bits(left.temperature_delta_c)
            == canonical_f64_bits(right.temperature_delta_c)
}

fn equal_bias_score(left: &CompatibleScore<'_>, right: &CompatibleScore<'_>) -> bool {
    canonical_f64_bits(left.temperature_delta_c) == canonical_f64_bits(right.temperature_delta_c)
}

fn missing_flat_match_fields(key: &StrictGroupingKey) -> Vec<PedestalMatchField> {
    let mut fields = Vec::new();
    if key.camera().is_none() {
        fields.push(PedestalMatchField::Camera);
    }
    if key.exposure_seconds().is_none() {
        fields.push(PedestalMatchField::Exposure);
    }
    if key.sensor_temperature_c().is_none() && key.set_temperature_c().is_none() {
        fields.push(PedestalMatchField::SensorTemperature);
        fields.push(PedestalMatchField::SetTemperature);
    }
    if key.gain().is_none() {
        fields.push(PedestalMatchField::Gain);
    }
    if key.offset().is_none() {
        fields.push(PedestalMatchField::Offset);
    }
    if key.binning().is_none() {
        fields.push(PedestalMatchField::Binning);
    }
    if key.bayer_pattern().is_none() {
        fields.push(PedestalMatchField::BayerPattern);
    }
    fields
}

fn validate_flat_product(
    product: &MasterProductPlan,
    association: &FlatPedestalAssociation,
    options: MasterPlanOptions,
) -> Result<(), MasterPlanError> {
    let mut candidate_ids = BTreeSet::new();
    let mut compatible_scores = Vec::new();
    compatible_scores
        .try_reserve_exact(product.pedestal_candidates.len())
        .map_err(|_| MasterPlanError::AllocationFailed)?;
    for candidate in &product.pedestal_candidates {
        if candidate.group_id.is_empty() || !candidate_ids.insert(candidate.group_id.as_str()) {
            return Err(MasterPlanError::InvalidCandidate {
                flat_group_id: product.source_group_id.clone(),
                candidate_group_id: candidate.group_id.clone(),
            });
        }
        match &candidate.compatibility {
            PedestalCandidateCompatibility::Compatible {
                exposure_delta_seconds,
                temperature_basis,
                temperature_delta_c,
            } => {
                if !valid_tolerance(*temperature_delta_c)
                    || exposure_delta_seconds.is_some_and(|value| !valid_tolerance(value))
                    || (candidate.source_kind == PedestalSourceKind::Dark
                        && exposure_delta_seconds.is_none())
                    || (candidate.source_kind == PedestalSourceKind::Bias
                        && exposure_delta_seconds.is_some())
                {
                    return Err(MasterPlanError::InvalidCandidate {
                        flat_group_id: product.source_group_id.clone(),
                        candidate_group_id: candidate.group_id.clone(),
                    });
                }
                compatible_scores.push(CompatibleScore {
                    group_id: &candidate.group_id,
                    source_kind: candidate.source_kind,
                    exposure_delta_seconds: *exposure_delta_seconds,
                    temperature_basis: *temperature_basis,
                    temperature_delta_c: *temperature_delta_c,
                });
            }
            PedestalCandidateCompatibility::Rejected { mismatches } => {
                if mismatches.is_empty()
                    || mismatches.iter().collect::<BTreeSet<_>>().len() != mismatches.len()
                {
                    return Err(MasterPlanError::InvalidCandidate {
                        flat_group_id: product.source_group_id.clone(),
                        candidate_group_id: candidate.group_id.clone(),
                    });
                }
            }
        }
    }

    match association {
        FlatPedestalAssociation::MatchedDark {
            exposure_delta_seconds,
            temperature_delta_c,
            ..
        } => {
            if !valid_tolerance(*exposure_delta_seconds) || !valid_tolerance(*temperature_delta_c) {
                return Err(MasterPlanError::InvalidProduct {
                    group_id: product.source_group_id.clone(),
                });
            }
        }
        FlatPedestalAssociation::Bias {
            temperature_delta_c,
            ..
        } => {
            if !valid_tolerance(*temperature_delta_c) {
                return Err(MasterPlanError::InvalidProduct {
                    group_id: product.source_group_id.clone(),
                });
            }
        }
        FlatPedestalAssociation::Unresolved { blocking_reason } => {
            if !valid_blocking_reason(blocking_reason) {
                return Err(MasterPlanError::InvalidProduct {
                    group_id: product.source_group_id.clone(),
                });
            }
        }
    }

    if !matches!(
        association,
        FlatPedestalAssociation::Unresolved {
            blocking_reason: FlatPedestalBlockingReason::MissingFlatMetadata { .. }
        }
    ) {
        let expected = select_compatible(&compatible_scores, options.flat_pedestal_policy)?;
        if &expected != association {
            return Err(MasterPlanError::InvalidCandidate {
                flat_group_id: product.source_group_id.clone(),
                candidate_group_id: selected_group_id(association)
                    .unwrap_or_default()
                    .to_owned(),
            });
        }
    }
    Ok(())
}

fn selected_group_id(association: &FlatPedestalAssociation) -> Option<&str> {
    match association {
        FlatPedestalAssociation::MatchedDark { group_id, .. }
        | FlatPedestalAssociation::Bias { group_id, .. } => Some(group_id),
        FlatPedestalAssociation::Unresolved { .. } => None,
    }
}

fn valid_blocking_reason(reason: &FlatPedestalBlockingReason) -> bool {
    match reason {
        FlatPedestalBlockingReason::MissingFlatMetadata { fields } => {
            !fields.is_empty()
                && fields.iter().copied().collect::<BTreeSet<_>>().len() == fields.len()
        }
        FlatPedestalBlockingReason::AmbiguousDark { group_ids }
        | FlatPedestalBlockingReason::AmbiguousBias { group_ids } => {
            group_ids.len() > 1
                && group_ids.iter().all(|id| !id.is_empty())
                && group_ids.iter().collect::<BTreeSet<_>>().len() == group_ids.len()
        }
        FlatPedestalBlockingReason::NoCompatibleDark
        | FlatPedestalBlockingReason::NoCompatibleBias
        | FlatPedestalBlockingReason::NoCompatibleDarkOrBias => true,
    }
}

const fn valid_tolerance(value: f64) -> bool {
    value.is_finite() && value >= 0.0
}

const fn canonical_non_negative(value: f64) -> f64 {
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

fn validate_candidate_evaluation_count(
    flat_count: usize,
    candidate_count: usize,
) -> Result<usize, MasterPlanError> {
    let attempted = flat_count.checked_mul(candidate_count).ok_or(
        MasterPlanError::TooManyCandidateEvaluations {
            attempted: usize::MAX,
            maximum: MAX_MASTER_PLAN_CANDIDATE_EVALUATIONS,
        },
    )?;
    if attempted > MAX_MASTER_PLAN_CANDIDATE_EVALUATIONS {
        return Err(MasterPlanError::TooManyCandidateEvaluations {
            attempted,
            maximum: MAX_MASTER_PLAN_CANDIDATE_EVALUATIONS,
        });
    }
    Ok(attempted)
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::path::Path;

    use aether_metadata::{
        BayerPattern, Binning, CameraModel, CanonicalMetadata, CanonicalValue, Confidence,
    };

    use super::*;
    use crate::{
        ClassificationPolicy, ManifestFile, ManifestValidationError, SourceFingerprint,
        classify_frame,
    };

    type TestResult<T = ()> = Result<T, Box<dyn StdError>>;

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
        bayer_pattern: Option<BayerPattern>,
        axes: Vec<u64>,
        filter: &'static str,
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
                bayer_pattern: Some(BayerPattern::Rggb),
                axes: vec![4_144, 2_822],
                filter: "UVIR",
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
                filter: Some(exact(self.filter.to_owned(), "FILTER")),
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

    fn options(policy: FlatPedestalPolicy) -> TestResult<MasterPlanOptions> {
        Ok(MasterPlanOptions::new(policy, 0.05, 1.0)?)
    }

    fn flat_product(plan: &MasterPlan) -> TestResult<&MasterProductPlan> {
        plan.products()
            .iter()
            .find(|product| product.kind() == MasterProductKind::Flat)
            .ok_or_else(|| std::io::Error::other("flat product missing").into())
    }

    #[test]
    fn matched_short_dark_is_selected_exclusively_over_bias() -> TestResult {
        let flat = GroupSpec::new("flat", FrameType::Flat, 2.0);
        let dark = GroupSpec::new("dark-short", FrameType::Dark, 2.02);
        let bias = GroupSpec::new("bias", FrameType::Bias, 0.001);
        let session = manifest(&[flat, dark, bias])?;

        let plan = MasterPlan::from_manifest(
            &session,
            options(FlatPedestalPolicy::PreferMatchedDarkThenBias)?,
        )?;

        assert!(plan.is_ready());
        assert_eq!(plan.products().len(), 3);
        assert!(matches!(
            flat_product(&plan)?.flat_pedestal(),
            Some(FlatPedestalAssociation::MatchedDark { group_id, .. })
                if group_id == "dark-short"
        ));
        assert_eq!(flat_product(&plan)?.pedestal_candidates().len(), 2);
        Ok(())
    }

    #[test]
    fn true_bias_is_used_when_no_short_dark_matches() -> TestResult {
        let flat = GroupSpec::new("flat", FrameType::Flat, 2.0);
        let dark = GroupSpec::new("dark-long", FrameType::Dark, 60.0);
        let bias = GroupSpec::new("bias", FrameType::Bias, 0.001);
        let session = manifest(&[flat, dark, bias])?;

        let plan = MasterPlan::from_manifest(
            &session,
            options(FlatPedestalPolicy::PreferMatchedDarkThenBias)?,
        )?;

        assert!(matches!(
            flat_product(&plan)?.flat_pedestal(),
            Some(FlatPedestalAssociation::Bias { group_id, .. }) if group_id == "bias"
        ));
        let rejected_dark = flat_product(&plan)?
            .pedestal_candidates()
            .iter()
            .find(|candidate| candidate.group_id() == "dark-long")
            .ok_or_else(|| std::io::Error::other("dark evaluation missing"))?;
        assert!(matches!(
            rejected_dark.compatibility(),
            PedestalCandidateCompatibility::Rejected { mismatches }
                if mismatches.contains(&PedestalMismatch {
                    field: PedestalMatchField::Exposure,
                    reason: PedestalMismatchReason::OutsideTolerance,
                })
        ));
        Ok(())
    }

    #[test]
    fn bias_only_policy_ignores_an_otherwise_compatible_dark() -> TestResult {
        let flat = GroupSpec::new("flat", FrameType::Flat, 2.0);
        let dark = GroupSpec::new("dark-short", FrameType::Dark, 2.0);
        let bias = GroupSpec::new("bias", FrameType::Bias, 0.001);
        let session = manifest(&[flat, dark, bias])?;

        let plan = MasterPlan::from_manifest(&session, options(FlatPedestalPolicy::RequireBias)?)?;

        assert!(matches!(
            flat_product(&plan)?.flat_pedestal(),
            Some(FlatPedestalAssociation::Bias { group_id, .. }) if group_id == "bias"
        ));
        Ok(())
    }

    #[test]
    fn required_policy_never_silently_falls_back() -> TestResult {
        let flat = GroupSpec::new("flat", FrameType::Flat, 2.0);
        let bias = GroupSpec::new("bias", FrameType::Bias, 0.001);
        let session = manifest(&[flat, bias])?;

        let plan =
            MasterPlan::from_manifest(&session, options(FlatPedestalPolicy::RequireMatchedDark)?)?;

        assert!(!plan.is_ready());
        assert!(matches!(
            flat_product(&plan)?.flat_pedestal(),
            Some(FlatPedestalAssociation::Unresolved {
                blocking_reason: FlatPedestalBlockingReason::NoCompatibleDark
            })
        ));
        Ok(())
    }

    #[test]
    fn tolerance_boundary_is_inclusive() -> TestResult {
        let flat = GroupSpec::new("flat", FrameType::Flat, 2.0);
        let mut dark = GroupSpec::new("dark", FrameType::Dark, 2.05);
        dark.sensor_temperature_c = Some(-9.0);
        let session = manifest(&[flat, dark])?;

        let plan =
            MasterPlan::from_manifest(&session, options(FlatPedestalPolicy::RequireMatchedDark)?)?;

        assert!(plan.is_ready());
        assert!(matches!(
            flat_product(&plan)?.flat_pedestal(),
            Some(FlatPedestalAssociation::MatchedDark {
                exposure_delta_seconds,
                temperature_delta_c,
                ..
            }) if (*exposure_delta_seconds - 0.05).abs() < 1.0e-12
                && (*temperature_delta_c - 1.0).abs() < 1.0e-12
        ));
        Ok(())
    }

    #[test]
    fn equally_ranked_darks_are_reported_as_ambiguous() -> TestResult {
        let flat = GroupSpec::new("flat", FrameType::Flat, 2.0);
        let first = GroupSpec::new("dark-a", FrameType::Dark, 1.75);
        let second = GroupSpec::new("dark-b", FrameType::Dark, 2.25);
        let session = manifest(&[flat, first, second])?;

        let plan = MasterPlan::from_manifest(
            &session,
            MasterPlanOptions::new(FlatPedestalPolicy::RequireMatchedDark, 0.25, 1.0)?,
        )?;

        assert!(!plan.is_ready());
        assert!(matches!(
            flat_product(&plan)?.flat_pedestal(),
            Some(FlatPedestalAssociation::Unresolved {
                blocking_reason: FlatPedestalBlockingReason::AmbiguousDark { group_ids }
            }) if group_ids == &["dark-a".to_owned(), "dark-b".to_owned()]
        ));
        Ok(())
    }

    #[test]
    fn equally_ranked_biases_are_reported_as_ambiguous() -> TestResult {
        let flat = GroupSpec::new("flat", FrameType::Flat, 2.0);
        let first = GroupSpec::new("bias-a", FrameType::Bias, 0.001);
        let second = GroupSpec::new("bias-b", FrameType::Bias, 0.002);
        let session = manifest(&[flat, first, second])?;

        let plan = MasterPlan::from_manifest(&session, options(FlatPedestalPolicy::RequireBias)?)?;

        assert!(!plan.is_ready());
        assert!(matches!(
            flat_product(&plan)?.flat_pedestal(),
            Some(FlatPedestalAssociation::Unresolved {
                blocking_reason: FlatPedestalBlockingReason::AmbiguousBias { group_ids }
            }) if group_ids == &["bias-a".to_owned(), "bias-b".to_owned()]
        ));
        Ok(())
    }

    #[test]
    fn different_camera_gain_and_dimensions_are_all_explained() -> TestResult {
        let flat = GroupSpec::new("flat", FrameType::Flat, 2.0);
        let mut dark = GroupSpec::new("dark", FrameType::Dark, 2.0);
        dark.camera = Some(CameraModel::TouptekAtr585C);
        dark.gain = Some(121.0);
        dark.axes = vec![3_840, 2_160];
        let session = manifest(&[flat, dark])?;

        let plan =
            MasterPlan::from_manifest(&session, options(FlatPedestalPolicy::RequireMatchedDark)?)?;
        let evaluation = flat_product(&plan)?
            .pedestal_candidates()
            .first()
            .ok_or_else(|| std::io::Error::other("candidate missing"))?;
        let PedestalCandidateCompatibility::Rejected { mismatches } = evaluation.compatibility()
        else {
            return Err(std::io::Error::other("candidate should be rejected").into());
        };
        assert_eq!(
            mismatches,
            &[
                PedestalMismatch {
                    field: PedestalMatchField::Camera,
                    reason: PedestalMismatchReason::Different,
                },
                PedestalMismatch {
                    field: PedestalMatchField::Axes,
                    reason: PedestalMismatchReason::Different,
                },
                PedestalMismatch {
                    field: PedestalMatchField::Gain,
                    reason: PedestalMismatchReason::Different,
                },
            ]
        );
        Ok(())
    }

    #[test]
    fn measured_temperature_is_not_silently_replaced_by_set_point() -> TestResult {
        let flat = GroupSpec::new("flat", FrameType::Flat, 2.0);
        let mut dark = GroupSpec::new("dark", FrameType::Dark, 2.0);
        dark.sensor_temperature_c = None;
        let session = manifest(&[flat, dark])?;

        let plan =
            MasterPlan::from_manifest(&session, options(FlatPedestalPolicy::RequireMatchedDark)?)?;
        assert!(matches!(
            flat_product(&plan)?.pedestal_candidates()[0].compatibility(),
            PedestalCandidateCompatibility::Rejected { mismatches }
                if mismatches.contains(&PedestalMismatch {
                    field: PedestalMatchField::SensorTemperature,
                    reason: PedestalMismatchReason::Missing,
                })
        ));
        Ok(())
    }

    #[test]
    fn missing_flat_metadata_blocks_even_without_candidates() -> TestResult {
        let mut flat = GroupSpec::new("flat", FrameType::Flat, 2.0);
        flat.camera = None;
        flat.sensor_temperature_c = None;
        flat.set_temperature_c = None;
        let session = manifest(&[flat])?;

        let plan = MasterPlan::from_manifest(
            &session,
            options(FlatPedestalPolicy::PreferMatchedDarkThenBias)?,
        )?;

        assert!(matches!(
            flat_product(&plan)?.flat_pedestal(),
            Some(FlatPedestalAssociation::Unresolved {
                blocking_reason: FlatPedestalBlockingReason::MissingFlatMetadata { fields }
            }) if fields == &[
                PedestalMatchField::Camera,
                PedestalMatchField::SensorTemperature,
                PedestalMatchField::SetTemperature,
            ]
        ));
        Ok(())
    }

    #[test]
    fn lights_do_not_create_master_products() -> TestResult {
        let light = GroupSpec::new("light", FrameType::Light, 60.0);
        let session = manifest(&[light])?;

        let plan = MasterPlan::from_manifest(
            &session,
            options(FlatPedestalPolicy::PreferMatchedDarkThenBias)?,
        )?;

        assert!(plan.products().is_empty());
        assert!(plan.is_ready());
        Ok(())
    }

    #[test]
    fn json_round_trip_is_deterministic_and_bound_to_manifest() -> TestResult {
        let flat = GroupSpec::new("flat", FrameType::Flat, 2.0);
        let dark = GroupSpec::new("dark", FrameType::Dark, 2.0);
        let bias = GroupSpec::new("bias", FrameType::Bias, 0.001);
        let session = manifest(&[flat, dark, bias])?;
        let plan = MasterPlan::from_manifest(
            &session,
            options(FlatPedestalPolicy::PreferMatchedDarkThenBias)?,
        )?;

        assert_eq!(plan.manifest_sha256(), session.canonical_sha256()?);
        let encoded = plan.to_json_pretty()?;
        assert_eq!(encoded.last(), Some(&b'\n'));
        let decoded = MasterPlan::from_json_slice(&encoded)?;
        assert_eq!(decoded, plan);
        assert_eq!(decoded.to_json_pretty()?, encoded);
        Ok(())
    }

    #[test]
    fn invalid_tolerances_are_rejected() {
        for value in [-1.0, f64::NAN, f64::INFINITY] {
            assert!(matches!(
                MasterPlanOptions::new(FlatPedestalPolicy::RequireMatchedDark, value, 1.0),
                Err(MasterPlanError::InvalidExposureTolerance { .. })
            ));
            assert!(matches!(
                MasterPlanOptions::new(FlatPedestalPolicy::RequireMatchedDark, 0.0, value),
                Err(MasterPlanError::InvalidTemperatureTolerance { .. })
            ));
        }
        let negative_zero =
            MasterPlanOptions::new(FlatPedestalPolicy::RequireMatchedDark, -0.0, -0.0);
        assert!(matches!(
            negative_zero,
            Ok(value)
                if value.maximum_exposure_delta_seconds().to_bits() == 0.0_f64.to_bits()
                    && value.maximum_temperature_delta_c().to_bits() == 0.0_f64.to_bits()
        ));
    }

    #[test]
    fn candidate_matrix_has_an_explicit_memory_bound() {
        assert!(matches!(
            validate_candidate_evaluation_count(1_000, 100),
            Ok(MAX_MASTER_PLAN_CANDIDATE_EVALUATIONS)
        ));
        assert!(matches!(
            validate_candidate_evaluation_count(1_001, 100),
            Err(MasterPlanError::TooManyCandidateEvaluations {
                attempted: 100_100,
                maximum: MAX_MASTER_PLAN_CANDIDATE_EVALUATIONS,
            })
        ));
        assert!(matches!(
            validate_candidate_evaluation_count(usize::MAX, 2),
            Err(MasterPlanError::TooManyCandidateEvaluations {
                attempted: usize::MAX,
                ..
            })
        ));
    }

    #[test]
    fn decoder_rejects_unknown_versions_and_oversized_input() -> TestResult {
        let light = GroupSpec::new("light", FrameType::Light, 60.0);
        let session = manifest(&[light])?;
        let plan = MasterPlan::from_manifest(
            &session,
            options(FlatPedestalPolicy::PreferMatchedDarkThenBias)?,
        )?;
        let mut value: serde_json::Value = serde_json::from_slice(&plan.to_json_pretty()?)?;
        value["schema_version"] = serde_json::json!(MASTER_PLAN_SCHEMA_VERSION + 1);
        let unsupported = serde_json::to_vec(&value)?;
        assert!(matches!(
            MasterPlan::from_json_slice(&unsupported),
            Err(MasterPlanError::UnsupportedSchemaVersion { .. })
        ));

        let oversized = vec![b' '; MAX_MASTER_PLAN_BYTES + 1];
        assert!(matches!(
            MasterPlan::from_json_slice(&oversized),
            Err(MasterPlanError::PlanTooLarge { .. })
        ));
        Ok(())
    }

    #[test]
    fn decoder_rejects_a_selection_that_contradicts_the_policy() -> TestResult {
        let flat = GroupSpec::new("flat", FrameType::Flat, 2.0);
        let dark = GroupSpec::new("dark", FrameType::Dark, 2.0);
        let bias = GroupSpec::new("bias", FrameType::Bias, 0.001);
        let session = manifest(&[flat, dark, bias])?;
        let plan =
            MasterPlan::from_manifest(&session, options(FlatPedestalPolicy::RequireMatchedDark)?)?;
        let mut value: serde_json::Value = serde_json::from_slice(&plan.to_json_pretty()?)?;
        let products = value["products"]
            .as_array_mut()
            .ok_or_else(|| std::io::Error::other("products must be an array"))?;
        let flat_product = products
            .iter_mut()
            .find(|product| product["kind"] == "flat")
            .ok_or_else(|| std::io::Error::other("flat product missing"))?;
        flat_product["flat_pedestal"] = serde_json::json!({
            "selection": "bias",
            "group_id": "bias",
            "temperature_basis": "sensor",
            "temperature_delta_c": 0.0
        });

        let tampered = serde_json::to_vec(&value)?;
        assert!(matches!(
            MasterPlan::from_json_slice(&tampered),
            Err(MasterPlanError::InvalidCandidate { .. })
        ));
        Ok(())
    }
}
