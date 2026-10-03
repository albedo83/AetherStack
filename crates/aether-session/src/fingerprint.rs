use std::error::Error;
use std::fmt::{Display, Formatter};
use std::io::{self, Read};
use std::num::NonZeroU64;

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
    fingerprint_reader_internal(reader, None, |_| {})
}

/// Computes a source fingerprint and reports bounded byte progress.
///
/// Progress is emitted after crossing each non-zero interval and once at EOF
/// when the final byte count was not already reported. The callback never sees
/// duplicate or decreasing counts. Reads remain fixed at 64 KiB, so callers can
/// wrap the reader to add cooperative cancellation without changing the
/// fingerprint implementation used by batch tools.
///
/// # Errors
///
/// Returns the same errors as [`fingerprint_reader`].
pub fn fingerprint_reader_with_progress<R, F>(
    reader: &mut R,
    progress_interval: NonZeroU64,
    on_progress: F,
) -> Result<SourceFingerprint, FingerprintError>
where
    R: Read,
    F: FnMut(u64),
{
    fingerprint_reader_internal(reader, Some(progress_interval), on_progress)
}

fn fingerprint_reader_internal<R, F>(
    reader: &mut R,
    progress_interval: Option<NonZeroU64>,
    mut on_progress: F,
) -> Result<SourceFingerprint, FingerprintError>
where
    R: Read,
    F: FnMut(u64),
{
    let mut hasher = Sha256::new();
    let mut byte_length = 0_u64;
    let mut buffer = [0_u8; FINGERPRINT_BUFFER_BYTES];
    let mut last_progress = 0_u64;
    let mut next_progress = progress_interval.map(NonZeroU64::get);

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
        if next_progress.is_some_and(|threshold| byte_length >= threshold) {
            on_progress(byte_length);
            last_progress = byte_length;
            next_progress =
                progress_interval.map(|interval| byte_length.saturating_add(interval.get()));
        }
    }

    if progress_interval.is_some() && byte_length > last_progress {
        on_progress(byte_length);
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

    #[test]
    fn reports_monotone_interval_and_final_progress_without_duplicates() {
        let input = vec![0x5a_u8; FINGERPRINT_BUFFER_BYTES * 2 + 17];
        let mut progress = Vec::new();
        let result = fingerprint_reader_with_progress(
            &mut Cursor::new(input),
            NonZeroU64::new(FINGERPRINT_BUFFER_BYTES as u64).unwrap_or(NonZeroU64::MIN),
            |bytes| progress.push(bytes),
        );

        assert!(result.is_ok());
        assert_eq!(
            progress,
            vec![
                FINGERPRINT_BUFFER_BYTES as u64,
                (FINGERPRINT_BUFFER_BYTES * 2) as u64,
                (FINGERPRINT_BUFFER_BYTES * 2 + 17) as u64,
            ]
        );

        let mut exact_progress = Vec::new();
        let exact_result = fingerprint_reader_with_progress(
            &mut Cursor::new(vec![0x5a_u8; FINGERPRINT_BUFFER_BYTES * 2]),
            NonZeroU64::new(FINGERPRINT_BUFFER_BYTES as u64).unwrap_or(NonZeroU64::MIN),
            |bytes| exact_progress.push(bytes),
        );
        assert!(exact_result.is_ok());
        assert_eq!(
            exact_progress,
            vec![
                FINGERPRINT_BUFFER_BYTES as u64,
                (FINGERPRINT_BUFFER_BYTES * 2) as u64,
            ]
        );
    }
}
