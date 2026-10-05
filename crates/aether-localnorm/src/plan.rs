use std::error::Error;
use std::fmt::{Display, Formatter};

use sha2::{Digest, Sha256};

use aether_quality::{
    GLOBAL_BACKGROUND_ALGORITHM_ID, STAR_MEASUREMENT_ALGORITHM_ID, StarMeasurementParameters,
};

use crate::{
    CELL_SAMPLING_ALGORITHM_ID, LOCAL_AFFINE_FIT_ALGORITHM_ID, LOCAL_APPLICATION_ALGORITHM_ID,
    LOCAL_SURFACE_ALGORITHM_ID, LocalFitParameters, PROTECTED_SOURCE_MASK_ALGORITHM_ID,
    ProtectionParameters, SamplingGridParameters, SurfaceParameters,
};

const PLAN_SCHEMA_ID: &str = "aether-local-normalization-plan-v2";
const PARAMETER_SCHEMA_ID: &str = "aether-local-normalization-parameters-v2";

/// Complete immutable controls for one local-normalization plan.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocalNormalizationParameters {
    detection: StarMeasurementParameters,
    protection: ProtectionParameters,
    sampling: SamplingGridParameters,
    fitting: LocalFitParameters,
    surface: SurfaceParameters,
}

impl LocalNormalizationParameters {
    /// Combines independently validated stage controls.
    #[must_use]
    pub const fn new(
        detection: StarMeasurementParameters,
        protection: ProtectionParameters,
        sampling: SamplingGridParameters,
        fitting: LocalFitParameters,
        surface: SurfaceParameters,
    ) -> Self {
        Self {
            detection,
            protection,
            sampling,
            fitting,
            surface,
        }
    }

    /// Robust background and stellar measurement controls.
    #[must_use]
    pub const fn detection(self) -> StarMeasurementParameters {
        self.detection
    }

    /// Stellar protection controls.
    #[must_use]
    pub const fn protection(self) -> ProtectionParameters {
        self.protection
    }

    /// Spatial sampling controls.
    #[must_use]
    pub const fn sampling(self) -> SamplingGridParameters {
        self.sampling
    }

    /// Robust per-cell fit controls.
    #[must_use]
    pub const fn fitting(self) -> LocalFitParameters {
        self.fitting
    }

    /// Guarded surface controls.
    #[must_use]
    pub const fn surface(self) -> SurfaceParameters {
        self.surface
    }

    /// SHA-256 of the canonical algorithm and control encoding.
    pub fn canonical_sha256(self) -> Result<String, PlanError> {
        let mut hasher = Sha256::new();
        update_string(&mut hasher, PARAMETER_SCHEMA_ID)?;
        for algorithm in [
            GLOBAL_BACKGROUND_ALGORITHM_ID,
            STAR_MEASUREMENT_ALGORITHM_ID,
            PROTECTED_SOURCE_MASK_ALGORITHM_ID,
            CELL_SAMPLING_ALGORITHM_ID,
            LOCAL_AFFINE_FIT_ALGORITHM_ID,
            LOCAL_SURFACE_ALGORITHM_ID,
            LOCAL_APPLICATION_ALGORITHM_ID,
        ] {
            update_string(&mut hasher, algorithm)?;
        }
        let background = self.detection.background();
        update_f64(&mut hasher, background.clipping_sigma());
        hasher.update(background.maximum_iterations().to_be_bytes());
        update_usize(&mut hasher, background.minimum_samples())?;
        update_f64(&mut hasher, self.detection.detection_sigma());
        update_f64(&mut hasher, self.detection.measurement_floor_sigma());
        update_usize(&mut hasher, self.detection.measurement_radius())?;
        update_usize(&mut hasher, self.detection.minimum_separation())?;
        update_usize(&mut hasher, self.detection.minimum_measurement_pixels())?;
        update_usize(&mut hasher, self.detection.maximum_candidates())?;
        match self.detection.saturation_level() {
            Some(value) => {
                hasher.update([1]);
                update_f64(&mut hasher, value);
            }
            None => hasher.update([0]),
        }
        update_f64(&mut hasher, self.protection.growth_factor());
        update_f64(&mut hasher, self.protection.saturated_growth_factor());
        update_usize(&mut hasher, self.protection.minimum_radius())?;
        update_usize(&mut hasher, self.protection.maximum_radius())?;
        update_usize(&mut hasher, self.protection.maximum_sources())?;
        update_usize(&mut hasher, self.protection.maximum_pixel_visits())?;
        update_usize(&mut hasher, self.sampling.cell_width())?;
        update_usize(&mut hasher, self.sampling.cell_height())?;
        update_usize(&mut hasher, self.sampling.maximum_samples_per_cell())?;
        update_usize(&mut hasher, self.sampling.maximum_cells())?;
        update_usize(&mut hasher, self.fitting.minimum_samples())?;
        update_usize(&mut hasher, self.fitting.maximum_samples())?;
        update_usize(&mut hasher, self.fitting.maximum_pairwise_slopes())?;
        update_f64(&mut hasher, self.fitting.minimum_absolute_scale());
        update_usize(&mut hasher, self.surface.minimum_control_points())?;
        update_usize(&mut hasher, self.surface.minimum_neighbors())?;
        update_usize(&mut hasher, self.surface.maximum_neighbors())?;
        update_f64(&mut hasher, self.surface.maximum_distance());
        Ok(lower_hex(hasher.finalize().as_slice()))
    }
}

/// Source-bound immutable identity of one local-normalization execution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalNormalizationPlan {
    source_sha256: String,
    reference_sha256: String,
    parameters_sha256: String,
    plan_sha256: String,
}

impl LocalNormalizationPlan {
    /// Binds exact source/reference bytes to the canonical scientific controls.
    pub fn new(
        source_sha256: impl Into<String>,
        reference_sha256: impl Into<String>,
        parameters: LocalNormalizationParameters,
    ) -> Result<Self, PlanError> {
        let source_sha256 = source_sha256.into();
        let reference_sha256 = reference_sha256.into();
        if !is_lower_sha256(&source_sha256) || !is_lower_sha256(&reference_sha256) {
            return Err(PlanError::InvalidSourceSha256);
        }
        let parameters_sha256 = parameters.canonical_sha256()?;
        let mut hasher = Sha256::new();
        update_string(&mut hasher, PLAN_SCHEMA_ID)?;
        update_string(&mut hasher, &source_sha256)?;
        update_string(&mut hasher, &reference_sha256)?;
        update_string(&mut hasher, &parameters_sha256)?;
        let plan_sha256 = lower_hex(hasher.finalize().as_slice());
        Ok(Self {
            source_sha256,
            reference_sha256,
            parameters_sha256,
            plan_sha256,
        })
    }

    /// Exact source-image byte identity.
    #[must_use]
    pub fn source_sha256(&self) -> &str {
        &self.source_sha256
    }

    /// Exact reference-image byte identity.
    #[must_use]
    pub fn reference_sha256(&self) -> &str {
        &self.reference_sha256
    }

    /// Canonical digest of every scientific control and algorithm identity.
    #[must_use]
    pub fn parameters_sha256(&self) -> &str {
        &self.parameters_sha256
    }

    /// Canonical digest binding both inputs and all parameters.
    #[must_use]
    pub fn plan_sha256(&self) -> &str {
        &self.plan_sha256
    }
}

/// Failure to encode or validate a canonical local-normalization plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanError {
    /// A source identity is not exactly 64 lowercase hexadecimal digits.
    InvalidSourceSha256,
    /// A platform-sized control cannot be represented canonically as `u64`.
    IntegerOutOfRange,
}

impl Display for PlanError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidSourceSha256 => "local-normalization source SHA-256 is invalid",
            Self::IntegerOutOfRange => "local-normalization control exceeds canonical u64 range",
        })
    }
}

impl Error for PlanError {}

fn update_string(hasher: &mut Sha256, value: &str) -> Result<(), PlanError> {
    update_usize(hasher, value.len())?;
    hasher.update(value.as_bytes());
    Ok(())
}

fn update_usize(hasher: &mut Sha256, value: usize) -> Result<(), PlanError> {
    let value = u64::try_from(value).map_err(|_| PlanError::IntegerOutOfRange)?;
    hasher.update(value.to_be_bytes());
    Ok(())
}

fn update_f64(hasher: &mut Sha256, value: f64) {
    hasher.update(value.to_bits().to_be_bytes());
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_quality::BackgroundParameters;

    type TestResult = Result<(), Box<dyn Error>>;

    fn parameters(cell_width: usize) -> Result<LocalNormalizationParameters, Box<dyn Error>> {
        parameters_with_detection(cell_width, 6.0)
    }

    fn parameters_with_detection(
        cell_width: usize,
        detection_sigma: f64,
    ) -> Result<LocalNormalizationParameters, Box<dyn Error>> {
        Ok(LocalNormalizationParameters::new(
            StarMeasurementParameters::new(
                BackgroundParameters::new(3.0, 8, 1_024)?,
                detection_sigma,
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

    #[test]
    fn canonical_digests_bind_sources_algorithms_and_every_control() -> TestResult {
        let controls = parameters(64)?;
        let parameter_digest = controls.canonical_sha256()?;
        assert_eq!(parameter_digest.len(), 64);
        let plan = LocalNormalizationPlan::new("a".repeat(64), "b".repeat(64), controls)?;
        assert_eq!(plan.source_sha256(), "a".repeat(64));
        assert_eq!(plan.reference_sha256(), "b".repeat(64));
        assert_eq!(plan.parameters_sha256(), parameter_digest);
        assert_eq!(plan.plan_sha256().len(), 64);

        let changed_control =
            LocalNormalizationPlan::new("a".repeat(64), "b".repeat(64), parameters(65)?)?;
        let changed_detection = LocalNormalizationPlan::new(
            "a".repeat(64),
            "b".repeat(64),
            parameters_with_detection(64, 7.0)?,
        )?;
        let changed_source = LocalNormalizationPlan::new("c".repeat(64), "b".repeat(64), controls)?;
        assert_ne!(
            changed_control.parameters_sha256(),
            plan.parameters_sha256()
        );
        assert_ne!(changed_control.plan_sha256(), plan.plan_sha256());
        assert_ne!(
            changed_detection.parameters_sha256(),
            plan.parameters_sha256()
        );
        assert_ne!(changed_detection.plan_sha256(), plan.plan_sha256());
        assert_ne!(changed_source.plan_sha256(), plan.plan_sha256());
        Ok(())
    }

    #[test]
    fn canonical_encoding_is_locked_and_rejects_noncanonical_identities() -> TestResult {
        let controls = parameters(64)?;
        assert_eq!(
            controls.canonical_sha256()?,
            "af94ecf0f963f4ff9ad763fd289cded91160b51b42955b4cdb63972c86696b46"
        );
        let plan = LocalNormalizationPlan::new("a".repeat(64), "b".repeat(64), controls)?;
        assert_eq!(
            plan.plan_sha256(),
            "7e53e79c21f7aea0353853bb70d7a6a0a54b676388ce9ef5087e956d44942d66"
        );
        assert_eq!(
            LocalNormalizationPlan::new("A".repeat(64), "b".repeat(64), controls),
            Err(PlanError::InvalidSourceSha256)
        );
        Ok(())
    }
}
