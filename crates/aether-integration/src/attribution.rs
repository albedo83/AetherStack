//! Compact per-source spatial rejection evidence.

use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_core::Dimensions;

/// Exact disposition of one source sample at one output position.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum SampleDisposition {
    /// Clear finite sample retained in the science estimator.
    Accepted = 0,
    /// Sample excluded by a pre-existing quality mask.
    Masked = 1,
    /// Clear sample excluded because its value was not finite.
    NonFinite = 2,
    /// Finite sample rejected from the low tail.
    RejectedLow = 3,
    /// Finite sample rejected from the high tail.
    RejectedHigh = 4,
}

/// Exact disposition totals for one source across a planar region.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SourceDispositionCounts {
    accepted: u64,
    masked: u64,
    non_finite: u64,
    rejected_low: u64,
    rejected_high: u64,
}

impl SourceDispositionCounts {
    /// Clear finite samples retained by the estimator.
    #[must_use]
    pub const fn accepted(self) -> u64 {
        self.accepted
    }

    /// Samples excluded by source quality masks.
    #[must_use]
    pub const fn masked(self) -> u64 {
        self.masked
    }

    /// Clear non-finite samples excluded before statistics.
    #[must_use]
    pub const fn non_finite(self) -> u64 {
        self.non_finite
    }

    /// Samples rejected from the low tail.
    #[must_use]
    pub const fn rejected_low(self) -> u64 {
        self.rejected_low
    }

    /// Samples rejected from the high tail.
    #[must_use]
    pub const fn rejected_high(self) -> u64 {
        self.rejected_high
    }

    /// Total planar samples represented for this source.
    #[must_use]
    pub const fn total(self) -> u64 {
        self.accepted + self.masked + self.non_finite + self.rejected_low + self.rejected_high
    }

    /// Adds independent spatial regions while rejecting counter overflow.
    #[must_use]
    pub fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            accepted: self.accepted.checked_add(other.accepted)?,
            masked: self.masked.checked_add(other.masked)?,
            non_finite: self.non_finite.checked_add(other.non_finite)?,
            rejected_low: self.rejected_low.checked_add(other.rejected_low)?,
            rejected_high: self.rejected_high.checked_add(other.rejected_high)?,
        })
    }
}

impl SampleDisposition {
    const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Accepted),
            1 => Some(Self::Masked),
            2 => Some(Self::NonFinite),
            3 => Some(Self::RejectedLow),
            4 => Some(Self::RejectedHigh),
            _ => None,
        }
    }
}

/// Invalid dimensions or coordinates for rejection attribution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RejectionAttributionError {
    /// At least one source is required.
    EmptySourceSet,
    /// The sample count or compact storage length overflowed `usize`.
    SizeOverflow,
    /// Compact evidence storage could not be reserved.
    AllocationFailed {
        /// Required packed bytes.
        bytes: usize,
    },
    /// A planar sample or source index was outside the declared cube.
    IndexOutsideCube {
        /// Requested planar sample index.
        sample_index: usize,
        /// Requested source index.
        source_index: usize,
    },
    /// Packed storage contained a code not assigned by this version.
    InvalidPackedCode {
        /// Invalid three-bit value.
        code: u8,
    },
}

impl Display for RejectionAttributionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptySourceSet => formatter.write_str("rejection attribution requires a source"),
            Self::SizeOverflow => formatter.write_str("rejection attribution size overflowed"),
            Self::AllocationFailed { bytes } => {
                write!(
                    formatter,
                    "cannot reserve {bytes} rejection-attribution bytes"
                )
            }
            Self::IndexOutsideCube {
                sample_index,
                source_index,
            } => write!(
                formatter,
                "rejection attribution index sample={sample_index}, source={source_index} is outside the cube"
            ),
            Self::InvalidPackedCode { code } => {
                write!(
                    formatter,
                    "rejection attribution contains invalid code {code}"
                )
            }
        }
    }
}

impl Error for RejectionAttributionError {}

/// Bit-packed sample dispositions in planar-sample-major, source-minor order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RejectionAttribution {
    dimensions: Dimensions,
    source_count: usize,
    packed: Vec<u8>,
}

impl RejectionAttribution {
    /// Allocates clear accepted evidence for one complete image cube.
    pub fn new(
        dimensions: Dimensions,
        source_count: usize,
    ) -> Result<Self, RejectionAttributionError> {
        if source_count == 0 {
            return Err(RejectionAttributionError::EmptySourceSet);
        }
        let dispositions = dimensions
            .pixel_count()
            .checked_mul(source_count)
            .ok_or(RejectionAttributionError::SizeOverflow)?;
        let bits = dispositions
            .checked_mul(3)
            .ok_or(RejectionAttributionError::SizeOverflow)?;
        let bytes = bits
            .checked_add(7)
            .ok_or(RejectionAttributionError::SizeOverflow)?
            / 8;
        let mut packed = Vec::new();
        packed
            .try_reserve_exact(bytes)
            .map_err(|_| RejectionAttributionError::AllocationFailed { bytes })?;
        packed.resize(bytes, 0);
        Ok(Self {
            dimensions,
            source_count,
            packed,
        })
    }

    /// Declared planar image dimensions.
    #[must_use]
    pub const fn dimensions(&self) -> Dimensions {
        self.dimensions
    }

    /// Number of source dispositions stored for every planar sample.
    #[must_use]
    pub const fn source_count(&self) -> usize {
        self.source_count
    }

    /// Exact packed storage length.
    #[must_use]
    pub fn packed_len(&self) -> usize {
        self.packed.len()
    }

    /// Returns one exact disposition.
    pub fn disposition(
        &self,
        sample_index: usize,
        source_index: usize,
    ) -> Result<SampleDisposition, RejectionAttributionError> {
        let (byte_index, shift) = self.packed_position(sample_index, source_index)?;
        let mut window = u16::from(self.packed[byte_index]);
        if let Some(next) = self.packed.get(byte_index + 1) {
            window |= u16::from(*next) << 8;
        }
        let code = ((window >> shift) & 0b111) as u8;
        SampleDisposition::from_code(code)
            .ok_or(RejectionAttributionError::InvalidPackedCode { code })
    }

    /// Replaces one exact disposition without modifying adjacent evidence.
    pub fn set_disposition(
        &mut self,
        sample_index: usize,
        source_index: usize,
        disposition: SampleDisposition,
    ) -> Result<(), RejectionAttributionError> {
        let (byte_index, shift) = self.packed_position(sample_index, source_index)?;
        let mut window = u16::from(self.packed[byte_index]);
        if let Some(next) = self.packed.get(byte_index + 1) {
            window |= u16::from(*next) << 8;
        }
        let mask = 0b111_u16 << shift;
        window = (window & !mask) | (u16::from(disposition as u8) << shift);
        self.packed[byte_index] = window as u8;
        if shift > 5
            && let Some(next) = self.packed.get_mut(byte_index + 1)
        {
            *next = (window >> 8) as u8;
        }
        Ok(())
    }

    /// Aggregates exact disposition totals independently for every source.
    pub fn source_counts(&self) -> Result<Vec<SourceDispositionCounts>, RejectionAttributionError> {
        let bytes = self
            .source_count
            .checked_mul(std::mem::size_of::<SourceDispositionCounts>())
            .ok_or(RejectionAttributionError::SizeOverflow)?;
        let mut counts = Vec::new();
        counts
            .try_reserve_exact(self.source_count)
            .map_err(|_| RejectionAttributionError::AllocationFailed { bytes })?;
        counts.resize(self.source_count, SourceDispositionCounts::default());
        for sample_index in 0..self.dimensions.pixel_count() {
            for (source_index, source_counts) in counts.iter_mut().enumerate() {
                match self.disposition(sample_index, source_index)? {
                    SampleDisposition::Accepted => source_counts.accepted += 1,
                    SampleDisposition::Masked => source_counts.masked += 1,
                    SampleDisposition::NonFinite => source_counts.non_finite += 1,
                    SampleDisposition::RejectedLow => source_counts.rejected_low += 1,
                    SampleDisposition::RejectedHigh => source_counts.rejected_high += 1,
                }
            }
        }
        Ok(counts)
    }

    fn packed_position(
        &self,
        sample_index: usize,
        source_index: usize,
    ) -> Result<(usize, u32), RejectionAttributionError> {
        if sample_index >= self.dimensions.pixel_count() || source_index >= self.source_count {
            return Err(RejectionAttributionError::IndexOutsideCube {
                sample_index,
                source_index,
            });
        }
        let disposition_index = sample_index * self.source_count + source_index;
        let bit_index = disposition_index * 3;
        Ok((bit_index / 8, (bit_index % 8) as u32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn Error>>;

    #[test]
    fn round_trips_every_disposition_across_byte_boundaries() -> TestResult {
        let dimensions = Dimensions::new(3, 2, 2)?;
        let mut attribution = RejectionAttribution::new(dimensions, 5)?;
        let dispositions = [
            SampleDisposition::Accepted,
            SampleDisposition::Masked,
            SampleDisposition::NonFinite,
            SampleDisposition::RejectedLow,
            SampleDisposition::RejectedHigh,
        ];
        for sample_index in 0..dimensions.pixel_count() {
            for source_index in 0..5 {
                attribution.set_disposition(
                    sample_index,
                    source_index,
                    dispositions[(sample_index + source_index) % dispositions.len()],
                )?;
            }
        }
        for sample_index in 0..dimensions.pixel_count() {
            for source_index in 0..5 {
                assert_eq!(
                    attribution.disposition(sample_index, source_index)?,
                    dispositions[(sample_index + source_index) % dispositions.len()]
                );
            }
        }
        assert_eq!(attribution.dimensions(), dimensions);
        assert_eq!(attribution.source_count(), 5);
        assert_eq!(attribution.packed_len(), 23);
        Ok(())
    }

    #[test]
    fn rejects_empty_and_out_of_bounds_access_without_mutation() -> TestResult {
        let dimensions = Dimensions::new(2, 1, 1)?;
        assert_eq!(
            RejectionAttribution::new(dimensions, 0),
            Err(RejectionAttributionError::EmptySourceSet)
        );
        let mut attribution = RejectionAttribution::new(dimensions, 2)?;
        let original = attribution.clone();
        assert!(matches!(
            attribution.set_disposition(2, 0, SampleDisposition::RejectedHigh),
            Err(RejectionAttributionError::IndexOutsideCube { .. })
        ));
        assert!(matches!(
            attribution.disposition(0, 2),
            Err(RejectionAttributionError::IndexOutsideCube { .. })
        ));
        assert_eq!(attribution, original);
        Ok(())
    }

    #[test]
    fn invalid_packed_codes_fail_closed() -> TestResult {
        let dimensions = Dimensions::new(1, 1, 1)?;
        let mut attribution = RejectionAttribution::new(dimensions, 1)?;
        attribution.packed[0] = 0b111;
        assert_eq!(
            attribution.disposition(0, 0),
            Err(RejectionAttributionError::InvalidPackedCode { code: 7 })
        );
        Ok(())
    }

    #[test]
    fn source_totals_partition_every_planar_sample() -> TestResult {
        let dimensions = Dimensions::new(2, 2, 1)?;
        let mut attribution = RejectionAttribution::new(dimensions, 2)?;
        attribution.set_disposition(0, 0, SampleDisposition::RejectedLow)?;
        attribution.set_disposition(1, 0, SampleDisposition::Masked)?;
        attribution.set_disposition(2, 1, SampleDisposition::RejectedHigh)?;
        attribution.set_disposition(3, 1, SampleDisposition::NonFinite)?;

        let counts = attribution.source_counts()?;
        assert_eq!(counts[0].accepted(), 2);
        assert_eq!(counts[0].masked(), 1);
        assert_eq!(counts[0].rejected_low(), 1);
        assert_eq!(counts[0].total(), 4);
        assert_eq!(counts[1].accepted(), 2);
        assert_eq!(counts[1].non_finite(), 1);
        assert_eq!(counts[1].rejected_high(), 1);
        assert_eq!(counts[1].total(), 4);
        Ok(())
    }
}
