use std::error::Error;
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};

use aether_core::PixelFlags;
use aether_drizzle::{DrizzleTileEvidence, DrizzleTileResult};
use aether_fits::{
    AtomicF64PrimaryStreamWriter, AtomicFitsSetWriteError, AtomicFitsWriteError,
    FitsOutputProvenance, FitsProvenanceError, FitsWriteSummary, HeaderReadOptions,
    PrimaryImageReader, publish_atomic_fits_set,
};

/// Provenance identity of the normalized Drizzle science image.
pub const DRIZZLE_SCIENCE_ALGORITHM_ID: &str = "drizzle-science-v1";
/// Provenance identity of the Drizzle accumulated-weight map.
pub const DRIZZLE_WEIGHT_ALGORITHM_ID: &str = "drizzle-weight-v1";
/// Provenance identity of the exact Drizzle detector-support map.
pub const DRIZZLE_SUPPORT_ALGORITHM_ID: &str = "drizzle-support-v1";

const SUPPORT_CONVERSION_CHUNK: usize = 4_096;
const MAX_EXACT_BINARY64_INTEGER: u64 = 1_u64 << 53;

/// Stable role of one companion in a Drizzle product set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DrizzleProductKind {
    /// Normalized integrated science samples.
    Science,
    /// Sum of geometric and frame weights.
    Weight,
    /// Number of detector contributions per output sample.
    Support,
}

impl Display for DrizzleProductKind {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Science => "science",
            Self::Weight => "weight",
            Self::Support => "support",
        })
    }
}

/// Three distinct create-new destinations for one Drizzle result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DrizzleProductDestinations {
    science: PathBuf,
    weight: PathBuf,
    support: PathBuf,
}

impl DrizzleProductDestinations {
    /// Validates absolute, pairwise-distinct product paths.
    pub fn new(
        science: PathBuf,
        weight: PathBuf,
        support: PathBuf,
    ) -> Result<Self, DrizzlePublicationError> {
        if !science.is_absolute() || !weight.is_absolute() || !support.is_absolute() {
            return Err(DrizzlePublicationError::InvalidDestinations);
        }
        if science == weight || science == support || weight == support {
            return Err(DrizzlePublicationError::InvalidDestinations);
        }
        Ok(Self {
            science,
            weight,
            support,
        })
    }

    /// Final normalized science path.
    #[must_use]
    pub fn science(&self) -> &Path {
        &self.science
    }

    /// Final accumulated-weight path.
    #[must_use]
    pub fn weight(&self) -> &Path {
        &self.weight
    }

    /// Final detector-support-count path.
    #[must_use]
    pub fn support(&self) -> &Path {
        &self.support
    }
}

/// Canonically linked provenance for all three Drizzle companions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DrizzleProductProvenance {
    science: FitsOutputProvenance,
    weight: FitsOutputProvenance,
    support: FitsOutputProvenance,
}

impl DrizzleProductProvenance {
    /// Builds product-specific provenance bound to one plan and parameter set.
    pub fn new(
        manifest_sha256: impl Into<String>,
        plan_sha256: impl Into<String>,
        parameters_sha256: impl Into<String>,
        group_id: impl Into<String>,
        source_count: u32,
    ) -> Result<Self, FitsProvenanceError> {
        let manifest_sha256 = manifest_sha256.into();
        let plan_sha256 = plan_sha256.into();
        let parameters_sha256 = parameters_sha256.into();
        let group_id = group_id.into();
        let build = |algorithm_id| {
            FitsOutputProvenance::new(
                manifest_sha256.clone(),
                group_id.clone(),
                algorithm_id,
                source_count,
            )?
            .with_plan_sha256(plan_sha256.clone())?
            .with_parameters_sha256(parameters_sha256.clone())
        };
        Ok(Self {
            science: build(DRIZZLE_SCIENCE_ALGORITHM_ID)?,
            weight: build(DRIZZLE_WEIGHT_ALGORITHM_ID)?,
            support: build(DRIZZLE_SUPPORT_ALGORITHM_ID)?,
        })
    }

    /// Science-image provenance.
    #[must_use]
    pub const fn science(&self) -> &FitsOutputProvenance {
        &self.science
    }

    /// Accumulated-weight provenance.
    #[must_use]
    pub const fn weight(&self) -> &FitsOutputProvenance {
        &self.weight
    }

    /// Detector-support provenance.
    #[must_use]
    pub const fn support(&self) -> &FitsOutputProvenance {
        &self.support
    }
}

/// Complete summaries for one coherently published Drizzle product set.
#[derive(Clone, Debug, PartialEq)]
pub struct DrizzlePublicationResult {
    destinations: DrizzleProductDestinations,
    science: FitsWriteSummary,
    weight: FitsWriteSummary,
    support: FitsWriteSummary,
    evidence: DrizzleTileEvidence,
}

impl DrizzlePublicationResult {
    /// Published create-new paths.
    #[must_use]
    pub const fn destinations(&self) -> &DrizzleProductDestinations {
        &self.destinations
    }

    /// Science FITS accounting.
    #[must_use]
    pub const fn science_summary(&self) -> FitsWriteSummary {
        self.science
    }

    /// Weight-map FITS accounting.
    #[must_use]
    pub const fn weight_summary(&self) -> FitsWriteSummary {
        self.weight
    }

    /// Support-map FITS accounting.
    #[must_use]
    pub const fn support_summary(&self) -> FitsWriteSummary {
        self.support
    }

    /// Accumulation evidence sealed into this publication result.
    #[must_use]
    pub const fn evidence(&self) -> DrizzleTileEvidence {
        self.evidence
    }
}

/// Failure while validating, staging, verifying, or publishing Drizzle products.
#[derive(Debug)]
pub enum DrizzlePublicationError {
    /// Destinations are relative or not pairwise distinct.
    InvalidDestinations,
    /// Only a complete result whose global origin is `(0, 0)` can be published.
    NonzeroOrigin,
    /// A support count exceeds the exact consecutive-integer binary64 domain.
    InexactSupportCount(u64),
    /// Temporary support conversion storage could not be reserved.
    AllocationFailed,
    /// One private FITS product could not be encoded.
    Stage {
        /// Product that failed before set publication.
        kind: DrizzleProductKind,
        /// Underlying atomic FITS staging failure.
        source: AtomicFitsWriteError,
    },
    /// Private readback found wrong dimensions or invalid checksums.
    InvalidStagedProduct(DrizzleProductKind),
    /// Private readback itself failed.
    Readback {
        /// Product that could not be verified.
        kind: DrizzleProductKind,
        /// Stable diagnostic without exposing a runtime path.
        message: String,
    },
    /// The coherent create-new product transaction failed.
    Publish(AtomicFitsSetWriteError),
}

impl Display for DrizzlePublicationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidDestinations => {
                formatter.write_str("Drizzle destinations must be absolute and distinct")
            }
            Self::NonzeroOrigin => formatter
                .write_str("only a complete origin-aligned Drizzle result can be published"),
            Self::InexactSupportCount(count) => write!(
                formatter,
                "Drizzle support count {count} is not exactly representable in binary64"
            ),
            Self::AllocationFailed => {
                formatter.write_str("cannot allocate bounded Drizzle publication scratch")
            }
            Self::Stage { kind, source } => {
                write!(formatter, "cannot stage Drizzle {kind} product: {source}")
            }
            Self::InvalidStagedProduct(kind) => {
                write!(
                    formatter,
                    "staged Drizzle {kind} product failed verification"
                )
            }
            Self::Readback { kind, message } => {
                write!(
                    formatter,
                    "cannot verify staged Drizzle {kind} product: {message}"
                )
            }
            Self::Publish(error) => {
                write!(formatter, "cannot publish Drizzle product set: {error}")
            }
        }
    }
}

impl Error for DrizzlePublicationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Stage { source, .. } => Some(source),
            Self::Publish(error) => Some(error),
            Self::InvalidDestinations
            | Self::NonzeroOrigin
            | Self::InexactSupportCount(_)
            | Self::AllocationFailed
            | Self::InvalidStagedProduct(_)
            | Self::Readback { .. } => None,
        }
    }
}

/// Stages, verifies, and transactionally publishes science, weight, and support.
///
/// Science samples retain the Drizzle missing mask and therefore encode missing
/// output as the canonical FITS NaN. Weight and support maps deliberately use a
/// clear mask so unsupported samples remain inspectable numeric zeros.
pub fn publish_drizzle_products(
    result: &DrizzleTileResult,
    destinations: DrizzleProductDestinations,
    provenance: &DrizzleProductProvenance,
) -> Result<DrizzlePublicationResult, DrizzlePublicationError> {
    let bounds = result.bounds();
    if bounds.origin_x() != 0 || bounds.origin_y() != 0 {
        return Err(DrizzlePublicationError::NonzeroOrigin);
    }
    for count in result.contribution_counts() {
        exact_support_value(*count)?;
    }
    let dimensions = bounds.dimensions();

    let mut science = AtomicF64PrimaryStreamWriter::create_with_provenance(
        destinations.science(),
        dimensions,
        provenance.science(),
    )
    .map_err(|source| DrizzlePublicationError::Stage {
        kind: DrizzleProductKind::Science,
        source,
    })?;
    science
        .write_samples(result.values(), result.flags())
        .map_err(|source| DrizzlePublicationError::Stage {
            kind: DrizzleProductKind::Science,
            source,
        })?;
    let science = science
        .finish()
        .map_err(|source| DrizzlePublicationError::Stage {
            kind: DrizzleProductKind::Science,
            source,
        })?;

    let mut weight = AtomicF64PrimaryStreamWriter::create_with_provenance(
        destinations.weight(),
        dimensions,
        provenance.weight(),
    )
    .map_err(|source| DrizzlePublicationError::Stage {
        kind: DrizzleProductKind::Weight,
        source,
    })?;
    write_clear_chunks(&mut weight, result.weights(), DrizzleProductKind::Weight)?;
    let weight = weight
        .finish()
        .map_err(|source| DrizzlePublicationError::Stage {
            kind: DrizzleProductKind::Weight,
            source,
        })?;

    let mut support = AtomicF64PrimaryStreamWriter::create_with_provenance(
        destinations.support(),
        dimensions,
        provenance.support(),
    )
    .map_err(|source| DrizzlePublicationError::Stage {
        kind: DrizzleProductKind::Support,
        source,
    })?;
    write_support_chunks(&mut support, result.contribution_counts())?;
    let support = support
        .finish()
        .map_err(|source| DrizzlePublicationError::Stage {
            kind: DrizzleProductKind::Support,
            source,
        })?;

    validate_staged(&science, dimensions, DrizzleProductKind::Science)?;
    validate_staged(&weight, dimensions, DrizzleProductKind::Weight)?;
    validate_staged(&support, dimensions, DrizzleProductKind::Support)?;

    let mut staged = Vec::new();
    staged
        .try_reserve_exact(3)
        .map_err(|_| DrizzlePublicationError::AllocationFailed)?;
    staged.push(science);
    staged.push(weight);
    staged.push(support);
    let summaries = publish_atomic_fits_set(staged).map_err(DrizzlePublicationError::Publish)?;
    let [science, weight, support] = summaries.as_slice() else {
        return Err(DrizzlePublicationError::AllocationFailed);
    };
    Ok(DrizzlePublicationResult {
        destinations,
        science: *science,
        weight: *weight,
        support: *support,
        evidence: result.evidence(),
    })
}

fn write_clear_chunks(
    writer: &mut AtomicF64PrimaryStreamWriter,
    values: &[f64],
    kind: DrizzleProductKind,
) -> Result<(), DrizzlePublicationError> {
    let flags = [PixelFlags::CLEAR; SUPPORT_CONVERSION_CHUNK];
    for chunk in values.chunks(SUPPORT_CONVERSION_CHUNK) {
        writer
            .write_samples(chunk, &flags[..chunk.len()])
            .map_err(|source| DrizzlePublicationError::Stage { kind, source })?;
    }
    Ok(())
}

fn write_support_chunks(
    writer: &mut AtomicF64PrimaryStreamWriter,
    counts: &[u64],
) -> Result<(), DrizzlePublicationError> {
    let capacity = counts.len().min(SUPPORT_CONVERSION_CHUNK);
    let mut values = Vec::new();
    values
        .try_reserve_exact(capacity)
        .map_err(|_| DrizzlePublicationError::AllocationFailed)?;
    for chunk in counts.chunks(SUPPORT_CONVERSION_CHUNK) {
        values.clear();
        for count in chunk {
            values.push(exact_support_value(*count)?);
        }
        write_clear_chunks(writer, &values, DrizzleProductKind::Support)?;
    }
    Ok(())
}

fn exact_support_value(count: u64) -> Result<f64, DrizzlePublicationError> {
    if count > MAX_EXACT_BINARY64_INTEGER {
        Err(DrizzlePublicationError::InexactSupportCount(count))
    } else {
        Ok(count as f64)
    }
}

fn validate_staged(
    staged: &aether_fits::CompletedAtomicFits,
    expected: aether_core::Dimensions,
    kind: DrizzleProductKind,
) -> Result<(), DrizzlePublicationError> {
    let file =
        staged
            .try_clone_for_readback()
            .map_err(|error| DrizzlePublicationError::Readback {
                kind,
                message: error.to_string(),
            })?;
    let mut reader =
        PrimaryImageReader::open(file, HeaderReadOptions::default()).map_err(|error| {
            DrizzlePublicationError::Readback {
                kind,
                message: error.to_string(),
            }
        })?;
    let axes = reader.descriptor().axes();
    let actual = match axes {
        [width, height] => aether_core::Dimensions::new(
            usize::try_from(*width)
                .map_err(|_| DrizzlePublicationError::InvalidStagedProduct(kind))?,
            usize::try_from(*height)
                .map_err(|_| DrizzlePublicationError::InvalidStagedProduct(kind))?,
            1,
        ),
        [width, height, planes] => aether_core::Dimensions::new(
            usize::try_from(*width)
                .map_err(|_| DrizzlePublicationError::InvalidStagedProduct(kind))?,
            usize::try_from(*height)
                .map_err(|_| DrizzlePublicationError::InvalidStagedProduct(kind))?,
            usize::try_from(*planes)
                .map_err(|_| DrizzlePublicationError::InvalidStagedProduct(kind))?,
        ),
        _ => return Err(DrizzlePublicationError::InvalidStagedProduct(kind)),
    }
    .map_err(|_| DrizzlePublicationError::InvalidStagedProduct(kind))?;
    let checksums =
        reader
            .verify_checksums()
            .map_err(|error| DrizzlePublicationError::Readback {
                kind,
                message: error.to_string(),
            })?;
    if actual != expected || !checksums.is_fully_verified() {
        return Err(DrizzlePublicationError::InvalidStagedProduct(kind));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File};
    use std::io;
    use std::sync::atomic::{AtomicU64, Ordering};

    use aether_drizzle::{
        DrizzleParameters, DrizzleTileAccumulator, DrizzleTileBounds, deposit_detector_footprint,
        project_detector_footprint,
    };
    use aether_fits::{PrimaryImageReader, SampleStatus};
    use aether_registration::ProjectiveTransform;

    use super::*;

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> io::Result<Self> {
            let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "aether-runtime-drizzle-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path)?;
            Ok(Self(path))
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ignored = fs::remove_dir_all(&self.0);
        }
    }

    type TestResult = Result<(), Box<dyn Error>>;

    fn provenance() -> Result<DrizzleProductProvenance, FitsProvenanceError> {
        DrizzleProductProvenance::new("a".repeat(64), "b".repeat(64), "c".repeat(64), "lights", 2)
    }

    fn destinations(root: &Path) -> Result<DrizzleProductDestinations, DrizzlePublicationError> {
        DrizzleProductDestinations::new(
            root.join("science.fits"),
            root.join("weight.fits"),
            root.join("support.fits"),
        )
    }

    fn result() -> Result<DrizzleTileResult, Box<dyn Error>> {
        let mut accumulator = DrizzleTileAccumulator::new(DrizzleTileBounds::new(0, 0, 2, 1, 1)?)?;
        let footprint = project_detector_footprint(
            0,
            0,
            ProjectiveTransform::IDENTITY,
            DrizzleParameters::new(1, 1.0)?,
        )?;
        accumulator.accumulate(
            &deposit_detector_footprint(footprint, 12.0, 2.0, 2, 1, 4)?,
            0,
        )?;
        Ok(accumulator.finish()?)
    }

    fn read_two(path: &Path) -> Result<([f64; 2], [SampleStatus; 2]), Box<dyn Error>> {
        let mut reader = PrimaryImageReader::open(File::open(path)?, HeaderReadOptions::default())?;
        let mut values = [0.0; 2];
        let mut statuses = [SampleStatus::Undefined; 2];
        reader.read_physical_samples(0, &mut values, &mut statuses)?;
        assert!(reader.verify_checksums()?.is_fully_verified());
        Ok((values, statuses))
    }

    #[test]
    fn publishes_science_weight_and_exact_support_together() -> TestResult {
        let directory = TestDirectory::new()?;
        let destinations = destinations(&directory.0)?;

        let published = publish_drizzle_products(&result()?, destinations, &provenance()?)?;

        assert_eq!(published.science_summary().samples_written(), 2);
        assert_eq!(published.science_summary().substituted_samples(), 1);
        assert_eq!(published.weight_summary().substituted_samples(), 0);
        assert_eq!(published.support_summary().substituted_samples(), 0);
        assert_eq!(published.evidence().unsupported_pixels(), 1);
        let (science, science_status) = read_two(published.destinations().science())?;
        assert_eq!(science[0].to_bits(), 12.0_f64.to_bits());
        assert!(science[1].is_nan());
        assert_eq!(
            science_status,
            [SampleStatus::Valid, SampleStatus::NonFinite]
        );
        let (weight, weight_status) = read_two(published.destinations().weight())?;
        assert_eq!(weight.map(f64::to_bits), [2.0, 0.0].map(f64::to_bits));
        assert_eq!(weight_status, [SampleStatus::Valid; 2]);
        let (support, support_status) = read_two(published.destinations().support())?;
        assert_eq!(support.map(f64::to_bits), [1.0, 0.0].map(f64::to_bits));
        assert_eq!(support_status, [SampleStatus::Valid; 2]);
        Ok(())
    }

    #[test]
    fn existing_companion_prevents_every_new_destination() -> TestResult {
        let directory = TestDirectory::new()?;
        let destinations = destinations(&directory.0)?;
        fs::write(destinations.weight(), b"existing")?;

        let error = publish_drizzle_products(&result()?, destinations.clone(), &provenance()?);

        assert!(matches!(
            error,
            Err(DrizzlePublicationError::Stage {
                kind: DrizzleProductKind::Weight,
                source: AtomicFitsWriteError::TargetExists,
            })
        ));
        assert!(!destinations.science().exists());
        assert!(!destinations.support().exists());
        assert_eq!(fs::read(destinations.weight())?, b"existing");
        Ok(())
    }

    #[test]
    fn validates_destinations_origin_and_exact_support_domain() -> TestResult {
        assert!(matches!(
            DrizzleProductDestinations::new(
                PathBuf::from("science.fits"),
                PathBuf::from("weight.fits"),
                PathBuf::from("support.fits"),
            ),
            Err(DrizzlePublicationError::InvalidDestinations)
        ));
        let directory = TestDirectory::new()?;
        assert!(matches!(
            DrizzleProductDestinations::new(
                directory.0.join("same.fits"),
                directory.0.join("same.fits"),
                directory.0.join("support.fits"),
            ),
            Err(DrizzlePublicationError::InvalidDestinations)
        ));
        assert_eq!(
            exact_support_value(MAX_EXACT_BINARY64_INTEGER)?.to_bits(),
            (MAX_EXACT_BINARY64_INTEGER as f64).to_bits()
        );
        assert!(matches!(
            exact_support_value(MAX_EXACT_BINARY64_INTEGER + 1),
            Err(DrizzlePublicationError::InexactSupportCount(_))
        ));

        let nonzero =
            DrizzleTileAccumulator::new(DrizzleTileBounds::new(1, 0, 1, 1, 1)?)?.finish()?;
        assert!(matches!(
            publish_drizzle_products(&nonzero, destinations(&directory.0)?, &provenance()?),
            Err(DrizzlePublicationError::NonzeroOrigin)
        ));
        Ok(())
    }
}
