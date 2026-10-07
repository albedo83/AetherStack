//! Source-specific spatial expansion of statistically rejected samples.

use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_core::Dimensions;
use sha2::{Digest, Sha256};

use crate::{RejectionAttribution, RejectionAttributionError, SampleDisposition};

/// Stable identity for the first source-owned large-scale rejection contract.
pub const LARGE_SCALE_REJECTION_ALGORITHM_ID: &str = "source-large-scale-rejection-v1";

/// Smallest supported dyadic detection layer count.
pub const MINIMUM_LARGE_SCALE_LAYERS: u8 = 1;

/// Largest supported dyadic detection layer count.
pub const MAXIMUM_LARGE_SCALE_LAYERS: u8 = 12;

/// Largest supported Chebyshev growth radius in pixels.
pub const MAXIMUM_LARGE_SCALE_GROWTH: u16 = 256;

/// Invalid source-owned large-scale rejection controls.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LargeScaleRejectionParameterError {
    /// Neither rejection tail was enabled.
    NoTailEnabled,
    /// A layer count was outside the supported dyadic range.
    LayersOutsideRange {
        /// Rejected layer count.
        layers: u8,
    },
    /// A growth radius exceeded the bounded implementation limit.
    GrowthOutsideRange {
        /// Rejected growth radius.
        growth: u16,
    },
    /// The final estimator support floor was zero.
    InvalidMinimumRetained,
}

impl Display for LargeScaleRejectionParameterError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoTailEnabled => {
                formatter.write_str("large-scale rejection requires at least one tail")
            }
            Self::LayersOutsideRange { layers } => write!(
                formatter,
                "large-scale rejection layers {layers} are outside {MINIMUM_LARGE_SCALE_LAYERS}..={MAXIMUM_LARGE_SCALE_LAYERS}"
            ),
            Self::GrowthOutsideRange { growth } => write!(
                formatter,
                "large-scale rejection growth {growth} exceeds {MAXIMUM_LARGE_SCALE_GROWTH} pixels"
            ),
            Self::InvalidMinimumRetained => formatter
                .write_str("large-scale rejection requires a positive retained-sample floor"),
        }
    }
}

impl Error for LargeScaleRejectionParameterError {}

/// Failure while classifying or expanding source-owned spatial rejection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LargeScaleRejectionError {
    /// The attribution cube could not be read safely.
    Attribution(RejectionAttributionError),
    /// A derived image, source, or summed-area size overflowed `usize`.
    SizeOverflow,
    /// A bounded working buffer could not be reserved.
    AllocationFailed {
        /// Number of requested elements.
        elements: usize,
    },
    /// A caller-supplied sample-major mask had the wrong length.
    MaskLengthMismatch {
        /// Exact required number of bytes.
        expected: usize,
        /// Supplied number of bytes.
        actual: usize,
    },
}

impl Display for LargeScaleRejectionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Attribution(error) => write!(formatter, "invalid rejection attribution: {error}"),
            Self::SizeOverflow => formatter.write_str("large-scale rejection size overflowed"),
            Self::AllocationFailed { elements } => write!(
                formatter,
                "cannot reserve {elements} large-scale rejection elements"
            ),
            Self::MaskLengthMismatch { expected, actual } => write!(
                formatter,
                "large-scale rejection mask has {actual} elements; expected {expected}"
            ),
        }
    }
}

impl Error for LargeScaleRejectionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Attribution(error) => Some(error),
            Self::SizeOverflow
            | Self::AllocationFailed { .. }
            | Self::MaskLengthMismatch { .. } => None,
        }
    }
}

impl From<RejectionAttributionError> for LargeScaleRejectionError {
    fn from(value: RejectionAttributionError) -> Self {
        Self::Attribution(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Statistical rejection tail examined by the spatial classifier.
pub enum LargeScaleRejectionTail {
    /// Low-valued statistical rejects.
    Low,
    /// High-valued statistical rejects.
    High,
}

impl LargeScaleRejectionTail {
    const fn disposition(self) -> SampleDisposition {
        match self {
            Self::Low => SampleDisposition::RejectedLow,
            Self::High => SampleDisposition::RejectedHigh,
        }
    }
}

/// Validated spatial controls for one rejection tail.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LargeScaleTailParameters {
    layers: u8,
    growth: u16,
}

impl LargeScaleTailParameters {
    /// Validates dyadic detection layers and a Chebyshev growth radius.
    pub fn new(layers: u8, growth: u16) -> Result<Self, LargeScaleRejectionParameterError> {
        if !(MINIMUM_LARGE_SCALE_LAYERS..=MAXIMUM_LARGE_SCALE_LAYERS).contains(&layers) {
            return Err(LargeScaleRejectionParameterError::LayersOutsideRange { layers });
        }
        if growth > MAXIMUM_LARGE_SCALE_GROWTH {
            return Err(LargeScaleRejectionParameterError::GrowthOutsideRange { growth });
        }
        Ok(Self { layers, growth })
    }

    /// Number of dyadic structure-detection layers.
    #[must_use]
    pub const fn layers(self) -> u8 {
        self.layers
    }

    /// Chebyshev growth radius in pixels.
    #[must_use]
    pub const fn growth(self) -> u16 {
        self.growth
    }

    /// Radius of the square structure-detection neighbourhood.
    #[must_use]
    pub const fn detection_radius(self) -> usize {
        1_usize << (self.layers - 1)
    }
}

/// Independent low-tail and high-tail large-scale rejection policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LargeScaleRejectionParameters {
    low: Option<LargeScaleTailParameters>,
    high: Option<LargeScaleTailParameters>,
    minimum_retained: u32,
}

impl LargeScaleRejectionParameters {
    /// Requires at least one explicitly configured rejection tail.
    pub fn new(
        low: Option<LargeScaleTailParameters>,
        high: Option<LargeScaleTailParameters>,
        minimum_retained: u32,
    ) -> Result<Self, LargeScaleRejectionParameterError> {
        if low.is_none() && high.is_none() {
            return Err(LargeScaleRejectionParameterError::NoTailEnabled);
        }
        if minimum_retained == 0 {
            return Err(LargeScaleRejectionParameterError::InvalidMinimumRetained);
        }
        Ok(Self {
            low,
            high,
            minimum_retained,
        })
    }

    /// Low-tail spatial controls when enabled.
    #[must_use]
    pub const fn low(self) -> Option<LargeScaleTailParameters> {
        self.low
    }

    /// High-tail spatial controls when enabled.
    #[must_use]
    pub const fn high(self) -> Option<LargeScaleTailParameters> {
        self.high
    }

    /// Smallest finite support allowed after spatial promotion.
    #[must_use]
    pub const fn minimum_retained(self) -> u32 {
        self.minimum_retained
    }

    /// Domain-separated canonical SHA-256 of every spatial control.
    #[must_use]
    pub fn parameters_sha256(self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"aetherstack/source-large-scale-rejection-v1\0");
        update_tail_digest(&mut hasher, self.low);
        update_tail_digest(&mut hasher, self.high);
        hasher.update(self.minimum_retained.to_be_bytes());
        hasher.finalize().into()
    }
}

fn update_tail_digest(hasher: &mut Sha256, tail: Option<LargeScaleTailParameters>) {
    match tail {
        Some(parameters) => {
            hasher.update([1, parameters.layers()]);
            hasher.update(parameters.growth().to_be_bytes());
        }
        None => hasher.update([0, 0, 0, 0]),
    }
}

/// Exact changes made by one simultaneous low/high spatial expansion.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LargeScaleExpansionSummary {
    promoted_low: u64,
    promoted_high: u64,
    conflicts_retained: u64,
    support_floor_retained: u64,
}

impl LargeScaleExpansionSummary {
    /// Previously accepted samples promoted to low-tail rejection.
    #[must_use]
    pub const fn promoted_low(self) -> u64 {
        self.promoted_low
    }

    /// Previously accepted samples promoted to high-tail rejection.
    #[must_use]
    pub const fn promoted_high(self) -> u64 {
        self.promoted_high
    }

    /// Accepted samples covered by both tails and conservatively retained.
    #[must_use]
    pub const fn conflicts_retained(self) -> u64 {
        self.conflicts_retained
    }

    /// Proposed promotions retained to preserve the support floor.
    #[must_use]
    pub const fn support_floor_retained(self) -> u64 {
        self.support_floor_retained
    }
}

/// Exact bounded working-set plan for one spatial rejection pass.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LargeScaleMemoryPlan {
    mask_elements: usize,
    integral_elements: usize,
    peak_working_bytes: usize,
}

impl LargeScaleMemoryPlan {
    /// Sample-major elements in one source-owned byte mask.
    #[must_use]
    pub const fn mask_elements(self) -> usize {
        self.mask_elements
    }

    /// `usize` elements in one reusable summed-area table.
    #[must_use]
    pub const fn integral_elements(self) -> usize {
        self.integral_elements
    }

    /// Conservative peak bytes held in addition to attribution and science.
    #[must_use]
    pub const fn peak_working_bytes(self) -> usize {
        self.peak_working_bytes
    }
}

/// Computes the exact conservative peak of masks and one summed-area table.
pub fn plan_large_scale_rejection_memory(
    dimensions: Dimensions,
    source_count: usize,
    parameters: LargeScaleRejectionParameters,
) -> Result<LargeScaleMemoryPlan, LargeScaleRejectionError> {
    let mask_elements = dimensions
        .pixel_count()
        .checked_mul(source_count)
        .ok_or(LargeScaleRejectionError::SizeOverflow)?;
    let integral_elements = dimensions
        .width()
        .checked_add(1)
        .and_then(|width| {
            dimensions
                .height()
                .checked_add(1)
                .and_then(|height| width.checked_mul(height))
        })
        .ok_or(LargeScaleRejectionError::SizeOverflow)?;
    let enabled_tails = usize::from(parameters.low().is_some())
        .checked_add(usize::from(parameters.high().is_some()))
        .ok_or(LargeScaleRejectionError::SizeOverflow)?;
    let simultaneous_masks = enabled_tails
        .checked_add(1)
        .ok_or(LargeScaleRejectionError::SizeOverflow)?;
    let mask_bytes = mask_elements
        .checked_mul(simultaneous_masks)
        .ok_or(LargeScaleRejectionError::SizeOverflow)?;
    let integral_bytes = integral_elements
        .checked_mul(std::mem::size_of::<usize>())
        .ok_or(LargeScaleRejectionError::SizeOverflow)?;
    let peak_working_bytes = mask_bytes
        .checked_add(integral_bytes)
        .ok_or(LargeScaleRejectionError::SizeOverflow)?;
    Ok(LargeScaleMemoryPlan {
        mask_elements,
        integral_elements,
        peak_working_bytes,
    })
}

/// Expands configured rejection tails simultaneously on one attribution cube.
///
/// Existing masked, non-finite, and statistical rejection evidence is never
/// rewritten. An accepted sample reached by exactly one grown tail is promoted
/// to that tail. If both tails reach it, the sample remains accepted and the
/// conflict is counted; iteration order therefore cannot bias the result.
pub fn expand_large_scale_rejections(
    attribution: &mut RejectionAttribution,
    parameters: LargeScaleRejectionParameters,
) -> Result<LargeScaleExpansionSummary, LargeScaleRejectionError> {
    let low = parameters
        .low()
        .map(|tail| {
            classify_large_scale_seeds(attribution, LargeScaleRejectionTail::Low, tail)
                .and_then(|seeds| grow_large_scale_seeds(&seeds, attribution, tail.growth()))
        })
        .transpose()?;
    let high = parameters
        .high()
        .map(|tail| {
            classify_large_scale_seeds(attribution, LargeScaleRejectionTail::High, tail)
                .and_then(|seeds| grow_large_scale_seeds(&seeds, attribution, tail.growth()))
        })
        .transpose()?;
    let source_count = attribution.source_count();
    let mut summary = LargeScaleExpansionSummary::default();
    for sample_index in 0..attribution.dimensions().pixel_count() {
        let mut accepted = 0_u32;
        let mut proposed = 0_u32;
        for source_index in 0..source_count {
            if attribution.disposition(sample_index, source_index)? != SampleDisposition::Accepted {
                continue;
            }
            accepted = accepted
                .checked_add(1)
                .ok_or(LargeScaleRejectionError::SizeOverflow)?;
            let offset = sample_index * source_count + source_index;
            let low_reached = low.as_ref().is_some_and(|mask| mask[offset] != 0);
            let high_reached = high.as_ref().is_some_and(|mask| mask[offset] != 0);
            match (low_reached, high_reached) {
                (true, false) | (false, true) => {
                    proposed = proposed
                        .checked_add(1)
                        .ok_or(LargeScaleRejectionError::SizeOverflow)?;
                }
                (true, true) => {
                    summary.conflicts_retained = summary
                        .conflicts_retained
                        .checked_add(1)
                        .ok_or(LargeScaleRejectionError::SizeOverflow)?;
                }
                (false, false) => {}
            }
        }
        if accepted.saturating_sub(proposed) < parameters.minimum_retained() {
            summary.support_floor_retained = summary
                .support_floor_retained
                .checked_add(u64::from(proposed))
                .ok_or(LargeScaleRejectionError::SizeOverflow)?;
            continue;
        }
        for source_index in 0..source_count {
            if attribution.disposition(sample_index, source_index)? != SampleDisposition::Accepted {
                continue;
            }
            let offset = sample_index * source_count + source_index;
            let low_reached = low.as_ref().is_some_and(|mask| mask[offset] != 0);
            let high_reached = high.as_ref().is_some_and(|mask| mask[offset] != 0);
            match (low_reached, high_reached) {
                (true, false) => {
                    attribution.set_disposition(
                        sample_index,
                        source_index,
                        SampleDisposition::RejectedLow,
                    )?;
                    summary.promoted_low = summary
                        .promoted_low
                        .checked_add(1)
                        .ok_or(LargeScaleRejectionError::SizeOverflow)?;
                }
                (false, true) => {
                    attribution.set_disposition(
                        sample_index,
                        source_index,
                        SampleDisposition::RejectedHigh,
                    )?;
                    summary.promoted_high = summary
                        .promoted_high
                        .checked_add(1)
                        .ok_or(LargeScaleRejectionError::SizeOverflow)?;
                }
                (true, true) | (false, false) => {}
            }
        }
    }
    Ok(summary)
}

/// Classifies source-owned large-scale seeds in sample-major, source-minor order.
///
/// Each returned byte is zero or one. A rejected sample becomes a seed only
/// when its square dyadic neighbourhood contains at least one full detection
/// diameter of same-tail rejected samples. Original attribution is never
/// modified by this operation.
pub fn classify_large_scale_seeds(
    attribution: &RejectionAttribution,
    tail: LargeScaleRejectionTail,
    parameters: LargeScaleTailParameters,
) -> Result<Vec<u8>, LargeScaleRejectionError> {
    let dimensions = attribution.dimensions();
    let width = dimensions.width();
    let height = dimensions.height();
    let source_count = attribution.source_count();
    let mask_elements = dimensions
        .pixel_count()
        .checked_mul(source_count)
        .ok_or(LargeScaleRejectionError::SizeOverflow)?;
    let mut seeds = zeroed_u8(mask_elements)?;
    let integral_width = width
        .checked_add(1)
        .ok_or(LargeScaleRejectionError::SizeOverflow)?;
    let integral_height = height
        .checked_add(1)
        .ok_or(LargeScaleRejectionError::SizeOverflow)?;
    let integral_elements = integral_width
        .checked_mul(integral_height)
        .ok_or(LargeScaleRejectionError::SizeOverflow)?;
    let mut integral = zeroed_usize(integral_elements)?;
    let radius = parameters.detection_radius();
    let full_diameter = radius
        .checked_mul(2)
        .and_then(|value| value.checked_add(1))
        .ok_or(LargeScaleRejectionError::SizeOverflow)?;
    let plane_pixels = width
        .checked_mul(height)
        .ok_or(LargeScaleRejectionError::SizeOverflow)?;

    for plane in 0..dimensions.planes() {
        let plane_offset = plane
            .checked_mul(plane_pixels)
            .ok_or(LargeScaleRejectionError::SizeOverflow)?;
        for source_index in 0..source_count {
            integral.fill(0);
            for y in 0..height {
                let mut row_sum = 0_usize;
                for x in 0..width {
                    let sample_index = plane_offset + y * width + x;
                    if attribution.disposition(sample_index, source_index)? == tail.disposition() {
                        row_sum += 1;
                    }
                    let integral_index = (y + 1) * integral_width + x + 1;
                    integral[integral_index] = integral[y * integral_width + x + 1] + row_sum;
                }
            }

            for y in 0..height {
                let top = y.saturating_sub(radius);
                let bottom = y.saturating_add(radius).saturating_add(1).min(height);
                for x in 0..width {
                    let sample_index = plane_offset + y * width + x;
                    if attribution.disposition(sample_index, source_index)? != tail.disposition() {
                        continue;
                    }
                    let left = x.saturating_sub(radius);
                    let right = x.saturating_add(radius).saturating_add(1).min(width);
                    let count = rectangle_sum(&integral, integral_width, left, top, right, bottom);
                    if count >= full_diameter {
                        seeds[sample_index * source_count + source_index] = 1;
                    }
                }
            }
        }
    }
    Ok(seeds)
}

/// Grows source-owned seeds by an exact square Chebyshev radius.
///
/// Planes and sources are independent. Image boundaries clip the neighbourhood
/// instead of wrapping it, and every output byte remains zero or one.
pub fn grow_large_scale_seeds(
    seeds: &[u8],
    attribution: &RejectionAttribution,
    growth: u16,
) -> Result<Vec<u8>, LargeScaleRejectionError> {
    let dimensions = attribution.dimensions();
    let width = dimensions.width();
    let height = dimensions.height();
    let source_count = attribution.source_count();
    let expected = dimensions
        .pixel_count()
        .checked_mul(source_count)
        .ok_or(LargeScaleRejectionError::SizeOverflow)?;
    if seeds.len() != expected {
        return Err(LargeScaleRejectionError::MaskLengthMismatch {
            expected,
            actual: seeds.len(),
        });
    }
    let mut grown = zeroed_u8(expected)?;
    let integral_width = width
        .checked_add(1)
        .ok_or(LargeScaleRejectionError::SizeOverflow)?;
    let integral_height = height
        .checked_add(1)
        .ok_or(LargeScaleRejectionError::SizeOverflow)?;
    let integral_elements = integral_width
        .checked_mul(integral_height)
        .ok_or(LargeScaleRejectionError::SizeOverflow)?;
    let mut integral = zeroed_usize(integral_elements)?;
    let plane_pixels = width
        .checked_mul(height)
        .ok_or(LargeScaleRejectionError::SizeOverflow)?;
    let radius = usize::from(growth);

    for plane in 0..dimensions.planes() {
        let plane_offset = plane
            .checked_mul(plane_pixels)
            .ok_or(LargeScaleRejectionError::SizeOverflow)?;
        for source_index in 0..source_count {
            integral.fill(0);
            for y in 0..height {
                let mut row_sum = 0_usize;
                for x in 0..width {
                    let sample_index = plane_offset + y * width + x;
                    row_sum += usize::from(seeds[sample_index * source_count + source_index] != 0);
                    let integral_index = (y + 1) * integral_width + x + 1;
                    integral[integral_index] = integral[y * integral_width + x + 1] + row_sum;
                }
            }
            for y in 0..height {
                let top = y.saturating_sub(radius);
                let bottom = y.saturating_add(radius).saturating_add(1).min(height);
                for x in 0..width {
                    let left = x.saturating_sub(radius);
                    let right = x.saturating_add(radius).saturating_add(1).min(width);
                    if rectangle_sum(&integral, integral_width, left, top, right, bottom) > 0 {
                        let sample_index = plane_offset + y * width + x;
                        grown[sample_index * source_count + source_index] = 1;
                    }
                }
            }
        }
    }
    Ok(grown)
}

fn rectangle_sum(
    integral: &[usize],
    stride: usize,
    left: usize,
    top: usize,
    right: usize,
    bottom: usize,
) -> usize {
    integral[bottom * stride + right] + integral[top * stride + left]
        - integral[top * stride + right]
        - integral[bottom * stride + left]
}

fn zeroed_u8(elements: usize) -> Result<Vec<u8>, LargeScaleRejectionError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(elements)
        .map_err(|_| LargeScaleRejectionError::AllocationFailed { elements })?;
    values.resize(elements, 0);
    Ok(values)
}

fn zeroed_usize(elements: usize) -> Result<Vec<usize>, LargeScaleRejectionError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(elements)
        .map_err(|_| LargeScaleRejectionError::AllocationFailed { elements })?;
    values.resize(elements, 0);
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn Error>>;

    #[test]
    fn validates_tail_controls_and_derived_scale() -> TestResult {
        let tail = LargeScaleTailParameters::new(4, 17)?;
        assert_eq!(tail.layers(), 4);
        assert_eq!(tail.growth(), 17);
        assert_eq!(tail.detection_radius(), 8);
        assert!(matches!(
            LargeScaleTailParameters::new(0, 0),
            Err(LargeScaleRejectionParameterError::LayersOutsideRange { layers: 0 })
        ));
        assert!(matches!(
            LargeScaleTailParameters::new(1, 257),
            Err(LargeScaleRejectionParameterError::GrowthOutsideRange { growth: 257 })
        ));
        Ok(())
    }

    #[test]
    fn requires_an_explicit_tail() -> TestResult {
        assert_eq!(
            LargeScaleRejectionParameters::new(None, None, 1),
            Err(LargeScaleRejectionParameterError::NoTailEnabled)
        );
        let high = LargeScaleTailParameters::new(2, 2)?;
        assert_eq!(
            LargeScaleRejectionParameters::new(None, Some(high), 3)?.high(),
            Some(high)
        );
        assert_eq!(
            LargeScaleRejectionParameters::new(None, Some(high), 0),
            Err(LargeScaleRejectionParameterError::InvalidMinimumRetained)
        );
        Ok(())
    }

    #[test]
    fn summed_area_classifier_keeps_lines_and_discards_isolated_rejects() -> TestResult {
        let dimensions = Dimensions::new(9, 7, 1)?;
        let mut attribution = RejectionAttribution::new(dimensions, 1)?;
        for x in 1..=7 {
            attribution.set_disposition(3 * 9 + x, 0, SampleDisposition::RejectedHigh)?;
        }
        attribution.set_disposition(9 + 1, 0, SampleDisposition::RejectedHigh)?;
        let parameters = LargeScaleTailParameters::new(2, 0)?;

        let seeds =
            classify_large_scale_seeds(&attribution, LargeScaleRejectionTail::High, parameters)?;
        let low_seeds =
            classify_large_scale_seeds(&attribution, LargeScaleRejectionTail::Low, parameters)?;

        assert_eq!(seeds[3 * 9 + 3], 1);
        assert_eq!(seeds[3 * 9 + 4], 1);
        assert_eq!(seeds[3 * 9 + 5], 1);
        assert_eq!(seeds[9 + 1], 0);
        assert_eq!(seeds.iter().filter(|value| **value != 0).count(), 4);
        assert!(low_seeds.iter().all(|value| *value == 0));
        Ok(())
    }

    #[test]
    fn growth_is_exact_and_does_not_cross_planes_or_sources() -> TestResult {
        let dimensions = Dimensions::new(5, 4, 2)?;
        let attribution = RejectionAttribution::new(dimensions, 2)?;
        let mut seeds = vec![0_u8; dimensions.pixel_count() * 2];
        let seed_sample = 7;
        seeds[seed_sample * 2 + 1] = 1;

        let grown = grow_large_scale_seeds(&seeds, &attribution, 1)?;

        for y in 0..4 {
            for x in 0..5 {
                let sample = y * 5 + x;
                let expected = (1..=3).contains(&x) && (0..=2).contains(&y);
                assert_eq!(grown[sample * 2 + 1] != 0, expected);
                assert_eq!(grown[sample * 2], 0);
                assert_eq!(grown[(20 + sample) * 2 + 1], 0);
            }
        }
        assert!(matches!(
            grow_large_scale_seeds(&seeds[..seeds.len() - 1], &attribution, 1),
            Err(LargeScaleRejectionError::MaskLengthMismatch { .. })
        ));
        Ok(())
    }

    #[test]
    fn simultaneous_tail_expansion_retains_conflicts_and_exclusions() -> TestResult {
        let dimensions = Dimensions::new(9, 7, 1)?;
        let mut attribution = RejectionAttribution::new(dimensions, 3)?;
        for x in 1..=7 {
            attribution.set_disposition(2 * 9 + x, 0, SampleDisposition::RejectedLow)?;
            attribution.set_disposition(4 * 9 + x, 0, SampleDisposition::RejectedHigh)?;
        }
        attribution.set_disposition(3 * 9, 0, SampleDisposition::Masked)?;
        let tail = LargeScaleTailParameters::new(1, 1)?;
        let parameters = LargeScaleRejectionParameters::new(Some(tail), Some(tail), 1)?;

        let summary = expand_large_scale_rejections(&mut attribution, parameters)?;

        assert!(summary.promoted_low() > 0);
        assert!(summary.promoted_high() > 0);
        assert!(summary.conflicts_retained() > 0);
        assert_eq!(
            attribution.disposition(3 * 9 + 4, 0)?,
            SampleDisposition::Accepted
        );
        assert_eq!(
            attribution.disposition(9 + 4, 0)?,
            SampleDisposition::RejectedLow
        );
        assert_eq!(
            attribution.disposition(5 * 9 + 4, 0)?,
            SampleDisposition::RejectedHigh
        );
        assert_eq!(
            attribution.disposition(3 * 9, 0)?,
            SampleDisposition::Masked
        );
        Ok(())
    }

    #[test]
    fn spatial_pass_rolls_back_pixels_that_would_cross_support_floor() -> TestResult {
        let dimensions = Dimensions::new(7, 5, 1)?;
        let mut attribution = RejectionAttribution::new(dimensions, 3)?;
        for x in 1..=5 {
            attribution.set_disposition(2 * 7 + x, 0, SampleDisposition::RejectedHigh)?;
        }
        let tail = LargeScaleTailParameters::new(1, 1)?;
        let parameters = LargeScaleRejectionParameters::new(None, Some(tail), 3)?;

        let summary = expand_large_scale_rejections(&mut attribution, parameters)?;

        assert_eq!(summary.promoted_high(), 0);
        assert!(summary.support_floor_retained() > 0);
        assert_eq!(
            attribution.disposition(7 + 3, 0)?,
            SampleDisposition::Accepted
        );
        Ok(())
    }

    #[test]
    fn memory_plan_accounts_for_both_retained_tails_and_peak_scratch() -> TestResult {
        let dimensions = Dimensions::new(100, 50, 3)?;
        let tail = LargeScaleTailParameters::new(2, 2)?;
        let both = LargeScaleRejectionParameters::new(Some(tail), Some(tail), 3)?;
        let one = LargeScaleRejectionParameters::new(None, Some(tail), 3)?;

        let both_plan = plan_large_scale_rejection_memory(dimensions, 20, both)?;
        let one_plan = plan_large_scale_rejection_memory(dimensions, 20, one)?;

        assert_eq!(both_plan.mask_elements(), 300_000);
        assert_eq!(both_plan.integral_elements(), 5_151);
        assert_eq!(
            both_plan.peak_working_bytes(),
            3 * 300_000 + 5_151 * std::mem::size_of::<usize>()
        );
        assert_eq!(
            one_plan.peak_working_bytes(),
            2 * 300_000 + 5_151 * std::mem::size_of::<usize>()
        );
        Ok(())
    }

    #[test]
    fn parameter_seal_binds_tail_role_scale_growth_and_support() -> TestResult {
        let first = LargeScaleTailParameters::new(2, 2)?;
        let changed_layers = LargeScaleTailParameters::new(3, 2)?;
        let changed_growth = LargeScaleTailParameters::new(2, 3)?;
        let baseline = LargeScaleRejectionParameters::new(None, Some(first), 3)?;

        assert_eq!(baseline.parameters_sha256(), baseline.parameters_sha256());
        assert_ne!(
            baseline.parameters_sha256(),
            LargeScaleRejectionParameters::new(Some(first), None, 3)?.parameters_sha256()
        );
        assert_ne!(
            baseline.parameters_sha256(),
            LargeScaleRejectionParameters::new(None, Some(changed_layers), 3)?.parameters_sha256()
        );
        assert_ne!(
            baseline.parameters_sha256(),
            LargeScaleRejectionParameters::new(None, Some(changed_growth), 3)?.parameters_sha256()
        );
        assert_ne!(
            baseline.parameters_sha256(),
            LargeScaleRejectionParameters::new(None, Some(first), 4)?.parameters_sha256()
        );
        Ok(())
    }
}
