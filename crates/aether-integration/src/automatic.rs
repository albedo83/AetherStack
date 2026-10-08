//! Deterministic planning for automatic registered-stack integration.
//!
//! Automatic mode is deliberately a planner, not a ninth estimator. It turns
//! the sealed source population into an explicit estimator and explicit
//! parameters before any pixel is read. Execution can therefore use the same
//! reviewed algorithms as manual mode, and a report can explain every choice.

use std::error::Error;
use std::fmt::{Display, Formatter};

use sha2::{Digest, Sha256};

/// Stable identifier for the first automatic-integration planning contract.
pub const AUTOMATIC_INTEGRATION_PLAN_ALGORITHM_ID: &str = "automatic-integration-plan-v1";

/// Smallest population for which iterative Winsorized statistics are useful.
pub const WINSORIZED_POPULATION_THRESHOLD: u32 = 8;

/// Smallest population admitted by AetherStack's generalized ESD estimator.
pub const ESD_POPULATION_THRESHOLD: u32 = 15;

/// Explicit estimator selected by the automatic planner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AutomaticEstimator {
    /// Arithmetic mean without sample rejection.
    StrictMean,
    /// Exact finite-sample median.
    Median,
    /// Iterative sigma clipping with Winsorized population statistics.
    WinsorizedSigmaClipped,
    /// Two-sided generalized ESD outlier rejection.
    GeneralizedEsd,
}

impl AutomaticEstimator {
    /// Stable snake-case value used at API and report boundaries.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StrictMean => "strict_mean",
            Self::Median => "median",
            Self::WinsorizedSigmaClipped => "winsorized_sigma_clipped",
            Self::GeneralizedEsd => "generalized_esd",
        }
    }
}

/// Population tier that caused the planner's estimator choice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AutomaticPopulationTier {
    /// One or two frames cannot support reliable outlier classification.
    Minimal,
    /// A small stack benefits from the median's exact breakdown behaviour.
    Small,
    /// A medium stack can support iterative Winsorized sigma statistics.
    Medium,
    /// A large stack can support generalized ESD's finite-sample test.
    Large,
}

impl AutomaticPopulationTier {
    /// Stable snake-case value used in provenance records.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
        }
    }
}

/// Fully explicit recommendation emitted before integration starts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutomaticIntegrationPlan {
    source_count: u32,
    tier: AutomaticPopulationTier,
    estimator: AutomaticEstimator,
    low_sigma: f64,
    high_sigma: f64,
    esd_outlier_fraction: f64,
    esd_significance: f64,
    maximum_iterations: u32,
    minimum_retained_samples: u32,
    generate_rejection_maps: bool,
    generate_support_map: bool,
}

impl AutomaticIntegrationPlan {
    /// Resolves a source population into an explicit, deterministic plan.
    ///
    /// # Errors
    ///
    /// Returns [`AutomaticIntegrationPlanError`] for an empty population.
    pub fn resolve(source_count: u32) -> Result<Self, AutomaticIntegrationPlanError> {
        if source_count == 0 {
            return Err(AutomaticIntegrationPlanError::EmptyPopulation);
        }

        let (tier, estimator) = if source_count < 3 {
            (
                AutomaticPopulationTier::Minimal,
                AutomaticEstimator::StrictMean,
            )
        } else if source_count < WINSORIZED_POPULATION_THRESHOLD {
            (AutomaticPopulationTier::Small, AutomaticEstimator::Median)
        } else if source_count < ESD_POPULATION_THRESHOLD {
            (
                AutomaticPopulationTier::Medium,
                AutomaticEstimator::WinsorizedSigmaClipped,
            )
        } else {
            (
                AutomaticPopulationTier::Large,
                AutomaticEstimator::GeneralizedEsd,
            )
        };

        Ok(Self {
            source_count,
            tier,
            estimator,
            low_sigma: 4.0,
            high_sigma: 3.0,
            esd_outlier_fraction: 0.30,
            esd_significance: 0.05,
            maximum_iterations: 8,
            minimum_retained_samples: 3.min(source_count),
            generate_rejection_maps: !matches!(estimator, AutomaticEstimator::StrictMean),
            generate_support_map: true,
        })
    }

    /// Number of sealed source frames used to make the decision.
    #[must_use]
    pub const fn source_count(self) -> u32 {
        self.source_count
    }

    /// Population tier selected by the versioned policy.
    #[must_use]
    pub const fn tier(self) -> AutomaticPopulationTier {
        self.tier
    }

    /// Explicit estimator that must be executed.
    #[must_use]
    pub const fn estimator(self) -> AutomaticEstimator {
        self.estimator
    }

    /// Lower-tail sigma threshold.
    #[must_use]
    pub const fn low_sigma(self) -> f64 {
        self.low_sigma
    }

    /// Upper-tail sigma threshold.
    #[must_use]
    pub const fn high_sigma(self) -> f64 {
        self.high_sigma
    }

    /// Maximum population fraction considered by generalized ESD.
    #[must_use]
    pub const fn esd_outlier_fraction(self) -> f64 {
        self.esd_outlier_fraction
    }

    /// Generalized ESD significance level.
    #[must_use]
    pub const fn esd_significance(self) -> f64 {
        self.esd_significance
    }

    /// Maximum number of iterative clipping passes.
    #[must_use]
    pub const fn maximum_iterations(self) -> u32 {
        self.maximum_iterations
    }

    /// Minimum number of source samples retained at each output sample.
    #[must_use]
    pub const fn minimum_retained_samples(self) -> u32 {
        self.minimum_retained_samples
    }

    /// Whether low/high rejection maps should be persisted.
    #[must_use]
    pub const fn generate_rejection_maps(self) -> bool {
        self.generate_rejection_maps
    }

    /// Whether the accepted-sample support map should be persisted.
    #[must_use]
    pub const fn generate_support_map(self) -> bool {
        self.generate_support_map
    }

    /// Human-readable reason suitable for an inspection surface and report.
    #[must_use]
    pub const fn rationale(self) -> &'static str {
        match self.tier {
            AutomaticPopulationTier::Minimal => {
                "Fewer than 3 frames: use an exact mean without unreliable rejection."
            }
            AutomaticPopulationTier::Small => {
                "3–7 frames: use the exact median for finite-sample robustness."
            }
            AutomaticPopulationTier::Medium => {
                "8–14 frames: use Winsorized sigma clipping for stable robust statistics."
            }
            AutomaticPopulationTier::Large => {
                "15 or more frames: use generalized ESD for finite-sample outlier testing."
            }
        }
    }

    /// Returns the SHA-256 seal of every execution-relevant plan field.
    ///
    /// Integers use big-endian bytes, floating-point values use canonical IEEE
    /// bit patterns, and stable textual identifiers are length-prefixed. This
    /// avoids locale, JSON-number, and map-order dependencies at API borders.
    #[must_use]
    pub fn plan_sha256(self) -> String {
        let mut hasher = Sha256::new();
        update_length_prefixed(
            &mut hasher,
            AUTOMATIC_INTEGRATION_PLAN_ALGORITHM_ID.as_bytes(),
        );
        hasher.update(self.source_count.to_be_bytes());
        update_length_prefixed(&mut hasher, self.tier.as_str().as_bytes());
        update_length_prefixed(&mut hasher, self.estimator.as_str().as_bytes());
        hasher.update(self.low_sigma.to_bits().to_be_bytes());
        hasher.update(self.high_sigma.to_bits().to_be_bytes());
        hasher.update(self.esd_outlier_fraction.to_bits().to_be_bytes());
        hasher.update(self.esd_significance.to_bits().to_be_bytes());
        hasher.update(self.maximum_iterations.to_be_bytes());
        hasher.update(self.minimum_retained_samples.to_be_bytes());
        hasher.update([u8::from(self.generate_rejection_maps)]);
        hasher.update([u8::from(self.generate_support_map)]);
        hex_digest(hasher.finalize())
    }
}

fn update_length_prefixed(hasher: &mut Sha256, value: &[u8]) {
    // Stable identifiers are compile-time ASCII constants and never approach
    // the u32 boundary; the fixed-width cast keeps the byte contract portable.
    #[allow(clippy::cast_possible_truncation)]
    let length = value.len() as u32;
    hasher.update(length.to_be_bytes());
    hasher.update(value);
}

fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let bytes = digest.as_ref();
    let mut hexadecimal = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        hexadecimal.push(char::from(HEX[usize::from(byte >> 4)]));
        hexadecimal.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    hexadecimal
}

/// Invalid input to automatic integration planning.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AutomaticIntegrationPlanError {
    /// At least one registered source is required.
    EmptyPopulation,
}

impl Display for AutomaticIntegrationPlanError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("automatic integration requires at least one source frame")
    }
}

impl Error for AutomaticIntegrationPlanError {}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn Error>>;

    #[test]
    fn rejects_an_empty_population() {
        assert_eq!(
            AutomaticIntegrationPlan::resolve(0),
            Err(AutomaticIntegrationPlanError::EmptyPopulation)
        );
    }

    #[test]
    fn locks_every_population_boundary() -> TestResult {
        let cases = [
            (
                1,
                AutomaticPopulationTier::Minimal,
                AutomaticEstimator::StrictMean,
            ),
            (
                2,
                AutomaticPopulationTier::Minimal,
                AutomaticEstimator::StrictMean,
            ),
            (
                3,
                AutomaticPopulationTier::Small,
                AutomaticEstimator::Median,
            ),
            (
                7,
                AutomaticPopulationTier::Small,
                AutomaticEstimator::Median,
            ),
            (
                8,
                AutomaticPopulationTier::Medium,
                AutomaticEstimator::WinsorizedSigmaClipped,
            ),
            (
                14,
                AutomaticPopulationTier::Medium,
                AutomaticEstimator::WinsorizedSigmaClipped,
            ),
            (
                15,
                AutomaticPopulationTier::Large,
                AutomaticEstimator::GeneralizedEsd,
            ),
            (
                u32::MAX,
                AutomaticPopulationTier::Large,
                AutomaticEstimator::GeneralizedEsd,
            ),
        ];

        for (source_count, expected_tier, expected_estimator) in cases {
            let plan = AutomaticIntegrationPlan::resolve(source_count)?;
            assert_eq!(plan.source_count(), source_count);
            assert_eq!(plan.tier(), expected_tier);
            assert_eq!(plan.estimator(), expected_estimator);
            assert_eq!(plan.minimum_retained_samples(), 3.min(source_count));
            assert!(plan.generate_support_map());
            assert_eq!(
                plan.generate_rejection_maps(),
                expected_estimator != AutomaticEstimator::StrictMean
            );
        }
        Ok(())
    }

    #[test]
    fn publishes_explicit_stable_values_and_rationale() -> TestResult {
        let plan = AutomaticIntegrationPlan::resolve(15)?;

        assert_eq!(
            AUTOMATIC_INTEGRATION_PLAN_ALGORITHM_ID,
            "automatic-integration-plan-v1"
        );
        assert_eq!(plan.tier().as_str(), "large");
        assert_eq!(plan.estimator().as_str(), "generalized_esd");
        assert_eq!(plan.low_sigma().to_bits(), 4.0_f64.to_bits());
        assert_eq!(plan.high_sigma().to_bits(), 3.0_f64.to_bits());
        assert_eq!(plan.esd_outlier_fraction().to_bits(), 0.30_f64.to_bits());
        assert_eq!(plan.esd_significance().to_bits(), 0.05_f64.to_bits());
        assert_eq!(plan.maximum_iterations(), 8);
        assert!(plan.rationale().contains("15 or more frames"));
        Ok(())
    }

    #[test]
    fn repeated_resolution_is_bit_identical() -> TestResult {
        for source_count in 1..=64 {
            let first = AutomaticIntegrationPlan::resolve(source_count)?;
            let second = AutomaticIntegrationPlan::resolve(source_count)?;
            assert_eq!(first, second);
        }
        Ok(())
    }

    #[test]
    fn plan_seal_is_stable_lowercase_sha256() -> TestResult {
        let plan = AutomaticIntegrationPlan::resolve(15)?;
        let first = plan.plan_sha256();
        let second = plan.plan_sha256();

        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(first, first.to_ascii_lowercase());
        Ok(())
    }

    #[test]
    fn plan_seal_binds_every_execution_field() -> TestResult {
        let baseline = AutomaticIntegrationPlan::resolve(15)?;
        let baseline_sha256 = baseline.plan_sha256();
        let variants = [
            AutomaticIntegrationPlan {
                source_count: 16,
                ..baseline
            },
            AutomaticIntegrationPlan {
                tier: AutomaticPopulationTier::Medium,
                ..baseline
            },
            AutomaticIntegrationPlan {
                estimator: AutomaticEstimator::Median,
                ..baseline
            },
            AutomaticIntegrationPlan {
                low_sigma: 4.1,
                ..baseline
            },
            AutomaticIntegrationPlan {
                high_sigma: 3.1,
                ..baseline
            },
            AutomaticIntegrationPlan {
                esd_outlier_fraction: 0.2,
                ..baseline
            },
            AutomaticIntegrationPlan {
                esd_significance: 0.01,
                ..baseline
            },
            AutomaticIntegrationPlan {
                maximum_iterations: 9,
                ..baseline
            },
            AutomaticIntegrationPlan {
                minimum_retained_samples: 4,
                ..baseline
            },
            AutomaticIntegrationPlan {
                generate_rejection_maps: false,
                ..baseline
            },
            AutomaticIntegrationPlan {
                generate_support_map: false,
                ..baseline
            },
        ];

        for variant in variants {
            assert_ne!(variant.plan_sha256(), baseline_sha256);
        }
        Ok(())
    }
}
