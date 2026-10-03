use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::{Display, Formatter};

use serde::Serialize;

use crate::FrameMetrics;

/// Versioned identity of the first automatic frame-selection rule evaluator.
pub const FRAME_SELECTION_ALGORITHM_ID: &str = "frame-selection-rules-v1";
/// Maximum number of independent metric rules in one selection plan.
pub const MAX_FRAME_SELECTION_RULES: usize = 32;

/// Explicit treatment of a frame whose requested metric is unavailable.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingMetricPolicy {
    /// Keep the frame eligible and retain visible missing evidence.
    Retain,
    /// Propose rejection because the configured rule cannot be proven.
    Reject,
}

/// Quality metric addressed by one automatic rule.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameSelectionMetric {
    /// Robust background location in decoded data units.
    Background,
    /// Robust background-noise estimate in decoded data units.
    Noise,
    /// Median background-referenced stellar signal-to-noise ratio.
    SignalToNoise,
    /// Count of detected local maxima before measurement filtering.
    DetectedStars,
    /// Count of usable unsaturated stellar measurements.
    UsableStars,
    /// Median stellar major-axis FWHM in source pixels.
    FwhmMajorPixels,
    /// Median stellar eccentricity in `[0, 1]`.
    Eccentricity,
}

/// Comparator retained in rule evidence and future provenance.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameSelectionComparator {
    /// The measured value must be strictly below the threshold.
    LessThan,
    /// The measured value must be strictly above the threshold.
    GreaterThan,
}

/// Typed measured or threshold value, avoiding lossy star-count conversion.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum FrameSelectionValue {
    /// Finite floating-point metric.
    Scalar(f64),
    /// Exact non-negative count.
    Count(u64),
}

/// One validated metric threshold.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct FrameSelectionRule {
    metric: FrameSelectionMetric,
    comparator: FrameSelectionComparator,
    threshold: FrameSelectionValue,
    missing_policy: MissingMetricPolicy,
}

impl FrameSelectionRule {
    /// Creates a strict floating-point comparison for a scalar metric.
    ///
    /// # Errors
    ///
    /// Returns [`FrameSelectionError::MetricTypeMismatch`] for a count metric,
    /// or [`FrameSelectionError::InvalidThreshold`] when the value is non-finite
    /// or outside the metric's physical domain.
    pub fn scalar(
        metric: FrameSelectionMetric,
        comparator: FrameSelectionComparator,
        threshold: f64,
        missing_policy: MissingMetricPolicy,
    ) -> Result<Self, FrameSelectionError> {
        if matches!(
            metric,
            FrameSelectionMetric::DetectedStars | FrameSelectionMetric::UsableStars
        ) {
            return Err(FrameSelectionError::MetricTypeMismatch { metric });
        }
        if !valid_scalar_threshold(metric, threshold) {
            return Err(FrameSelectionError::InvalidThreshold { metric });
        }
        Ok(Self {
            metric,
            comparator,
            threshold: FrameSelectionValue::Scalar(threshold),
            missing_policy,
        })
    }

    /// Creates a strict integer comparison for a star-count metric.
    ///
    /// # Errors
    ///
    /// Returns [`FrameSelectionError::MetricTypeMismatch`] for a scalar metric.
    pub const fn count(
        metric: FrameSelectionMetric,
        comparator: FrameSelectionComparator,
        threshold: u64,
        missing_policy: MissingMetricPolicy,
    ) -> Result<Self, FrameSelectionError> {
        if !matches!(
            metric,
            FrameSelectionMetric::DetectedStars | FrameSelectionMetric::UsableStars
        ) {
            return Err(FrameSelectionError::MetricTypeMismatch { metric });
        }
        Ok(Self {
            metric,
            comparator,
            threshold: FrameSelectionValue::Count(threshold),
            missing_policy,
        })
    }

    /// Addressed metric.
    #[must_use]
    pub const fn metric(self) -> FrameSelectionMetric {
        self.metric
    }

    /// Strict comparison direction.
    #[must_use]
    pub const fn comparator(self) -> FrameSelectionComparator {
        self.comparator
    }

    /// Validated threshold with its exact numeric kind.
    #[must_use]
    pub const fn threshold(self) -> FrameSelectionValue {
        self.threshold
    }

    /// Explicit behavior when the addressed metric is absent.
    #[must_use]
    pub const fn missing_policy(self) -> MissingMetricPolicy {
        self.missing_policy
    }
}

/// Result of applying one rule to one frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameSelectionRuleState {
    /// A measured value satisfied the strict comparison.
    Passed,
    /// A measured value failed the strict comparison.
    Failed,
    /// The metric was absent and the configured policy retained the frame.
    MissingRetained,
    /// The metric was absent and the configured policy rejected the frame.
    MissingRejected,
}

/// Complete evidence for one evaluated rule.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct FrameSelectionRuleEvidence {
    rule: FrameSelectionRule,
    measured: Option<FrameSelectionValue>,
    state: FrameSelectionRuleState,
}

impl FrameSelectionRuleEvidence {
    /// Rule that produced this evidence.
    #[must_use]
    pub const fn rule(self) -> FrameSelectionRule {
        self.rule
    }

    /// Measured value, or `None` when the metric was missing.
    #[must_use]
    pub const fn measured(self) -> Option<FrameSelectionValue> {
        self.measured
    }

    /// Explicit comparison or missing-value result.
    #[must_use]
    pub const fn state(self) -> FrameSelectionRuleState {
        self.state
    }
}

/// Automatic proposal kept separate from explicit manual review decisions.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameSelectionProposal {
    /// Every rule passed or explicitly retained missing evidence.
    Retain,
    /// At least one rule failed or rejected missing evidence.
    Reject,
}

/// Full non-short-circuiting evaluation for one frame.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FrameSelectionEvaluation {
    algorithm_id: &'static str,
    proposal: FrameSelectionProposal,
    evidence: Vec<FrameSelectionRuleEvidence>,
}

impl FrameSelectionEvaluation {
    /// Evaluates every rule and preserves all evidence in caller order.
    ///
    /// This function never mutates a [`crate::ReviewBook`]. Manual decisions
    /// remain an independent layer until an explicit, previewed bulk action is
    /// requested by a higher-level client.
    ///
    /// # Errors
    ///
    /// Returns a typed error for an empty or oversized rule set, duplicate
    /// metrics, or a platform count that cannot be represented as `u64`.
    pub fn evaluate(
        metrics: FrameMetrics,
        rules: &[FrameSelectionRule],
    ) -> Result<Self, FrameSelectionError> {
        validate_rules(rules)?;
        let mut evidence = Vec::new();
        evidence.try_reserve_exact(rules.len()).map_err(|_| {
            FrameSelectionError::AllocationFailed {
                elements: rules.len(),
            }
        })?;
        let mut proposal = FrameSelectionProposal::Retain;
        for rule in rules {
            let measured = measured_value(metrics, rule.metric)?;
            let state = match measured {
                Some(value) if comparison_passes(value, rule.comparator, rule.threshold) => {
                    FrameSelectionRuleState::Passed
                }
                Some(_) => FrameSelectionRuleState::Failed,
                None if rule.missing_policy == MissingMetricPolicy::Retain => {
                    FrameSelectionRuleState::MissingRetained
                }
                None => FrameSelectionRuleState::MissingRejected,
            };
            if matches!(
                state,
                FrameSelectionRuleState::Failed | FrameSelectionRuleState::MissingRejected
            ) {
                proposal = FrameSelectionProposal::Reject;
            }
            evidence.push(FrameSelectionRuleEvidence {
                rule: *rule,
                measured,
                state,
            });
        }
        Ok(Self {
            algorithm_id: FRAME_SELECTION_ALGORITHM_ID,
            proposal,
            evidence,
        })
    }

    /// Versioned evaluator identity.
    #[must_use]
    pub const fn algorithm_id(&self) -> &str {
        self.algorithm_id
    }

    /// Proposed automatic state, independent of manual review state.
    #[must_use]
    pub const fn proposal(&self) -> FrameSelectionProposal {
        self.proposal
    }

    /// Complete evidence in configured rule order.
    #[must_use]
    pub fn evidence(&self) -> &[FrameSelectionRuleEvidence] {
        &self.evidence
    }
}

/// Invalid automatic frame-selection configuration or evaluation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FrameSelectionError {
    /// At least one rule is required.
    EmptyRules,
    /// The rule count exceeds the public bound.
    TooManyRules {
        /// Rejected count.
        count: usize,
        /// Maximum accepted count.
        maximum: usize,
    },
    /// One metric appeared more than once.
    DuplicateMetric {
        /// Repeated metric.
        metric: FrameSelectionMetric,
    },
    /// Threshold kind does not match the addressed metric.
    MetricTypeMismatch {
        /// Metric that rejected the threshold kind.
        metric: FrameSelectionMetric,
    },
    /// Scalar threshold is non-finite or outside the metric domain.
    InvalidThreshold {
        /// Metric whose threshold was rejected.
        metric: FrameSelectionMetric,
    },
    /// A platform count cannot be represented in portable evidence.
    CountOverflow,
    /// Evidence allocation failed.
    AllocationFailed {
        /// Requested element count.
        elements: usize,
    },
}

impl Display for FrameSelectionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyRules => formatter.write_str("frame selection requires at least one rule"),
            Self::TooManyRules { count, maximum } => write!(
                formatter,
                "frame selection has {count} rules but accepts at most {maximum}"
            ),
            Self::DuplicateMetric { metric } => {
                write!(formatter, "frame selection repeats metric {metric:?}")
            }
            Self::MetricTypeMismatch { metric } => {
                write!(
                    formatter,
                    "frame selection threshold type does not match {metric:?}"
                )
            }
            Self::InvalidThreshold { metric } => {
                write!(
                    formatter,
                    "frame selection threshold is invalid for {metric:?}"
                )
            }
            Self::CountOverflow => formatter.write_str("frame metric count exceeds u64"),
            Self::AllocationFailed { elements } => {
                write!(
                    formatter,
                    "cannot allocate {elements} frame-selection results"
                )
            }
        }
    }
}

impl Error for FrameSelectionError {}

fn validate_rules(rules: &[FrameSelectionRule]) -> Result<(), FrameSelectionError> {
    if rules.is_empty() {
        return Err(FrameSelectionError::EmptyRules);
    }
    if rules.len() > MAX_FRAME_SELECTION_RULES {
        return Err(FrameSelectionError::TooManyRules {
            count: rules.len(),
            maximum: MAX_FRAME_SELECTION_RULES,
        });
    }
    let mut metrics = BTreeSet::new();
    for rule in rules {
        if !metrics.insert(rule.metric) {
            return Err(FrameSelectionError::DuplicateMetric {
                metric: rule.metric,
            });
        }
    }
    Ok(())
}

fn measured_value(
    metrics: FrameMetrics,
    metric: FrameSelectionMetric,
) -> Result<Option<FrameSelectionValue>, FrameSelectionError> {
    let value = match metric {
        FrameSelectionMetric::Background => metrics.background().map(FrameSelectionValue::Scalar),
        FrameSelectionMetric::Noise => metrics.noise().map(FrameSelectionValue::Scalar),
        FrameSelectionMetric::SignalToNoise => {
            metrics.signal_to_noise().map(FrameSelectionValue::Scalar)
        }
        FrameSelectionMetric::DetectedStars => metrics
            .detected_stars()
            .map(portable_count)
            .transpose()?
            .map(FrameSelectionValue::Count),
        FrameSelectionMetric::UsableStars => metrics
            .usable_stars()
            .map(portable_count)
            .transpose()?
            .map(FrameSelectionValue::Count),
        FrameSelectionMetric::FwhmMajorPixels => {
            metrics.fwhm_major_pixels().map(FrameSelectionValue::Scalar)
        }
        FrameSelectionMetric::Eccentricity => {
            metrics.eccentricity().map(FrameSelectionValue::Scalar)
        }
    };
    Ok(value)
}

fn portable_count(value: usize) -> Result<u64, FrameSelectionError> {
    u64::try_from(value).map_err(|_| FrameSelectionError::CountOverflow)
}

fn comparison_passes(
    measured: FrameSelectionValue,
    comparator: FrameSelectionComparator,
    threshold: FrameSelectionValue,
) -> bool {
    match (measured, threshold, comparator) {
        (
            FrameSelectionValue::Scalar(measured),
            FrameSelectionValue::Scalar(threshold),
            FrameSelectionComparator::LessThan,
        ) => measured < threshold,
        (
            FrameSelectionValue::Scalar(measured),
            FrameSelectionValue::Scalar(threshold),
            FrameSelectionComparator::GreaterThan,
        ) => measured > threshold,
        (
            FrameSelectionValue::Count(measured),
            FrameSelectionValue::Count(threshold),
            FrameSelectionComparator::LessThan,
        ) => measured < threshold,
        (
            FrameSelectionValue::Count(measured),
            FrameSelectionValue::Count(threshold),
            FrameSelectionComparator::GreaterThan,
        ) => measured > threshold,
        _ => false,
    }
}

fn valid_scalar_threshold(metric: FrameSelectionMetric, threshold: f64) -> bool {
    if !threshold.is_finite() {
        return false;
    }
    match metric {
        FrameSelectionMetric::Noise | FrameSelectionMetric::FwhmMajorPixels => threshold >= 0.0,
        FrameSelectionMetric::SignalToNoise => threshold > 0.0,
        FrameSelectionMetric::Eccentricity => (0.0..=1.0).contains(&threshold),
        FrameSelectionMetric::Background => true,
        FrameSelectionMetric::DetectedStars | FrameSelectionMetric::UsableStars => false,
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use super::*;
    use crate::{FrameId, FrameSpec, ManualDecision, ReviewBook, ReviewState};

    type TestResult<T = ()> = Result<T, Box<dyn StdError>>;

    fn measured_metrics() -> TestResult<FrameMetrics> {
        Ok(FrameMetrics::new(
            Some(1_000.0),
            Some(12.0),
            Some(800),
            Some(720),
            Some(3.2),
            Some(0.42),
        )?
        .with_signal_to_noise(Some(35.0))?)
    }

    #[test]
    fn validates_threshold_types_domains_and_rule_sets() -> TestResult {
        assert!(
            FrameSelectionRule::scalar(
                FrameSelectionMetric::DetectedStars,
                FrameSelectionComparator::GreaterThan,
                10.0,
                MissingMetricPolicy::Reject,
            )
            .is_err()
        );
        assert!(
            FrameSelectionRule::count(
                FrameSelectionMetric::FwhmMajorPixels,
                FrameSelectionComparator::LessThan,
                4,
                MissingMetricPolicy::Reject,
            )
            .is_err()
        );
        assert!(
            FrameSelectionRule::scalar(
                FrameSelectionMetric::Eccentricity,
                FrameSelectionComparator::LessThan,
                1.1,
                MissingMetricPolicy::Reject,
            )
            .is_err()
        );
        assert!(matches!(
            FrameSelectionEvaluation::evaluate(measured_metrics()?, &[]),
            Err(FrameSelectionError::EmptyRules)
        ));

        let rule = FrameSelectionRule::scalar(
            FrameSelectionMetric::Noise,
            FrameSelectionComparator::LessThan,
            20.0,
            MissingMetricPolicy::Reject,
        )?;
        assert!(matches!(
            FrameSelectionEvaluation::evaluate(measured_metrics()?, &[rule, rule]),
            Err(FrameSelectionError::DuplicateMetric {
                metric: FrameSelectionMetric::Noise
            })
        ));
        Ok(())
    }

    #[test]
    fn evaluates_every_rule_without_losing_failure_evidence() -> TestResult {
        let rules = [
            FrameSelectionRule::scalar(
                FrameSelectionMetric::FwhmMajorPixels,
                FrameSelectionComparator::LessThan,
                4.0,
                MissingMetricPolicy::Reject,
            )?,
            FrameSelectionRule::scalar(
                FrameSelectionMetric::Eccentricity,
                FrameSelectionComparator::LessThan,
                0.4,
                MissingMetricPolicy::Reject,
            )?,
            FrameSelectionRule::count(
                FrameSelectionMetric::DetectedStars,
                FrameSelectionComparator::GreaterThan,
                500,
                MissingMetricPolicy::Reject,
            )?,
        ];
        let evaluation = FrameSelectionEvaluation::evaluate(measured_metrics()?, &rules)?;

        assert_eq!(evaluation.algorithm_id(), FRAME_SELECTION_ALGORITHM_ID);
        assert_eq!(evaluation.proposal(), FrameSelectionProposal::Reject);
        assert_eq!(evaluation.evidence().len(), 3);
        assert_eq!(
            evaluation.evidence()[0].state(),
            FrameSelectionRuleState::Passed
        );
        assert_eq!(
            evaluation.evidence()[1].state(),
            FrameSelectionRuleState::Failed
        );
        assert_eq!(
            evaluation.evidence()[2].measured(),
            Some(FrameSelectionValue::Count(800))
        );
        Ok(())
    }

    #[test]
    fn missing_policy_is_explicit_and_manual_state_is_untouched() -> TestResult {
        let retain = FrameSelectionRule::scalar(
            FrameSelectionMetric::SignalToNoise,
            FrameSelectionComparator::GreaterThan,
            10.0,
            MissingMetricPolicy::Retain,
        )?;
        let reject = FrameSelectionRule::scalar(
            FrameSelectionMetric::SignalToNoise,
            FrameSelectionComparator::GreaterThan,
            10.0,
            MissingMetricPolicy::Reject,
        )?;
        let missing = FrameMetrics::default();
        let retained = FrameSelectionEvaluation::evaluate(missing, &[retain])?;
        let rejected = FrameSelectionEvaluation::evaluate(missing, &[reject])?;
        assert_eq!(retained.proposal(), FrameSelectionProposal::Retain);
        assert_eq!(
            retained.evidence()[0].state(),
            FrameSelectionRuleState::MissingRetained
        );
        assert_eq!(rejected.proposal(), FrameSelectionProposal::Reject);
        assert_eq!(
            rejected.evidence()[0].state(),
            FrameSelectionRuleState::MissingRejected
        );

        let id = FrameId::new("a".repeat(64))?;
        let mut book =
            ReviewBook::new(vec![FrameSpec::new(id.clone(), "light.fits", missing)?], 4)?;
        let preview = book.preview_changes(&[crate::DecisionChange::set(
            id.clone(),
            ManualDecision::accept(None)?,
        )])?;
        book.apply_preview(preview)?;
        let _proposal = FrameSelectionEvaluation::evaluate(missing, &[reject])?;
        assert_eq!(book.state(&id), Some(ReviewState::Accepted));
        Ok(())
    }
}
