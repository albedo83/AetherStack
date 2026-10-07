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
}
