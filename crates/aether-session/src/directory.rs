use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

use aether_fits::{HeaderReadOptions, ValidationMode};

use crate::{
    ClassificationPolicy, FINGERPRINT_BUFFER_BYTES, ManifestFile, ManifestGenerationError,
    SessionManifest, SourceAnalysisError, analyze_fits_source, generate_manifest,
};

/// Default maximum number of entries accepted from one directory.
pub const DEFAULT_MAX_ENTRIES_PER_DIRECTORY: usize = 100_000;
/// Default maximum number of filesystem entries visited in one scan.
pub const DEFAULT_MAX_TOTAL_ENTRIES: usize = 500_000;
/// Default maximum number of FITS sources analyzed in one scan.
pub const DEFAULT_MAX_FITS_FILES: usize = 100_000;
/// Default maximum number of retained traversal and source failures.
pub const DEFAULT_MAX_FAILURES: usize = 10_000;
/// Default maximum directory depth below the session root.
pub const DEFAULT_MAX_DEPTH: usize = 64;
/// Default maximum size of one FITS source (one tebibyte).
pub const DEFAULT_MAX_SOURCE_BYTES: u64 = 1_u64 << 40;
/// Default maximum source bytes hashed by one scan (64 tebibytes).
pub const DEFAULT_MAX_TOTAL_SOURCE_BYTES: u64 = 64_u64 << 40;

/// Explicit resource limits for directory-to-manifest ingestion.
///
/// These limits bound traversal queues, retained records, failure details, and
/// hashing work. Reaching a structural limit aborts the scan instead of
/// returning a silently truncated manifest.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DirectoryScanLimits {
    /// Maximum entries accepted from a single directory.
    pub max_entries_per_directory: usize,
    /// Maximum filesystem entries visited across the complete scan.
    pub max_total_entries: usize,
    /// Maximum FITS files considered for analysis.
    pub max_fits_files: usize,
    /// Maximum failures retained before the scan aborts.
    pub max_failures: usize,
    /// Maximum child-directory depth, where files in the root have depth zero.
    pub max_depth: usize,
    /// Maximum byte length accepted for one source.
    pub max_source_bytes: u64,
    /// Maximum aggregate bytes admitted for hashing.
    pub max_total_source_bytes: u64,
}

impl Default for DirectoryScanLimits {
    fn default() -> Self {
        Self {
            max_entries_per_directory: DEFAULT_MAX_ENTRIES_PER_DIRECTORY,
            max_total_entries: DEFAULT_MAX_TOTAL_ENTRIES,
            max_fits_files: DEFAULT_MAX_FITS_FILES,
            max_failures: DEFAULT_MAX_FAILURES,
            max_depth: DEFAULT_MAX_DEPTH,
            max_source_bytes: DEFAULT_MAX_SOURCE_BYTES,
            max_total_source_bytes: DEFAULT_MAX_TOTAL_SOURCE_BYTES,
        }
    }
}

/// Policies and limits used to build a manifest from a directory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DirectoryManifestOptions {
    /// FITS primary-header allocation and parsing limits.
    pub header: HeaderReadOptions,
    /// FITS conformance policy persisted in the generated manifest.
    pub fits_validation_mode: ValidationMode,
    /// Evidence policy used to resolve frame classifications.
    pub classification_policy: ClassificationPolicy,
    /// Filesystem and hashing limits.
    pub limits: DirectoryScanLimits,
}

impl Default for DirectoryManifestOptions {
    fn default() -> Self {
        Self {
            header: HeaderReadOptions::default(),
            fits_validation_mode: ValidationMode::Strict,
            classification_policy: ClassificationPolicy::RequireAgreement,
            limits: DirectoryScanLimits::default(),
        }
    }
}

/// Stable category for one recoverable directory-ingestion failure.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum DirectoryFailureCode {
    /// A child directory could not be opened for enumeration.
    ReadDirectory,
    /// One directory entry could not be read.
    ReadEntry,
    /// The type of one directory entry could not be read.
    ReadFileType,
    /// A relative source path cannot be represented as portable UTF-8.
    NonPortablePath,
    /// A source file could not be opened.
    OpenFile,
    /// Open-file metadata could not be read.
    ReadFileMetadata,
    /// A source exceeded the configured per-file byte limit.
    SourceTooLarge,
    /// FITS analysis, fingerprinting, or manifest-file validation failed.
    AnalyzeSource,
}

impl Display for DirectoryFailureCode {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::ReadDirectory => "read_directory",
            Self::ReadEntry => "read_entry",
            Self::ReadFileType => "read_file_type",
            Self::NonPortablePath => "non_portable_path",
            Self::OpenFile => "open_file",
            Self::ReadFileMetadata => "read_file_metadata",
            Self::SourceTooLarge => "source_too_large",
            Self::AnalyzeSource => "analyze_source",
        };
        formatter.write_str(value)
    }
}

/// Detailed reason for a recoverable directory-ingestion failure.
#[derive(Debug)]
pub enum DirectoryFailureReason {
    /// A directory could not be opened.
    ReadDirectory(io::Error),
    /// An entry returned by a directory iterator could not be read.
    ReadEntry(io::Error),
    /// Entry type inspection failed.
    ReadFileType(io::Error),
    /// The relative path was not portable UTF-8.
    NonPortablePath,
    /// Opening a FITS source failed.
    OpenFile(io::Error),
    /// Metadata for an already opened source could not be read.
    ReadFileMetadata(io::Error),
    /// The source exceeded the per-file byte limit.
    SourceTooLarge {
        /// Observed byte length.
        actual: u64,
        /// Configured maximum byte length.
        limit: u64,
    },
    /// Parsing, validation, hashing, or source verification failed.
    AnalyzeSource(SourceAnalysisError),
}

impl DirectoryFailureReason {
    /// Stable machine-readable category for this failure.
    #[must_use]
    pub const fn code(&self) -> DirectoryFailureCode {
        match self {
            Self::ReadDirectory(_) => DirectoryFailureCode::ReadDirectory,
            Self::ReadEntry(_) => DirectoryFailureCode::ReadEntry,
            Self::ReadFileType(_) => DirectoryFailureCode::ReadFileType,
            Self::NonPortablePath => DirectoryFailureCode::NonPortablePath,
            Self::OpenFile(_) => DirectoryFailureCode::OpenFile,
            Self::ReadFileMetadata(_) => DirectoryFailureCode::ReadFileMetadata,
            Self::SourceTooLarge { .. } => DirectoryFailureCode::SourceTooLarge,
            Self::AnalyzeSource(_) => DirectoryFailureCode::AnalyzeSource,
        }
    }
}

impl Display for DirectoryFailureReason {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReadDirectory(error) => write!(formatter, "cannot read directory: {error}"),
            Self::ReadEntry(error) => write!(formatter, "cannot read directory entry: {error}"),
            Self::ReadFileType(error) => write!(formatter, "cannot read entry type: {error}"),
            Self::NonPortablePath => formatter.write_str("relative path is not portable UTF-8"),
            Self::OpenFile(error) => write!(formatter, "cannot open source: {error}"),
            Self::ReadFileMetadata(error) => {
                write!(formatter, "cannot read source metadata: {error}")
            }
            Self::SourceTooLarge { actual, limit } => {
                write!(formatter, "source has {actual} bytes; limit is {limit}")
            }
            Self::AnalyzeSource(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for DirectoryFailureReason {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ReadDirectory(error)
            | Self::ReadEntry(error)
            | Self::ReadFileType(error)
            | Self::OpenFile(error)
            | Self::ReadFileMetadata(error) => Some(error),
            Self::AnalyzeSource(error) => Some(error),
            Self::NonPortablePath | Self::SourceTooLarge { .. } => None,
        }
    }
}

/// One recoverable failure associated with a path below the session root.
#[derive(Debug)]
pub struct DirectoryScanFailure {
    relative_path: PathBuf,
    reason: DirectoryFailureReason,
}

impl DirectoryScanFailure {
    fn new(relative_path: PathBuf, reason: DirectoryFailureReason) -> Self {
        Self {
            relative_path,
            reason,
        }
    }

    /// Path relative to the session root; it may be non-UTF-8.
    #[must_use]
    pub fn relative_path(&self) -> &Path {
        &self.relative_path
    }

    /// Typed failure reason.
    #[must_use]
    pub const fn reason(&self) -> &DirectoryFailureReason {
        &self.reason
    }

    /// Stable category of the failure.
    #[must_use]
    pub const fn code(&self) -> DirectoryFailureCode {
        self.reason.code()
    }
}

impl Display for DirectoryScanFailure {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}: {}",
            self.relative_path.display(),
            self.reason
        )
    }
}

impl Error for DirectoryScanFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.reason.source()
    }
}

/// Completed directory scan and its deterministic session manifest.
#[derive(Debug)]
pub struct DirectoryManifestReport {
    manifest: SessionManifest,
    failures: Vec<DirectoryScanFailure>,
    unassigned_sources: Vec<String>,
    entries_visited: usize,
    fits_files_considered: usize,
    total_source_bytes: u64,
    skipped_symlinks: usize,
    skipped_non_fits_files: usize,
    skipped_special_entries: usize,
}

impl DirectoryManifestReport {
    /// Generated manifest containing every successfully analyzed source.
    #[must_use]
    pub const fn manifest(&self) -> &SessionManifest {
        &self.manifest
    }

    /// Recoverable failures, sorted deterministically by relative path and code.
    #[must_use]
    pub fn failures(&self) -> &[DirectoryScanFailure] {
        &self.failures
    }

    /// Successfully analyzed sources that were not assigned to an exact group.
    #[must_use]
    pub fn unassigned_sources(&self) -> &[String] {
        &self.unassigned_sources
    }

    /// Number of filesystem entries visited below the root.
    #[must_use]
    pub const fn entries_visited(&self) -> usize {
        self.entries_visited
    }

    /// Number of extension-matched FITS files considered.
    #[must_use]
    pub const fn fits_files_considered(&self) -> usize {
        self.fits_files_considered
    }

    /// Aggregate size of sources admitted for hashing.
    #[must_use]
    pub const fn total_source_bytes(&self) -> u64 {
        self.total_source_bytes
    }

    /// Number of symbolic links deliberately not followed.
    #[must_use]
    pub const fn skipped_symlinks(&self) -> usize {
        self.skipped_symlinks
    }

    /// Number of regular files ignored because their extension was not FITS.
    #[must_use]
    pub const fn skipped_non_fits_files(&self) -> usize {
        self.skipped_non_fits_files
    }

    /// Number of non-file, non-directory, non-symlink entries ignored.
    #[must_use]
    pub const fn skipped_special_entries(&self) -> usize {
        self.skipped_special_entries
    }
}

/// Fatal error that prevents a complete, non-truncated directory scan.
#[derive(Debug)]
pub enum DirectoryManifestError {
    /// One configured limit is zero where a positive bound is required.
    InvalidLimit(&'static str),
    /// Metadata for the requested root could not be read.
    RootMetadata(io::Error),
    /// The requested root is not a physical directory.
    RootNotDirectory,
    /// The root directory itself could not be enumerated.
    RootRead(io::Error),
    /// A directory exceeded its entry limit.
    DirectoryEntryLimitExceeded {
        /// Relative directory path.
        relative_path: PathBuf,
        /// Configured maximum entries in one directory.
        limit: usize,
    },
    /// The complete scan exceeded its entry limit.
    TotalEntryLimitExceeded {
        /// Configured maximum visited entries.
        limit: usize,
    },
    /// A directory was found beyond the configured depth.
    DepthLimitExceeded {
        /// Relative directory path.
        relative_path: PathBuf,
        /// Configured maximum depth.
        limit: usize,
    },
    /// The scan found more FITS files than permitted.
    FitsFileLimitExceeded {
        /// Configured maximum FITS file count.
        limit: usize,
    },
    /// Recoverable failures exceeded their retention bound.
    FailureLimitExceeded {
        /// Configured maximum retained failures.
        limit: usize,
    },
    /// Admitted sources exceeded the aggregate hashing budget.
    TotalSourceBytesLimitExceeded {
        /// Configured aggregate byte limit.
        limit: u64,
    },
    /// Successfully analyzed records could not form a valid manifest.
    Generate(ManifestGenerationError),
}

impl Display for DirectoryManifestError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidLimit(name) => write!(formatter, "{name} must be greater than zero"),
            Self::RootMetadata(error) => write!(formatter, "cannot inspect session root: {error}"),
            Self::RootNotDirectory => {
                formatter.write_str("session root must be a physical directory")
            }
            Self::RootRead(error) => write!(formatter, "cannot read session root: {error}"),
            Self::DirectoryEntryLimitExceeded {
                relative_path,
                limit,
            } => write!(
                formatter,
                "directory {} exceeds the {limit}-entry limit",
                relative_path.display()
            ),
            Self::TotalEntryLimitExceeded { limit } => {
                write!(formatter, "directory scan exceeds the {limit}-entry limit")
            }
            Self::DepthLimitExceeded {
                relative_path,
                limit,
            } => write!(
                formatter,
                "directory {} exceeds the depth limit of {limit}",
                relative_path.display()
            ),
            Self::FitsFileLimitExceeded { limit } => {
                write!(
                    formatter,
                    "directory scan exceeds the {limit}-FITS-file limit"
                )
            }
            Self::FailureLimitExceeded { limit } => {
                write!(
                    formatter,
                    "directory scan exceeds the {limit}-failure limit"
                )
            }
            Self::TotalSourceBytesLimitExceeded { limit } => write!(
                formatter,
                "directory scan exceeds the aggregate source limit of {limit} bytes"
            ),
            Self::Generate(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for DirectoryManifestError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::RootMetadata(error) | Self::RootRead(error) => Some(error),
            Self::Generate(error) => Some(error),
            Self::InvalidLimit(_)
            | Self::RootNotDirectory
            | Self::DirectoryEntryLimitExceeded { .. }
            | Self::TotalEntryLimitExceeded { .. }
            | Self::DepthLimitExceeded { .. }
            | Self::FitsFileLimitExceeded { .. }
            | Self::FailureLimitExceeded { .. }
            | Self::TotalSourceBytesLimitExceeded { .. } => None,
        }
    }
}

#[derive(Debug)]
struct PendingEntry {
    path: PathBuf,
    file_type: fs::FileType,
    depth: usize,
}

#[derive(Default)]
struct ScanState {
    files: Vec<ManifestFile>,
    failures: Vec<DirectoryScanFailure>,
    entries_visited: usize,
    fits_files_considered: usize,
    total_source_bytes: u64,
    skipped_symlinks: usize,
    skipped_non_fits_files: usize,
    skipped_special_entries: usize,
}

/// Pins reads to the length observed immediately after opening the file.
///
/// This closes the gap between the directory byte budget and the bytes seen by
/// the fingerprint pass. A source that grows or shrinks concurrently becomes an
/// I/O failure instead of producing a digest outside the admitted budget.
struct FixedLengthReader<R> {
    inner: R,
    expected_length: u64,
    position: u64,
}

impl<R> FixedLengthReader<R> {
    const fn new(inner: R, expected_length: u64) -> Self {
        Self {
            inner,
            expected_length,
            position: 0,
        }
    }
}

impl<R: Read> Read for FixedLengthReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let remaining = self.expected_length.saturating_sub(self.position);
        if remaining == 0 {
            let mut probe = [0_u8; 1];
            return match self.inner.read(&mut probe) {
                Ok(0) => Ok(0),
                Ok(_) => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "source grew after its length was admitted",
                )),
                Err(error) => Err(error),
            };
        }
        let limit = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(buffer.len());
        let read = self.inner.read(&mut buffer[..limit])?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "source shrank after its length was admitted",
            ));
        }
        let read_u64 = u64::try_from(read).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "read length exceeds 64 bits")
        })?;
        self.position = self.position.checked_add(read_u64).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "source position overflow")
        })?;
        Ok(read)
    }
}

impl<R: Seek> Seek for FixedLengthReader<R> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let next = self.inner.seek(position)?;
        if next > self.expected_length {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek exceeds admitted source length",
            ));
        }
        self.position = next;
        Ok(next)
    }
}

/// Builds a deterministic manifest from a bounded, non-symlinked directory walk.
///
/// Regular files ending in `.fits`, `.fit`, or `.fts` (case-insensitive) are
/// analyzed one at a time. Symbolic links are counted but never followed.
/// Recoverable traversal and source failures are retained in the report; hard
/// resource limits abort instead of yielding a partial manifest.
///
/// # Errors
///
/// Returns a fatal error when the root is invalid, a configured bound is
/// reached, or successful records violate a manifest invariant.
pub fn generate_manifest_from_directory(
    root: &Path,
    options: DirectoryManifestOptions,
) -> Result<DirectoryManifestReport, DirectoryManifestError> {
    validate_options(options)?;
    let root_metadata = fs::symlink_metadata(root).map_err(DirectoryManifestError::RootMetadata)?;
    if !root_metadata.file_type().is_dir() {
        return Err(DirectoryManifestError::RootNotDirectory);
    }

    let root_entries = fs::read_dir(root).map_err(DirectoryManifestError::RootRead)?;
    let mut state = ScanState::default();
    let mut pending = read_children(root, Path::new(""), 0, root_entries, options, &mut state)?;

    while let Some(entry) = pending.pop() {
        if entry.file_type.is_symlink() {
            state.skipped_symlinks += 1;
        } else if entry.file_type.is_dir() {
            if entry.depth > options.limits.max_depth {
                return Err(DirectoryManifestError::DepthLimitExceeded {
                    relative_path: relative_path(root, &entry.path),
                    limit: options.limits.max_depth,
                });
            }
            let relative = relative_path(root, &entry.path);
            match fs::read_dir(&entry.path) {
                Ok(entries) => {
                    let mut children =
                        read_children(root, &relative, entry.depth, entries, options, &mut state)?;
                    pending.append(&mut children);
                }
                Err(error) => push_failure(
                    &mut state,
                    options.limits.max_failures,
                    DirectoryScanFailure::new(
                        relative,
                        DirectoryFailureReason::ReadDirectory(error),
                    ),
                )?,
            }
        } else if entry.file_type.is_file() {
            if is_fits_path(&entry.path) {
                analyze_file(root, &entry.path, options, &mut state)?;
            } else {
                state.skipped_non_fits_files += 1;
            }
        } else {
            state.skipped_special_entries += 1;
        }
    }

    let manifest = generate_manifest(
        options.fits_validation_mode,
        options.classification_policy,
        state.files,
    )
    .map_err(DirectoryManifestError::Generate)?;
    let assigned: BTreeSet<&str> = manifest
        .groups()
        .iter()
        .flat_map(|group| group.files().iter().map(String::as_str))
        .collect();
    let unassigned_sources = manifest
        .files()
        .iter()
        .filter(|file| !assigned.contains(file.relative_path()))
        .map(|file| file.relative_path().to_owned())
        .collect();
    state.failures.sort_by(|left, right| {
        left.relative_path
            .cmp(&right.relative_path)
            .then_with(|| left.code().cmp(&right.code()))
    });

    Ok(DirectoryManifestReport {
        manifest,
        failures: state.failures,
        unassigned_sources,
        entries_visited: state.entries_visited,
        fits_files_considered: state.fits_files_considered,
        total_source_bytes: state.total_source_bytes,
        skipped_symlinks: state.skipped_symlinks,
        skipped_non_fits_files: state.skipped_non_fits_files,
        skipped_special_entries: state.skipped_special_entries,
    })
}

fn validate_options(options: DirectoryManifestOptions) -> Result<(), DirectoryManifestError> {
    let limits = options.limits;
    for (name, value) in [
        (
            "max_entries_per_directory",
            limits.max_entries_per_directory,
        ),
        ("max_total_entries", limits.max_total_entries),
        ("max_fits_files", limits.max_fits_files),
        ("max_failures", limits.max_failures),
        ("max_header_blocks", options.header.max_blocks),
    ] {
        if value == 0 {
            return Err(DirectoryManifestError::InvalidLimit(name));
        }
    }
    if limits.max_source_bytes == 0 {
        return Err(DirectoryManifestError::InvalidLimit("max_source_bytes"));
    }
    if limits.max_total_source_bytes == 0 {
        return Err(DirectoryManifestError::InvalidLimit(
            "max_total_source_bytes",
        ));
    }
    Ok(())
}

fn read_children(
    root: &Path,
    relative_directory: &Path,
    parent_depth: usize,
    entries: fs::ReadDir,
    options: DirectoryManifestOptions,
    state: &mut ScanState,
) -> Result<Vec<PendingEntry>, DirectoryManifestError> {
    let mut children = Vec::new();
    for entry in entries {
        if children.len() >= options.limits.max_entries_per_directory {
            return Err(DirectoryManifestError::DirectoryEntryLimitExceeded {
                relative_path: relative_directory.to_owned(),
                limit: options.limits.max_entries_per_directory,
            });
        }
        state.entries_visited = state.entries_visited.checked_add(1).ok_or(
            DirectoryManifestError::TotalEntryLimitExceeded {
                limit: options.limits.max_total_entries,
            },
        )?;
        if state.entries_visited > options.limits.max_total_entries {
            return Err(DirectoryManifestError::TotalEntryLimitExceeded {
                limit: options.limits.max_total_entries,
            });
        }

        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                push_failure(
                    state,
                    options.limits.max_failures,
                    DirectoryScanFailure::new(
                        relative_directory.to_owned(),
                        DirectoryFailureReason::ReadEntry(error),
                    ),
                )?;
                continue;
            }
        };
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                push_failure(
                    state,
                    options.limits.max_failures,
                    DirectoryScanFailure::new(
                        relative_path(root, &path),
                        DirectoryFailureReason::ReadFileType(error),
                    ),
                )?;
                continue;
            }
        };
        children.push(PendingEntry {
            path,
            file_type,
            depth: parent_depth.saturating_add(1),
        });
    }
    children.sort_by(|left, right| right.path.cmp(&left.path));
    Ok(children)
}

fn analyze_file(
    root: &Path,
    path: &Path,
    options: DirectoryManifestOptions,
    state: &mut ScanState,
) -> Result<(), DirectoryManifestError> {
    state.fits_files_considered = state.fits_files_considered.checked_add(1).ok_or(
        DirectoryManifestError::FitsFileLimitExceeded {
            limit: options.limits.max_fits_files,
        },
    )?;
    if state.fits_files_considered > options.limits.max_fits_files {
        return Err(DirectoryManifestError::FitsFileLimitExceeded {
            limit: options.limits.max_fits_files,
        });
    }
    let relative = relative_path(root, path);
    let Some(portable_path) = portable_path(&relative) else {
        return push_failure(
            state,
            options.limits.max_failures,
            DirectoryScanFailure::new(relative, DirectoryFailureReason::NonPortablePath),
        );
    };
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) => {
            return push_failure(
                state,
                options.limits.max_failures,
                DirectoryScanFailure::new(relative, DirectoryFailureReason::OpenFile(error)),
            );
        }
    };
    let byte_length = match file.metadata() {
        Ok(metadata) => metadata.len(),
        Err(error) => {
            return push_failure(
                state,
                options.limits.max_failures,
                DirectoryScanFailure::new(
                    relative,
                    DirectoryFailureReason::ReadFileMetadata(error),
                ),
            );
        }
    };
    if byte_length > options.limits.max_source_bytes {
        return push_failure(
            state,
            options.limits.max_failures,
            DirectoryScanFailure::new(
                relative,
                DirectoryFailureReason::SourceTooLarge {
                    actual: byte_length,
                    limit: options.limits.max_source_bytes,
                },
            ),
        );
    }
    let total_source_bytes = state.total_source_bytes.checked_add(byte_length).ok_or(
        DirectoryManifestError::TotalSourceBytesLimitExceeded {
            limit: options.limits.max_total_source_bytes,
        },
    )?;
    if total_source_bytes > options.limits.max_total_source_bytes {
        return Err(DirectoryManifestError::TotalSourceBytesLimitExceeded {
            limit: options.limits.max_total_source_bytes,
        });
    }
    state.total_source_bytes = total_source_bytes;

    let reader = BufReader::with_capacity(
        FINGERPRINT_BUFFER_BYTES,
        FixedLengthReader::new(file, byte_length),
    );
    match analyze_fits_source(
        portable_path,
        reader,
        options.header,
        options.fits_validation_mode,
        options.classification_policy,
    ) {
        Ok(file) => state.files.push(file),
        Err(error) => push_failure(
            state,
            options.limits.max_failures,
            DirectoryScanFailure::new(relative, DirectoryFailureReason::AnalyzeSource(error)),
        )?,
    }
    Ok(())
}

fn push_failure(
    state: &mut ScanState,
    limit: usize,
    failure: DirectoryScanFailure,
) -> Result<(), DirectoryManifestError> {
    if state.failures.len() >= limit {
        return Err(DirectoryManifestError::FailureLimitExceeded { limit });
    }
    state.failures.push(failure);
    Ok(())
}

fn relative_path(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root)
        .map_or_else(|_| path.to_owned(), Path::to_owned)
}

fn portable_path(path: &Path) -> Option<String> {
    let mut components = Vec::new();
    for component in path.components() {
        let Component::Normal(value) = component else {
            return None;
        };
        let value = value.to_str()?;
        if value.is_empty() || value == "." || value == ".." {
            return None;
        }
        components.push(value);
    }
    if components.is_empty() {
        None
    } else {
        Some(components.join("/"))
    }
}

fn is_fits_path(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("fits")
                || extension.eq_ignore_ascii_case("fit")
                || extension.eq_ignore_ascii_case("fts")
        })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicU64, Ordering};

    use aether_fits::{BLOCK_SIZE, CARD_SIZE};

    use super::*;

    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new() -> io::Result<Self> {
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "aether-session-directory-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path)?;
            Ok(Self { path })
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ignored = fs::remove_dir_all(&self.path);
        }
    }

    fn fixed_card(keyword: &str, value: &str) -> String {
        format!("{keyword:<8}= {value:>20}")
    }

    fn fits_source(frame_type: Option<&str>, complete: bool) -> Vec<u8> {
        let mut cards = vec![
            fixed_card("SIMPLE", "T"),
            fixed_card("BITPIX", "16"),
            fixed_card("NAXIS", "2"),
            fixed_card("NAXIS1", "2"),
            fixed_card("NAXIS2", "2"),
            "INSTRUME= 'ATR585C'".to_owned(),
        ];
        if let Some(frame_type) = frame_type {
            cards.push(format!("IMAGETYP= '{frame_type}'"));
        }
        if complete {
            cards.extend([
                "BAYERPAT= 'RGGB'".to_owned(),
                fixed_card("EXPTIME", "12.5"),
                fixed_card("CCD-TEMP", "-7.0"),
                fixed_card("SET-TEMP", "-10.0"),
                fixed_card("GAIN", "42"),
                fixed_card("OFFSET", "7"),
                fixed_card("XBINNING", "1"),
                fixed_card("YBINNING", "1"),
                "FILTER  = 'SYNTHETIC'".to_owned(),
            ]);
        }
        cards.push("END".to_owned());
        let mut bytes = vec![b' '; BLOCK_SIZE];
        for (index, card) in cards.iter().enumerate() {
            let start = index * CARD_SIZE;
            let end = start + CARD_SIZE;
            if let Some(destination) = bytes.get_mut(start..end) {
                let source = card.as_bytes();
                let length = source.len().min(CARD_SIZE);
                destination[..length].copy_from_slice(&source[..length]);
            }
        }
        bytes.extend([0_u8; 8]);
        bytes.resize(bytes.len().next_multiple_of(BLOCK_SIZE), 0);
        bytes
    }

    #[test]
    fn generates_groups_and_reports_unassigned_and_failed_sources() -> Result<(), Box<dyn Error>> {
        let directory = TestDirectory::new()?;
        fs::create_dir(directory.path().join("lights"))?;
        fs::write(
            directory.path().join("lights/frame_0002.fits"),
            fits_source(Some("Light"), true),
        )?;
        fs::write(
            directory.path().join("lights/frame_0001.fit"),
            fits_source(Some("Light"), true),
        )?;
        fs::write(
            directory.path().join("unclassified.fts"),
            fits_source(None, false),
        )?;
        fs::write(directory.path().join("broken.fits"), b"not a FITS file")?;
        fs::write(directory.path().join("notes.txt"), b"ignored")?;

        let report = generate_manifest_from_directory(
            directory.path(),
            DirectoryManifestOptions::default(),
        )?;

        assert_eq!(report.fits_files_considered(), 4);
        assert_eq!(report.manifest().files().len(), 3);
        assert_eq!(report.manifest().groups().len(), 1);
        assert_eq!(
            report.manifest().groups()[0].files(),
            &["lights/frame_0001.fit", "lights/frame_0002.fits"]
        );
        assert_eq!(report.unassigned_sources(), &["unclassified.fts"]);
        assert_eq!(report.failures().len(), 1);
        assert_eq!(
            report.failures()[0].relative_path(),
            Path::new("broken.fits")
        );
        assert_eq!(
            report.failures()[0].code(),
            DirectoryFailureCode::AnalyzeSource
        );
        assert_eq!(report.skipped_non_fits_files(), 1);
        Ok(())
    }

    #[test]
    fn discovery_order_does_not_change_manifest_json() -> Result<(), Box<dyn Error>> {
        let first = TestDirectory::new()?;
        let second = TestDirectory::new()?;
        let sources = [
            ("z.fits", fits_source(Some("Dark"), true)),
            ("a.fits", fits_source(Some("Dark"), true)),
        ];
        for (name, bytes) in &sources {
            fs::write(first.path().join(name), bytes)?;
        }
        for (name, bytes) in sources.iter().rev() {
            fs::write(second.path().join(name), bytes)?;
        }

        let first_report =
            generate_manifest_from_directory(first.path(), DirectoryManifestOptions::default())?;
        let second_report =
            generate_manifest_from_directory(second.path(), DirectoryManifestOptions::default())?;

        assert_eq!(
            first_report.manifest().to_json_pretty()?,
            second_report.manifest().to_json_pretty()?
        );
        Ok(())
    }

    #[test]
    fn retains_oversize_source_as_a_recoverable_failure() -> Result<(), Box<dyn Error>> {
        let directory = TestDirectory::new()?;
        let bytes = fits_source(Some("Light"), true);
        fs::write(directory.path().join("large.fits"), &bytes)?;
        let mut options = DirectoryManifestOptions::default();
        options.limits.max_source_bytes = (bytes.len() - 1) as u64;

        let report = generate_manifest_from_directory(directory.path(), options)?;

        assert!(report.manifest().files().is_empty());
        assert_eq!(report.failures().len(), 1);
        assert_eq!(
            report.failures()[0].code(),
            DirectoryFailureCode::SourceTooLarge
        );
        Ok(())
    }

    #[test]
    fn aborts_instead_of_truncating_at_hard_limits() -> Result<(), Box<dyn Error>> {
        let directory = TestDirectory::new()?;
        fs::write(
            directory.path().join("first.fits"),
            fits_source(Some("Light"), true),
        )?;
        fs::write(
            directory.path().join("second.fits"),
            fits_source(Some("Light"), true),
        )?;
        let mut options = DirectoryManifestOptions::default();
        options.limits.max_fits_files = 1;

        assert!(matches!(
            generate_manifest_from_directory(directory.path(), options),
            Err(DirectoryManifestError::FitsFileLimitExceeded { limit: 1 })
        ));
        Ok(())
    }

    #[test]
    fn enforces_aggregate_source_budget() -> Result<(), Box<dyn Error>> {
        let directory = TestDirectory::new()?;
        let bytes = fits_source(Some("Light"), true);
        fs::write(directory.path().join("first.fits"), &bytes)?;
        fs::write(directory.path().join("second.fits"), &bytes)?;
        let mut options = DirectoryManifestOptions::default();
        options.limits.max_total_source_bytes = bytes.len() as u64;

        assert!(matches!(
            generate_manifest_from_directory(directory.path(), options),
            Err(DirectoryManifestError::TotalSourceBytesLimitExceeded { limit })
                if limit == bytes.len() as u64
        ));
        Ok(())
    }

    #[test]
    fn enforces_entry_and_depth_limits() -> Result<(), Box<dyn Error>> {
        let directory = TestDirectory::new()?;
        fs::write(directory.path().join("one.txt"), b"one")?;
        fs::write(directory.path().join("two.txt"), b"two")?;
        let mut per_directory = DirectoryManifestOptions::default();
        per_directory.limits.max_entries_per_directory = 1;
        assert!(matches!(
            generate_manifest_from_directory(directory.path(), per_directory),
            Err(DirectoryManifestError::DirectoryEntryLimitExceeded { limit: 1, .. })
        ));

        let mut total = DirectoryManifestOptions::default();
        total.limits.max_total_entries = 1;
        assert!(matches!(
            generate_manifest_from_directory(directory.path(), total),
            Err(DirectoryManifestError::TotalEntryLimitExceeded { limit: 1 })
        ));

        let nested = directory.path().join("nested");
        fs::create_dir(&nested)?;
        let mut depth = DirectoryManifestOptions::default();
        depth.limits.max_depth = 0;
        assert!(matches!(
            generate_manifest_from_directory(directory.path(), depth),
            Err(DirectoryManifestError::DepthLimitExceeded { limit: 0, .. })
        ));
        Ok(())
    }

    #[test]
    fn aborts_when_recoverable_failures_exceed_their_bound() -> Result<(), Box<dyn Error>> {
        let directory = TestDirectory::new()?;
        fs::write(directory.path().join("first.fits"), b"broken")?;
        fs::write(directory.path().join("second.fits"), b"broken")?;
        let mut options = DirectoryManifestOptions::default();
        options.limits.max_failures = 1;

        assert!(matches!(
            generate_manifest_from_directory(directory.path(), options),
            Err(DirectoryManifestError::FailureLimitExceeded { limit: 1 })
        ));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn counts_but_does_not_follow_symbolic_links() -> Result<(), Box<dyn Error>> {
        use std::os::unix::fs::symlink;

        let directory = TestDirectory::new()?;
        let source = directory.path().join("source.fits");
        fs::write(&source, fits_source(Some("Light"), true))?;
        symlink(&source, directory.path().join("linked.fits"))?;

        let report = generate_manifest_from_directory(
            directory.path(),
            DirectoryManifestOptions::default(),
        )?;

        assert_eq!(report.skipped_symlinks(), 1);
        assert_eq!(report.fits_files_considered(), 1);
        assert_eq!(report.manifest().files().len(), 1);
        Ok(())
    }

    #[test]
    fn validates_limits_and_root_type() -> Result<(), Box<dyn Error>> {
        let directory = TestDirectory::new()?;
        let file = directory.path().join("source.fits");
        fs::write(&file, fits_source(Some("Light"), true))?;
        let mut options = DirectoryManifestOptions::default();
        options.limits.max_failures = 0;

        assert!(matches!(
            generate_manifest_from_directory(directory.path(), options),
            Err(DirectoryManifestError::InvalidLimit("max_failures"))
        ));
        assert!(matches!(
            generate_manifest_from_directory(&file, DirectoryManifestOptions::default()),
            Err(DirectoryManifestError::RootNotDirectory)
        ));
        Ok(())
    }

    #[test]
    fn portable_paths_use_forward_slashes_and_reject_parent_components() {
        assert_eq!(
            portable_path(Path::new("lights/session/frame.fits")),
            Some("lights/session/frame.fits".to_owned())
        );
        assert_eq!(portable_path(Path::new("../frame.fits")), None);
        assert_eq!(portable_path(Path::new("")), None);
    }

    #[test]
    fn recognizes_supported_extensions_only() {
        assert!(is_fits_path(Path::new("frame.FITS")));
        assert!(is_fits_path(Path::new("frame.fit")));
        assert!(is_fits_path(Path::new("frame.FtS")));
        assert!(!is_fits_path(Path::new("frame.fits.fz")));
        assert!(!is_fits_path(Path::new("frame.txt")));
    }

    #[test]
    fn fixed_length_reader_rejects_growth_shrinkage_and_out_of_range_seek() {
        let mut grown = FixedLengthReader::new(Cursor::new(b"abcd"), 3);
        let mut output = [0_u8; 4];
        assert!(matches!(grown.read(&mut output), Ok(3)));
        assert!(matches!(
            grown.read(&mut output),
            Err(error) if error.kind() == io::ErrorKind::InvalidData
        ));

        let mut shrunk = FixedLengthReader::new(Cursor::new(b"ab"), 3);
        assert!(matches!(shrunk.read(&mut output), Ok(2)));
        assert!(matches!(
            shrunk.read(&mut output),
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof
        ));

        let mut seek = FixedLengthReader::new(Cursor::new(b"abc"), 3);
        assert!(matches!(
            seek.seek(SeekFrom::Start(4)),
            Err(error) if error.kind() == io::ErrorKind::InvalidInput
        ));
    }
}
