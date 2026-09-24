use std::error::Error;
use std::fmt::{Display, Formatter};

const WORD_BYTES: usize = 4;
const ASCII_ZERO: u8 = b'0';
const NEGATIVE_ZERO: u32 = u32::MAX;

/// Failure to calculate a FITS checksum over an exact byte range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FitsChecksumError {
    /// FITS checksum words are 32-bit and require four-byte alignment.
    UnalignedLength {
        /// Number of bytes supplied by the caller.
        bytes: usize,
    },
}

impl Display for FitsChecksumError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnalignedLength { bytes } => {
                write!(
                    formatter,
                    "FITS checksum input has unaligned length {bytes}"
                )
            }
        }
    }
}

impl Error for FitsChecksumError {}

/// Verification state of the optional `DATASUM` card.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatasumVerification {
    /// The HDU does not claim a data checksum.
    Missing,
    /// The keyword explicitly contains a blank undefined value.
    Undefined,
    /// The keyword is duplicated, has the wrong type, or is not unsigned decimal.
    Malformed,
    /// The declared and calculated data checksums agree.
    Valid {
        /// Calculated one's-complement data checksum.
        checksum: u32,
    },
    /// The data unit no longer matches the declared checksum.
    Mismatch {
        /// Unsigned decimal checksum stored in the header.
        declared: u32,
        /// Checksum calculated from the current data records.
        calculated: u32,
    },
}

/// Verification state of the optional `CHECKSUM` card.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HduChecksumVerification {
    /// The HDU does not claim an embedded complete-HDU checksum.
    Missing,
    /// The keyword explicitly contains a blank undefined value.
    Undefined,
    /// The keyword is duplicated or is not a 16-character alphanumeric string.
    Malformed,
    /// The complete HDU checksum equals one's-complement negative zero.
    Valid,
    /// The complete HDU no longer satisfies the embedded checksum.
    Mismatch {
        /// Checksum calculated over the current header and padded data records.
        calculated: u32,
    },
}

/// Independent results for the two FITS checksum-convention keywords.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrimaryChecksumReport {
    datasum: DatasumVerification,
    checksum: HduChecksumVerification,
}

impl PrimaryChecksumReport {
    pub(crate) const fn new(
        datasum: DatasumVerification,
        checksum: HduChecksumVerification,
    ) -> Self {
        Self { datasum, checksum }
    }

    /// Data-only checksum state.
    #[must_use]
    pub const fn datasum(self) -> DatasumVerification {
        self.datasum
    }

    /// Complete-HDU checksum state.
    #[must_use]
    pub const fn checksum(self) -> HduChecksumVerification {
        self.checksum
    }

    /// Returns true only when both keywords are present, well formed, and valid.
    #[must_use]
    pub const fn is_fully_verified(self) -> bool {
        matches!(self.datasum, DatasumVerification::Valid { .. })
            && matches!(self.checksum, HduChecksumVerification::Valid)
    }
}

/// Streaming 32-bit one's-complement accumulator used by the FITS convention.
///
/// Bytes are interpreted as big-endian 32-bit words. Arbitrary chunk boundaries
/// are accepted, so a caller can checksum the same stream while writing it. A
/// final partial word is zero-padded by [`Self::finish_zero_padded`]. Complete
/// FITS logical records are already four-byte aligned.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FitsChecksum {
    sum: u32,
    pending: [u8; WORD_BYTES],
    pending_len: u8,
}

impl FitsChecksum {
    /// Creates an empty positive-zero accumulator.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            sum: 0,
            pending: [0; WORD_BYTES],
            pending_len: 0,
        }
    }

    /// Adds the next consecutive bytes from the FITS stream.
    pub fn update(&mut self, mut bytes: &[u8]) {
        let pending_len = usize::from(self.pending_len);
        if pending_len != 0 {
            let needed = WORD_BYTES - pending_len;
            let copied = needed.min(bytes.len());
            self.pending[pending_len..pending_len + copied].copy_from_slice(&bytes[..copied]);
            self.pending_len += copied as u8;
            bytes = &bytes[copied..];
            if usize::from(self.pending_len) == WORD_BYTES {
                self.sum = add_word(self.sum, u32::from_be_bytes(self.pending));
                self.pending = [0; WORD_BYTES];
                self.pending_len = 0;
            } else {
                return;
            }
        }

        let mut words = bytes.chunks_exact(WORD_BYTES);
        for word in &mut words {
            let word = [word[0], word[1], word[2], word[3]];
            self.sum = add_word(self.sum, u32::from_be_bytes(word));
        }
        let remainder = words.remainder();
        self.pending[..remainder.len()].copy_from_slice(remainder);
        self.pending_len = remainder.len() as u8;
    }

    /// Returns the accumulated sum, padding a final partial word with zeroes.
    #[must_use]
    pub fn finish_zero_padded(mut self) -> u32 {
        if self.pending_len != 0 {
            self.sum = add_word(self.sum, u32::from_be_bytes(self.pending));
        }
        self.sum
    }
}

/// Calculates the checksum of a four-byte-aligned FITS byte range.
///
/// # Errors
///
/// Returns [`FitsChecksumError::UnalignedLength`] when the supplied range cannot
/// be interpreted as complete big-endian 32-bit words.
pub fn checksum_aligned(bytes: &[u8]) -> Result<u32, FitsChecksumError> {
    if !bytes.len().is_multiple_of(WORD_BYTES) {
        return Err(FitsChecksumError::UnalignedLength { bytes: bytes.len() });
    }
    let mut checksum = FitsChecksum::new();
    checksum.update(bytes);
    Ok(checksum.finish_zero_padded())
}

/// Combines checksums for adjacent four-byte-aligned byte ranges.
///
/// This property lets a writer checksum the header and data units separately,
/// then combine their values without reading the data a second time.
#[must_use]
pub const fn combine_checksums(first: u32, second: u32) -> u32 {
    add_word(first, second)
}

/// Returns whether a complete HDU satisfies the embedded checksum convention.
#[must_use]
pub const fn is_negative_zero(checksum: u32) -> bool {
    checksum == NEGATIVE_ZERO
}

/// Encodes the complement of an HDU checksum as the recommended 16 characters.
#[must_use]
pub fn encode_checksum(checksum: u32) -> [u8; 16] {
    let complement = !checksum;
    let mut unrotated = [0_u8; 16];

    for byte_index in 0..4 {
        let shift = (3 - byte_index) * 8;
        let byte = ((complement >> shift) & 0xff) as u8;
        let quotient = byte / 4 + ASCII_ZERO;
        let remainder = byte % 4;
        let mut characters = [quotient; 4];
        characters[0] += remainder;

        for pair_start in [0, 2] {
            while is_excluded_ascii(characters[pair_start])
                || is_excluded_ascii(characters[pair_start + 1])
            {
                characters[pair_start] += 1;
                characters[pair_start + 1] -= 1;
            }
        }

        for (quarter, character) in characters.into_iter().enumerate() {
            unrotated[4 * quarter + byte_index] = character;
        }
    }

    let mut encoded = [0_u8; 16];
    for (index, destination) in encoded.iter_mut().enumerate() {
        *destination = unrotated[(index + 15) % 16];
    }
    encoded
}

const fn add_word(first: u32, second: u32) -> u32 {
    let wide = first as u64 + second as u64;
    let folded = (wide & u32::MAX as u64) + (wide >> 32);
    let folded = (folded & u32::MAX as u64) + (folded >> 32);
    folded as u32
}

const fn is_excluded_ascii(value: u8) -> bool {
    matches!(value, b':'..=b'@' | b'['..=b'`')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_registered_ascii_encoding_example() {
        // Appendix A.4 starts with HDU checksum 0x33c0201d and gives this
        // exact recommended encoding of its complement.
        assert_eq!(encode_checksum(0x33c0_201d), *b"hcHjjc9ghcEghc9g");
    }

    #[test]
    fn streaming_chunk_boundaries_do_not_change_the_sum() {
        let bytes = b"abcdefghijklmnop";
        let expected = checksum_aligned(bytes);
        assert!(expected.is_ok());

        for split in 0..=bytes.len() {
            let mut checksum = FitsChecksum::new();
            checksum.update(&bytes[..split]);
            checksum.update(&bytes[split..]);
            assert_eq!(Some(checksum.finish_zero_padded()), expected.ok());
        }

        let mut bytewise = FitsChecksum::new();
        for byte in bytes {
            bytewise.update(std::slice::from_ref(byte));
        }
        assert_eq!(Some(bytewise.finish_zero_padded()), expected.ok());
    }

    #[test]
    fn end_around_carry_and_negative_zero_are_explicit() {
        assert_eq!(combine_checksums(u32::MAX, 1), 1);
        assert_eq!(combine_checksums(u32::MAX - 1, 1), u32::MAX);
        assert!(is_negative_zero(combine_checksums(u32::MAX - 1, 1)));
    }

    #[test]
    fn rejects_unaligned_exact_ranges_but_streaming_can_pad_them() {
        assert_eq!(
            checksum_aligned(&[1, 2, 3]),
            Err(FitsChecksumError::UnalignedLength { bytes: 3 })
        );
        let mut checksum = FitsChecksum::new();
        checksum.update(&[1, 2, 3]);
        assert_eq!(checksum.finish_zero_padded(), 0x0102_0300);
    }
}
