use std::error::Error;
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};

use aether_fits::{FitsOutputProvenance, HeaderReadOptions, ValidationMode};
use aether_localnorm::{
    LOCAL_APPLICATION_ALGORITHM_ID, LocalNormalizationParameters, LocalNormalizationPlan, PlanError,
};

use crate::PipelineSource;

/// A source/reference local-normalization transaction sealed before FITS I/O.
#[derive(Clone, Debug)]
pub struct LocalNormalizationRequest {
    source: PipelineSource,
    reference: PipelineSource,
    output: PathBuf,
    provenance: FitsOutputProvenance,
    parameters: LocalNormalizationParameters,
    plan: LocalNormalizationPlan,
    header_options: HeaderReadOptions,
    validation_mode: ValidationMode,
}

impl LocalNormalizationRequest {
    /// Validates every identity that will be attached to the output product.
    ///
    /// The plan must bind the supplied source and reference byte fingerprints.
    /// FITS provenance must name the local-application algorithm, represent two
    /// inputs, and carry the exact canonical plan and parameter digests. The
    /// destination is reserved for a later create-new atomic publication and
    /// therefore never participates in scientific identity.
    ///
    /// # Errors
    ///
    /// Returns a typed mismatch before any source file is opened or output is
    /// created.
    pub fn new(
        source: PipelineSource,
        reference: PipelineSource,
        output: PathBuf,
        provenance: FitsOutputProvenance,
        parameters: LocalNormalizationParameters,
        plan: LocalNormalizationPlan,
    ) -> Result<Self, LocalNormalizationRequestError> {
        if source.fingerprint().sha256() != plan.source_sha256() {
            return Err(LocalNormalizationRequestError::SourcePlanMismatch);
        }
        if reference.fingerprint().sha256() != plan.reference_sha256() {
            return Err(LocalNormalizationRequestError::ReferencePlanMismatch);
        }
        let parameters_sha256 = parameters
            .canonical_sha256()
            .map_err(LocalNormalizationRequestError::Plan)?;
        if parameters_sha256 != plan.parameters_sha256() {
            return Err(LocalNormalizationRequestError::ParametersPlanMismatch);
        }
        if provenance.algorithm_id() != LOCAL_APPLICATION_ALGORITHM_ID {
            return Err(LocalNormalizationRequestError::ProvenanceAlgorithmMismatch);
        }
        if provenance.source_count() != 2 {
            return Err(LocalNormalizationRequestError::ProvenanceSourceCount {
                actual: provenance.source_count(),
            });
        }
        if provenance.plan_sha256() != Some(plan.plan_sha256()) {
            return Err(LocalNormalizationRequestError::ProvenancePlanMismatch);
        }
        if provenance.parameters_sha256() != Some(plan.parameters_sha256()) {
            return Err(LocalNormalizationRequestError::ProvenanceParametersMismatch);
        }
        Ok(Self {
            source,
            reference,
            output,
            provenance,
            parameters,
            plan,
            header_options: HeaderReadOptions::default(),
            validation_mode: ValidationMode::Strict,
        })
    }

    /// Replaces FITS header limits and diagnostic acceptance policy.
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

    /// Immutable image whose background and scale will be transformed.
    #[must_use]
    pub const fn source(&self) -> &PipelineSource {
        &self.source
    }

    /// Immutable registered image defining the target background and scale.
    #[must_use]
    pub const fn reference(&self) -> &PipelineSource {
        &self.reference
    }

    /// Atomic create-new destination.
    #[must_use]
    pub fn output(&self) -> &Path {
        &self.output
    }

    /// Provenance cards already cross-checked against the complete plan.
    #[must_use]
    pub const fn provenance(&self) -> &FitsOutputProvenance {
        &self.provenance
    }

    /// Exact scientific controls bound by the plan.
    #[must_use]
    pub const fn parameters(&self) -> LocalNormalizationParameters {
        self.parameters
    }

    /// Immutable identity of inputs, algorithms, and controls.
    #[must_use]
    pub const fn plan(&self) -> &LocalNormalizationPlan {
        &self.plan
    }

    /// FITS header resource limits selected for both inputs.
    #[must_use]
    pub const fn header_options(&self) -> HeaderReadOptions {
        self.header_options
    }

    /// Diagnostic acceptance policy selected for both inputs.
    #[must_use]
    pub const fn validation_mode(&self) -> ValidationMode {
        self.validation_mode
    }
}

/// Failure to construct a provenance-safe local-normalization transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalNormalizationRequestError {
    /// The image fingerprint differs from the image digest sealed in the plan.
    SourcePlanMismatch,
    /// The reference fingerprint differs from the reference digest in the plan.
    ReferencePlanMismatch,
    /// Re-encoding the supplied controls does not reproduce the plan digest.
    ParametersPlanMismatch,
    /// Provenance names a different scientific algorithm.
    ProvenanceAlgorithmMismatch,
    /// Local normalization must represent one source and one reference.
    ProvenanceSourceCount {
        /// Received source count.
        actual: u32,
    },
    /// Provenance does not carry the exact execution-plan digest.
    ProvenancePlanMismatch,
    /// Provenance does not carry the exact parameter digest.
    ProvenanceParametersMismatch,
    /// Canonical parameter encoding failed.
    Plan(PlanError),
}

impl Display for LocalNormalizationRequestError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SourcePlanMismatch => {
                formatter.write_str("local-normalization source does not match its sealed plan")
            }
            Self::ReferencePlanMismatch => {
                formatter.write_str("local-normalization reference does not match its sealed plan")
            }
            Self::ParametersPlanMismatch => formatter.write_str(
                "local-normalization parameters do not reproduce the sealed plan digest",
            ),
            Self::ProvenanceAlgorithmMismatch => formatter
                .write_str("local-normalization provenance names a different scientific algorithm"),
            Self::ProvenanceSourceCount { actual } => write!(
                formatter,
                "local-normalization provenance must represent two sources, received {actual}"
            ),
            Self::ProvenancePlanMismatch => formatter
                .write_str("local-normalization provenance does not carry the sealed plan digest"),
            Self::ProvenanceParametersMismatch => formatter.write_str(
                "local-normalization provenance does not carry the sealed parameter digest",
            ),
            Self::Plan(error) => write!(
                formatter,
                "cannot encode local-normalization parameters: {error}"
            ),
        }
    }
}

impl Error for LocalNormalizationRequestError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Plan(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use aether_fits::FitsProvenanceError;
    use aether_localnorm::{
        LocalFitParameters, ProtectionParameters, SamplingGridParameters, SurfaceParameters,
    };
    use aether_quality::{BackgroundParameters, StarMeasurementParameters};
    use aether_session::SourceFingerprint;

    use super::*;

    type TestResult = Result<(), Box<dyn Error>>;

    fn parameters(cell_width: usize) -> Result<LocalNormalizationParameters, Box<dyn Error>> {
        Ok(LocalNormalizationParameters::new(
            StarMeasurementParameters::new(
                BackgroundParameters::new(3.0, 8, 1_024)?,
                6.0,
                2.0,
                8,
                4,
                6,
                10_000,
                Some(65_000.0),
            )?,
            ProtectionParameters::new(1.5, 2.0, 2, 32, 10_000, 20_000_000)?,
            SamplingGridParameters::new(cell_width, 64, 256, 16_384)?,
            LocalFitParameters::new(32, 256, 32_640, 1.0e-6)?,
            SurfaceParameters::new(16, 4, 12, 256.0)?,
        ))
    }

    fn source(path: &str, digest: char) -> Result<PipelineSource, Box<dyn Error>> {
        Ok(PipelineSource::new(
            PathBuf::from(path),
            SourceFingerprint::new(1, digest.to_string().repeat(64))?,
        ))
    }

    fn provenance(
        plan: &LocalNormalizationPlan,
        algorithm: &str,
        source_count: u32,
    ) -> Result<FitsOutputProvenance, FitsProvenanceError> {
        FitsOutputProvenance::new("c".repeat(64), "light-l", algorithm, source_count)?
            .with_plan_sha256(plan.plan_sha256())?
            .with_parameters_sha256(plan.parameters_sha256())
    }

    #[test]
    fn accepts_only_a_fully_cross_checked_request() -> TestResult {
        let parameters = parameters(64)?;
        let plan = LocalNormalizationPlan::new("a".repeat(64), "b".repeat(64), parameters)?;
        let request = LocalNormalizationRequest::new(
            source("source.fits", 'a')?,
            source("reference.fits", 'b')?,
            PathBuf::from("normalized.fits"),
            provenance(&plan, LOCAL_APPLICATION_ALGORITHM_ID, 2)?,
            parameters,
            plan.clone(),
        )?;
        assert_eq!(request.plan(), &plan);
        assert_eq!(request.output(), Path::new("normalized.fits"));
        assert_eq!(request.parameters(), parameters);
        assert_eq!(request.provenance().plan_sha256(), Some(plan.plan_sha256()));
        assert_eq!(request.header_options(), HeaderReadOptions::default());
        assert_eq!(request.validation_mode(), ValidationMode::Strict);
        Ok(())
    }

    #[test]
    fn rejects_input_and_parameter_identity_drift() -> TestResult {
        let controls = parameters(64)?;
        let changed_parameters = parameters(65)?;
        let plan = LocalNormalizationPlan::new("a".repeat(64), "b".repeat(64), controls)?;
        let bound = provenance(&plan, LOCAL_APPLICATION_ALGORITHM_ID, 2)?;

        let wrong_source = LocalNormalizationRequest::new(
            source("source.fits", 'd')?,
            source("reference.fits", 'b')?,
            PathBuf::from("normalized.fits"),
            bound.clone(),
            controls,
            plan.clone(),
        );
        assert_eq!(
            wrong_source.err(),
            Some(LocalNormalizationRequestError::SourcePlanMismatch)
        );

        let wrong_reference = LocalNormalizationRequest::new(
            source("source.fits", 'a')?,
            source("reference.fits", 'd')?,
            PathBuf::from("normalized.fits"),
            bound.clone(),
            controls,
            plan.clone(),
        );
        assert_eq!(
            wrong_reference.err(),
            Some(LocalNormalizationRequestError::ReferencePlanMismatch)
        );

        let wrong_parameters = LocalNormalizationRequest::new(
            source("source.fits", 'a')?,
            source("reference.fits", 'b')?,
            PathBuf::from("normalized.fits"),
            bound,
            changed_parameters,
            plan,
        );
        assert_eq!(
            wrong_parameters.err(),
            Some(LocalNormalizationRequestError::ParametersPlanMismatch)
        );
        Ok(())
    }

    #[test]
    fn rejects_incomplete_or_foreign_fits_provenance() -> TestResult {
        let parameters = parameters(64)?;
        let plan = LocalNormalizationPlan::new("a".repeat(64), "b".repeat(64), parameters)?;
        let image_source = source("source.fits", 'a')?;
        let image_reference = source("reference.fits", 'b')?;
        let make_request = |provenance| {
            LocalNormalizationRequest::new(
                image_source.clone(),
                image_reference.clone(),
                PathBuf::from("normalized.fits"),
                provenance,
                parameters,
                plan.clone(),
            )
        };

        assert_eq!(
            make_request(provenance(&plan, "foreign-v1", 2)?).err(),
            Some(LocalNormalizationRequestError::ProvenanceAlgorithmMismatch)
        );
        assert_eq!(
            make_request(provenance(&plan, LOCAL_APPLICATION_ALGORITHM_ID, 1)?).err(),
            Some(LocalNormalizationRequestError::ProvenanceSourceCount { actual: 1 })
        );
        let no_plan = FitsOutputProvenance::new(
            "c".repeat(64),
            "light-l",
            LOCAL_APPLICATION_ALGORITHM_ID,
            2,
        )?
        .with_parameters_sha256(plan.parameters_sha256())?;
        assert_eq!(
            make_request(no_plan).err(),
            Some(LocalNormalizationRequestError::ProvenancePlanMismatch)
        );
        let no_parameters = FitsOutputProvenance::new(
            "c".repeat(64),
            "light-l",
            LOCAL_APPLICATION_ALGORITHM_ID,
            2,
        )?
        .with_plan_sha256(plan.plan_sha256())?;
        assert_eq!(
            make_request(no_parameters).err(),
            Some(LocalNormalizationRequestError::ProvenanceParametersMismatch)
        );
        Ok(())
    }
}
