use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs;
use std::mem::size_of;
use std::path::{Path, PathBuf};

use aether_core::{Dimensions, PixelFlags};
use aether_fits::{
    AtomicF64PrimaryStreamWriter, AtomicFitsWriteError, FitsOutputProvenance, FitsWriteSummary,
    HeaderReadOptions, ImageReadError, ImageRegion, PrimaryImageReader, SampleStatus,
    ValidationMode,
};
pub use aether_integration::{
    BALANCED_PSF_WEIGHT_ALGORITHM_ID, PERCENTILE_REJECTION_MAP_ALGORITHM_ID,
    PercentileClipParameters, QualityWeightMetrics,
};
use aether_integration::{
    ClippedPixelSupport, FrameWeight, IntegrationError, PixelSupport, balanced_psf_weight,
    integrate_mean, integrate_percentile_clipped_mean, integrate_weighted_mean,
    materialize_percentile_rejection_map,
};
use aether_registration::{ProjectiveRegistrationPlan, RegistrationPlan};
use aether_review::FrameId;
use sha2::{Digest, Sha256};

use crate::pipeline::{dimensions_from_axes, open_reader, verify_source};
use crate::{
    CancellationToken, Cancelled, MemoryBudget, MemoryBudgetError, PipelineInput, PipelineSource,
    ProgressEvent, ProgressEventError, ProgressSequence, ProgressState, StageId, StageIdError,
    StrictPipelineError,
};

/// Strict estimator identity for a plan-bound registered common-crop stack.
pub const REGISTERED_CROP_MEAN_ALGORITHM_ID: &str = "registered-crop-mean-v1";
/// Versioned deterministic percentile-clipped registered stack identity.
pub const REGISTERED_PERCENTILE_CLIPPED_MEAN_ALGORITHM_ID: &str = "registered-percentile-mean-v1";
/// Versioned deterministic frame-weighted registered stack identity.
pub const REGISTERED_WEIGHTED_MEAN_ALGORITHM_ID: &str = "registered-weighted-mean-v1";
const REGISTERED_STACK_STAGE_ID: &str = "registered-stack";
const DEFAULT_BAND_HEIGHT: usize = 128;
const STREAM_WRITER_BUFFER_BYTES: usize = 64 * 1_024;

/// Scientific estimator selected for one registered common-crop stack.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RegisteredStackEstimator {
    /// Strict unweighted mean of every usable sample.
    StrictMean,
    /// Strict weighted mean with a separately bound canonical weight set.
    WeightedMean,
    /// Sorted low/high percentile rejection followed by the strict mean.
    PercentileClipped(PercentileClipParameters),
}

impl RegisteredStackEstimator {
    /// Stable provenance identity required for this estimator.
    #[must_use]
    pub const fn algorithm_id(self) -> &'static str {
        match self {
            Self::StrictMean => REGISTERED_CROP_MEAN_ALGORITHM_ID,
            Self::WeightedMean => REGISTERED_WEIGHTED_MEAN_ALGORITHM_ID,
            Self::PercentileClipped(_) => REGISTERED_PERCENTILE_CLIPPED_MEAN_ALGORITHM_ID,
        }
    }
}

/// One reviewed frame identity and its validated integration weight.
#[derive(Clone, Debug, PartialEq)]
pub struct RegisteredFrameWeight {
    frame_id: FrameId,
    weight: FrameWeight,
}

impl RegisteredFrameWeight {
    /// Binds a weight to the reviewed frame that produced its quality metrics.
    #[must_use]
    pub const fn new(frame_id: FrameId, weight: FrameWeight) -> Self {
        Self { frame_id, weight }
    }

    /// Stable reviewed frame identity.
    #[must_use]
    pub const fn frame_id(&self) -> &FrameId {
        &self.frame_id
    }

    /// Validated dimensionless weight.
    #[must_use]
    pub const fn weight(&self) -> FrameWeight {
        self.weight
    }
}

/// One reviewed frame identity and its validated PSF quality metrics.
#[derive(Clone, Debug, PartialEq)]
pub struct RegisteredFrameQuality {
    frame_id: FrameId,
    metrics: QualityWeightMetrics,
}

impl RegisteredFrameQuality {
    /// Binds measured PSF quality to the reviewed source identity.
    #[must_use]
    pub const fn new(frame_id: FrameId, metrics: QualityWeightMetrics) -> Self {
        Self { frame_id, metrics }
    }

    /// Stable reviewed frame identity.
    #[must_use]
    pub const fn frame_id(&self) -> &FrameId {
        &self.frame_id
    }

    /// Metrics consumed by the versioned weight expression.
    #[must_use]
    pub const fn metrics(&self) -> QualityWeightMetrics {
        self.metrics
    }
}

/// Canonical, identity-bound weights and their reproducibility digest.
#[derive(Clone, Debug)]
pub struct RegisteredWeightSet {
    weight_algorithm_id: String,
    weights: BTreeMap<FrameId, FrameWeight>,
    sha256: String,
}

impl RegisteredWeightSet {
    /// Canonicalizes an explicit set of reviewed-frame weights.
    ///
    /// The SHA-256 covers a domain/version marker, the weight-expression
    /// identifier, sorted frame identities, and exact binary64 weight bits.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid algorithm identifier, an empty or
    /// duplicated frame set, excessive cardinality, or allocation failure.
    pub fn new(
        weight_algorithm_id: impl Into<String>,
        entries: Vec<RegisteredFrameWeight>,
    ) -> Result<Self, RegisteredStackError> {
        let weight_algorithm_id = weight_algorithm_id.into();
        if !is_weight_algorithm_id(&weight_algorithm_id) {
            return Err(RegisteredStackError::InvalidWeightAlgorithmId);
        }
        if entries.is_empty() {
            return Err(RegisteredStackError::WeightSetMismatch);
        }
        let mut weights = BTreeMap::new();
        for entry in entries {
            if weights.insert(entry.frame_id, entry.weight).is_some() {
                return Err(RegisteredStackError::WeightSetMismatch);
            }
        }
        let count =
            u32::try_from(weights.len()).map_err(|_| RegisteredStackError::WorkSizeOverflow)?;
        let mut hasher = Sha256::new();
        hasher.update(b"aetherstack-registered-weight-set-v1\0");
        update_digest_string(&mut hasher, &weight_algorithm_id)?;
        hasher.update(count.to_be_bytes());
        for (frame_id, weight) in &weights {
            update_digest_string(&mut hasher, frame_id.as_str())?;
            hasher.update(weight.get().to_bits().to_be_bytes());
        }
        let sha256 = encode_lower_hex(hasher.finalize().as_slice());
        Ok(Self {
            weight_algorithm_id,
            weights,
            sha256,
        })
    }

    /// Derives identity-bound weights from reviewed PSF measurements.
    ///
    /// The parameter digest binds the reference metrics, every source metric,
    /// every resulting weight, and the expression identifier. This preserves
    /// the complete numerical evidence used to construct the request.
    pub fn from_balanced_psf_metrics(
        reference: QualityWeightMetrics,
        entries: Vec<RegisteredFrameQuality>,
    ) -> Result<Self, RegisteredStackError> {
        if entries.is_empty() {
            return Err(RegisteredStackError::WeightSetMismatch);
        }
        let mut metrics = BTreeMap::new();
        for entry in entries {
            if metrics.insert(entry.frame_id, entry.metrics).is_some() {
                return Err(RegisteredStackError::WeightSetMismatch);
            }
        }
        let weighted_entries = metrics
            .iter()
            .map(|(frame_id, frame_metrics)| {
                RegisteredFrameWeight::new(
                    frame_id.clone(),
                    balanced_psf_weight(*frame_metrics, reference),
                )
            })
            .collect();
        let mut set = Self::new(BALANCED_PSF_WEIGHT_ALGORITHM_ID, weighted_entries)?;

        let count =
            u32::try_from(metrics.len()).map_err(|_| RegisteredStackError::WorkSizeOverflow)?;
        let mut hasher = Sha256::new();
        hasher.update(b"aetherstack-balanced-psf-weight-evidence-v1\0");
        update_digest_string(&mut hasher, BALANCED_PSF_WEIGHT_ALGORITHM_ID)?;
        update_quality_metrics(&mut hasher, reference);
        hasher.update(count.to_be_bytes());
        for (frame_id, frame_metrics) in metrics {
            update_digest_string(&mut hasher, frame_id.as_str())?;
            update_quality_metrics(&mut hasher, frame_metrics);
            let weight = set
                .weights
                .get(&frame_id)
                .ok_or(RegisteredStackError::WeightSetMismatch)?;
            hasher.update(weight.get().to_bits().to_be_bytes());
        }
        set.sha256 = encode_lower_hex(hasher.finalize().as_slice());
        Ok(set)
    }

    /// Versioned expression that produced the stored weights.
    #[must_use]
    pub fn weight_algorithm_id(&self) -> &str {
        &self.weight_algorithm_id
    }

    /// SHA-256 of the exact canonical weight-set encoding.
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// Number of identity-bound weights.
    #[must_use]
    pub fn len(&self) -> usize {
        self.weights.len()
    }

    /// Whether no weights are present. Valid constructed sets are never empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.weights.is_empty()
    }

    /// Returns the exact derived weight for one reviewed frame identity.
    #[must_use]
    pub fn weight_for(&self, frame_id: &FrameId) -> Option<FrameWeight> {
        self.weights.get(frame_id).copied()
    }
}

/// One immutable registered FITS artifact bound to its reviewed frame.
#[derive(Clone, Debug)]
pub struct RegisteredStackSource {
    frame_id: FrameId,
    source: PipelineSource,
}

impl RegisteredStackSource {
    /// Binds one registered artifact to the identity carried in `AETHFID`.
    #[must_use]
    pub const fn new(frame_id: FrameId, source: PipelineSource) -> Self {
        Self { frame_id, source }
    }

    /// Reviewed frame identity expected in the artifact header.
    #[must_use]
    pub const fn frame_id(&self) -> &FrameId {
        &self.frame_id
    }

    /// Fingerprinted registered FITS source.
    #[must_use]
    pub const fn source(&self) -> &PipelineSource {
        &self.source
    }
}

/// Validated request for one bounded common-crop registered integration.
#[derive(Clone, Debug)]
pub struct RegisteredStackRequest {
    plan: RegisteredStackPlan,
    sources: Vec<RegisteredStackSource>,
    output: PathBuf,
    provenance: FitsOutputProvenance,
    band_height: usize,
    header_options: HeaderReadOptions,
    validation_mode: ValidationMode,
    estimator: RegisteredStackEstimator,
    weights: Option<Vec<FrameWeight>>,
    rejection_map: Option<RegisteredRejectionMapOutput>,
}

#[derive(Clone, Debug)]
enum RegisteredStackPlan {
    Affine(RegistrationPlan),
    Projective(ProjectiveRegistrationPlan),
}

impl RegisteredStackPlan {
    fn frame_count(&self) -> usize {
        match self {
            Self::Affine(plan) => plan.frames().len(),
            Self::Projective(plan) => plan.frames().len(),
        }
    }

    fn frame_ids(&self) -> Vec<FrameId> {
        match self {
            Self::Affine(plan) => plan
                .frames()
                .iter()
                .map(|frame| frame.frame_id().clone())
                .collect(),
            Self::Projective(plan) => plan
                .frames()
                .iter()
                .map(|frame| frame.frame_id().clone())
                .collect(),
        }
    }

    fn plan_sha256(&self) -> &str {
        match self {
            Self::Affine(plan) => plan.plan_sha256(),
            Self::Projective(plan) => plan.plan_sha256(),
        }
    }

    fn reference_dimensions(&self) -> (usize, usize) {
        match self {
            Self::Affine(plan) => (plan.reference_width(), plan.reference_height()),
            Self::Projective(plan) => (plan.reference_width(), plan.reference_height()),
        }
    }

    fn crop(&self) -> Option<aether_registration::ReferenceRectangle> {
        match self {
            Self::Affine(plan) => plan.common_footprint().crop(),
            Self::Projective(plan) => plan.common_footprint().crop(),
        }
    }
}

/// Optional companion FITS containing exact low/high rejection counts.
#[derive(Clone, Debug)]
pub struct RegisteredRejectionMapOutput {
    low_output: PathBuf,
    low_provenance: FitsOutputProvenance,
    high_output: PathBuf,
    high_provenance: FitsOutputProvenance,
}

impl RegisteredRejectionMapOutput {
    /// Binds distinct create-new low/high destinations to their provenance.
    #[must_use]
    pub const fn new(
        low_output: PathBuf,
        low_provenance: FitsOutputProvenance,
        high_output: PathBuf,
        high_provenance: FitsOutputProvenance,
    ) -> Self {
        Self {
            low_output,
            low_provenance,
            high_output,
            high_provenance,
        }
    }

    /// Low-tail rejection-count FITS destination.
    #[must_use]
    pub fn low_output(&self) -> &Path {
        &self.low_output
    }

    /// High-tail rejection-count FITS destination.
    #[must_use]
    pub fn high_output(&self) -> &Path {
        &self.high_output
    }
}

impl RegisteredStackRequest {
    /// Builds a request and canonicalizes sources into plan identity order.
    pub fn new(
        plan: RegistrationPlan,
        sources: Vec<RegisteredStackSource>,
        output: PathBuf,
        provenance: FitsOutputProvenance,
    ) -> Result<Self, RegisteredStackError> {
        Self::new_with_estimator(
            plan,
            sources,
            output,
            provenance,
            RegisteredStackEstimator::StrictMean,
        )
    }

    /// Builds a request for an explicit versioned estimator.
    pub fn new_with_estimator(
        plan: RegistrationPlan,
        sources: Vec<RegisteredStackSource>,
        output: PathBuf,
        provenance: FitsOutputProvenance,
        estimator: RegisteredStackEstimator,
    ) -> Result<Self, RegisteredStackError> {
        if matches!(estimator, RegisteredStackEstimator::WeightedMean) {
            return Err(RegisteredStackError::WeightedEstimatorRequiresWeights);
        }
        Self::new_canonical(
            RegisteredStackPlan::Affine(plan),
            sources,
            output,
            provenance,
            estimator,
            None,
        )
    }

    /// Builds a request bound to an immutable projective registration plan.
    pub fn new_projective_with_estimator(
        plan: ProjectiveRegistrationPlan,
        sources: Vec<RegisteredStackSource>,
        output: PathBuf,
        provenance: FitsOutputProvenance,
        estimator: RegisteredStackEstimator,
    ) -> Result<Self, RegisteredStackError> {
        if matches!(estimator, RegisteredStackEstimator::WeightedMean) {
            return Err(RegisteredStackError::WeightedEstimatorRequiresWeights);
        }
        Self::new_canonical(
            RegisteredStackPlan::Projective(plan),
            sources,
            output,
            provenance,
            estimator,
            None,
        )
    }

    /// Builds a weighted request and binds every weight to a planned frame.
    ///
    /// The output provenance must carry the exact weight-set digest in
    /// `AETHPAR`; this prevents scientifically distinct weights from producing
    /// indistinguishable output headers.
    pub fn new_weighted(
        plan: RegistrationPlan,
        sources: Vec<RegisteredStackSource>,
        output: PathBuf,
        provenance: FitsOutputProvenance,
        weight_set: RegisteredWeightSet,
    ) -> Result<Self, RegisteredStackError> {
        Self::new_weighted_canonical(
            RegisteredStackPlan::Affine(plan),
            sources,
            output,
            provenance,
            weight_set,
        )
    }

    /// Builds a weighted request bound to a projective registration plan.
    pub fn new_projective_weighted(
        plan: ProjectiveRegistrationPlan,
        sources: Vec<RegisteredStackSource>,
        output: PathBuf,
        provenance: FitsOutputProvenance,
        weight_set: RegisteredWeightSet,
    ) -> Result<Self, RegisteredStackError> {
        Self::new_weighted_canonical(
            RegisteredStackPlan::Projective(plan),
            sources,
            output,
            provenance,
            weight_set,
        )
    }

    fn new_weighted_canonical(
        plan: RegisteredStackPlan,
        sources: Vec<RegisteredStackSource>,
        output: PathBuf,
        provenance: FitsOutputProvenance,
        weight_set: RegisteredWeightSet,
    ) -> Result<Self, RegisteredStackError> {
        if provenance.parameters_sha256() != Some(weight_set.sha256()) {
            return Err(RegisteredStackError::WeightProvenanceMismatch);
        }
        if weight_set.len() != plan.frame_count() {
            return Err(RegisteredStackError::WeightSetMismatch);
        }
        let mut weights = weight_set.weights;
        let mut canonical = Vec::new();
        canonical
            .try_reserve_exact(plan.frame_count())
            .map_err(|_| RegisteredStackError::AllocationFailed)?;
        for frame_id in plan.frame_ids() {
            canonical.push(
                weights
                    .remove(&frame_id)
                    .ok_or(RegisteredStackError::WeightSetMismatch)?,
            );
        }
        if !weights.is_empty() {
            return Err(RegisteredStackError::WeightSetMismatch);
        }
        Self::new_canonical(
            plan,
            sources,
            output,
            provenance,
            RegisteredStackEstimator::WeightedMean,
            Some(canonical),
        )
    }

    fn new_canonical(
        plan: RegisteredStackPlan,
        sources: Vec<RegisteredStackSource>,
        output: PathBuf,
        provenance: FitsOutputProvenance,
        estimator: RegisteredStackEstimator,
        weights: Option<Vec<FrameWeight>>,
    ) -> Result<Self, RegisteredStackError> {
        if sources.len() != plan.frame_count() {
            return Err(RegisteredStackError::SourceSetMismatch);
        }
        let source_count =
            u32::try_from(sources.len()).map_err(|_| RegisteredStackError::WorkSizeOverflow)?;
        if provenance.algorithm_id() != estimator.algorithm_id() {
            return Err(RegisteredStackError::ProvenanceAlgorithmMismatch);
        }
        if provenance.source_count() != source_count {
            return Err(RegisteredStackError::ProvenanceSourceCountMismatch);
        }
        if provenance.plan_sha256() != Some(plan.plan_sha256()) {
            return Err(RegisteredStackError::ProvenancePlanMismatch);
        }
        let mut by_id = BTreeMap::new();
        for source in sources {
            if by_id.insert(source.frame_id.clone(), source).is_some() {
                return Err(RegisteredStackError::SourceSetMismatch);
            }
        }
        let mut canonical = Vec::new();
        canonical
            .try_reserve_exact(plan.frame_count())
            .map_err(|_| RegisteredStackError::AllocationFailed)?;
        for frame_id in plan.frame_ids() {
            canonical.push(
                by_id
                    .remove(&frame_id)
                    .ok_or(RegisteredStackError::SourceSetMismatch)?,
            );
        }
        if !by_id.is_empty() {
            return Err(RegisteredStackError::SourceSetMismatch);
        }
        Ok(Self {
            plan,
            sources: canonical,
            output,
            provenance,
            band_height: DEFAULT_BAND_HEIGHT,
            header_options: HeaderReadOptions::default(),
            validation_mode: ValidationMode::Strict,
            estimator,
            weights,
            rejection_map: None,
        })
    }

    /// Replaces the maximum output rows retained per plane band.
    pub fn with_band_height(mut self, height: usize) -> Result<Self, RegisteredStackError> {
        if height == 0 {
            return Err(RegisteredStackError::ZeroBandHeight);
        }
        self.band_height = height;
        Ok(self)
    }

    /// Replaces FITS header limits and conformance policy.
    #[must_use]
    pub const fn with_header_policy(
        mut self,
        options: HeaderReadOptions,
        mode: ValidationMode,
    ) -> Self {
        self.header_options = options;
        self.validation_mode = mode;
        self
    }

    /// Exact sealed registration-plan digest.
    #[must_use]
    pub fn plan_sha256(&self) -> &str {
        self.plan.plan_sha256()
    }

    /// Canonical registered source order.
    #[must_use]
    pub fn sources(&self) -> &[RegisteredStackSource] {
        &self.sources
    }

    /// Atomic create-new output path.
    #[must_use]
    pub fn output(&self) -> &Path {
        &self.output
    }

    /// Versioned integration estimator.
    #[must_use]
    pub const fn estimator(&self) -> RegisteredStackEstimator {
        self.estimator
    }

    /// Adds a low/high rejection-map product to the same publication unit.
    pub fn with_rejection_map(
        mut self,
        output: RegisteredRejectionMapOutput,
    ) -> Result<Self, RegisteredStackError> {
        if !matches!(
            self.estimator,
            RegisteredStackEstimator::PercentileClipped(_)
        ) {
            return Err(RegisteredStackError::RejectionMapRequiresPercentileEstimator);
        }
        let provenances = [&output.low_provenance, &output.high_provenance];
        if provenances.iter().any(|provenance| {
            provenance.algorithm_id() != PERCENTILE_REJECTION_MAP_ALGORITHM_ID
                || provenance.source_count() != self.provenance.source_count()
                || provenance.plan_sha256() != Some(self.plan.plan_sha256())
        }) {
            return Err(RegisteredStackError::RejectionMapProvenanceMismatch);
        }
        if output.low_output == self.output
            || output.high_output == self.output
            || output.low_output == output.high_output
        {
            return Err(RegisteredStackError::DuplicateOutputPath);
        }
        self.rejection_map = Some(output);
        Ok(self)
    }
}

/// Completed common-crop registered stack.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegisteredStackResult {
    dimensions: Dimensions,
    summary: FitsWriteSummary,
    peak_reserved_bytes: usize,
    rejection_map_summary: Option<RegisteredRejectionMapSummary>,
}

/// Write accounting for a published pair of low/high rejection maps.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegisteredRejectionMapSummary {
    low: FitsWriteSummary,
    high: FitsWriteSummary,
}

impl RegisteredRejectionMapSummary {
    /// Low-tail map write accounting.
    #[must_use]
    pub const fn low(self) -> FitsWriteSummary {
        self.low
    }

    /// High-tail map write accounting.
    #[must_use]
    pub const fn high(self) -> FitsWriteSummary {
        self.high
    }
}

impl RegisteredStackResult {
    /// Published cropped output dimensions.
    #[must_use]
    pub const fn dimensions(self) -> Dimensions {
        self.dimensions
    }

    /// Exact sample, substitution, byte, and checksum accounting.
    #[must_use]
    pub const fn summary(self) -> FitsWriteSummary {
        self.summary
    }

    /// Peak logical working set observed by the shared budget.
    #[must_use]
    pub const fn peak_reserved_bytes(self) -> usize {
        self.peak_reserved_bytes
    }

    /// Published rejection-map accounting when requested.
    #[must_use]
    pub const fn rejection_map_summary(self) -> Option<RegisteredRejectionMapSummary> {
        self.rejection_map_summary
    }
}

/// Failure of strict registered common-crop integration.
#[derive(Debug)]
pub enum RegisteredStackError {
    /// Supplied identities are missing, duplicated, or foreign to the plan.
    SourceSetMismatch,
    /// Output provenance names a different estimator.
    ProvenanceAlgorithmMismatch,
    /// Output provenance does not represent every registered source.
    ProvenanceSourceCountMismatch,
    /// Output provenance is not bound to the exact registration plan.
    ProvenancePlanMismatch,
    /// Weighted execution must use the identity-binding constructor.
    WeightedEstimatorRequiresWeights,
    /// Weight-expression identifier is not canonical.
    InvalidWeightAlgorithmId,
    /// Weights are missing, duplicated, or foreign to the sealed plan.
    WeightSetMismatch,
    /// Output provenance is not bound to the exact canonical weight set.
    WeightProvenanceMismatch,
    /// Rejection maps are meaningful only for a rejecting estimator.
    RejectionMapRequiresPercentileEstimator,
    /// Rejection-map provenance is inconsistent with the stack request.
    RejectionMapProvenanceMismatch,
    /// Science and companion products cannot target the same path.
    DuplicateOutputPath,
    /// A band cannot contain zero rows.
    ZeroBandHeight,
    /// A registered artifact does not carry its expected reviewed identity.
    ArtifactIdentityMismatch {
        /// Expected reviewed frame.
        frame_id: FrameId,
    },
    /// A registered artifact was produced by another registration plan.
    ArtifactPlanMismatch {
        /// Artifact whose plan binding failed.
        frame_id: FrameId,
    },
    /// A registered artifact does not use the complete reference canvas.
    DimensionMismatch {
        /// Artifact whose dimensions failed.
        frame_id: FrameId,
        /// Required reference-canvas dimensions.
        expected: Dimensions,
        /// Decoded artifact dimensions.
        actual: Dimensions,
    },
    /// Registered artifacts disagree on their mono or planar-RGB shape.
    PlaneCountMismatch {
        /// Plane count established by the first canonical source.
        expected: usize,
        /// Conflicting plane count.
        actual: usize,
    },
    /// Shared strict FITS or fingerprint validation failed.
    Input(StrictPipelineError),
    /// A FITS pixel region or checksum could not be decoded.
    ReadInput(ImageReadError),
    /// Strict mean integration failed.
    Integration(IntegrationError),
    /// The logical working set exceeded the supplied budget.
    Memory(MemoryBudgetError),
    /// Private FITS construction or atomic publication failed.
    Publish(AtomicFitsWriteError),
    /// A product created by this transaction could not be removed during rollback.
    RollbackPublishedOutput(std::io::Error),
    /// Complete private output or a registered input failed checksum validation.
    InvalidStagedOutput,
    /// Work-unit, coordinate, or byte accounting overflowed.
    WorkSizeOverflow,
    /// A bounded source or reference vector could not be allocated.
    AllocationFailed,
    /// Execution stopped at a cooperative checkpoint.
    Cancelled(Cancelled),
    /// The fixed stage identifier unexpectedly failed validation.
    StageId(StageIdError),
    /// A machine-readable progress event violated its invariant.
    Progress(ProgressEventError),
}

impl RegisteredStackError {
    const fn code(&self) -> &'static str {
        match self {
            Self::SourceSetMismatch => "registered-stack-source-set",
            Self::ProvenanceAlgorithmMismatch => "registered-stack-provenance-algorithm",
            Self::ProvenanceSourceCountMismatch => "registered-stack-provenance-count",
            Self::ProvenancePlanMismatch => "registered-stack-provenance-plan",
            Self::WeightedEstimatorRequiresWeights => "registered-stack-weight-constructor",
            Self::InvalidWeightAlgorithmId => "registered-stack-weight-algorithm",
            Self::WeightSetMismatch => "registered-stack-weight-set",
            Self::WeightProvenanceMismatch => "registered-stack-weight-provenance",
            Self::RejectionMapRequiresPercentileEstimator => "registered-stack-map-estimator",
            Self::RejectionMapProvenanceMismatch => "registered-stack-map-provenance",
            Self::DuplicateOutputPath => "registered-stack-output-path",
            Self::ZeroBandHeight => "registered-stack-band-height",
            Self::ArtifactIdentityMismatch { .. } => "registered-stack-artifact-identity",
            Self::ArtifactPlanMismatch { .. } => "registered-stack-artifact-plan",
            Self::DimensionMismatch { .. } => "registered-stack-dimensions",
            Self::PlaneCountMismatch { .. } => "registered-stack-planes",
            Self::Input(_) => "registered-stack-input",
            Self::ReadInput(_) => "registered-stack-read",
            Self::Integration(_) => "registered-stack-integration",
            Self::Memory(_) => "registered-stack-memory",
            Self::Publish(_) => "registered-stack-publish",
            Self::RollbackPublishedOutput(_) => "registered-stack-rollback",
            Self::InvalidStagedOutput => "registered-stack-readback",
            Self::WorkSizeOverflow => "registered-stack-work-size",
            Self::AllocationFailed => "registered-stack-allocation",
            Self::Cancelled(_) => "cancelled",
            Self::StageId(_) => "registered-stack-stage",
            Self::Progress(_) => "registered-stack-progress",
        }
    }
}

impl Display for RegisteredStackError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SourceSetMismatch => {
                formatter.write_str("registered source set does not match the sealed plan")
            }
            Self::ProvenanceAlgorithmMismatch => {
                formatter.write_str("output provenance names another integration algorithm")
            }
            Self::ProvenanceSourceCountMismatch => {
                formatter.write_str("output provenance source count is inconsistent")
            }
            Self::ProvenancePlanMismatch => {
                formatter.write_str("output provenance does not bind the sealed registration plan")
            }
            Self::WeightedEstimatorRequiresWeights => formatter
                .write_str("weighted integration requires an identity-bound frame-weight set"),
            Self::InvalidWeightAlgorithmId => {
                formatter.write_str("weight-expression algorithm identifier is not canonical")
            }
            Self::WeightSetMismatch => {
                formatter.write_str("frame-weight set does not match the sealed registration plan")
            }
            Self::WeightProvenanceMismatch => formatter
                .write_str("output provenance does not bind the canonical frame-weight set"),
            Self::RejectionMapRequiresPercentileEstimator => {
                formatter.write_str("rejection maps require the percentile-clipped estimator")
            }
            Self::RejectionMapProvenanceMismatch => {
                formatter.write_str("rejection-map provenance does not match the stack request")
            }
            Self::DuplicateOutputPath => {
                formatter.write_str("science and rejection-map outputs must use distinct paths")
            }
            Self::ZeroBandHeight => {
                formatter.write_str("registered stack band height must be positive")
            }
            Self::ArtifactIdentityMismatch { frame_id } => write!(
                formatter,
                "registered artifact identity does not match {}",
                frame_id.as_str()
            ),
            Self::ArtifactPlanMismatch { frame_id } => write!(
                formatter,
                "registered artifact for {} belongs to another plan",
                frame_id.as_str()
            ),
            Self::DimensionMismatch {
                frame_id,
                expected,
                actual,
            } => write!(
                formatter,
                "registered artifact {} is {}x{}x{}; expected {}x{}x{}",
                frame_id.as_str(),
                actual.width(),
                actual.height(),
                actual.planes(),
                expected.width(),
                expected.height(),
                expected.planes()
            ),
            Self::PlaneCountMismatch { expected, actual } => write!(
                formatter,
                "registered source plane count {actual} does not match {expected}"
            ),
            Self::Input(error) => Display::fmt(error, formatter),
            Self::ReadInput(error) => Display::fmt(error, formatter),
            Self::Integration(error) => Display::fmt(error, formatter),
            Self::Memory(error) => Display::fmt(error, formatter),
            Self::Publish(error) => Display::fmt(error, formatter),
            Self::RollbackPublishedOutput(error) => write!(
                formatter,
                "cannot remove a published registered-stack product during rollback: {error}"
            ),
            Self::InvalidStagedOutput => {
                formatter.write_str("private registered stack failed checksum readback")
            }
            Self::WorkSizeOverflow => {
                formatter.write_str("registered stack work accounting overflowed")
            }
            Self::AllocationFailed => formatter.write_str("registered stack allocation failed"),
            Self::Cancelled(error) => Display::fmt(error, formatter),
            Self::StageId(error) => Display::fmt(error, formatter),
            Self::Progress(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for RegisteredStackError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Input(error) => Some(error),
            Self::ReadInput(error) => Some(error),
            Self::Integration(error) => Some(error),
            Self::Memory(error) => Some(error),
            Self::Publish(error) => Some(error),
            Self::RollbackPublishedOutput(error) => Some(error),
            Self::Cancelled(error) => Some(error),
            Self::StageId(error) => Some(error),
            Self::Progress(error) => Some(error),
            _ => None,
        }
    }
}

/// Integrates the sealed common crop and publishes one checksum-verified FITS.
pub fn run_registered_stack<F>(
    request: &RegisteredStackRequest,
    cancellation: &CancellationToken,
    memory: &MemoryBudget,
    mut progress: F,
) -> Result<RegisteredStackResult, RegisteredStackError>
where
    F: FnMut(ProgressEvent),
{
    let stage = StageId::new(REGISTERED_STACK_STAGE_ID).map_err(RegisteredStackError::StageId)?;
    let sequence = ProgressSequence::new();
    emit(
        &sequence,
        &stage,
        ProgressState::Started,
        0,
        None,
        None,
        &mut progress,
    )?;
    let mut completed = 0;
    let mut total = None;
    let result = execute(
        request,
        cancellation,
        memory,
        &sequence,
        &stage,
        &mut completed,
        &mut total,
        &mut progress,
    );
    match result {
        Ok(value) => {
            emit(
                &sequence,
                &stage,
                ProgressState::Completed,
                completed,
                total,
                None,
                &mut progress,
            )?;
            Ok(value)
        }
        Err(error) => {
            let state = if matches!(error, RegisteredStackError::Cancelled(_)) {
                ProgressState::Cancelled
            } else {
                ProgressState::Failed
            };
            let _ignored = emit(
                &sequence,
                &stage,
                state,
                completed,
                total,
                Some(error.code().to_owned()),
                &mut progress,
            );
            Err(error)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn execute<F>(
    request: &RegisteredStackRequest,
    cancellation: &CancellationToken,
    memory: &MemoryBudget,
    sequence: &ProgressSequence,
    stage: &StageId,
    completed: &mut u64,
    total: &mut Option<u64>,
    progress: &mut F,
) -> Result<RegisteredStackResult, RegisteredStackError>
where
    F: FnMut(ProgressEvent),
{
    cancellation
        .checkpoint()
        .map_err(RegisteredStackError::Cancelled)?;
    let crop = request
        .plan
        .crop()
        .ok_or(RegisteredStackError::SourceSetMismatch)?;
    let planes = validate_sources(request, cancellation)?;
    let output_dimensions = Dimensions::new(crop.width(), crop.height(), planes)
        .map_err(|_| RegisteredStackError::WorkSizeOverflow)?;
    let bands = crop.height().div_ceil(request.band_height);
    *total = bands
        .checked_mul(planes)
        .and_then(|value| value.checked_add(2))
        .and_then(|value| u64::try_from(value).ok())
        .ok_or(RegisteredStackError::WorkSizeOverflow)
        .map(Some)?;
    emit(
        sequence,
        stage,
        ProgressState::Running,
        *completed,
        *total,
        None,
        progress,
    )?;

    let writer_count = if request.rejection_map.is_some() {
        3
    } else {
        1
    };
    let writer_bytes = STREAM_WRITER_BUFFER_BYTES
        .checked_mul(writer_count)
        .ok_or(RegisteredStackError::WorkSizeOverflow)?;
    let _writer = memory
        .try_reserve(writer_bytes)
        .map_err(RegisteredStackError::Memory)?;
    let mut writer = AtomicF64PrimaryStreamWriter::create_with_provenance(
        &request.output,
        output_dimensions,
        &request.provenance,
    )
    .map_err(RegisteredStackError::Publish)?;
    let mut rejection_writers = request
        .rejection_map
        .as_ref()
        .map(|maps| {
            Ok::<_, RegisteredStackError>((
                AtomicF64PrimaryStreamWriter::create_with_provenance(
                    &maps.low_output,
                    output_dimensions,
                    &maps.low_provenance,
                )
                .map_err(RegisteredStackError::Publish)?,
                AtomicF64PrimaryStreamWriter::create_with_provenance(
                    &maps.high_output,
                    output_dimensions,
                    &maps.high_provenance,
                )
                .map_err(RegisteredStackError::Publish)?,
            ))
        })
        .transpose()?;
    for plane in 0..planes {
        for offset_y in (0..crop.height()).step_by(request.band_height) {
            cancellation
                .checkpoint()
                .map_err(RegisteredStackError::Cancelled)?;
            let height = (crop.height() - offset_y).min(request.band_height);
            let reserved = planned_band_bytes(
                crop.width(),
                height,
                request.sources.len(),
                request.estimator,
                request.rejection_map.is_some(),
            )?;
            let _band = memory
                .try_reserve(reserved)
                .map_err(RegisteredStackError::Memory)?;
            let mut images = Vec::new();
            images
                .try_reserve_exact(request.sources.len())
                .map_err(|_| RegisteredStackError::AllocationFailed)?;
            for (index, source) in request.sources.iter().enumerate() {
                let mut reader = open_reader(
                    source.source.path(),
                    PipelineInput::Signal { index },
                    request.header_options,
                    request.validation_mode,
                )
                .map_err(RegisteredStackError::Input)?;
                let region = ImageRegion::new(
                    u64::try_from(plane).map_err(|_| RegisteredStackError::WorkSizeOverflow)?,
                    u64::try_from(crop.x()).map_err(|_| RegisteredStackError::WorkSizeOverflow)?,
                    u64::try_from(crop.y() + offset_y)
                        .map_err(|_| RegisteredStackError::WorkSizeOverflow)?,
                    u64::try_from(crop.width())
                        .map_err(|_| RegisteredStackError::WorkSizeOverflow)?,
                    u64::try_from(height).map_err(|_| RegisteredStackError::WorkSizeOverflow)?,
                );
                images.push(
                    reader
                        .read_region_image(region)
                        .map_err(RegisteredStackError::ReadInput)?,
                );
            }
            match request.estimator {
                RegisteredStackEstimator::StrictMean => {
                    let references = images.iter().collect::<Vec<_>>();
                    let integrated =
                        integrate_mean(&references).map_err(RegisteredStackError::Integration)?;
                    writer
                        .write_image_chunk(integrated.image())
                        .map_err(RegisteredStackError::Publish)?;
                }
                RegisteredStackEstimator::WeightedMean => {
                    let weights = request
                        .weights
                        .as_ref()
                        .ok_or(RegisteredStackError::WeightedEstimatorRequiresWeights)?;
                    let weighted = images
                        .iter()
                        .zip(weights.iter().copied())
                        .collect::<Vec<_>>();
                    let integrated = integrate_weighted_mean(&weighted)
                        .map_err(RegisteredStackError::Integration)?;
                    writer
                        .write_image_chunk(integrated.image())
                        .map_err(RegisteredStackError::Publish)?;
                }
                RegisteredStackEstimator::PercentileClipped(parameters) => {
                    let references = images.iter().collect::<Vec<_>>();
                    let integrated = integrate_percentile_clipped_mean(&references, parameters)
                        .map_err(RegisteredStackError::Integration)?;
                    writer
                        .write_image_chunk(integrated.image())
                        .map_err(RegisteredStackError::Publish)?;
                    if let Some((low_writer, high_writer)) = rejection_writers.as_mut() {
                        let maps = materialize_percentile_rejection_map(
                            integrated.image().dimensions(),
                            integrated.support(),
                        )
                        .map_err(RegisteredStackError::Integration)?;
                        low_writer
                            .write_image_chunk(maps.low())
                            .map_err(RegisteredStackError::Publish)?;
                        high_writer
                            .write_image_chunk(maps.high())
                            .map_err(RegisteredStackError::Publish)?;
                    }
                }
            }
            *completed = completed
                .checked_add(1)
                .ok_or(RegisteredStackError::WorkSizeOverflow)?;
            emit(
                sequence,
                stage,
                ProgressState::Running,
                *completed,
                *total,
                None,
                progress,
            )?;
        }
    }
    let staged = writer.finish().map_err(RegisteredStackError::Publish)?;
    validate_staged(&staged, output_dimensions)?;
    let staged_rejection_maps = rejection_writers
        .map(|(low, high)| {
            let low = low.finish().map_err(RegisteredStackError::Publish)?;
            validate_staged(&low, output_dimensions)?;
            let high = high.finish().map_err(RegisteredStackError::Publish)?;
            validate_staged(&high, output_dimensions)?;
            Ok::<_, RegisteredStackError>((low, high))
        })
        .transpose()?;
    *completed = completed
        .checked_add(1)
        .ok_or(RegisteredStackError::WorkSizeOverflow)?;
    emit(
        sequence,
        stage,
        ProgressState::Running,
        *completed,
        *total,
        None,
        progress,
    )?;
    revalidate_sources(request, cancellation)?;
    cancellation
        .checkpoint()
        .map_err(RegisteredStackError::Cancelled)?;
    let summary = match staged.publish() {
        Ok(summary) => summary,
        Err(error) => {
            if error.output_is_published() {
                rollback_published(&[&request.output])?;
            }
            return Err(RegisteredStackError::Publish(error));
        }
    };
    let rejection_map_summary = if let Some((low, high)) = staged_rejection_maps {
        let map_paths = request
            .rejection_map
            .as_ref()
            .ok_or(RegisteredStackError::RejectionMapProvenanceMismatch)?;
        let low_summary = match low.publish() {
            Ok(summary) => summary,
            Err(error) => {
                if error.output_is_published() {
                    rollback_published(&[&map_paths.low_output, &request.output])?;
                } else {
                    rollback_published(&[&request.output])?;
                }
                return Err(RegisteredStackError::Publish(error));
            }
        };
        let high_summary = match high.publish() {
            Ok(summary) => summary,
            Err(error) => {
                if error.output_is_published() {
                    rollback_published(&[
                        &map_paths.high_output,
                        &map_paths.low_output,
                        &request.output,
                    ])?;
                } else {
                    rollback_published(&[&map_paths.low_output, &request.output])?;
                }
                return Err(RegisteredStackError::Publish(error));
            }
        };
        Some(RegisteredRejectionMapSummary {
            low: low_summary,
            high: high_summary,
        })
    } else {
        None
    };
    *completed = completed
        .checked_add(1)
        .ok_or(RegisteredStackError::WorkSizeOverflow)?;
    Ok(RegisteredStackResult {
        dimensions: output_dimensions,
        summary,
        peak_reserved_bytes: memory.peak(),
        rejection_map_summary,
    })
}

fn rollback_published(paths: &[&Path]) -> Result<(), RegisteredStackError> {
    for path in paths {
        fs::remove_file(path).map_err(RegisteredStackError::RollbackPublishedOutput)?;
    }
    Ok(())
}

fn validate_sources(
    request: &RegisteredStackRequest,
    cancellation: &CancellationToken,
) -> Result<usize, RegisteredStackError> {
    let mut planes = None;
    for (index, source) in request.sources.iter().enumerate() {
        cancellation
            .checkpoint()
            .map_err(RegisteredStackError::Cancelled)?;
        let input = PipelineInput::Signal { index };
        verify_source(&source.source, input).map_err(RegisteredStackError::Input)?;
        let mut reader = open_reader(
            source.source.path(),
            input,
            request.header_options,
            request.validation_mode,
        )
        .map_err(RegisteredStackError::Input)?;
        if reader.report().header().string("AETHFID") != Some(source.frame_id.as_str()) {
            return Err(RegisteredStackError::ArtifactIdentityMismatch {
                frame_id: source.frame_id.clone(),
            });
        }
        if reader.report().header().string("AETHPLN") != Some(request.plan.plan_sha256()) {
            return Err(RegisteredStackError::ArtifactPlanMismatch {
                frame_id: source.frame_id.clone(),
            });
        }
        let actual = dimensions_from_axes(input, reader.descriptor().axes())
            .map_err(RegisteredStackError::Input)?;
        let (reference_width, reference_height) = request.plan.reference_dimensions();
        let expected = Dimensions::new(reference_width, reference_height, actual.planes())
            .map_err(|_| RegisteredStackError::WorkSizeOverflow)?;
        if actual != expected {
            return Err(RegisteredStackError::DimensionMismatch {
                frame_id: source.frame_id.clone(),
                expected,
                actual,
            });
        }
        if let Some(expected_planes) = planes
            && expected_planes != actual.planes()
        {
            return Err(RegisteredStackError::PlaneCountMismatch {
                expected: expected_planes,
                actual: actual.planes(),
            });
        }
        planes = Some(actual.planes());
        let checksums = reader
            .verify_checksums()
            .map_err(RegisteredStackError::ReadInput)?;
        if !checksums.is_fully_verified() {
            return Err(RegisteredStackError::InvalidStagedOutput);
        }
    }
    planes.ok_or(RegisteredStackError::SourceSetMismatch)
}

fn revalidate_sources(
    request: &RegisteredStackRequest,
    cancellation: &CancellationToken,
) -> Result<(), RegisteredStackError> {
    for (index, source) in request.sources.iter().enumerate() {
        cancellation
            .checkpoint()
            .map_err(RegisteredStackError::Cancelled)?;
        verify_source(&source.source, PipelineInput::Signal { index })
            .map_err(RegisteredStackError::Input)?;
    }
    Ok(())
}

fn planned_band_bytes(
    width: usize,
    height: usize,
    source_count: usize,
    estimator: RegisteredStackEstimator,
    rejection_maps: bool,
) -> Result<usize, RegisteredStackError> {
    let samples = width
        .checked_mul(height)
        .ok_or(RegisteredStackError::WorkSizeOverflow)?;
    let image = samples
        .checked_mul(size_of::<f64>() + size_of::<PixelFlags>())
        .ok_or(RegisteredStackError::WorkSizeOverflow)?;
    let sources = image
        .checked_mul(source_count)
        .ok_or(RegisteredStackError::WorkSizeOverflow)?;
    let support_size = match estimator {
        RegisteredStackEstimator::StrictMean | RegisteredStackEstimator::WeightedMean => {
            size_of::<PixelSupport>()
        }
        RegisteredStackEstimator::PercentileClipped(_) => size_of::<ClippedPixelSupport>(),
    };
    let output = image
        .checked_add(
            samples
                .checked_mul(support_size)
                .ok_or(RegisteredStackError::WorkSizeOverflow)?,
        )
        .ok_or(RegisteredStackError::WorkSizeOverflow)?;
    let decode = samples
        .checked_mul(size_of::<SampleStatus>())
        .ok_or(RegisteredStackError::WorkSizeOverflow)?;
    let integration_input_size = match estimator {
        RegisteredStackEstimator::WeightedMean => {
            size_of::<(&aether_core::ScientificImage, FrameWeight)>()
        }
        RegisteredStackEstimator::StrictMean | RegisteredStackEstimator::PercentileClipped(_) => {
            size_of::<&aether_core::ScientificImage>()
        }
    };
    let vector_storage = source_count
        .checked_mul(size_of::<aether_core::ScientificImage>() + integration_input_size)
        .ok_or(RegisteredStackError::WorkSizeOverflow)?;
    let estimator_scratch = match estimator {
        RegisteredStackEstimator::StrictMean | RegisteredStackEstimator::WeightedMean => 0,
        RegisteredStackEstimator::PercentileClipped(_) => source_count
            .checked_mul(size_of::<f64>())
            .ok_or(RegisteredStackError::WorkSizeOverflow)?,
    };
    let rejection_map_images = if rejection_maps {
        image
            .checked_mul(2)
            .ok_or(RegisteredStackError::WorkSizeOverflow)?
    } else {
        0
    };
    sources
        .checked_add(output)
        .and_then(|value| value.checked_add(decode))
        .and_then(|value| value.checked_add(vector_storage))
        .and_then(|value| value.checked_add(estimator_scratch))
        .and_then(|value| value.checked_add(rejection_map_images))
        .ok_or(RegisteredStackError::WorkSizeOverflow)
}

fn validate_staged(
    staged: &aether_fits::CompletedAtomicFits,
    expected: Dimensions,
) -> Result<(), RegisteredStackError> {
    let file = staged
        .try_clone_for_readback()
        .map_err(RegisteredStackError::Publish)?;
    let mut reader = PrimaryImageReader::open(file, HeaderReadOptions::default())
        .map_err(RegisteredStackError::ReadInput)?;
    let actual = dimensions_from_axes(
        PipelineInput::Signal { index: 0 },
        reader.descriptor().axes(),
    )
    .map_err(RegisteredStackError::Input)?;
    let checksums = reader
        .verify_checksums()
        .map_err(RegisteredStackError::ReadInput)?;
    if actual != expected || !checksums.is_fully_verified() {
        return Err(RegisteredStackError::InvalidStagedOutput);
    }
    Ok(())
}

fn is_weight_algorithm_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 32
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

fn update_digest_string(hasher: &mut Sha256, value: &str) -> Result<(), RegisteredStackError> {
    let length = u32::try_from(value.len()).map_err(|_| RegisteredStackError::WorkSizeOverflow)?;
    hasher.update(length.to_be_bytes());
    hasher.update(value.as_bytes());
    Ok(())
}

fn update_quality_metrics(hasher: &mut Sha256, metrics: QualityWeightMetrics) {
    hasher.update(metrics.signal_to_noise().to_bits().to_be_bytes());
    hasher.update(metrics.fwhm_pixels().to_bits().to_be_bytes());
    hasher.update(metrics.eccentricity().to_bits().to_be_bytes());
}

fn encode_lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[allow(clippy::too_many_arguments)]
fn emit<F>(
    sequence: &ProgressSequence,
    stage: &StageId,
    state: ProgressState,
    completed: u64,
    total: Option<u64>,
    code: Option<String>,
    progress: &mut F,
) -> Result<(), RegisteredStackError>
where
    F: FnMut(ProgressEvent),
{
    let event = sequence
        .next(stage.clone(), state, completed, total, code)
        .map_err(RegisteredStackError::Progress)?;
    progress(event);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fs::{self, File};
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    use aether_core::ScientificImage;
    use aether_fits::write_f64_primary_atomic_new_with_provenance;
    use aether_registration::{
        AffineTransform, LANCZOS3_RESAMPLING_ALGORITHM_ID, PlannedRegistrationFrame,
        ProjectivePlannedRegistrationFrame, ProjectiveTransform,
    };
    use aether_session::fingerprint_reader;

    use super::*;

    type TestResult = Result<(), Box<dyn Error>>;
    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> std::io::Result<Self> {
            let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "aether-registered-stack-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path)?;
            Ok(Self(path))
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ignored = fs::remove_dir_all(&self.0);
        }
    }

    fn id(digit: char) -> Result<FrameId, aether_review::ReviewError> {
        FrameId::new(digit.to_string().repeat(64))
    }

    fn plan() -> Result<RegistrationPlan, Box<dyn Error>> {
        Ok(RegistrationPlan::new(
            id('a')?,
            12,
            10,
            vec![
                PlannedRegistrationFrame::new(id('a')?, 12, 10, AffineTransform::IDENTITY),
                PlannedRegistrationFrame::new(
                    id('b')?,
                    12,
                    10,
                    AffineTransform::new(1.0, 0.0, 0.0, 1.0, 2.0, 1.0)?,
                ),
            ],
        )?)
    }

    fn registered_source(
        directory: &TestDirectory,
        plan: &RegistrationPlan,
        frame_id: FrameId,
        value: f64,
    ) -> Result<RegisteredStackSource, Box<dyn Error>> {
        let image = ScientificImage::filled(Dimensions::new(12, 10, 3)?, value)?;
        let path = directory
            .0
            .join(format!("registered-{}.fits", frame_id.as_str()));
        let provenance = FitsOutputProvenance::new(
            "a".repeat(64),
            "registered-light",
            LANCZOS3_RESAMPLING_ALGORITHM_ID,
            1,
        )?
        .with_plan_sha256(plan.plan_sha256())?
        .with_frame_id_sha256(frame_id.as_str())?;
        write_f64_primary_atomic_new_with_provenance(&path, &image, &provenance)?;
        let mut file = File::open(&path)?;
        let fingerprint = fingerprint_reader(&mut file)?;
        Ok(RegisteredStackSource::new(
            frame_id,
            PipelineSource::new(path, fingerprint),
        ))
    }

    fn projective_plan() -> Result<ProjectiveRegistrationPlan, Box<dyn Error>> {
        Ok(ProjectiveRegistrationPlan::new(
            id('a')?,
            12,
            10,
            vec![
                ProjectivePlannedRegistrationFrame::new(
                    id('a')?,
                    12,
                    10,
                    ProjectiveTransform::IDENTITY,
                ),
                ProjectivePlannedRegistrationFrame::new(
                    id('b')?,
                    12,
                    10,
                    ProjectiveTransform::new([
                        [1.0, 0.0, 0.2],
                        [0.0, 1.0, -0.1],
                        [2.0e-4, -1.0e-4, 1.0],
                    ])?,
                ),
            ],
        )?)
    }

    fn projective_registered_source(
        directory: &TestDirectory,
        plan: &ProjectiveRegistrationPlan,
        frame_id: FrameId,
        value: f64,
    ) -> Result<RegisteredStackSource, Box<dyn Error>> {
        let image = ScientificImage::filled(Dimensions::new(12, 10, 3)?, value)?;
        let path = directory
            .0
            .join(format!("projective-registered-{}.fits", frame_id.as_str()));
        let provenance = FitsOutputProvenance::new(
            "a".repeat(64),
            "registered-light-projective",
            LANCZOS3_RESAMPLING_ALGORITHM_ID,
            1,
        )?
        .with_plan_sha256(plan.plan_sha256())?
        .with_frame_id_sha256(frame_id.as_str())?;
        write_f64_primary_atomic_new_with_provenance(&path, &image, &provenance)?;
        let mut file = File::open(&path)?;
        let fingerprint = fingerprint_reader(&mut file)?;
        Ok(RegisteredStackSource::new(
            frame_id,
            PipelineSource::new(path, fingerprint),
        ))
    }

    #[test]
    fn integrates_the_exact_projective_common_crop() -> TestResult {
        let directory = TestDirectory::new()?;
        let plan = projective_plan()?;
        let crop = plan.common_footprint().crop().ok_or("crop missing")?;
        let sources = vec![
            projective_registered_source(&directory, &plan, id('b')?, 4.0)?,
            projective_registered_source(&directory, &plan, id('a')?, 2.0)?,
        ];
        let provenance = FitsOutputProvenance::new(
            "a".repeat(64),
            "registered-stack-projective",
            REGISTERED_CROP_MEAN_ALGORITHM_ID,
            2,
        )?
        .with_plan_sha256(plan.plan_sha256())?;
        let one_row_output = directory.0.join("integrated-projective-one-row.fits");
        let four_row_output = directory.0.join("integrated-projective-four-rows.fits");
        let request = RegisteredStackRequest::new_projective_with_estimator(
            plan.clone(),
            sources.clone(),
            one_row_output.clone(),
            provenance.clone(),
            RegisteredStackEstimator::StrictMean,
        )?
        .with_band_height(1)?;
        let four_row_request = RegisteredStackRequest::new_projective_with_estimator(
            plan,
            sources,
            four_row_output.clone(),
            provenance,
            RegisteredStackEstimator::StrictMean,
        )?
        .with_band_height(4)?;

        let result = run_registered_stack(
            &request,
            &CancellationToken::new(),
            &MemoryBudget::new(2_000_000)?,
            |_| {},
        )?;
        run_registered_stack(
            &four_row_request,
            &CancellationToken::new(),
            &MemoryBudget::new(2_000_000)?,
            |_| {},
        )?;

        assert_eq!(
            result.dimensions(),
            Dimensions::new(crop.width(), crop.height(), 3)?
        );
        assert_eq!(fs::read(&one_row_output)?, fs::read(&four_row_output)?);
        let mut reader =
            PrimaryImageReader::open(File::open(request.output())?, HeaderReadOptions::default())?;
        for plane in 0..3 {
            let image = reader.read_region_image(ImageRegion::new(
                plane,
                0,
                0,
                u64::try_from(crop.width())?,
                u64::try_from(crop.height())?,
            ))?;
            assert!(
                image
                    .pixels()
                    .iter()
                    .all(|value| value.to_bits() == 3.0_f64.to_bits())
            );
        }
        Ok(())
    }

    fn stack_request(directory: &TestDirectory) -> Result<RegisteredStackRequest, Box<dyn Error>> {
        let plan = plan()?;
        let sources = vec![
            registered_source(directory, &plan, id('b')?, 4.0)?,
            registered_source(directory, &plan, id('a')?, 2.0)?,
        ];
        let provenance = FitsOutputProvenance::new(
            "a".repeat(64),
            "registered-stack",
            REGISTERED_CROP_MEAN_ALGORITHM_ID,
            2,
        )?
        .with_plan_sha256(plan.plan_sha256())?;
        Ok(RegisteredStackRequest::new(
            plan,
            sources,
            directory.0.join("integrated-registered.fits"),
            provenance,
        )?
        .with_band_height(2)?)
    }

    fn percentile_stack_request(
        directory: &TestDirectory,
    ) -> Result<(RegisteredStackRequest, PathBuf, PathBuf), Box<dyn Error>> {
        let identities = ['a', 'b', 'c', 'd', 'e']
            .into_iter()
            .map(id)
            .collect::<Result<Vec<_>, _>>()?;
        let plan = RegistrationPlan::new(
            identities[0].clone(),
            12,
            10,
            identities
                .iter()
                .cloned()
                .map(|frame_id| {
                    PlannedRegistrationFrame::new(frame_id, 12, 10, AffineTransform::IDENTITY)
                })
                .collect(),
        )?;
        let sources = identities
            .into_iter()
            .zip([0.0, 10.0, 11.0, 12.0, 100.0])
            .map(|(frame_id, value)| registered_source(directory, &plan, frame_id, value))
            .collect::<Result<Vec<_>, _>>()?;
        let provenance = FitsOutputProvenance::new(
            "a".repeat(64),
            "registered-stack",
            REGISTERED_PERCENTILE_CLIPPED_MEAN_ALGORITHM_ID,
            5,
        )?
        .with_plan_sha256(plan.plan_sha256())?;
        let low_path = directory.0.join("percentile-low-rejection.fits");
        let high_path = directory.0.join("percentile-high-rejection.fits");
        let low_provenance = FitsOutputProvenance::new(
            "a".repeat(64),
            "registered-rejection-low",
            PERCENTILE_REJECTION_MAP_ALGORITHM_ID,
            5,
        )?
        .with_plan_sha256(plan.plan_sha256())?;
        let high_provenance = FitsOutputProvenance::new(
            "a".repeat(64),
            "registered-rejection-high",
            PERCENTILE_REJECTION_MAP_ALGORITHM_ID,
            5,
        )?
        .with_plan_sha256(plan.plan_sha256())?;
        let request = RegisteredStackRequest::new_with_estimator(
            plan,
            sources,
            directory.0.join("percentile-stack.fits"),
            provenance,
            RegisteredStackEstimator::PercentileClipped(PercentileClipParameters::new(
                0.2, 0.2, 3,
            )?),
        )?
        .with_band_height(3)?
        .with_rejection_map(RegisteredRejectionMapOutput::new(
            low_path.clone(),
            low_provenance,
            high_path.clone(),
            high_provenance,
        ))?;
        Ok((request, low_path, high_path))
    }

    fn weighted_stack_request(
        directory: &TestDirectory,
    ) -> Result<(RegisteredStackRequest, String), Box<dyn Error>> {
        let plan = plan()?;
        let sources = vec![
            registered_source(directory, &plan, id('b')?, 8.0)?,
            registered_source(directory, &plan, id('a')?, 2.0)?,
        ];
        let weight_set = RegisteredWeightSet::new(
            BALANCED_PSF_WEIGHT_ALGORITHM_ID,
            vec![
                RegisteredFrameWeight::new(id('b')?, FrameWeight::new(3.0)?),
                RegisteredFrameWeight::new(id('a')?, FrameWeight::new(1.0)?),
            ],
        )?;
        let digest = weight_set.sha256().to_owned();
        let provenance = FitsOutputProvenance::new(
            "a".repeat(64),
            "registered-weighted-stack",
            REGISTERED_WEIGHTED_MEAN_ALGORITHM_ID,
            2,
        )?
        .with_plan_sha256(plan.plan_sha256())?
        .with_parameters_sha256(&digest)?;
        let request = RegisteredStackRequest::new_weighted(
            plan,
            sources,
            directory.0.join("weighted-stack.fits"),
            provenance,
            weight_set,
        )?
        .with_band_height(2)?;
        Ok((request, digest))
    }

    #[test]
    fn integrates_only_the_sealed_crop_and_publishes_checksums() -> TestResult {
        let directory = TestDirectory::new()?;
        let request = stack_request(&directory)?;
        let memory = MemoryBudget::new(16 * 1_024 * 1_024)?;
        let mut progress = Vec::new();

        let result = run_registered_stack(&request, &CancellationToken::new(), &memory, |event| {
            progress.push(event)
        })?;

        assert_eq!(result.dimensions(), Dimensions::new(10, 9, 3)?);
        assert!(result.peak_reserved_bytes() <= memory.limit());
        assert_eq!(
            progress.first().map(ProgressEvent::state),
            Some(ProgressState::Started)
        );
        assert_eq!(
            progress.last().map(ProgressEvent::state),
            Some(ProgressState::Completed)
        );
        let file = File::open(request.output())?;
        let mut reader = PrimaryImageReader::open(file, HeaderReadOptions::default())?;
        assert!(reader.verify_checksums()?.is_fully_verified());
        let output = reader.read_region_image(ImageRegion::new(0, 0, 0, 10, 9))?;
        assert!(
            output
                .pixels()
                .iter()
                .all(|value| value.to_bits() == 3.0_f64.to_bits())
        );
        Ok(())
    }

    #[test]
    fn weight_set_digest_is_order_independent_but_identity_and_value_sensitive() -> TestResult {
        let first = RegisteredFrameWeight::new(id('a')?, FrameWeight::new(1.0)?);
        let second = RegisteredFrameWeight::new(id('b')?, FrameWeight::new(2.0)?);
        let forward = RegisteredWeightSet::new(
            BALANCED_PSF_WEIGHT_ALGORITHM_ID,
            vec![first.clone(), second.clone()],
        )?;
        let reversed = RegisteredWeightSet::new(
            BALANCED_PSF_WEIGHT_ALGORITHM_ID,
            vec![second.clone(), first.clone()],
        )?;
        let changed = RegisteredWeightSet::new(
            BALANCED_PSF_WEIGHT_ALGORITHM_ID,
            vec![
                first,
                RegisteredFrameWeight::new(id('b')?, FrameWeight::new(3.0)?),
            ],
        )?;

        assert_eq!(forward.sha256(), reversed.sha256());
        assert_ne!(forward.sha256(), changed.sha256());
        assert_eq!(forward.sha256().len(), 64);
        assert_eq!(forward.len(), 2);
        assert!(!forward.is_empty());
        assert_eq!(
            forward.weight_algorithm_id(),
            BALANCED_PSF_WEIGHT_ALGORITHM_ID
        );
        assert!(matches!(
            RegisteredWeightSet::new(
                BALANCED_PSF_WEIGHT_ALGORITHM_ID,
                vec![second.clone(), second]
            ),
            Err(RegisteredStackError::WeightSetMismatch)
        ));
        assert!(matches!(
            RegisteredWeightSet::new("Invalid/algorithm", vec![]),
            Err(RegisteredStackError::InvalidWeightAlgorithmId)
        ));
        Ok(())
    }

    #[test]
    fn balanced_metric_evidence_is_canonical_and_binds_the_reference() -> TestResult {
        let reference = QualityWeightMetrics::new(10.0, 2.0, 0.0)?;
        let first = RegisteredFrameQuality::new(id('a')?, reference);
        let second =
            RegisteredFrameQuality::new(id('b')?, QualityWeightMetrics::new(20.0, 2.0, 0.0)?);
        let forward = RegisteredWeightSet::from_balanced_psf_metrics(
            reference,
            vec![first.clone(), second.clone()],
        )?;
        let reversed =
            RegisteredWeightSet::from_balanced_psf_metrics(reference, vec![second, first])?;

        assert_eq!(forward.sha256(), reversed.sha256());
        assert_eq!(
            forward
                .weights
                .get(&id('a')?)
                .map(|weight| weight.get().to_bits()),
            Some(1.0_f64.to_bits())
        );
        assert!(
            (forward
                .weights
                .get(&id('b')?)
                .ok_or("missing weight")?
                .get()
                - 4.0)
                .abs()
                < 1.0e-14
        );

        let scaled_reference = QualityWeightMetrics::new(20.0, 2.0, 0.0)?;
        let same_final_weights = RegisteredWeightSet::from_balanced_psf_metrics(
            scaled_reference,
            vec![
                RegisteredFrameQuality::new(id('a')?, scaled_reference),
                RegisteredFrameQuality::new(id('b')?, QualityWeightMetrics::new(40.0, 2.0, 0.0)?),
            ],
        )?;
        assert_eq!(forward.weights, same_final_weights.weights);
        assert_ne!(forward.sha256(), same_final_weights.sha256());
        Ok(())
    }

    #[test]
    fn executes_identity_bound_weighted_stack_and_publishes_parameter_digest() -> TestResult {
        let directory = TestDirectory::new()?;
        let (request, digest) = weighted_stack_request(&directory)?;
        let memory = MemoryBudget::new(16 * 1_024 * 1_024)?;

        let result = run_registered_stack(&request, &CancellationToken::new(), &memory, |_| {})?;

        assert_eq!(result.dimensions(), Dimensions::new(10, 9, 3)?);
        assert!(result.peak_reserved_bytes() <= memory.limit());
        let file = File::open(request.output())?;
        let mut reader = PrimaryImageReader::open(file, HeaderReadOptions::default())?;
        assert!(reader.verify_checksums()?.is_fully_verified());
        assert_eq!(
            reader.report().header().string("AETHPAR"),
            Some(digest.as_str())
        );
        let output = reader.read_region_image(ImageRegion::new(0, 0, 0, 10, 9))?;
        assert!(
            output
                .pixels()
                .iter()
                .all(|value| value.to_bits() == 6.5_f64.to_bits())
        );
        Ok(())
    }

    #[test]
    fn weighted_request_refuses_missing_weights_and_unbound_provenance() -> TestResult {
        let directory = TestDirectory::new()?;
        let plan = plan()?;
        let sources = vec![
            registered_source(&directory, &plan, id('a')?, 2.0)?,
            registered_source(&directory, &plan, id('b')?, 8.0)?,
        ];
        let provenance = FitsOutputProvenance::new(
            "a".repeat(64),
            "registered-weighted-stack",
            REGISTERED_WEIGHTED_MEAN_ALGORITHM_ID,
            2,
        )?
        .with_plan_sha256(plan.plan_sha256())?;
        assert!(matches!(
            RegisteredStackRequest::new_with_estimator(
                plan.clone(),
                sources.clone(),
                directory.0.join("missing-weights.fits"),
                provenance.clone(),
                RegisteredStackEstimator::WeightedMean,
            ),
            Err(RegisteredStackError::WeightedEstimatorRequiresWeights)
        ));
        let incomplete = RegisteredWeightSet::new(
            BALANCED_PSF_WEIGHT_ALGORITHM_ID,
            vec![RegisteredFrameWeight::new(id('a')?, FrameWeight::new(1.0)?)],
        )?;
        assert!(matches!(
            RegisteredStackRequest::new_weighted(
                plan,
                sources,
                directory.0.join("unbound-weights.fits"),
                provenance,
                incomplete,
            ),
            Err(RegisteredStackError::WeightProvenanceMismatch)
        ));
        Ok(())
    }

    #[test]
    fn executes_versioned_percentile_rejection_in_bounded_bands() -> TestResult {
        let directory = TestDirectory::new()?;
        let (request, low_path, high_path) = percentile_stack_request(&directory)?;
        let memory = MemoryBudget::new(16 * 1_024 * 1_024)?;

        let result = run_registered_stack(&request, &CancellationToken::new(), &memory, |_| {})?;

        assert_eq!(result.dimensions(), Dimensions::new(12, 10, 3)?);
        assert!(result.peak_reserved_bytes() <= memory.limit());
        let map_summary = result
            .rejection_map_summary()
            .ok_or("rejection maps were not published")?;
        assert_eq!(map_summary.low().samples_written(), 12 * 10 * 3);
        assert_eq!(map_summary.high().samples_written(), 12 * 10 * 3);
        let file = File::open(request.output())?;
        let mut reader = PrimaryImageReader::open(file, HeaderReadOptions::default())?;
        assert!(reader.verify_checksums()?.is_fully_verified());
        let output = reader.read_region_image(ImageRegion::new(0, 0, 0, 12, 10))?;
        assert!(
            output
                .pixels()
                .iter()
                .all(|value| value.to_bits() == 11.0_f64.to_bits())
        );
        for path in [low_path, high_path] {
            let file = File::open(path)?;
            let mut reader = PrimaryImageReader::open(file, HeaderReadOptions::default())?;
            assert!(reader.verify_checksums()?.is_fully_verified());
            let map = reader.read_region_image(ImageRegion::new(0, 0, 0, 12, 10))?;
            assert!(
                map.pixels()
                    .iter()
                    .all(|value| value.to_bits() == 1.0_f64.to_bits())
            );
        }
        Ok(())
    }

    #[test]
    fn companion_collision_rolls_back_every_product_created_by_the_run() -> TestResult {
        let directory = TestDirectory::new()?;
        let (request, low_path, high_path) = percentile_stack_request(&directory)?;
        let science_path = request.output().to_owned();
        let blocker = b"pre-existing user data";
        let final_private_validation = 13;
        let collision_error = RefCell::new(None);

        let error = run_registered_stack(
            &request,
            &CancellationToken::new(),
            &MemoryBudget::new(16 * 1_024 * 1_024)?,
            |event| {
                if event.state() == ProgressState::Running
                    && event.completed_units() == final_private_validation
                {
                    *collision_error.borrow_mut() = fs::write(&high_path, blocker).err();
                }
            },
        )
        .err()
        .ok_or("a colliding companion destination unexpectedly succeeded")?;

        if let Some(error) = collision_error.into_inner() {
            return Err(error.into());
        }

        assert!(matches!(error, RegisteredStackError::Publish(_)));
        assert!(!science_path.exists());
        assert!(!low_path.exists());
        assert_eq!(fs::read(&high_path)?, blocker);
        Ok(())
    }

    #[test]
    fn strict_mean_rejects_rejection_map_configuration() -> TestResult {
        let directory = TestDirectory::new()?;
        let request = stack_request(&directory)?;
        let provenance = FitsOutputProvenance::new(
            "a".repeat(64),
            "registered-rejection",
            PERCENTILE_REJECTION_MAP_ALGORITHM_ID,
            2,
        )?
        .with_plan_sha256(request.plan_sha256())?;

        let error = request
            .with_rejection_map(RegisteredRejectionMapOutput::new(
                directory.0.join("low.fits"),
                provenance.clone(),
                directory.0.join("high.fits"),
                provenance,
            ))
            .err()
            .ok_or("strict mean unexpectedly accepted rejection maps")?;

        assert!(matches!(
            error,
            RegisteredStackError::RejectionMapRequiresPercentileEstimator
        ));
        Ok(())
    }

    #[test]
    fn rejection_map_paths_and_provenance_fail_closed() -> TestResult {
        let directory = TestDirectory::new()?;
        let (request, _, _) = percentile_stack_request(&directory)?;
        let configured = request
            .rejection_map
            .as_ref()
            .ok_or("test request is missing rejection-map configuration")?;
        let duplicate = RegisteredRejectionMapOutput::new(
            request.output().to_owned(),
            configured.low_provenance.clone(),
            directory.0.join("distinct-high.fits"),
            configured.high_provenance.clone(),
        );
        assert!(matches!(
            request.clone().with_rejection_map(duplicate),
            Err(RegisteredStackError::DuplicateOutputPath)
        ));

        let wrong_plan = FitsOutputProvenance::new(
            "a".repeat(64),
            "registered-rejection-low",
            PERCENTILE_REJECTION_MAP_ALGORITHM_ID,
            5,
        )?
        .with_plan_sha256("b".repeat(64))?;
        let mismatched = RegisteredRejectionMapOutput::new(
            directory.0.join("wrong-low.fits"),
            wrong_plan,
            directory.0.join("valid-high.fits"),
            configured.high_provenance.clone(),
        );
        assert!(matches!(
            request.with_rejection_map(mismatched),
            Err(RegisteredStackError::RejectionMapProvenanceMismatch)
        ));
        Ok(())
    }

    #[test]
    fn cancellation_and_memory_failure_publish_nothing() -> TestResult {
        let directory = TestDirectory::new()?;
        let request = stack_request(&directory)?;
        let cancellation = CancellationToken::new();
        let _was_first_request = cancellation.cancel();
        assert!(matches!(
            run_registered_stack(
                &request,
                &cancellation,
                &MemoryBudget::new(16 * 1_024 * 1_024)?,
                |_| {},
            ),
            Err(RegisteredStackError::Cancelled(_))
        ));
        assert!(!request.output().exists());

        assert!(matches!(
            run_registered_stack(
                &request,
                &CancellationToken::new(),
                &MemoryBudget::new(1)?,
                |_| {},
            ),
            Err(RegisteredStackError::Memory(_))
        ));
        assert!(!request.output().exists());
        Ok(())
    }

    #[test]
    fn cancellation_between_bands_and_late_source_mutation_publish_nothing() -> TestResult {
        let directory = TestDirectory::new()?;
        let request = stack_request(&directory)?;
        let cancellation = CancellationToken::new();
        let cancellation_error = run_registered_stack(
            &request,
            &cancellation,
            &MemoryBudget::new(16 * 1_024 * 1_024)?,
            |event| {
                if event.state() == ProgressState::Running && event.completed_units() == 1 {
                    let _was_first_request = cancellation.cancel();
                }
            },
        )
        .err()
        .ok_or("mid-run cancellation unexpectedly published a stack")?;
        assert!(matches!(
            cancellation_error,
            RegisteredStackError::Cancelled(_)
        ));
        assert!(!request.output().exists());

        let mutation_directory = TestDirectory::new()?;
        let request = stack_request(&mutation_directory)?;
        let changed_path = request.sources()[0].source().path().to_owned();
        let final_band = 15;
        let error = run_registered_stack(
            &request,
            &CancellationToken::new(),
            &MemoryBudget::new(16 * 1_024 * 1_024)?,
            |event| {
                if event.state() == ProgressState::Running
                    && event.completed_units() == final_band
                    && let Ok(mut file) = fs::OpenOptions::new().append(true).open(&changed_path)
                {
                    let _ignored = file.write_all(&[0]);
                }
            },
        )
        .err()
        .ok_or("mutated source unexpectedly published a stack")?;
        assert!(matches!(error, RegisteredStackError::Input(_)));
        assert!(!request.output().exists());
        Ok(())
    }

    #[test]
    fn rejects_stale_plan_binding_before_output() -> TestResult {
        let directory = TestDirectory::new()?;
        let mut request = stack_request(&directory)?;
        request.plan = RegisteredStackPlan::Affine(RegistrationPlan::new(
            id('a')?,
            12,
            10,
            vec![
                PlannedRegistrationFrame::new(id('a')?, 12, 10, AffineTransform::IDENTITY),
                PlannedRegistrationFrame::new(id('b')?, 12, 10, AffineTransform::IDENTITY),
            ],
        )?);

        assert!(matches!(
            run_registered_stack(
                &request,
                &CancellationToken::new(),
                &MemoryBudget::new(16 * 1_024 * 1_024)?,
                |_| {},
            ),
            Err(RegisteredStackError::ArtifactPlanMismatch { .. })
        ));
        assert!(!request.output().exists());
        Ok(())
    }
}
