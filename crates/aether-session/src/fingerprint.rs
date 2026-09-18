use std::error::Error;
use std::fmt::{Display, Formatter};
use std::io::{self, Read};

use sha2::{Digest, Sha256};

use crate::{ManifestValidationError, SourceFingerprint};

/// Fixed scratch storage used while hashing one source (64 KiB).
pub const FINGERPRINT_BUFFER_BYTES: usize = 64 * 1_024;

/// Error raised while computing an immutable source fingerprint.
#[derive(Debug)]
pub enum FingerprintError {
    /// The source stream could not be read.
    Io(io::Error),
    /// The number of bytes read cannot be represented by the manifest.
    LengthOverflow,
    /// The resulting size or digest violates the manifest contract.
    InvalidFingerprint(ManifestValidationError),
}

impl Display for FingerprintError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "cannot read source for fingerprinting: {error}"),
            Self::LengthOverflow => {
                formatter.write_str("source length exceeds 64-bit manifest size")
            }
            Self::InvalidFingerprint(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for FingerprintError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::InvalidFingerprint(error) => Some(error),
            Self::LengthOverflow => None,
        }
    }
}

/// Computes SHA-256 and exact byte length with fixed scratch memory.
///
/// The function reads from the stream's current position until EOF. Interrupted
/// reads are retried. It never buffers the complete source, making it suitable
/// for large FITS or SER files.
///
/// # Errors
///
/// Returns [`FingerprintError::Io`] for a read failure,
/// [`FingerprintError::LengthOverflow`] when the byte count exceeds `u64`, or
/// [`FingerprintError::InvalidFingerprint`] for an empty source.
pub fn fingerprint_reader<R: Read>(reader: &mut R) -> Result<SourceFingerprint, FingerprintError> {
    let mut hasher = Sha256::new();
    let mut byte_length = 0_u64;
    let mut buffer = [0_u8; FINGERPRINT_BUFFER_BYTES];

    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(FingerprintError::Io(error)),
        };
        byte_length = checked_byte_length(byte_length, read)?;
        let Some(bytes) = buffer.get(..read) else {
            return Err(FingerprintError::LengthOverflow);
        };
        hasher.update(bytes);
    }

    let digest = hasher.finalize();
    let encoded = encode_lower_hex(&digest);

    SourceFingerprint::new(byte_length, encoded).map_err(FingerprintError::InvalidFingerprint)
}

pub(crate) fn encode_lower_hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(*byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(*byte & 0x0f)]));
    }
    encoded
}

fn checked_byte_length(current: u64, read: usize) -> Result<u64, FingerprintError> {
    let read = u64::try_from(read).map_err(|_| FingerprintError::LengthOverflow)?;
    current
        .checked_add(read)
        .ok_or(FingerprintError::LengthOverflow)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    struct InterruptOnce<R> {
        inner: R,
        interrupted: bool,
    }

    impl<R: Read> Read for InterruptOnce<R> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(io::Error::from(io::ErrorKind::Interrupted));
            }
            self.inner.read(buffer)
        }
    }

    struct AlwaysFails;

    impl Read for AlwaysFails {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied"))
        }
    }

    #[test]
    fn matches_the_sha256_standard_vector() {
        let result = fingerprint_reader(&mut Cursor::new(b"abc"));
        assert!(result.is_ok());
        let Some(fingerprint) = result.ok() else {
            return;
        };

        assert_eq!(fingerprint.byte_length(), 3);
        assert_eq!(
            fingerprint.sha256(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn retries_interrupted_reads_and_crosses_scratch_boundaries() {
        let input = vec![0x5a_u8; FINGERPRINT_BUFFER_BYTES + 17];
        let expected = Sha256::digest(&input);
        let mut reader = InterruptOnce {
            inner: Cursor::new(input),
            interrupted: false,
        };

        let result = fingerprint_reader(&mut reader);
        assert!(result.is_ok());
        let Some(fingerprint) = result.ok() else {
            return;
        };
        let mut expected_hex = String::new();
        for byte in expected {
            expected_hex.push_str(&format!("{byte:02x}"));
        }

        assert_eq!(
            fingerprint.byte_length(),
            (FINGERPRINT_BUFFER_BYTES + 17) as u64
        );
        assert_eq!(fingerprint.sha256(), expected_hex);
    }

    #[test]
    fn rejects_empty_sources() {
        assert!(matches!(
            fingerprint_reader(&mut Cursor::new(Vec::<u8>::new())),
            Err(FingerprintError::InvalidFingerprint(_))
        ));
    }

    #[test]
    fn reports_read_failure() {
        assert!(matches!(
            fingerprint_reader(&mut AlwaysFails),
            Err(FingerprintError::Io(error))
                if error.kind() == io::ErrorKind::PermissionDenied
        ));
    }

    #[test]
    fn reports_length_overflow_without_reading_an_impossible_source() {
        assert!(matches!(
            checked_byte_length(u64::MAX, 1),
            Err(FingerprintError::LengthOverflow)
        ));
    }
}
