use std::error::Error;
use std::ffi::OsString;
use std::fmt::{Display, Formatter};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Take, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

use crate::CacheKey;
use crate::key::encode_lower_hex;

/// On-disk cache artifact format written by this release.
pub const ARTIFACT_FORMAT_VERSION: u32 = 1;

const ARTIFACT_MAGIC: &[u8; 8] = b"AETHCACH";
const ARTIFACT_HEADER_BYTES: usize = 8 + 4 + 8 + 32;
const COPY_BUFFER_BYTES: usize = 64 * 1_024;
const FILE_BUFFER_BYTES: usize = 64 * 1_024;
const MAX_TEMPORARY_NAME_ATTEMPTS: usize = 128;
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// SHA-256 digest of the exact artifact payload bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactDigest(String);

impl ArtifactDigest {
    fn from_bytes(bytes: &[u8]) -> Self {
        Self(encode_lower_hex(bytes))
    }

    /// Canonical lowercase hexadecimal representation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for ArtifactDigest {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Outcome of publishing bytes under an immutable operation key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtifactPublicationState {
    /// This call made a new complete artifact visible.
    Published,
    /// An independently verified artifact with identical bytes already existed.
    AlreadyPresent,
}

/// Verified accounting for one cache publication attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactPublication {
    key: CacheKey,
    digest: ArtifactDigest,
    payload_bytes: u64,
    state: ArtifactPublicationState,
}

impl ArtifactPublication {
    /// Operation key naming the artifact.
    #[must_use]
    pub const fn key(&self) -> &CacheKey {
        &self.key
    }

    /// SHA-256 of the exact payload bytes.
    #[must_use]
    pub const fn digest(&self) -> &ArtifactDigest {
        &self.digest
    }

    /// Exact payload length, excluding the cache header.
    #[must_use]
    pub const fn payload_bytes(&self) -> u64 {
        self.payload_bytes
    }

    /// Whether this call published bytes or reused an identical artifact.
    #[must_use]
    pub const fn state(&self) -> ArtifactPublicationState {
        self.state
    }
}

/// Reader positioned at a fully verified immutable artifact payload.
pub struct VerifiedArtifact {
    reader: Take<File>,
    digest: ArtifactDigest,
    payload_bytes: u64,
}

impl VerifiedArtifact {
    /// SHA-256 verified before this reader was returned.
    #[must_use]
    pub const fn digest(&self) -> &ArtifactDigest {
        &self.digest
    }

    /// Exact number of readable payload bytes.
    #[must_use]
    pub const fn payload_bytes(&self) -> u64 {
        self.payload_bytes
    }
}

impl Read for VerifiedArtifact {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.reader.read(buffer)
    }
}

/// Rooted immutable cache using two hexadecimal shard characters per directory.
#[derive(Clone, Debug)]
pub struct ArtifactStore {
    root: PathBuf,
}

impl ArtifactStore {
    /// Creates a cache handle without touching the filesystem.
    ///
    /// # Errors
    ///
    /// Returns [`CacheWriteError::InvalidRoot`] for an empty path.
    pub fn new(root: PathBuf) -> Result<Self, CacheWriteError> {
        if root.as_os_str().is_empty() {
            return Err(CacheWriteError::InvalidRoot);
        }
        Ok(Self { root })
    }

    /// Cache root supplied by the caller.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Streams, verifies, and atomically publishes one immutable artifact.
    ///
    /// The payload is first written to a unique file in its final shard while a
    /// SHA-256 digest and exact length are calculated. A fixed header is then
    /// filled, the file is flushed and synchronized, and a hard link publishes
    /// it only if the key is absent. Concurrent identical writers safely reuse
    /// the winner. Different bytes under one operation key return a collision
    /// and never overwrite the existing artifact.
    ///
    /// # Errors
    ///
    /// Returns a typed directory, source, encoding, durability, publication,
    /// existing-artifact validation, or key-collision failure.
    pub fn publish<R: Read>(
        &self,
        key: &CacheKey,
        source: &mut R,
    ) -> Result<ArtifactPublication, CacheWriteError> {
        let shard = self.shard_path(key);
        fs::create_dir_all(&shard).map_err(CacheWriteError::CreateDirectory)?;
        let target = self.artifact_path(key);
        let (temporary_path, file) = create_temporary(&shard, key)?;
        let mut guard = TemporaryPathGuard::new(temporary_path.clone());
        let mut writer = BufWriter::with_capacity(FILE_BUFFER_BYTES, file);

        writer
            .write_all(&[0_u8; ARTIFACT_HEADER_BYTES])
            .map_err(CacheWriteError::WriteTemporary)?;
        let (payload_bytes, digest) = copy_payload(source, &mut writer)?;
        writer
            .seek(SeekFrom::Start(0))
            .map_err(CacheWriteError::SeekTemporary)?;
        write_header(&mut writer, payload_bytes, &digest)?;
        writer.flush().map_err(CacheWriteError::FlushTemporary)?;
        writer
            .get_ref()
            .sync_all()
            .map_err(CacheWriteError::SyncTemporary)?;
        drop(writer);

        match fs::hard_link(&temporary_path, &target) {
            Ok(()) => {
                if let Err(source) = fs::remove_file(&temporary_path) {
                    guard.disarm();
                    return Err(CacheWriteError::CleanupAfterPublish {
                        temporary_path,
                        source,
                    });
                }
                guard.disarm();
                sync_directory_after_publish(&shard)?;
                Ok(ArtifactPublication {
                    key: key.clone(),
                    digest,
                    payload_bytes,
                    state: ArtifactPublicationState::Published,
                })
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let existing = self
                    .open_verified(key)
                    .map_err(CacheWriteError::ExistingInvalid)?;
                if existing.payload_bytes() == payload_bytes && existing.digest() == &digest {
                    Ok(ArtifactPublication {
                        key: key.clone(),
                        digest,
                        payload_bytes,
                        state: ArtifactPublicationState::AlreadyPresent,
                    })
                } else {
                    Err(CacheWriteError::KeyCollision {
                        existing: existing.digest().clone(),
                        candidate: digest,
                    })
                }
            }
            Err(error) => Err(CacheWriteError::Publish(error)),
        }
    }

    /// Opens an artifact only after verifying its complete header and payload.
    ///
    /// Validation checks the magic, format version, exact file length, and
    /// payload SHA-256. The returned reader is bounded to the declared payload
    /// length and begins at its first byte.
    ///
    /// # Errors
    ///
    /// Returns a typed error for absence, non-regular targets, malformed
    /// metadata, truncation, trailing bytes, I/O failure, or digest mismatch.
    pub fn open_verified(&self, key: &CacheKey) -> Result<VerifiedArtifact, CacheReadError> {
        let path = self.artifact_path(key);
        let metadata = fs::symlink_metadata(&path).map_err(CacheReadError::Open)?;
        if !metadata.file_type().is_file() {
            return Err(CacheReadError::NotRegularFile);
        }
        let mut file = File::open(path).map_err(CacheReadError::Open)?;
        let (payload_bytes, digest) = verify_file(&mut file, metadata.len())?;
        file.seek(SeekFrom::Start(ARTIFACT_HEADER_BYTES as u64))
            .map_err(CacheReadError::Seek)?;
        Ok(VerifiedArtifact {
            reader: file.take(payload_bytes),
            digest,
            payload_bytes,
        })
    }

    fn shard_path(&self, key: &CacheKey) -> PathBuf {
        self.root.join(&key.as_str()[..2])
    }

    fn artifact_path(&self, key: &CacheKey) -> PathBuf {
        self.shard_path(key)
            .join(format!("{}.artifact", key.as_str()))
    }
}

/// Failure while reading or validating an existing cache artifact.
#[derive(Debug)]
pub enum CacheReadError {
    /// Artifact path cannot be inspected or opened.
    Open(io::Error),
    /// Artifact path exists but is not a regular file.
    NotRegularFile,
    /// The fixed header is truncated or unreadable.
    ReadHeader(io::Error),
    /// Header magic does not identify an AetherStack cache artifact.
    InvalidMagic,
    /// Header version is unsupported.
    UnsupportedVersion {
        /// Version read from the artifact.
        version: u32,
    },
    /// Header fields could not be decoded despite their fixed size.
    InvalidHeader,
    /// Declared payload plus header overflows the file-length domain.
    LengthOverflow,
    /// Actual file length differs from the exact declared length.
    LengthMismatch {
        /// Complete expected length including the header.
        expected: u64,
        /// Complete observed file length.
        actual: u64,
    },
    /// Payload could not be read completely.
    ReadPayload(io::Error),
    /// Payload bytes do not match their stored SHA-256 digest.
    DigestMismatch {
        /// Digest recorded in the header.
        expected: ArtifactDigest,
        /// Digest calculated from the payload.
        actual: ArtifactDigest,
    },
    /// Verified file could not be rewound to its payload.
    Seek(io::Error),
}

impl Display for CacheReadError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open(error) => write!(formatter, "cannot open cache artifact: {error}"),
            Self::NotRegularFile => formatter.write_str("cache artifact is not a regular file"),
            Self::ReadHeader(error) => write!(formatter, "cannot read cache header: {error}"),
            Self::InvalidMagic => formatter.write_str("cache artifact magic is invalid"),
            Self::UnsupportedVersion { version } => {
                write!(formatter, "cache artifact version {version} is unsupported")
            }
            Self::InvalidHeader => formatter.write_str("cache artifact header is invalid"),
            Self::LengthOverflow => formatter.write_str("cache artifact length overflows"),
            Self::LengthMismatch { expected, actual } => write!(
                formatter,
                "cache artifact length is {actual} bytes; expected {expected}"
            ),
            Self::ReadPayload(error) => write!(formatter, "cannot read cache payload: {error}"),
            Self::DigestMismatch { expected, actual } => write!(
                formatter,
                "cache payload digest {actual} does not match stored digest {expected}"
            ),
            Self::Seek(error) => write!(formatter, "cannot seek verified cache artifact: {error}"),
        }
    }
}

impl Error for CacheReadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Open(error)
            | Self::ReadHeader(error)
            | Self::ReadPayload(error)
            | Self::Seek(error) => Some(error),
            Self::NotRegularFile
            | Self::InvalidMagic
            | Self::UnsupportedVersion { .. }
            | Self::InvalidHeader
            | Self::LengthOverflow
            | Self::LengthMismatch { .. }
            | Self::DigestMismatch { .. } => None,
        }
    }
}

/// Failure while streaming or atomically publishing a cache artifact.
#[derive(Debug)]
pub enum CacheWriteError {
    /// Cache root path is empty.
    InvalidRoot,
    /// Artifact shard directory could not be created.
    CreateDirectory(io::Error),
    /// No unique temporary file could be created.
    CreateTemporary(io::Error),
    /// Source stream failed while producing payload bytes.
    ReadSource(io::Error),
    /// Payload byte count exceeded `u64`.
    PayloadTooLarge,
    /// Temporary header or payload bytes could not be written.
    WriteTemporary(io::Error),
    /// Temporary file could not be rewound to finalize its header.
    SeekTemporary(io::Error),
    /// Buffered temporary bytes could not be flushed.
    FlushTemporary(io::Error),
    /// Temporary file could not be synchronized before publication.
    SyncTemporary(io::Error),
    /// Complete temporary file could not be linked to the immutable key.
    Publish(io::Error),
    /// Existing artifact under the requested key failed verification.
    ExistingInvalid(CacheReadError),
    /// Existing verified bytes differ from the candidate under the same key.
    KeyCollision {
        /// Existing payload digest.
        existing: ArtifactDigest,
        /// Candidate payload digest.
        candidate: ArtifactDigest,
    },
    /// Destination is published, but its temporary hard link remains.
    CleanupAfterPublish {
        /// Temporary path that may require manual cleanup.
        temporary_path: PathBuf,
        /// Cleanup failure.
        source: io::Error,
    },
    /// Destination is published, but shard metadata could not be synchronized.
    DirectorySyncAfterPublish(io::Error),
}

impl CacheWriteError {
    /// Returns whether a complete destination is already visible.
    #[must_use]
    pub const fn artifact_is_published(&self) -> bool {
        matches!(
            self,
            Self::CleanupAfterPublish { .. } | Self::DirectorySyncAfterPublish(_)
        )
    }
}

impl Display for CacheWriteError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRoot => formatter.write_str("cache root must not be empty"),
            Self::CreateDirectory(error) => {
                write!(formatter, "cannot create cache shard: {error}")
            }
            Self::CreateTemporary(error) => {
                write!(formatter, "cannot create temporary cache artifact: {error}")
            }
            Self::ReadSource(error) => write!(formatter, "cannot read artifact source: {error}"),
            Self::PayloadTooLarge => formatter.write_str("artifact payload exceeds 64-bit length"),
            Self::WriteTemporary(error) => {
                write!(formatter, "cannot write temporary cache artifact: {error}")
            }
            Self::SeekTemporary(error) => {
                write!(formatter, "cannot seek temporary cache artifact: {error}")
            }
            Self::FlushTemporary(error) => {
                write!(formatter, "cannot flush temporary cache artifact: {error}")
            }
            Self::SyncTemporary(error) => {
                write!(
                    formatter,
                    "cannot synchronize temporary cache artifact: {error}"
                )
            }
            Self::Publish(error) => write!(formatter, "cannot publish cache artifact: {error}"),
            Self::ExistingInvalid(error) => {
                write!(formatter, "existing cache artifact is invalid: {error}")
            }
            Self::KeyCollision {
                existing,
                candidate,
            } => write!(
                formatter,
                "cache key collision: existing digest {existing}, candidate digest {candidate}"
            ),
            Self::CleanupAfterPublish {
                temporary_path,
                source,
            } => write!(
                formatter,
                "cache artifact is published but temporary link {} could not be removed: {source}",
                temporary_path.display()
            ),
            Self::DirectorySyncAfterPublish(error) => write!(
                formatter,
                "cache artifact is published but shard metadata could not be synchronized: {error}"
            ),
        }
    }
}

impl Error for CacheWriteError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CreateDirectory(error)
            | Self::CreateTemporary(error)
            | Self::ReadSource(error)
            | Self::WriteTemporary(error)
            | Self::SeekTemporary(error)
            | Self::FlushTemporary(error)
            | Self::SyncTemporary(error)
            | Self::Publish(error)
            | Self::DirectorySyncAfterPublish(error) => Some(error),
            Self::ExistingInvalid(error) => Some(error),
            Self::CleanupAfterPublish { source, .. } => Some(source),
            Self::InvalidRoot | Self::PayloadTooLarge | Self::KeyCollision { .. } => None,
        }
    }
}

struct TemporaryPathGuard {
    path: PathBuf,
    armed: bool,
}

impl TemporaryPathGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TemporaryPathGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ignored = fs::remove_file(&self.path);
        }
    }
}

fn create_temporary(shard: &Path, key: &CacheKey) -> Result<(PathBuf, File), CacheWriteError> {
    let mut last_collision = None;
    for _ in 0..MAX_TEMPORARY_NAME_ATTEMPTS {
        let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let mut name = OsString::from(".");
        name.push(key.as_str());
        name.push(format!(".{}-{sequence}.tmp", std::process::id()));
        let path = shard.join(name);
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                last_collision = Some(error);
            }
            Err(error) => return Err(CacheWriteError::CreateTemporary(error)),
        }
    }
    Err(CacheWriteError::CreateTemporary(
        last_collision.unwrap_or_else(|| {
            io::Error::new(
                io::ErrorKind::AlreadyExists,
                "temporary cache name attempts exhausted",
            )
        }),
    ))
}

fn copy_payload<R: Read, W: Write>(
    source: &mut R,
    destination: &mut W,
) -> Result<(u64, ArtifactDigest), CacheWriteError> {
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    let mut length = 0_u64;
    let mut hasher = Sha256::new();
    loop {
        let read = match source.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(CacheWriteError::ReadSource(error)),
        };
        let bytes = buffer.get(..read).ok_or(CacheWriteError::PayloadTooLarge)?;
        let read = u64::try_from(read).map_err(|_| CacheWriteError::PayloadTooLarge)?;
        length = length
            .checked_add(read)
            .ok_or(CacheWriteError::PayloadTooLarge)?;
        destination
            .write_all(bytes)
            .map_err(CacheWriteError::WriteTemporary)?;
        hasher.update(bytes);
    }
    Ok((length, ArtifactDigest::from_bytes(&hasher.finalize())))
}

fn write_header<W: Write>(
    writer: &mut W,
    payload_bytes: u64,
    digest: &ArtifactDigest,
) -> Result<(), CacheWriteError> {
    let digest_bytes = decode_lower_hex(digest.as_str()).ok_or(CacheWriteError::PayloadTooLarge)?;
    writer
        .write_all(ARTIFACT_MAGIC)
        .and_then(|()| writer.write_all(&ARTIFACT_FORMAT_VERSION.to_be_bytes()))
        .and_then(|()| writer.write_all(&payload_bytes.to_be_bytes()))
        .and_then(|()| writer.write_all(&digest_bytes))
        .map_err(CacheWriteError::WriteTemporary)
}

fn verify_file(
    file: &mut File,
    actual_length: u64,
) -> Result<(u64, ArtifactDigest), CacheReadError> {
    let mut header = [0_u8; ARTIFACT_HEADER_BYTES];
    file.read_exact(&mut header)
        .map_err(CacheReadError::ReadHeader)?;
    if header.get(..8) != Some(ARTIFACT_MAGIC.as_slice()) {
        return Err(CacheReadError::InvalidMagic);
    }
    let version = read_array::<4>(&header, 8)
        .map(u32::from_be_bytes)
        .ok_or(CacheReadError::InvalidHeader)?;
    if version != ARTIFACT_FORMAT_VERSION {
        return Err(CacheReadError::UnsupportedVersion { version });
    }
    let payload_bytes = read_array::<8>(&header, 12)
        .map(u64::from_be_bytes)
        .ok_or(CacheReadError::InvalidHeader)?;
    let expected_digest_bytes =
        read_array::<32>(&header, 20).ok_or(CacheReadError::InvalidHeader)?;
    let expected_digest = ArtifactDigest::from_bytes(&expected_digest_bytes);
    let expected_length = (ARTIFACT_HEADER_BYTES as u64)
        .checked_add(payload_bytes)
        .ok_or(CacheReadError::LengthOverflow)?;
    if actual_length != expected_length {
        return Err(CacheReadError::LengthMismatch {
            expected: expected_length,
            actual: actual_length,
        });
    }

    let mut remaining = payload_bytes;
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    let mut hasher = Sha256::new();
    while remaining > 0 {
        let requested = usize::try_from(remaining.min(COPY_BUFFER_BYTES as u64))
            .map_err(|_| CacheReadError::LengthOverflow)?;
        let bytes = buffer
            .get_mut(..requested)
            .ok_or(CacheReadError::LengthOverflow)?;
        file.read_exact(bytes)
            .map_err(CacheReadError::ReadPayload)?;
        hasher.update(bytes);
        remaining -= requested as u64;
    }
    let actual_digest = ArtifactDigest::from_bytes(&hasher.finalize());
    if actual_digest != expected_digest {
        return Err(CacheReadError::DigestMismatch {
            expected: expected_digest,
            actual: actual_digest,
        });
    }
    Ok((payload_bytes, actual_digest))
}

fn read_array<const LENGTH: usize>(bytes: &[u8], start: usize) -> Option<[u8; LENGTH]> {
    let end = start.checked_add(LENGTH)?;
    bytes.get(start..end)?.try_into().ok()
}

fn decode_lower_hex(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 {
        return None;
    }
    let mut output = [0_u8; 32];
    for (destination, pair) in output.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
        let high = decode_nibble(*pair.first()?)?;
        let low = decode_nibble(*pair.get(1)?)?;
        *destination = (high << 4) | low;
    }
    Some(output)
}

const fn decode_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

#[cfg(unix)]
fn sync_directory_after_publish(shard: &Path) -> Result<(), CacheWriteError> {
    let directory = File::open(shard).map_err(CacheWriteError::DirectorySyncAfterPublish)?;
    directory
        .sync_all()
        .map_err(CacheWriteError::DirectorySyncAfterPublish)
}

#[cfg(not(unix))]
fn sync_directory_after_publish(_shard: &Path) -> Result<(), CacheWriteError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read as _, Seek as _, Write as _};
    use std::sync::{Arc, Barrier};
    use std::thread;

    use super::*;

    static TEST_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new() -> io::Result<Self> {
            let sequence = TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "aether-cache-test-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path)?;
            Ok(Self { path })
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ignored = fs::remove_dir_all(&self.path);
        }
    }

    struct AlwaysFails;

    impl Read for AlwaysFails {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied"))
        }
    }

    fn store() -> Result<(TestDirectory, ArtifactStore), Box<dyn Error>> {
        let directory = TestDirectory::new()?;
        let store = ArtifactStore::new(directory.path.join("cache"))?;
        Ok((directory, store))
    }

    fn key() -> Result<CacheKey, Box<dyn Error>> {
        Ok(CacheKey::derive("test-artifact-v1", b"operation")?)
    }

    #[test]
    fn publishes_and_reopens_a_verified_payload() -> Result<(), Box<dyn Error>> {
        let (_directory, store) = store()?;
        let key = key()?;
        let payload = b"deterministic payload";

        let publication = store.publish(&key, &mut Cursor::new(payload))?;

        assert_eq!(publication.key(), &key);
        assert_eq!(publication.payload_bytes(), payload.len() as u64);
        assert_eq!(publication.state(), ArtifactPublicationState::Published);
        let mut artifact = store.open_verified(&key)?;
        assert_eq!(artifact.digest(), publication.digest());
        assert_eq!(artifact.payload_bytes(), payload.len() as u64);
        let mut decoded = Vec::new();
        artifact.read_to_end(&mut decoded)?;
        assert_eq!(decoded, payload);

        let entries: Vec<_> = fs::read_dir(store.shard_path(&key))?.collect::<Result<_, _>>()?;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path(), store.artifact_path(&key));
        Ok(())
    }

    #[test]
    fn identical_publication_reuses_the_verified_winner() -> Result<(), Box<dyn Error>> {
        let (_directory, store) = store()?;
        let key = key()?;
        let first = store.publish(&key, &mut Cursor::new(b"same bytes"))?;
        let second = store.publish(&key, &mut Cursor::new(b"same bytes"))?;

        assert_eq!(first.state(), ArtifactPublicationState::Published);
        assert_eq!(second.state(), ArtifactPublicationState::AlreadyPresent);
        assert_eq!(first.digest(), second.digest());
        Ok(())
    }

    #[test]
    fn collision_never_overwrites_existing_bytes() -> Result<(), Box<dyn Error>> {
        let (_directory, store) = store()?;
        let key = key()?;
        store.publish(&key, &mut Cursor::new(b"winner"))?;

        assert!(matches!(
            store.publish(&key, &mut Cursor::new(b"different")),
            Err(CacheWriteError::KeyCollision { .. })
        ));
        let mut artifact = store.open_verified(&key)?;
        let mut payload = Vec::new();
        artifact.read_to_end(&mut payload)?;
        assert_eq!(payload, b"winner");
        Ok(())
    }

    #[test]
    fn corruption_and_trailing_bytes_are_rejected() -> Result<(), Box<dyn Error>> {
        let (_directory, store) = store()?;
        let first_key = CacheKey::derive("test-artifact-v1", b"first")?;
        store.publish(&first_key, &mut Cursor::new(b"payload"))?;
        let mut file = OpenOptions::new()
            .write(true)
            .open(store.artifact_path(&first_key))?;
        file.seek(SeekFrom::Start(ARTIFACT_HEADER_BYTES as u64 + 1))?;
        file.write_all(b"X")?;
        file.sync_all()?;
        assert!(matches!(
            store.open_verified(&first_key),
            Err(CacheReadError::DigestMismatch { .. })
        ));

        let second_key = CacheKey::derive("test-artifact-v1", b"second")?;
        store.publish(&second_key, &mut Cursor::new(b"payload"))?;
        let file = OpenOptions::new()
            .write(true)
            .open(store.artifact_path(&second_key))?;
        file.set_len(ARTIFACT_HEADER_BYTES as u64 + 8)?;
        assert!(matches!(
            store.open_verified(&second_key),
            Err(CacheReadError::LengthMismatch { .. })
        ));
        Ok(())
    }

    #[test]
    fn malformed_headers_and_corrupt_existing_entries_remain_explicit() -> Result<(), Box<dyn Error>>
    {
        let (_directory, store) = store()?;
        let magic_key = CacheKey::derive("test-artifact-v1", b"magic")?;
        store.publish(&magic_key, &mut Cursor::new(b"payload"))?;
        let mut file = OpenOptions::new()
            .write(true)
            .open(store.artifact_path(&magic_key))?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(b"X")?;
        file.sync_all()?;
        assert!(matches!(
            store.open_verified(&magic_key),
            Err(CacheReadError::InvalidMagic)
        ));
        assert!(matches!(
            store.publish(&magic_key, &mut Cursor::new(b"payload")),
            Err(CacheWriteError::ExistingInvalid(
                CacheReadError::InvalidMagic
            ))
        ));

        let version_key = CacheKey::derive("test-artifact-v1", b"version")?;
        store.publish(&version_key, &mut Cursor::new(b"payload"))?;
        let mut file = OpenOptions::new()
            .write(true)
            .open(store.artifact_path(&version_key))?;
        file.seek(SeekFrom::Start(8))?;
        file.write_all(&2_u32.to_be_bytes())?;
        file.sync_all()?;
        assert!(matches!(
            store.open_verified(&version_key),
            Err(CacheReadError::UnsupportedVersion { version: 2 })
        ));

        let truncated_key = CacheKey::derive("test-artifact-v1", b"truncated")?;
        store.publish(&truncated_key, &mut Cursor::new(b"payload"))?;
        OpenOptions::new()
            .write(true)
            .open(store.artifact_path(&truncated_key))?
            .set_len(10)?;
        assert!(matches!(
            store.open_verified(&truncated_key),
            Err(CacheReadError::ReadHeader(error))
                if error.kind() == io::ErrorKind::UnexpectedEof
        ));
        Ok(())
    }

    #[test]
    fn source_failure_removes_the_temporary_artifact() -> Result<(), Box<dyn Error>> {
        let (_directory, store) = store()?;
        let key = key()?;

        assert!(matches!(
            store.publish(&key, &mut AlwaysFails),
            Err(CacheWriteError::ReadSource(error))
                if error.kind() == io::ErrorKind::PermissionDenied
        ));
        assert!(!store.artifact_path(&key).exists());
        let entries: Vec<_> = fs::read_dir(store.shard_path(&key))?.collect::<Result<_, _>>()?;
        assert!(entries.is_empty());
        Ok(())
    }

    #[test]
    fn concurrent_different_writers_publish_exactly_one_payload() -> Result<(), Box<dyn Error>> {
        let (_directory, store) = store()?;
        let store = Arc::new(store);
        let key = Arc::new(key()?);
        let barrier = Arc::new(Barrier::new(3));
        let mut workers = Vec::new();
        for payload in [b"first".as_slice(), b"second".as_slice()] {
            let worker_store = Arc::clone(&store);
            let worker_key = Arc::clone(&key);
            let worker_barrier = Arc::clone(&barrier);
            let payload = payload.to_vec();
            workers.push(thread::spawn(move || {
                worker_barrier.wait();
                match worker_store.publish(&worker_key, &mut Cursor::new(payload)) {
                    Ok(publication) => Ok(publication.state()),
                    Err(CacheWriteError::KeyCollision { .. }) => {
                        Ok(ArtifactPublicationState::AlreadyPresent)
                    }
                    Err(error) => Err(error.to_string()),
                }
            }));
        }
        barrier.wait();

        let mut published = 0;
        for worker in workers {
            if worker
                .join()
                .map_err(|_| io::Error::other("cache writer panicked"))?
                .map_err(io::Error::other)?
                == ArtifactPublicationState::Published
            {
                published += 1;
            }
        }
        assert_eq!(published, 1);
        let mut artifact = store.open_verified(&key)?;
        let mut bytes = Vec::new();
        artifact.read_to_end(&mut bytes)?;
        assert!(bytes == b"first" || bytes == b"second");
        let entries: Vec<_> = fs::read_dir(store.shard_path(&key))?.collect::<Result<_, _>>()?;
        assert_eq!(entries.len(), 1);
        Ok(())
    }

    #[test]
    fn rejects_empty_roots_and_non_regular_artifacts() -> Result<(), Box<dyn Error>> {
        assert!(matches!(
            ArtifactStore::new(PathBuf::new()),
            Err(CacheWriteError::InvalidRoot)
        ));
        let (_directory, store) = store()?;
        let key = key()?;
        fs::create_dir_all(store.artifact_path(&key))?;
        assert!(matches!(
            store.open_verified(&key),
            Err(CacheReadError::NotRegularFile)
        ));
        Ok(())
    }
}
