use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt::{Display, Formatter};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use aether_core::{Dimensions, PixelFlags, ScientificImage};

use crate::{F64PrimaryStreamWriter, FitsOutputProvenance, FitsWriteError, FitsWriteSummary};

const FILE_BUFFER_BYTES: usize = 64 * 1_024;
const MAX_TEMPORARY_NAME_ATTEMPTS: usize = 128;
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Failure while publishing a newly created FITS file atomically.
#[derive(Debug)]
pub enum AtomicFitsWriteError {
    /// The output path has no usable file name.
    InvalidOutputPath,
    /// Existing-target inspection failed for a reason other than absence.
    InspectTarget(io::Error),
    /// The destination already exists and was deliberately not modified.
    TargetExists,
    /// No temporary output could be created in the destination directory.
    CreateTemporary(io::Error),
    /// FITS stream encoding failed before publication.
    Encode(FitsWriteError),
    /// Buffered bytes could not be flushed before publication.
    Flush(io::Error),
    /// The completed temporary file could not be cloned for bounded readback.
    CloneTemporary(io::Error),
    /// Temporary file contents could not be synchronized before publication.
    SyncTemporary(io::Error),
    /// The complete temporary file could not be linked to the destination.
    Publish(io::Error),
    /// The destination is published, but its temporary hard link remains.
    CleanupAfterPublish {
        /// Temporary path that may require manual cleanup.
        temporary_path: PathBuf,
        /// Cleanup failure.
        source: io::Error,
    },
    /// The destination is published, but directory metadata could not be synced.
    DirectorySyncAfterPublish(io::Error),
}

impl AtomicFitsWriteError {
    /// Returns whether the destination is already complete and visible.
    ///
    /// Callers must not retry under a different name when this returns `true`;
    /// only cleanup or durability confirmation remains unresolved.
    #[must_use]
    pub const fn output_is_published(&self) -> bool {
        matches!(
            self,
            Self::CleanupAfterPublish { .. } | Self::DirectorySyncAfterPublish(_)
        )
    }
}

impl Display for AtomicFitsWriteError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidOutputPath => formatter.write_str("output path has no file name"),
            Self::InspectTarget(error) => write!(formatter, "cannot inspect output path: {error}"),
            Self::TargetExists => {
                formatter.write_str("output path already exists and was not modified")
            }
            Self::CreateTemporary(error) => {
                write!(formatter, "cannot create temporary FITS output: {error}")
            }
            Self::Encode(error) => Display::fmt(error, formatter),
            Self::Flush(error) => write!(formatter, "cannot flush temporary FITS output: {error}"),
            Self::CloneTemporary(error) => {
                write!(formatter, "cannot clone temporary FITS output: {error}")
            }
            Self::SyncTemporary(error) => {
                write!(
                    formatter,
                    "cannot synchronize temporary FITS output: {error}"
                )
            }
            Self::Publish(error) => write!(formatter, "cannot publish FITS output: {error}"),
            Self::CleanupAfterPublish {
                temporary_path,
                source,
            } => write!(
                formatter,
                "FITS output is published but temporary link {} could not be removed: {source}",
                temporary_path.display()
            ),
            Self::DirectorySyncAfterPublish(error) => write!(
                formatter,
                "FITS output is published but directory metadata could not be synchronized: {error}"
            ),
        }
    }
}

impl Error for AtomicFitsWriteError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InspectTarget(error)
            | Self::CreateTemporary(error)
            | Self::Flush(error)
            | Self::CloneTemporary(error)
            | Self::SyncTemporary(error)
            | Self::Publish(error)
            | Self::DirectorySyncAfterPublish(error) => Some(error),
            Self::Encode(error) => Some(error),
            Self::CleanupAfterPublish { source, .. } => Some(source),
            Self::InvalidOutputPath | Self::TargetExists => None,
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

/// Incremental binary64 FITS output staged for atomic create-new publication.
///
/// Dropping this value at any point removes its private temporary file. The
/// destination remains absent until [`CompletedAtomicFits::publish`] succeeds.
pub struct AtomicF64PrimaryStreamWriter {
    destination: PathBuf,
    parent: PathBuf,
    temporary_path: PathBuf,
    encoder: F64PrimaryStreamWriter<BufWriter<File>>,
    // Keep the guard after the encoder so the file is closed before abandoned
    // staging data is removed on platforms that forbid unlinking open files.
    guard: TemporaryPathGuard,
}

impl AtomicF64PrimaryStreamWriter {
    /// Creates a private incremental output and writes its primary header.
    ///
    /// # Errors
    ///
    /// Returns a path, collision, temporary-file, or FITS header error. An
    /// existing destination is never modified.
    pub fn create(path: &Path, dimensions: Dimensions) -> Result<Self, AtomicFitsWriteError> {
        Self::create_inner(path, dimensions, None)
    }

    /// Creates a private incremental output with validated provenance cards.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::create`].
    pub fn create_with_provenance(
        path: &Path,
        dimensions: Dimensions,
        provenance: &FitsOutputProvenance,
    ) -> Result<Self, AtomicFitsWriteError> {
        Self::create_inner(path, dimensions, Some(provenance))
    }

    fn create_inner(
        path: &Path,
        dimensions: Dimensions,
        provenance: Option<&FitsOutputProvenance>,
    ) -> Result<Self, AtomicFitsWriteError> {
        let (destination, parent, temporary_path, file, guard) = prepare_temporary(path)?;
        let writer = BufWriter::with_capacity(FILE_BUFFER_BYTES, file);
        let encoder = F64PrimaryStreamWriter::new_checksummed(writer, dimensions, provenance)
            .map_err(AtomicFitsWriteError::Encode)?;
        Ok(Self {
            destination,
            parent,
            temporary_path,
            encoder,
            guard,
        })
    }

    /// Appends one consecutive sample chunk in primary-image order.
    ///
    /// # Errors
    ///
    /// Returns a FITS encoding, accounting, invariant, or temporary I/O error.
    pub fn write_samples(
        &mut self,
        pixels: &[f64],
        flags: &[PixelFlags],
    ) -> Result<(), AtomicFitsWriteError> {
        self.encoder
            .write_samples(pixels, flags)
            .map_err(AtomicFitsWriteError::Encode)
    }

    /// Appends all samples in one image-shaped consecutive chunk.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::write_samples`].
    pub fn write_image_chunk(
        &mut self,
        image: &ScientificImage,
    ) -> Result<(), AtomicFitsWriteError> {
        self.encoder
            .write_image_chunk(image)
            .map_err(AtomicFitsWriteError::Encode)
    }

    /// Completes FITS padding while keeping the destination unpublished.
    ///
    /// The returned value may be read back for validation before the final
    /// atomic publication. Dropping it still removes the private temporary.
    ///
    /// # Errors
    ///
    /// Returns a sample-count, padding, or flush failure.
    pub fn finish(self) -> Result<CompletedAtomicFits, AtomicFitsWriteError> {
        let Self {
            destination,
            parent,
            temporary_path,
            encoder,
            guard,
        } = self;
        let (mut writer, summary) = encoder
            .finish_with_checksums()
            .map_err(AtomicFitsWriteError::Encode)?;
        writer.flush().map_err(AtomicFitsWriteError::Flush)?;
        Ok(CompletedAtomicFits {
            destination,
            parent,
            temporary_path,
            writer: Some(writer),
            guard,
            summary,
        })
    }
}

/// Complete private FITS stream awaiting validation and atomic publication.
pub struct CompletedAtomicFits {
    destination: PathBuf,
    parent: PathBuf,
    temporary_path: PathBuf,
    writer: Option<BufWriter<File>>,
    // Field order is deliberate: close the file handle before path cleanup.
    guard: TemporaryPathGuard,
    summary: FitsWriteSummary,
}

impl CompletedAtomicFits {
    /// Opens an independent handle to the complete unpublished bytes.
    ///
    /// This supports bounded readback and statistics without exposing a partial
    /// destination. The returned handle must not be used to modify the stream.
    ///
    /// # Errors
    ///
    /// Returns an operating-system clone failure.
    pub fn try_clone_for_readback(&self) -> Result<File, AtomicFitsWriteError> {
        let writer = self.writer.as_ref().ok_or_else(|| {
            AtomicFitsWriteError::CloneTemporary(io::Error::other(
                "temporary FITS writer is unavailable",
            ))
        })?;
        let mut file = writer
            .get_ref()
            .try_clone()
            .map_err(AtomicFitsWriteError::CloneTemporary)?;
        file.seek(SeekFrom::Start(0))
            .map_err(AtomicFitsWriteError::CloneTemporary)?;
        Ok(file)
    }

    /// Atomically publishes the already complete stream without overwriting.
    ///
    /// # Errors
    ///
    /// Returns synchronization, create-new publication, cleanup, or directory
    /// durability failures. See [`AtomicFitsWriteError::output_is_published`]
    /// to distinguish post-publication failures.
    pub fn publish(mut self) -> Result<FitsWriteSummary, AtomicFitsWriteError> {
        let writer = self.writer.take().ok_or_else(|| {
            AtomicFitsWriteError::SyncTemporary(io::Error::other(
                "temporary FITS writer is unavailable",
            ))
        })?;
        writer
            .get_ref()
            .sync_all()
            .map_err(AtomicFitsWriteError::SyncTemporary)?;
        drop(writer);

        match fs::hard_link(&self.temporary_path, &self.destination) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                return Err(AtomicFitsWriteError::TargetExists);
            }
            Err(error) => return Err(AtomicFitsWriteError::Publish(error)),
        }

        if let Err(source) = fs::remove_file(&self.temporary_path) {
            self.guard.disarm();
            return Err(AtomicFitsWriteError::CleanupAfterPublish {
                temporary_path: self.temporary_path.clone(),
                source,
            });
        }
        self.guard.disarm();
        sync_directory_after_publish(&self.parent)?;
        Ok(self.summary)
    }
}

/// Writes and atomically publishes a new binary64 FITS file without overwriting.
///
/// A unique temporary file is created in the destination directory, encoded,
/// flushed, and synchronized. A same-filesystem hard link then makes the complete
/// bytes visible at `path` only if that name is still absent. The temporary link
/// is removed afterward. Readers therefore observe either no destination or one
/// complete FITS stream, never a partially written destination.
///
/// This function intentionally implements create-new semantics. Replacing an
/// existing scientific artifact requires a separate, explicitly versioned
/// policy. On Unix, directory metadata is synchronized after publication; Rust's
/// portable standard library does not expose equivalent directory handles on all
/// supported systems.
///
/// # Errors
///
/// Returns [`AtomicFitsWriteError::TargetExists`] without modifying an existing
/// path. Errors whose [`AtomicFitsWriteError::output_is_published`] method returns
/// `false` leave no destination. Post-publication cleanup or directory-sync
/// errors explicitly report that the complete destination is already visible.
pub fn write_f64_primary_atomic_new(
    path: &Path,
    image: &ScientificImage,
) -> Result<FitsWriteSummary, AtomicFitsWriteError> {
    write_f64_primary_atomic_new_inner(path, image, None)
}

/// Writes and atomically publishes a new binary64 FITS file with provenance.
///
/// Publication and durability guarantees match
/// [`write_f64_primary_atomic_new`]. The validated provenance cards are part of
/// the temporary file before its complete byte stream becomes visible at
/// `path`.
///
/// # Errors
///
/// Returns the same publication and encoding errors as
/// [`write_f64_primary_atomic_new`].
pub fn write_f64_primary_atomic_new_with_provenance(
    path: &Path,
    image: &ScientificImage,
    provenance: &FitsOutputProvenance,
) -> Result<FitsWriteSummary, AtomicFitsWriteError> {
    write_f64_primary_atomic_new_inner(path, image, Some(provenance))
}

fn write_f64_primary_atomic_new_inner(
    path: &Path,
    image: &ScientificImage,
    provenance: Option<&FitsOutputProvenance>,
) -> Result<FitsWriteSummary, AtomicFitsWriteError> {
    let mut staged = match provenance {
        Some(provenance) => AtomicF64PrimaryStreamWriter::create_with_provenance(
            path,
            image.dimensions(),
            provenance,
        )?,
        None => AtomicF64PrimaryStreamWriter::create(path, image.dimensions())?,
    };
    staged.write_image_chunk(image)?;
    staged.finish()?.publish()
}

fn prepare_temporary(
    path: &Path,
) -> Result<(PathBuf, PathBuf, PathBuf, File, TemporaryPathGuard), AtomicFitsWriteError> {
    let file_name = path
        .file_name()
        .filter(|value| !value.is_empty())
        .ok_or(AtomicFitsWriteError::InvalidOutputPath)?;
    let parent = path
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    match fs::symlink_metadata(path) {
        Ok(_) => return Err(AtomicFitsWriteError::TargetExists),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(AtomicFitsWriteError::InspectTarget(error)),
    }

    let (temporary_path, file) = create_temporary(parent, file_name)?;
    let guard = TemporaryPathGuard::new(temporary_path.clone());
    Ok((
        path.to_path_buf(),
        parent.to_path_buf(),
        temporary_path,
        file,
        guard,
    ))
}

fn create_temporary(
    parent: &Path,
    file_name: &OsStr,
) -> Result<(PathBuf, File), AtomicFitsWriteError> {
    let mut last_collision = None;
    for _ in 0..MAX_TEMPORARY_NAME_ATTEMPTS {
        let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let mut temporary_name = OsString::from(".");
        temporary_name.push(file_name);
        temporary_name.push(format!(
            ".aetherstack-{}-{sequence}.tmp",
            std::process::id()
        ));
        let temporary_path = parent.join(temporary_name);
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&temporary_path)
        {
            Ok(file) => return Ok((temporary_path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                last_collision = Some(error);
            }
            Err(error) => return Err(AtomicFitsWriteError::CreateTemporary(error)),
        }
    }

    Err(AtomicFitsWriteError::CreateTemporary(
        last_collision.unwrap_or_else(|| {
            io::Error::new(
                io::ErrorKind::AlreadyExists,
                "temporary FITS name attempts exhausted",
            )
        }),
    ))
}

#[cfg(unix)]
fn sync_directory_after_publish(parent: &Path) -> Result<(), AtomicFitsWriteError> {
    let directory = File::open(parent).map_err(AtomicFitsWriteError::DirectorySyncAfterPublish)?;
    directory
        .sync_all()
        .map_err(AtomicFitsWriteError::DirectorySyncAfterPublish)
}

#[cfg(not(unix))]
fn sync_directory_after_publish(_parent: &Path) -> Result<(), AtomicFitsWriteError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::io::Read;
    use std::sync::{Arc, Barrier};
    use std::thread;

    use aether_core::Dimensions;

    use crate::{
        BLOCK_SIZE, HeaderReadOptions, PrimaryImageReader, SampleStatus, checksum_aligned,
        is_negative_zero,
    };

    use super::*;

    static TEST_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new() -> io::Result<Self> {
            let sequence = TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "aether-fits-atomic-{}-{sequence}",
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

    fn image(value: f64) -> Result<ScientificImage, Box<dyn StdError>> {
        let dimensions = Dimensions::new(2, 1, 1)?;
        Ok(ScientificImage::from_pixels(
            dimensions,
            vec![value, value + 1.0],
        )?)
    }

    fn read_values(path: &Path) -> Result<[f64; 2], Box<dyn StdError>> {
        let file = File::open(path)?;
        let mut reader = PrimaryImageReader::open(file, HeaderReadOptions::default())?;
        let mut values = [0.0; 2];
        let mut statuses = [SampleStatus::Undefined; 2];
        reader.read_physical_samples(0, &mut values, &mut statuses)?;
        assert_eq!(statuses, [SampleStatus::Valid; 2]);
        Ok(values)
    }

    #[test]
    fn publishes_one_complete_file_and_removes_temporary_link() -> Result<(), Box<dyn StdError>> {
        let directory = TestDirectory::new()?;
        let path = directory.path.join("result.fits");

        let summary = write_f64_primary_atomic_new(&path, &image(10.0)?)?;

        assert_eq!(summary.samples_written(), 2);
        assert_eq!(
            read_values(&path)?.map(f64::to_bits),
            [10.0, 11.0].map(f64::to_bits)
        );
        let entries: Vec<_> = fs::read_dir(&directory.path)?.collect::<Result<_, _>>()?;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path(), path);
        Ok(())
    }

    #[test]
    fn staged_stream_is_readable_but_private_until_publication() -> Result<(), Box<dyn StdError>> {
        let directory = TestDirectory::new()?;
        let path = directory.path.join("result.fits");
        let dimensions = Dimensions::new(2, 1, 1)?;
        let mut staged = AtomicF64PrimaryStreamWriter::create(&path, dimensions)?;

        assert!(!path.exists());
        staged.write_samples(&[10.0], &[PixelFlags::CLEAR])?;
        staged.write_samples(&[11.0], &[PixelFlags::CLEAR])?;
        let completed = staged.finish()?;
        assert!(!path.exists());

        let readback = completed.try_clone_for_readback()?;
        let mut reader = PrimaryImageReader::open(readback, HeaderReadOptions::default())?;
        let mut values = [0.0; 2];
        let mut statuses = [SampleStatus::Undefined; 2];
        reader.read_physical_samples(0, &mut values, &mut statuses)?;
        assert_eq!(values.map(f64::to_bits), [10.0, 11.0].map(f64::to_bits));
        assert_eq!(statuses, [SampleStatus::Valid; 2]);

        let summary = completed.publish()?;
        assert_eq!(summary.samples_written(), 2);
        assert_eq!(
            read_values(&path)?.map(f64::to_bits),
            [10.0, 11.0].map(f64::to_bits)
        );
        let entries: Vec<_> = fs::read_dir(&directory.path)?.collect::<Result<_, _>>()?;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path(), path);
        Ok(())
    }

    #[test]
    fn abandoning_a_staged_stream_removes_its_private_file() -> Result<(), Box<dyn StdError>> {
        let directory = TestDirectory::new()?;
        let path = directory.path.join("result.fits");
        let dimensions = Dimensions::new(2, 1, 1)?;

        {
            let mut staged = AtomicF64PrimaryStreamWriter::create(&path, dimensions)?;
            staged.write_samples(&[10.0], &[PixelFlags::CLEAR])?;
        }

        assert!(!path.exists());
        let entries: Vec<_> = fs::read_dir(&directory.path)?.collect::<Result<_, _>>()?;
        assert!(entries.is_empty());
        Ok(())
    }

    #[test]
    fn publishes_provenance_as_part_of_the_atomic_file() -> Result<(), Box<dyn StdError>> {
        let directory = TestDirectory::new()?;
        let path = directory.path.join("result.fits");
        let provenance =
            FitsOutputProvenance::new("a".repeat(64), "b".repeat(64), "strict-mean-v1", 2)?;

        write_f64_primary_atomic_new_with_provenance(&path, &image(10.0)?, &provenance)?;
        let reader = PrimaryImageReader::open(File::open(&path)?, HeaderReadOptions::default())?;

        assert!(reader.report().is_conformant());
        assert_eq!(
            reader.report().header().string("AETHMAN"),
            Some(provenance.manifest_sha256())
        );
        assert_eq!(
            reader.report().header().string("AETHALG"),
            Some(provenance.algorithm_id())
        );
        Ok(())
    }

    #[test]
    fn publishes_valid_datasum_and_checksum_cards() -> Result<(), Box<dyn StdError>> {
        let directory = TestDirectory::new()?;
        let path = directory.path.join("result.fits");

        let summary = write_f64_primary_atomic_new(&path, &image(10.0)?)?;
        let bytes = fs::read(&path)?;
        let data = bytes
            .get(BLOCK_SIZE..)
            .ok_or_else(|| io::Error::other("test FITS data unit is missing"))?;
        let data_checksum = checksum_aligned(data)?;
        let hdu_checksum = checksum_aligned(&bytes)?;
        let reader = PrimaryImageReader::open(File::open(&path)?, HeaderReadOptions::default())?;
        let header = reader.report().header();

        assert_eq!(summary.data_checksum(), Some(data_checksum));
        assert_eq!(
            header.string("DATASUM"),
            Some(data_checksum.to_string().as_str())
        );
        let encoded = summary
            .encoded_checksum()
            .ok_or_else(|| io::Error::other("write summary omitted CHECKSUM"))?;
        let encoded: String = encoded.into_iter().map(char::from).collect();
        assert_eq!(header.string("CHECKSUM"), Some(encoded.as_str()));
        assert!(is_negative_zero(hdu_checksum));

        let mut corrupted = bytes;
        let sample = corrupted
            .get_mut(BLOCK_SIZE)
            .ok_or_else(|| io::Error::other("test sample is missing"))?;
        *sample ^= 1;
        assert!(!is_negative_zero(checksum_aligned(&corrupted)?));
        Ok(())
    }

    #[test]
    fn refuses_to_modify_an_existing_destination() -> Result<(), Box<dyn StdError>> {
        let directory = TestDirectory::new()?;
        let path = directory.path.join("result.fits");
        fs::write(&path, b"existing")?;

        assert!(matches!(
            write_f64_primary_atomic_new(&path, &image(20.0)?),
            Err(AtomicFitsWriteError::TargetExists)
        ));
        let mut bytes = Vec::new();
        File::open(&path)?.read_to_end(&mut bytes)?;
        assert_eq!(bytes, b"existing");
        Ok(())
    }

    #[test]
    fn concurrent_publishers_cannot_overwrite_each_other() -> Result<(), Box<dyn StdError>> {
        let directory = TestDirectory::new()?;
        let path = Arc::new(directory.path.join("result.fits"));
        let barrier = Arc::new(Barrier::new(3));
        let mut workers = Vec::new();
        for value in [100.0, 200.0] {
            let worker_path = Arc::clone(&path);
            let worker_barrier = Arc::clone(&barrier);
            workers.push(thread::spawn(move || {
                let image = image(value).map_err(|error| error.to_string())?;
                worker_barrier.wait();
                match write_f64_primary_atomic_new(&worker_path, &image) {
                    Ok(_) => Ok(true),
                    Err(AtomicFitsWriteError::TargetExists) => Ok(false),
                    Err(error) => Err(error.to_string()),
                }
            }));
        }
        barrier.wait();

        let mut published = 0;
        for worker in workers {
            if worker
                .join()
                .map_err(|_| io::Error::other("atomic writer worker panicked"))?
                .map_err(io::Error::other)?
            {
                published += 1;
            }
        }
        assert_eq!(published, 1);
        let values = read_values(&path)?;
        let first = values[0].to_bits();
        assert!(first == 100.0_f64.to_bits() || first == 200.0_f64.to_bits());
        assert_eq!(values[1].to_bits(), (values[0] + 1.0).to_bits());
        let entries: Vec<_> = fs::read_dir(&directory.path)?.collect::<Result<_, _>>()?;
        assert_eq!(entries.len(), 1);
        Ok(())
    }

    #[test]
    fn rejects_a_path_without_a_file_name() -> Result<(), Box<dyn StdError>> {
        assert!(matches!(
            write_f64_primary_atomic_new(Path::new(""), &image(1.0)?),
            Err(AtomicFitsWriteError::InvalidOutputPath)
        ));
        Ok(())
    }

    #[test]
    fn prepublication_errors_are_not_reported_as_published() {
        let error = AtomicFitsWriteError::TargetExists;
        assert!(!error.output_is_published());
        let published = AtomicFitsWriteError::DirectorySyncAfterPublish(io::Error::other("sync"));
        assert!(published.output_is_published());
    }
}
