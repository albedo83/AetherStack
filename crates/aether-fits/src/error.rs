use std::error::Error;
use std::fmt::{Display, Formatter};
use std::io;

/// Structural errors that prevent reliable FITS header reading.
#[derive(Debug)]
pub enum FitsError {
    /// Error reported by the file system or input stream.
    Io(io::Error),
    /// The stream ends within a 2,880-byte header block.
    TruncatedHeader {
        /// Number of bytes present in the incomplete block.
        bytes_in_partial_block: usize,
    },
    /// The stream ends on a block boundary before any `END` card.
    MissingEndCard {
        /// Number of complete blocks read.
        blocks_read: usize,
    },
    /// No `END` card was found before the configured limit.
    HeaderBlockLimitExceeded {
        /// Maximum allowed block count.
        max_blocks: usize,
    },
}

impl Display for FitsError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "FITS input/output error: {error}"),
            Self::TruncatedHeader {
                bytes_in_partial_block,
            } => write!(
                formatter,
                "truncated FITS header block: received {bytes_in_partial_block} of 2880 bytes"
            ),
            Self::MissingEndCard { blocks_read } => write!(
                formatter,
                "FITS header ended after {blocks_read} blocks without an END card"
            ),
            Self::HeaderBlockLimitExceeded { max_blocks } => write!(
                formatter,
                "FITS header exceeds the configured limit of {max_blocks} blocks"
            ),
        }
    }
}

impl Error for FitsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::TruncatedHeader { .. }
            | Self::MissingEndCard { .. }
            | Self::HeaderBlockLimitExceeded { .. } => None,
        }
    }
}

impl From<io::Error> for FitsError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
