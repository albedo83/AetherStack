use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_review::{FrameId, MAX_REVIEW_FRAMES};

/// Complete diagnostic metrics required for automatic reference selection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReferenceMetrics {
    fwhm_pixels: f64,
    eccentricity: f64,
    detected_stars: usize,
    noise: f64,
}

impl ReferenceMetrics {
    /// Creates finite positive shape/noise metrics and a nonzero star count.
    pub fn new(
        fwhm_pixels: f64,
        eccentricity: f64,
        detected_stars: usize,
        noise: f64,
    ) -> Result<Self, ReferenceSelectionError> {
        if !fwhm_pixels.is_finite()
            || fwhm_pixels <= 0.0
            || !eccentricity.is_finite()
            || !(0.0..1.0).contains(&eccentricity)
            || detected_stars == 0
            || !noise.is_finite()
            || noise <= 0.0
        {
            return Err(ReferenceSelectionError::InvalidMetrics);
        }
        Ok(Self {
            fwhm_pixels,
            eccentricity: canonical_zero(eccentricity),
            detected_stars,
            noise,
        })
    }

    /// Median major-axis FWHM in source pixels; lower ranks better.
    #[must_use]
    pub const fn fwhm_pixels(self) -> f64 {
        self.fwhm_pixels
    }

    /// Median stellar eccentricity; lower ranks better.
    #[must_use]
    pub const fn eccentricity(self) -> f64 {
        self.eccentricity
    }

    /// Number of usable stellar measurements; higher ranks better.
    #[must_use]
    pub const fn detected_stars(self) -> usize {
        self.detected_stars
    }

    /// Robust background noise in source units; lower ranks better.
    #[must_use]
    pub const fn noise(self) -> f64 {
        self.noise
    }
}

/// One reviewed frame eligible to become the registration reference.
#[derive(Clone, Debug, PartialEq)]
pub struct ReferenceCandidate {
    frame_id: FrameId,
    metrics: ReferenceMetrics,
}

impl ReferenceCandidate {
    /// Binds stable source identity to its immutable diagnostic metrics.
    #[must_use]
    pub const fn new(frame_id: FrameId, metrics: ReferenceMetrics) -> Self {
        Self { frame_id, metrics }
    }

    /// Stable content-derived review identity.
    #[must_use]
    pub const fn frame_id(&self) -> &FrameId {
        &self.frame_id
    }

    /// Metrics used by the documented ranking policy.
    #[must_use]
    pub const fn metrics(&self) -> ReferenceMetrics {
        self.metrics
    }
}

/// Per-metric ordinal ranks where zero is best and exact ties share a rank.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CandidateRanks {
    fwhm: usize,
    eccentricity: usize,
    detected_stars: usize,
    noise: usize,
}

impl CandidateRanks {
    /// FWHM rank.
    #[must_use]
    pub const fn fwhm(self) -> usize {
        self.fwhm
    }

    /// Eccentricity rank.
    #[must_use]
    pub const fn eccentricity(self) -> usize {
        self.eccentricity
    }

    /// Descending star-count rank.
    #[must_use]
    pub const fn detected_stars(self) -> usize {
        self.detected_stars
    }

    /// Background-noise rank.
    #[must_use]
    pub const fn noise(self) -> usize {
        self.noise
    }

    fn total(self) -> Result<usize, ReferenceSelectionError> {
        self.fwhm
            .checked_add(self.eccentricity)
            .and_then(|value| value.checked_add(self.detected_stars))
            .and_then(|value| value.checked_add(self.noise))
            .ok_or(ReferenceSelectionError::ScoreOverflow)
    }

    const fn worst(self) -> usize {
        let shape = if self.fwhm > self.eccentricity {
            self.fwhm
        } else {
            self.eccentricity
        };
        let support = if self.detected_stars > self.noise {
            self.detected_stars
        } else {
            self.noise
        };
        if shape > support { shape } else { support }
    }
}

/// Inspectable ranking evidence for one candidate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReferenceSelectionEvidence {
    frame_id: FrameId,
    ranks: CandidateRanks,
    total_rank: usize,
    worst_rank: usize,
}

impl ReferenceSelectionEvidence {
    /// Candidate identity.
    #[must_use]
    pub const fn frame_id(&self) -> &FrameId {
        &self.frame_id
    }

    /// Individual metric ranks.
    #[must_use]
    pub const fn ranks(&self) -> CandidateRanks {
        self.ranks
    }

    /// Sum of the four equal-weight ranks.
    #[must_use]
    pub const fn total_rank(&self) -> usize {
        self.total_rank
    }

    /// Worst individual rank, used as the first tie breaker.
    #[must_use]
    pub const fn worst_rank(&self) -> usize {
        self.worst_rank
    }
}

/// Selected reference plus complete canonical ranking evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReferenceSelection {
    selected_frame_id: FrameId,
    evidence: Vec<ReferenceSelectionEvidence>,
}

impl ReferenceSelection {
    /// Winning stable frame identity.
    #[must_use]
    pub const fn selected_frame_id(&self) -> &FrameId {
        &self.selected_frame_id
    }

    /// Evidence ordered by stable frame identity, not input order.
    #[must_use]
    pub fn evidence(&self) -> &[ReferenceSelectionEvidence] {
        &self.evidence
    }
}

/// Selects an automatic reference by equal-weight ordinal rank aggregation.
///
/// Lower FWHM, eccentricity, and noise rank better; higher detected-star count
/// ranks better. Exact metric ties share a rank. The minimum total rank wins,
/// followed by minimum worst rank, FWHM rank, star-count rank, eccentricity
/// rank, noise rank, and finally stable frame identity. Input order therefore
/// cannot alter either the winner or evidence.
pub fn select_reference(
    candidates: &[ReferenceCandidate],
) -> Result<ReferenceSelection, ReferenceSelectionError> {
    if candidates.is_empty() {
        return Err(ReferenceSelectionError::NoCandidates);
    }
    if candidates.len() > MAX_REVIEW_FRAMES {
        return Err(ReferenceSelectionError::TooManyCandidates {
            maximum: MAX_REVIEW_FRAMES,
            actual: candidates.len(),
        });
    }
    let mut canonical = Vec::new();
    canonical
        .try_reserve_exact(candidates.len())
        .map_err(|_| ReferenceSelectionError::AllocationFailed)?;
    canonical.extend_from_slice(candidates);
    canonical.sort_by(|left, right| left.frame_id.cmp(&right.frame_id));
    if canonical
        .windows(2)
        .any(|pair| pair[0].frame_id == pair[1].frame_id)
    {
        return Err(ReferenceSelectionError::DuplicateFrameId);
    }

    let count = canonical.len();
    let empty_ranks = CandidateRanks {
        fwhm: 0,
        eccentricity: 0,
        detected_stars: 0,
        noise: 0,
    };
    let mut ranks = Vec::new();
    ranks
        .try_reserve_exact(count)
        .map_err(|_| ReferenceSelectionError::AllocationFailed)?;
    ranks.resize(count, empty_ranks);
    assign_f64_ranks(
        &canonical,
        &mut ranks,
        |metrics| metrics.fwhm_pixels,
        |rank, value| rank.fwhm = value,
    )?;
    assign_f64_ranks(
        &canonical,
        &mut ranks,
        |metrics| metrics.eccentricity,
        |rank, value| rank.eccentricity = value,
    )?;
    assign_star_ranks(&canonical, &mut ranks)?;
    assign_f64_ranks(
        &canonical,
        &mut ranks,
        |metrics| metrics.noise,
        |rank, value| rank.noise = value,
    )?;

    let mut evidence = Vec::new();
    evidence
        .try_reserve_exact(count)
        .map_err(|_| ReferenceSelectionError::AllocationFailed)?;
    for (candidate, candidate_ranks) in canonical.iter().zip(ranks) {
        evidence.push(ReferenceSelectionEvidence {
            frame_id: candidate.frame_id.clone(),
            ranks: candidate_ranks,
            total_rank: candidate_ranks.total()?,
            worst_rank: candidate_ranks.worst(),
        });
    }
    let selected = evidence
        .iter()
        .min_by_key(|item| {
            (
                item.total_rank,
                item.worst_rank,
                item.ranks.fwhm,
                item.ranks.detected_stars,
                item.ranks.eccentricity,
                item.ranks.noise,
                &item.frame_id,
            )
        })
        .ok_or(ReferenceSelectionError::NoCandidates)?;
    Ok(ReferenceSelection {
        selected_frame_id: selected.frame_id.clone(),
        evidence,
    })
}

fn assign_f64_ranks<G, S>(
    candidates: &[ReferenceCandidate],
    ranks: &mut [CandidateRanks],
    get: G,
    mut set: S,
) -> Result<(), ReferenceSelectionError>
where
    G: Fn(ReferenceMetrics) -> f64,
    S: FnMut(&mut CandidateRanks, usize),
{
    let mut order = Vec::new();
    order
        .try_reserve_exact(candidates.len())
        .map_err(|_| ReferenceSelectionError::AllocationFailed)?;
    order.extend(0..candidates.len());
    order.sort_by(|left, right| {
        get(candidates[*left].metrics)
            .total_cmp(&get(candidates[*right].metrics))
            .then_with(|| candidates[*left].frame_id.cmp(&candidates[*right].frame_id))
    });
    let mut rank = 0;
    for position in 0..order.len() {
        if position > 0 {
            let previous = get(candidates[order[position - 1]].metrics);
            let current = get(candidates[order[position]].metrics);
            if previous.total_cmp(&current).is_ne() {
                rank = position;
            }
        }
        set(&mut ranks[order[position]], rank);
    }
    Ok(())
}

fn assign_star_ranks(
    candidates: &[ReferenceCandidate],
    ranks: &mut [CandidateRanks],
) -> Result<(), ReferenceSelectionError> {
    let mut order = Vec::new();
    order
        .try_reserve_exact(candidates.len())
        .map_err(|_| ReferenceSelectionError::AllocationFailed)?;
    order.extend(0..candidates.len());
    order.sort_by(|left, right| {
        candidates[*right]
            .metrics
            .detected_stars
            .cmp(&candidates[*left].metrics.detected_stars)
            .then_with(|| candidates[*left].frame_id.cmp(&candidates[*right].frame_id))
    });
    let mut rank = 0;
    for position in 0..order.len() {
        if position > 0
            && candidates[order[position - 1]].metrics.detected_stars
                != candidates[order[position]].metrics.detected_stars
        {
            rank = position;
        }
        ranks[order[position]].detected_stars = rank;
    }
    Ok(())
}

/// Invalid reference candidate set or score calculation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReferenceSelectionError {
    /// No eligible frames were supplied.
    NoCandidates,
    /// The candidate set exceeded the review model's hard bound.
    TooManyCandidates {
        /// Maximum accepted candidates.
        maximum: usize,
        /// Supplied candidates.
        actual: usize,
    },
    /// Two candidates used the same stable identity.
    DuplicateFrameId,
    /// A metric was non-finite or outside its physical domain.
    InvalidMetrics,
    /// Temporary or evidence storage allocation failed.
    AllocationFailed,
    /// Aggregate rank arithmetic overflowed.
    ScoreOverflow,
}

impl Display for ReferenceSelectionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoCandidates => formatter.write_str("reference selection has no candidates"),
            Self::TooManyCandidates { maximum, actual } => write!(
                formatter,
                "reference selection accepts at most {maximum} candidates, received {actual}"
            ),
            Self::DuplicateFrameId => {
                formatter.write_str("reference candidates contain a duplicate frame identity")
            }
            Self::InvalidMetrics => formatter.write_str("reference candidate metrics are invalid"),
            Self::AllocationFailed => {
                formatter.write_str("reference selection could not allocate evidence")
            }
            Self::ScoreOverflow => formatter.write_str("reference rank score overflowed"),
        }
    }
}

impl Error for ReferenceSelectionError {}

const fn canonical_zero(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn Error>>;

    fn candidate(
        digit: char,
        fwhm: f64,
        eccentricity: f64,
        stars: usize,
        noise: f64,
    ) -> TestResult<ReferenceCandidate> {
        Ok(ReferenceCandidate::new(
            FrameId::new(digit.to_string().repeat(64))?,
            ReferenceMetrics::new(fwhm, eccentricity, stars, noise)?,
        ))
    }

    #[test]
    fn selects_balanced_quality_with_complete_rank_evidence() -> TestResult {
        let selection = select_reference(&[
            candidate('a', 2.0, 0.30, 700, 12.0)?,
            candidate('b', 2.2, 0.32, 900, 10.0)?,
            candidate('c', 3.0, 0.50, 1_100, 20.0)?,
        ])?;

        assert_eq!(selection.selected_frame_id().as_str(), "b".repeat(64));
        assert_eq!(selection.evidence().len(), 3);
        let winner = selection
            .evidence()
            .iter()
            .find(|item| item.frame_id() == selection.selected_frame_id())
            .ok_or("selected evidence missing")?;
        assert_eq!(
            winner.ranks(),
            CandidateRanks {
                fwhm: 1,
                eccentricity: 1,
                detected_stars: 1,
                noise: 0,
            }
        );
        assert_eq!(winner.total_rank(), 3);
        Ok(())
    }

    #[test]
    fn input_order_and_exact_metric_ties_do_not_change_selection() -> TestResult {
        let first = candidate('a', 2.0, 0.3, 100, 10.0)?;
        let second = candidate('b', 2.0, 0.3, 100, 10.0)?;
        let forward = select_reference(&[first.clone(), second.clone()])?;
        let reverse = select_reference(&[second, first])?;

        assert_eq!(forward, reverse);
        assert_eq!(forward.selected_frame_id().as_str(), "a".repeat(64));
        assert!(forward.evidence().iter().all(|item| item.total_rank() == 0));
        Ok(())
    }

    #[test]
    fn rejects_invalid_empty_and_duplicate_candidates() -> TestResult {
        assert_eq!(
            ReferenceMetrics::new(0.0, 0.3, 10, 2.0),
            Err(ReferenceSelectionError::InvalidMetrics)
        );
        assert_eq!(
            select_reference(&[]),
            Err(ReferenceSelectionError::NoCandidates)
        );
        let duplicate = candidate('a', 2.0, 0.3, 100, 10.0)?;
        assert_eq!(
            select_reference(&[duplicate.clone(), duplicate]),
            Err(ReferenceSelectionError::DuplicateFrameId)
        );
        Ok(())
    }
}
