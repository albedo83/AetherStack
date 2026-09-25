use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use aether_calibration::CalibrationParameters;
use aether_core::ImageStatistics;
use aether_fits::{
    FitsOutputProvenance, FitsProvenanceError, FitsWriteSummary, HeaderReadOptions, ImageReadError,
    PrimaryImageReader,
};
use aether_metadata::FrameType;
use aether_session::{
    FingerprintError, LightCalibrationPlan, LightCalibrationPlanError, LightMasterAssociation,
    LightMasterKind, ManifestError, ManifestFile, ManifestGroup, MasterPlan, MasterPlanError,
    MasterProductKind, SessionManifest, SourceFingerprint, fingerprint_reader,
};

use crate::master_plan::product_file_name;
use crate::{
    CancellationToken, Cancelled, MemoryBudget, PipelineSource, ProgressEvent,
    STRICT_FLAT_MASTER_ALGORITHM_ID, STRICT_MEAN_ALGORITHM_ID, StrictPipelineError,
    StrictPipelineRequest, run_strict_pipeline,
};

const MAX_STAGING_DIRECTORY_ATTEMPTS: usize = 128;
static STAGING_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Complete request for executing one immutable Light calibration plan.
#[derive(Clone, Debug)]
pub struct LightPlanExecutionRequest {
    session_root: PathBuf,
    master_directory: PathBuf,
    output_directory: PathBuf,
    manifest: Arc<SessionManifest>,
    master_plan: MasterPlan,
    light_plan: LightCalibrationPlan,
    calibration: CalibrationParameters,
    tile_width: usize,
    tile_height: usize,
}

impl LightPlanExecutionRequest {
    /// Binds source frames, generated masters, both plans, and one destination.
    ///
    /// All paths must be absolute. The two supplied plans are regenerated from
    /// the manifest and their serialized options before they are accepted. This
    /// prevents a structurally valid but incomplete plan from reaching runtime.
    /// The default tile shape is 256 by 256 pixels.
    ///
    /// # Errors
    ///
    /// Returns a typed path, digest, plan-readiness, or graph error.
    pub fn new(
        session_root: PathBuf,
        master_directory: PathBuf,
        output_directory: PathBuf,
        manifest: SessionManifest,
        master_plan: MasterPlan,
        light_plan: LightCalibrationPlan,
        calibration: CalibrationParameters,
    ) -> Result<Self, LightPlanExecutionError> {
        Self::new_shared(
            session_root,
            master_directory,
            output_directory,
            Arc::new(manifest),
            master_plan,
            light_plan,
            calibration,
        )
    }

    /// Binds an already shared native manifest without copying its file set.
    ///
    /// # Errors
    ///
    /// Returns the same validation failures as [`Self::new`].
    pub fn new_shared(
        session_root: PathBuf,
        master_directory: PathBuf,
        output_directory: PathBuf,
        manifest: Arc<SessionManifest>,
        master_plan: MasterPlan,
        light_plan: LightCalibrationPlan,
        calibration: CalibrationParameters,
    ) -> Result<Self, LightPlanExecutionError> {
        if !session_root.is_absolute() {
            return Err(LightPlanExecutionError::SessionRootNotAbsolute);
        }
        if !master_directory.is_absolute() {
            return Err(LightPlanExecutionError::MasterDirectoryNotAbsolute);
        }
        if !output_directory.is_absolute() {
            return Err(LightPlanExecutionError::OutputDirectoryNotAbsolute);
        }
        validate_plan_graph(&manifest, &master_plan, &light_plan)?;
        Ok(Self {
            session_root,
            master_directory,
            output_directory,
            manifest,
            master_plan,
            light_plan,
            calibration,
            tile_width: 256,
            tile_height: 256,
        })
    }

    /// Replaces the tile shape used by every Light product.
    ///
    /// # Errors
    ///
    /// Returns an error when either extent is zero.
    pub fn with_tile_shape(
        mut self,
        width: usize,
        height: usize,
    ) -> Result<Self, LightPlanExecutionError> {
        if width == 0 || height == 0 {
            return Err(LightPlanExecutionError::ZeroTileExtent { width, height });
        }
        self.tile_width = width;
        self.tile_height = height;
        Ok(self)
    }

    /// Canonical Light plan being executed.
    #[must_use]
    pub const fn light_plan(&self) -> &LightCalibrationPlan {
        &self.light_plan
    }

    /// Absolute directory containing exact master-plan products.
    #[must_use]
    pub fn master_directory(&self) -> &Path {
        &self.master_directory
    }

    /// Absolute destination for the complete calibrated product set.
    #[must_use]
    pub fn output_directory(&self) -> &Path {
        &self.output_directory
    }
}

/// Product-local progress bound to one canonical Light group.
#[derive(Clone, Debug, PartialEq)]
pub struct LightPlanProgressEvent {
    product_index: usize,
    product_count: usize,
    group_id: String,
    stage: ProgressEvent,
}

impl LightPlanProgressEvent {
    /// Zero-based canonical Light-product index.
    #[must_use]
    pub const fn product_index(&self) -> usize {
        self.product_index
    }

    /// Total Light products in the plan.
    #[must_use]
    pub const fn product_count(&self) -> usize {
        self.product_count
    }

    /// Exact manifest Light group currently processed.
    #[must_use]
    pub fn group_id(&self) -> &str {
        &self.group_id
    }

    /// Underlying strict-pipeline progress event.
    #[must_use]
    pub const fn stage(&self) -> &ProgressEvent {
        &self.stage
    }
}

/// One atomically published calibrated and integrated Light-group product.
#[derive(Clone, Debug, PartialEq)]
pub struct LightProductExecutionResult {
    group_id: String,
    dark_group_id: String,
    flat_group_id: String,
    output: PathBuf,
    statistics: ImageStatistics,
    write_summary: FitsWriteSummary,
    tiles_processed: u64,
    tiles_reused: u64,
}

impl LightProductExecutionResult {
    /// Exact source Light group.
    #[must_use]
    pub fn group_id(&self) -> &str {
        &self.group_id
    }

    /// Dark master group selected by the Light plan.
    #[must_use]
    pub fn dark_group_id(&self) -> &str {
        &self.dark_group_id
    }

    /// Flat master group selected by the Light plan.
    #[must_use]
    pub fn flat_group_id(&self) -> &str {
        &self.flat_group_id
    }

    /// Final public FITS path.
    #[must_use]
    pub fn output(&self) -> &Path {
        &self.output
    }

    /// Exact full-output statistics measured before publication.
    #[must_use]
    pub const fn statistics(&self) -> ImageStatistics {
        self.statistics
    }

    /// FITS sample, checksum, and byte accounting.
    #[must_use]
    pub const fn write_summary(&self) -> FitsWriteSummary {
        self.write_summary
    }

    /// Spatial-plane tiles processed.
    #[must_use]
    pub const fn tiles_processed(&self) -> u64 {
        self.tiles_processed
    }

    /// Verified cached tiles reused by the strict pipeline.
    #[must_use]
    pub const fn tiles_reused(&self) -> u64 {
        self.tiles_reused
    }
}

/// Complete transaction result ordered by canonical Light group identifier.
#[derive(Clone, Debug, PartialEq)]
pub struct LightPlanExecutionResult {
    manifest_sha256: String,
    master_plan_sha256: String,
    light_plan_sha256: String,
    products: Vec<LightProductExecutionResult>,
}

impl LightPlanExecutionResult {
    /// SHA-256 of the canonical session manifest.
    #[must_use]
    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }

    /// SHA-256 of the exact master plan.
    #[must_use]
    pub fn master_plan_sha256(&self) -> &str {
        &self.master_plan_sha256
    }

    /// SHA-256 of the exact Light association plan.
    #[must_use]
    pub fn light_plan_sha256(&self) -> &str {
        &self.light_plan_sha256
    }

    /// Published products in canonical Light-group order.
    #[must_use]
    pub fn products(&self) -> &[LightProductExecutionResult] {
        &self.products
    }
}

/// Failure while validating or executing a complete Light-plan transaction.
#[derive(Debug)]
pub enum LightPlanExecutionError {
    /// Session root must be absolute.
    SessionRootNotAbsolute,
    /// Master directory must be absolute.
    MasterDirectoryNotAbsolute,
    /// Output directory must be absolute.
    OutputDirectoryNotAbsolute,
    /// Tile extents must both be non-zero.
    ZeroTileExtent {
        /// Requested tile width in pixels.
        width: usize,
        /// Requested tile height in pixels.
        height: usize,
    },
    /// Canonical manifest encoding or hashing failed.
    Manifest(ManifestError),
    /// Canonical master-plan validation failed.
    MasterPlan(MasterPlanError),
    /// Canonical Light-plan validation failed.
    LightPlan(LightCalibrationPlanError),
    /// The master plan is not canonical for the bound manifest and options.
    MasterPlanMismatch,
    /// The Light plan is not canonical for the bound manifest, master plan, and options.
    LightPlanMismatch,
    /// At least one Light association remains unresolved.
    LightPlanNotReady,
    /// The canonical plan contains no Light products to execute.
    NoLightProducts,
    /// A referenced manifest group does not exist or has the wrong role.
    InvalidGroup {
        /// Manifest group identifier.
        group_id: String,
    },
    /// A selected master is absent from the master plan or has the wrong role.
    InvalidMasterAssociation {
        /// Light group whose selection is invalid.
        light_group_id: String,
        /// Selected master group identifier.
        master_group_id: String,
    },
    /// A manifest group references an unknown file.
    MissingManifestFile {
        /// Portable session-relative source path.
        relative_path: String,
    },
    /// Source, master, or output directory metadata could not be inspected.
    InspectDirectory(std::io::Error),
    /// One required runtime directory is not a physical directory.
    DirectoryNotPhysical {
        /// Stable description of the directory's runtime role.
        role: &'static str,
    },
    /// A required generated master is absent or not a regular file.
    MasterMissing {
        /// Master source group identifier.
        group_id: String,
    },
    /// A generated master header cannot be opened or parsed.
    OpenMaster {
        /// Master source group identifier.
        group_id: String,
        /// FITS open or header-parse failure.
        source: ImageReadError,
    },
    /// Generated master provenance does not match the exact upstream plan.
    MasterProvenanceMismatch {
        /// Master source group identifier.
        group_id: String,
    },
    /// A generated master could not be fingerprinted.
    FingerprintMaster {
        /// Master source group identifier.
        group_id: String,
        /// Bounded fingerprinting failure.
        source: FingerprintError,
    },
    /// A source or master changed before whole-set publication.
    SourceChanged {
        /// Portable source path or master group identifier.
        identity: String,
    },
    /// Two planned products map to the same portable filename.
    DestinationNameCollision {
        /// Colliding portable filename.
        file_name: String,
    },
    /// A planned destination already exists.
    DestinationExists {
        /// Existing destination that would otherwise be replaced.
        path: PathBuf,
    },
    /// Private sibling staging could not be created.
    CreateStagingDirectory(std::io::Error),
    /// Output provenance could not be constructed.
    Provenance(FitsProvenanceError),
    /// One strict calibrated integration failed.
    ProductPipeline {
        /// Light group being processed.
        group_id: String,
        /// Strict-pipeline failure.
        source: StrictPipelineError,
    },
    /// A product could not be atomically exposed.
    PublishProduct {
        /// Light group being published.
        group_id: String,
        /// Atomic hard-link failure.
        source: std::io::Error,
    },
    /// The output directory could not be synchronized.
    SyncOutputDirectory(std::io::Error),
    /// A failed publication could not be completely rolled back.
    RollbackPublication {
        /// Transaction-created link that could not be removed.
        path: PathBuf,
        /// Filesystem rollback failure.
        source: std::io::Error,
    },
    /// Execution stopped at a cooperative checkpoint.
    Cancelled(Cancelled),
    /// Bounded state could not be allocated.
    AllocationFailed,
}

impl Display for LightPlanExecutionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionRootNotAbsolute => formatter.write_str("session root must be absolute"),
            Self::MasterDirectoryNotAbsolute => {
                formatter.write_str("master directory must be absolute")
            }
            Self::OutputDirectoryNotAbsolute => {
                formatter.write_str("Light output directory must be absolute")
            }
            Self::ZeroTileExtent { width, height } => {
                write!(
                    formatter,
                    "Light tile extent must be non-zero, received {width}x{height}"
                )
            }
            Self::Manifest(error) => Display::fmt(error, formatter),
            Self::MasterPlan(error) => Display::fmt(error, formatter),
            Self::LightPlan(error) => Display::fmt(error, formatter),
            Self::MasterPlanMismatch => {
                formatter.write_str("master plan is not canonical for the bound manifest")
            }
            Self::LightPlanMismatch => formatter
                .write_str("Light plan is not canonical for the bound manifest and master plan"),
            Self::LightPlanNotReady => {
                formatter.write_str("every Light requires one exact Dark and Flat association")
            }
            Self::NoLightProducts => formatter.write_str("Light plan contains no products"),
            Self::InvalidGroup { group_id } => {
                write!(formatter, "invalid Light execution group `{group_id}`")
            }
            Self::InvalidMasterAssociation {
                light_group_id,
                master_group_id,
            } => write!(
                formatter,
                "Light `{light_group_id}` selects invalid master `{master_group_id}`"
            ),
            Self::MissingManifestFile { relative_path } => {
                write!(
                    formatter,
                    "manifest references missing file `{relative_path}`"
                )
            }
            Self::InspectDirectory(error) => {
                write!(formatter, "cannot inspect runtime directory: {error}")
            }
            Self::DirectoryNotPhysical { role } => {
                write!(formatter, "{role} path is not a physical directory")
            }
            Self::MasterMissing { group_id } => {
                write!(formatter, "generated master `{group_id}` is missing")
            }
            Self::OpenMaster { group_id, source } => {
                write!(
                    formatter,
                    "cannot inspect generated master `{group_id}`: {source}"
                )
            }
            Self::MasterProvenanceMismatch { group_id } => {
                write!(
                    formatter,
                    "generated master `{group_id}` has stale provenance"
                )
            }
            Self::FingerprintMaster { group_id, source } => {
                write!(
                    formatter,
                    "cannot fingerprint generated master `{group_id}`: {source}"
                )
            }
            Self::SourceChanged { identity } => {
                write!(
                    formatter,
                    "runtime input `{identity}` changed before publication"
                )
            }
            Self::DestinationNameCollision { file_name } => {
                write!(formatter, "Light destination name collides: `{file_name}`")
            }
            Self::DestinationExists { path } => {
                write!(
                    formatter,
                    "Light destination already exists: {}",
                    path.display()
                )
            }
            Self::CreateStagingDirectory(error) => {
                write!(
                    formatter,
                    "cannot create private Light staging directory: {error}"
                )
            }
            Self::Provenance(error) => Display::fmt(error, formatter),
            Self::ProductPipeline { group_id, source } => {
                write!(formatter, "Light product `{group_id}` failed: {source}")
            }
            Self::PublishProduct { group_id, source } => {
                write!(
                    formatter,
                    "cannot publish Light product `{group_id}`: {source}"
                )
            }
            Self::SyncOutputDirectory(error) => {
                write!(
                    formatter,
                    "cannot synchronize Light output directory: {error}"
                )
            }
            Self::RollbackPublication { path, source } => write!(
                formatter,
                "cannot roll back Light publication `{}`: {source}",
                path.display()
            ),
            Self::Cancelled(error) => Display::fmt(error, formatter),
            Self::AllocationFailed => formatter.write_str("cannot allocate Light execution state"),
        }
    }
}

impl Error for LightPlanExecutionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Manifest(error) => Some(error),
            Self::MasterPlan(error) => Some(error),
            Self::LightPlan(error) => Some(error),
            Self::InspectDirectory(error)
            | Self::CreateStagingDirectory(error)
            | Self::SyncOutputDirectory(error) => Some(error),
            Self::OpenMaster { source, .. } => Some(source),
            Self::FingerprintMaster { source, .. } => Some(source),
            Self::Provenance(error) => Some(error),
            Self::ProductPipeline { source, .. } => Some(source),
            Self::PublishProduct { source, .. } | Self::RollbackPublication { source, .. } => {
                Some(source)
            }
            Self::Cancelled(error) => Some(error),
            Self::SessionRootNotAbsolute
            | Self::MasterDirectoryNotAbsolute
            | Self::OutputDirectoryNotAbsolute
            | Self::ZeroTileExtent { .. }
            | Self::MasterPlanMismatch
            | Self::LightPlanMismatch
            | Self::LightPlanNotReady
            | Self::NoLightProducts
            | Self::InvalidGroup { .. }
            | Self::InvalidMasterAssociation { .. }
            | Self::MissingManifestFile { .. }
            | Self::DirectoryNotPhysical { .. }
            | Self::MasterMissing { .. }
            | Self::MasterProvenanceMismatch { .. }
            | Self::SourceChanged { .. }
            | Self::DestinationNameCollision { .. }
            | Self::DestinationExists { .. }
            | Self::AllocationFailed => None,
        }
    }
}

/// Executes every Light group as one all-or-nothing publication transaction.
///
/// Each group is calibrated with its plan-selected generated Dark and normalized
/// Flat, then integrated in canonical source order by the strict double-precision
/// reference pipeline. Products remain private until every group succeeds and
/// all raw and generated inputs are fingerprinted again. Existing files are
/// never replaced.
///
/// # Errors
///
/// Returns a typed plan, filesystem, provenance, pipeline, mutation,
/// cancellation, allocation, publication, or rollback failure.
pub fn run_light_plan<F>(
    request: &LightPlanExecutionRequest,
    cancellation: &CancellationToken,
    memory: &MemoryBudget,
    mut progress: F,
) -> Result<LightPlanExecutionResult, LightPlanExecutionError>
where
    F: FnMut(LightPlanProgressEvent),
{
    validate_runtime_directories(request)?;
    let manifest_sha256 = request
        .manifest
        .canonical_sha256()
        .map_err(LightPlanExecutionError::Manifest)?;
    let master_plan_sha256 = request
        .master_plan
        .canonical_sha256()
        .map_err(LightPlanExecutionError::MasterPlan)?;
    let light_plan_sha256 = request
        .light_plan
        .canonical_sha256()
        .map_err(LightPlanExecutionError::LightPlan)?;
    let destinations = planned_destinations(request)?;
    preflight_destinations(&destinations)?;
    cancellation
        .checkpoint()
        .map_err(LightPlanExecutionError::Cancelled)?;

    let master_sources = load_master_sources(request, &manifest_sha256, &master_plan_sha256)?;
    let staging = StagingDirectory::create(&request.output_directory)?;
    let product_count = request.light_plan.products().len();
    let mut results = Vec::new();
    results
        .try_reserve_exact(product_count)
        .map_err(|_| LightPlanExecutionError::AllocationFailed)?;
    let mut staged_by_group = BTreeMap::new();

    for (product_index, product) in request.light_plan.products().iter().enumerate() {
        cancellation
            .checkpoint()
            .map_err(LightPlanExecutionError::Cancelled)?;
        let group = find_group(&request.manifest, product.source_group_id())?;
        let signals = group_sources(request, group)?;
        let dark_group_id = selected_group(product.dark(), LightMasterKind::Dark)
            .ok_or(LightPlanExecutionError::LightPlanNotReady)?;
        let flat_group_id = selected_group(product.flat(), LightMasterKind::Flat)
            .ok_or(LightPlanExecutionError::LightPlanNotReady)?;
        let dark = master_sources.get(dark_group_id).cloned().ok_or_else(|| {
            LightPlanExecutionError::InvalidMasterAssociation {
                light_group_id: group.id().to_owned(),
                master_group_id: dark_group_id.to_owned(),
            }
        })?;
        let flat = master_sources.get(flat_group_id).cloned().ok_or_else(|| {
            LightPlanExecutionError::InvalidMasterAssociation {
                light_group_id: group.id().to_owned(),
                master_group_id: flat_group_id.to_owned(),
            }
        })?;
        let staged_output = staging.path().join(light_product_file_name(group.id()));
        let public_output = destinations.get(group.id()).cloned().ok_or_else(|| {
            LightPlanExecutionError::InvalidGroup {
                group_id: group.id().to_owned(),
            }
        })?;
        let source_count =
            u32::try_from(signals.len()).map_err(|_| LightPlanExecutionError::AllocationFailed)?;
        let provenance = FitsOutputProvenance::new(
            &manifest_sha256,
            group.id(),
            STRICT_MEAN_ALGORITHM_ID,
            source_count,
        )
        .and_then(|provenance| provenance.with_plan_sha256(&light_plan_sha256))
        .map_err(LightPlanExecutionError::Provenance)?;
        let pipeline = StrictPipelineRequest::new(
            signals,
            dark,
            flat,
            staged_output.clone(),
            provenance,
            request.calibration,
        )
        .and_then(|builder| builder.with_tile_shape(request.tile_width, request.tile_height))
        .map(|builder| {
            builder.with_header_policy(
                HeaderReadOptions::default(),
                request.manifest.fits_validation_mode(),
            )
        })
        .map_err(|source| LightPlanExecutionError::ProductPipeline {
            group_id: group.id().to_owned(),
            source,
        })?;
        let completed = run_strict_pipeline(&pipeline, cancellation, memory, |stage| {
            progress(LightPlanProgressEvent {
                product_index,
                product_count,
                group_id: group.id().to_owned(),
                stage,
            });
        })
        .map_err(|source| LightPlanExecutionError::ProductPipeline {
            group_id: group.id().to_owned(),
            source,
        })?;
        staged_by_group.insert(group.id().to_owned(), staged_output);
        results.push(LightProductExecutionResult {
            group_id: group.id().to_owned(),
            dark_group_id: dark_group_id.to_owned(),
            flat_group_id: flat_group_id.to_owned(),
            output: public_output,
            statistics: completed.statistics(),
            write_summary: completed.write_summary(),
            tiles_processed: completed.tiles_processed(),
            tiles_reused: completed.tiles_reused(),
        });
    }

    cancellation
        .checkpoint()
        .map_err(LightPlanExecutionError::Cancelled)?;
    revalidate_all_inputs(request, &master_sources)?;
    publish_product_set(
        &staged_by_group,
        &destinations,
        &request.output_directory,
        cancellation,
    )?;
    results.sort_by(|left, right| left.group_id.cmp(&right.group_id));
    Ok(LightPlanExecutionResult {
        manifest_sha256,
        master_plan_sha256,
        light_plan_sha256,
        products: results,
    })
}

fn validate_plan_graph(
    manifest: &SessionManifest,
    master_plan: &MasterPlan,
    light_plan: &LightCalibrationPlan,
) -> Result<(), LightPlanExecutionError> {
    let canonical_master = MasterPlan::from_manifest(manifest, master_plan.options())
        .map_err(LightPlanExecutionError::MasterPlan)?;
    let supplied_master_sha = master_plan
        .canonical_sha256()
        .map_err(LightPlanExecutionError::MasterPlan)?;
    let canonical_master_sha = canonical_master
        .canonical_sha256()
        .map_err(LightPlanExecutionError::MasterPlan)?;
    if supplied_master_sha != canonical_master_sha {
        return Err(LightPlanExecutionError::MasterPlanMismatch);
    }
    let canonical_light = LightCalibrationPlan::from_manifest_and_master_plan(
        manifest,
        master_plan,
        light_plan.options(),
    )
    .map_err(LightPlanExecutionError::LightPlan)?;
    let supplied_light_sha = light_plan
        .canonical_sha256()
        .map_err(LightPlanExecutionError::LightPlan)?;
    let canonical_light_sha = canonical_light
        .canonical_sha256()
        .map_err(LightPlanExecutionError::LightPlan)?;
    if supplied_light_sha != canonical_light_sha {
        return Err(LightPlanExecutionError::LightPlanMismatch);
    }
    if !light_plan.is_ready() {
        return Err(LightPlanExecutionError::LightPlanNotReady);
    }
    if light_plan.products().is_empty() {
        return Err(LightPlanExecutionError::NoLightProducts);
    }
    for product in light_plan.products() {
        let group = find_group(manifest, product.source_group_id())?;
        if group.key().frame_type() != &FrameType::Light {
            return Err(LightPlanExecutionError::InvalidGroup {
                group_id: group.id().to_owned(),
            });
        }
        validate_selected_master(master_plan, product.source_group_id(), product.dark())?;
        validate_selected_master(master_plan, product.source_group_id(), product.flat())?;
        for relative_path in group.files() {
            if find_file(manifest, relative_path).is_none() {
                return Err(LightPlanExecutionError::MissingManifestFile {
                    relative_path: relative_path.clone(),
                });
            }
        }
    }
    Ok(())
}

fn validate_selected_master(
    master_plan: &MasterPlan,
    light_group_id: &str,
    association: &LightMasterAssociation,
) -> Result<(), LightPlanExecutionError> {
    let kind = match association {
        LightMasterAssociation::Matched { kind, .. } => *kind,
        LightMasterAssociation::Unresolved { .. } => {
            return Err(LightPlanExecutionError::LightPlanNotReady);
        }
    };
    let group_id =
        selected_group(association, kind).ok_or(LightPlanExecutionError::LightPlanNotReady)?;
    let expected = match kind {
        LightMasterKind::Dark => MasterProductKind::Dark,
        LightMasterKind::Flat => MasterProductKind::Flat,
    };
    if !master_plan
        .products()
        .iter()
        .any(|product| product.source_group_id() == group_id && product.kind() == expected)
    {
        return Err(LightPlanExecutionError::InvalidMasterAssociation {
            light_group_id: light_group_id.to_owned(),
            master_group_id: group_id.to_owned(),
        });
    }
    Ok(())
}

fn validate_runtime_directories(
    request: &LightPlanExecutionRequest,
) -> Result<(), LightPlanExecutionError> {
    for (role, path) in [
        ("session root", request.session_root.as_path()),
        ("master directory", request.master_directory.as_path()),
        ("Light output directory", request.output_directory.as_path()),
    ] {
        let metadata =
            fs::symlink_metadata(path).map_err(LightPlanExecutionError::InspectDirectory)?;
        if !metadata.file_type().is_dir() {
            return Err(LightPlanExecutionError::DirectoryNotPhysical { role });
        }
    }
    Ok(())
}

fn load_master_sources(
    request: &LightPlanExecutionRequest,
    manifest_sha256: &str,
    master_plan_sha256: &str,
) -> Result<BTreeMap<String, PipelineSource>, LightPlanExecutionError> {
    let selected_groups = selected_master_groups(&request.light_plan)?;
    let mut sources = BTreeMap::new();
    for product in request.master_plan.products() {
        if !selected_groups.contains(product.source_group_id()) {
            continue;
        }
        let group = find_group(&request.manifest, product.source_group_id())?;
        let path = request
            .master_directory
            .join(product_file_name(product.kind(), group.id()));
        let metadata =
            fs::symlink_metadata(&path).map_err(|_| LightPlanExecutionError::MasterMissing {
                group_id: group.id().to_owned(),
            })?;
        if !metadata.file_type().is_file() {
            return Err(LightPlanExecutionError::MasterMissing {
                group_id: group.id().to_owned(),
            });
        }
        validate_master_provenance(
            &path,
            manifest_sha256,
            master_plan_sha256,
            group,
            product.kind(),
        )?;
        let fingerprint = fingerprint_path(&path).map_err(|source| {
            LightPlanExecutionError::FingerprintMaster {
                group_id: group.id().to_owned(),
                source,
            }
        })?;
        sources.insert(
            group.id().to_owned(),
            PipelineSource::new(path, fingerprint),
        );
    }
    Ok(sources)
}

fn selected_master_groups(
    plan: &LightCalibrationPlan,
) -> Result<BTreeSet<&str>, LightPlanExecutionError> {
    let mut groups = BTreeSet::new();
    for product in plan.products() {
        groups.insert(
            selected_group(product.dark(), LightMasterKind::Dark)
                .ok_or(LightPlanExecutionError::LightPlanNotReady)?,
        );
        groups.insert(
            selected_group(product.flat(), LightMasterKind::Flat)
                .ok_or(LightPlanExecutionError::LightPlanNotReady)?,
        );
    }
    Ok(groups)
}

fn validate_master_provenance(
    path: &Path,
    manifest_sha256: &str,
    master_plan_sha256: &str,
    group: &ManifestGroup,
    kind: MasterProductKind,
) -> Result<(), LightPlanExecutionError> {
    let file = File::open(path).map_err(|source| LightPlanExecutionError::OpenMaster {
        group_id: group.id().to_owned(),
        source: ImageReadError::Io(source),
    })?;
    let reader =
        PrimaryImageReader::open(file, HeaderReadOptions::default()).map_err(|source| {
            LightPlanExecutionError::OpenMaster {
                group_id: group.id().to_owned(),
                source,
            }
        })?;
    let header = reader.report().header();
    let algorithm = match kind {
        MasterProductKind::Dark => STRICT_MEAN_ALGORITHM_ID,
        MasterProductKind::Flat => STRICT_FLAT_MASTER_ALGORITHM_ID,
        MasterProductKind::Bias => {
            return Err(LightPlanExecutionError::MasterProvenanceMismatch {
                group_id: group.id().to_owned(),
            });
        }
    };
    let source_count = i64::try_from(group.files().len()).ok();
    if header.string("AETHMAN") != Some(manifest_sha256)
        || header.string("AETHPLN") != Some(master_plan_sha256)
        || header.string("AETHGRP") != Some(group.id())
        || header.string("AETHALG") != Some(algorithm)
        || header.integer("AETHSRC") != source_count
    {
        return Err(LightPlanExecutionError::MasterProvenanceMismatch {
            group_id: group.id().to_owned(),
        });
    }
    Ok(())
}

fn group_sources(
    request: &LightPlanExecutionRequest,
    group: &ManifestGroup,
) -> Result<Vec<PipelineSource>, LightPlanExecutionError> {
    let mut sources = Vec::new();
    sources
        .try_reserve_exact(group.files().len())
        .map_err(|_| LightPlanExecutionError::AllocationFailed)?;
    for relative_path in group.files() {
        let file = find_file(&request.manifest, relative_path).ok_or_else(|| {
            LightPlanExecutionError::MissingManifestFile {
                relative_path: relative_path.clone(),
            }
        })?;
        sources.push(PipelineSource::new(
            request.session_root.join(relative_path),
            file.fingerprint().clone(),
        ));
    }
    Ok(sources)
}

fn planned_destinations(
    request: &LightPlanExecutionRequest,
) -> Result<BTreeMap<String, PathBuf>, LightPlanExecutionError> {
    let mut destinations = BTreeMap::new();
    let mut portable_names = BTreeSet::new();
    for product in request.light_plan.products() {
        let file_name = light_product_file_name(product.source_group_id());
        if !portable_names.insert(file_name.to_ascii_lowercase()) {
            return Err(LightPlanExecutionError::DestinationNameCollision { file_name });
        }
        destinations.insert(
            product.source_group_id().to_owned(),
            request.output_directory.join(file_name),
        );
    }
    Ok(destinations)
}

fn preflight_destinations(
    destinations: &BTreeMap<String, PathBuf>,
) -> Result<(), LightPlanExecutionError> {
    for path in destinations.values() {
        match fs::symlink_metadata(path) {
            Ok(_) => return Err(LightPlanExecutionError::DestinationExists { path: path.clone() }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(LightPlanExecutionError::InspectDirectory(error)),
        }
    }
    Ok(())
}

fn revalidate_all_inputs(
    request: &LightPlanExecutionRequest,
    masters: &BTreeMap<String, PipelineSource>,
) -> Result<(), LightPlanExecutionError> {
    for product in request.light_plan.products() {
        let group = find_group(&request.manifest, product.source_group_id())?;
        for relative_path in group.files() {
            let file = find_file(&request.manifest, relative_path).ok_or_else(|| {
                LightPlanExecutionError::MissingManifestFile {
                    relative_path: relative_path.clone(),
                }
            })?;
            let actual =
                fingerprint_path(&request.session_root.join(relative_path)).map_err(|_| {
                    LightPlanExecutionError::SourceChanged {
                        identity: relative_path.clone(),
                    }
                })?;
            if &actual != file.fingerprint() {
                return Err(LightPlanExecutionError::SourceChanged {
                    identity: relative_path.clone(),
                });
            }
        }
    }
    for (group_id, source) in masters {
        let actual = fingerprint_path(source.path()).map_err(|_| {
            LightPlanExecutionError::SourceChanged {
                identity: group_id.clone(),
            }
        })?;
        if &actual != source.fingerprint() {
            return Err(LightPlanExecutionError::SourceChanged {
                identity: group_id.clone(),
            });
        }
    }
    Ok(())
}

fn fingerprint_path(path: &Path) -> Result<SourceFingerprint, FingerprintError> {
    let mut file = File::open(path).map_err(FingerprintError::Io)?;
    fingerprint_reader(&mut file)
}

fn selected_group(association: &LightMasterAssociation, kind: LightMasterKind) -> Option<&str> {
    match association {
        LightMasterAssociation::Matched {
            group_id,
            kind: selected_kind,
            ..
        } if *selected_kind == kind => Some(group_id),
        LightMasterAssociation::Matched { .. } | LightMasterAssociation::Unresolved { .. } => None,
    }
}

fn find_group<'a>(
    manifest: &'a SessionManifest,
    group_id: &str,
) -> Result<&'a ManifestGroup, LightPlanExecutionError> {
    manifest
        .groups()
        .iter()
        .find(|group| group.id() == group_id)
        .ok_or_else(|| LightPlanExecutionError::InvalidGroup {
            group_id: group_id.to_owned(),
        })
}

fn find_file<'a>(manifest: &'a SessionManifest, relative_path: &str) -> Option<&'a ManifestFile> {
    manifest
        .files()
        .iter()
        .find(|file| file.relative_path() == relative_path)
}

fn light_product_file_name(group_id: &str) -> String {
    format!("integrated-light-{group_id}.fits")
}

fn publish_product_set(
    staged_by_group: &BTreeMap<String, PathBuf>,
    destinations: &BTreeMap<String, PathBuf>,
    output_directory: &Path,
    cancellation: &CancellationToken,
) -> Result<(), LightPlanExecutionError> {
    let mut published = Vec::new();
    published
        .try_reserve_exact(staged_by_group.len())
        .map_err(|_| LightPlanExecutionError::AllocationFailed)?;
    for (group_id, staged) in staged_by_group {
        if let Err(cancelled) = cancellation.checkpoint() {
            rollback_publications(&published)?;
            return Err(LightPlanExecutionError::Cancelled(cancelled));
        }
        let destination = &destinations[group_id];
        if let Err(source) = fs::hard_link(staged, destination) {
            rollback_publications(&published)?;
            return Err(LightPlanExecutionError::PublishProduct {
                group_id: group_id.clone(),
                source,
            });
        }
        published.push(destination.clone());
    }
    if let Err(source) = sync_output_directory(output_directory) {
        rollback_publications(&published)?;
        return Err(LightPlanExecutionError::SyncOutputDirectory(source));
    }
    Ok(())
}

fn rollback_publications(paths: &[PathBuf]) -> Result<(), LightPlanExecutionError> {
    for path in paths.iter().rev() {
        fs::remove_file(path).map_err(|source| LightPlanExecutionError::RollbackPublication {
            path: path.clone(),
            source,
        })?;
    }
    Ok(())
}

#[cfg(unix)]
fn sync_output_directory(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_output_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

struct StagingDirectory {
    path: PathBuf,
}

impl StagingDirectory {
    fn create(output_directory: &Path) -> Result<Self, LightPlanExecutionError> {
        for _ in 0..MAX_STAGING_DIRECTORY_ATTEMPTS {
            let sequence = STAGING_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = output_directory.join(format!(
                ".aether-light-stage-{}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(LightPlanExecutionError::CreateStagingDirectory(error)),
            }
        }
        Err(LightPlanExecutionError::CreateStagingDirectory(
            std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "exhausted private Light staging names",
            ),
        ))
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for StagingDirectory {
    fn drop(&mut self) {
        let _ignored = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::sync::atomic::{AtomicU64, Ordering};

    use aether_calibration::FlatNormalizationParameters;
    use aether_core::{Dimensions, ScientificImage};
    use aether_fits::{ImageRegion, write_f64_primary_atomic_new};
    use aether_metadata::{
        BayerPattern, Binning, CameraModel, CanonicalMetadata, CanonicalValue, Confidence,
    };
    use aether_session::{
        ClassificationPolicy, FlatPedestalPolicy, LightCalibrationPlanOptions, ManifestFile,
        MasterPlanOptions, StrictGroupingKey, classify_frame,
    };

    use crate::{MasterPlanExecutionRequest, run_master_plan};

    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn StdError>>;

    static TEST_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new() -> std::io::Result<Self> {
            let sequence = TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "aether-runtime-light-plan-{}-{sequence}",
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

    fn exact<T>(value: T, keyword: &str) -> CanonicalValue<T> {
        CanonicalValue::new(value, keyword, Confidence::Exact)
    }

    fn metadata(frame_type: FrameType) -> CanonicalMetadata {
        CanonicalMetadata {
            camera: Some(exact(CameraModel::ZwoAsi294McPro, "INSTRUME")),
            frame_type: Some(exact(frame_type, "IMAGETYP")),
            exposure_seconds: Some(exact(2.0, "EXPTIME")),
            sensor_temperature_c: Some(exact(-10.0, "CCD-TEMP")),
            set_temperature_c: Some(exact(-10.0, "SET-TEMP")),
            gain: Some(exact(120.0, "GAIN")),
            offset: Some(exact(30.0, "OFFSET")),
            binning: Some(exact(Binning { x: 1, y: 1 }, "XBINNING")),
            filter: Some(exact("UVIR".to_owned(), "FILTER")),
            bayer_pattern: Some(exact(BayerPattern::Rggb, "BAYERPAT")),
            issues: Vec::new(),
        }
    }

    fn write_source(
        root: &Path,
        relative_path: &str,
        values: Vec<f64>,
        frame_type: FrameType,
    ) -> TestResult<ManifestFile> {
        let path = root.join(relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let dimensions = Dimensions::new(values.len(), 1, 1)?;
        write_f64_primary_atomic_new(&path, &ScientificImage::from_pixels(dimensions, values)?)?;
        let mut source = File::open(&path)?;
        let fingerprint = fingerprint_reader(&mut source)?;
        let metadata = metadata(frame_type);
        let classification = classify_frame(Path::new(relative_path), &metadata);
        Ok(ManifestFile::from_analysis(
            relative_path,
            fingerprint,
            vec![dimensions.width() as u64, 1],
            metadata,
            Vec::new(),
            classification,
            ClassificationPolicy::RequireAgreement,
        )?)
    }

    fn grouped_manifest(root: &Path) -> TestResult<SessionManifest> {
        let dark = write_source(root, "darks/dark.fits", vec![1.0; 3], FrameType::Dark)?;
        let flat = write_source(root, "flats/flat.fits", vec![3.0; 3], FrameType::Flat)?;
        let light_one = write_source(root, "lights/light-1.fits", vec![5.0; 3], FrameType::Light)?;
        let light_two = write_source(root, "lights/light-2.fits", vec![7.0; 3], FrameType::Light)?;
        let groups = vec![
            ManifestGroup::new(
                "dark",
                StrictGroupingKey::from_metadata(FrameType::Dark, dark.metadata(), dark.axes())?,
                vec![dark.relative_path().to_owned()],
                Vec::new(),
                None,
            )?,
            ManifestGroup::new(
                "flat",
                StrictGroupingKey::from_metadata(FrameType::Flat, flat.metadata(), flat.axes())?,
                vec![flat.relative_path().to_owned()],
                Vec::new(),
                None,
            )?,
            ManifestGroup::new(
                "light",
                StrictGroupingKey::from_metadata(
                    FrameType::Light,
                    light_one.metadata(),
                    light_one.axes(),
                )?,
                vec![
                    light_one.relative_path().to_owned(),
                    light_two.relative_path().to_owned(),
                ],
                Vec::new(),
                None,
            )?,
        ];
        Ok(SessionManifest::new(
            ClassificationPolicy::RequireAgreement,
            vec![dark, flat, light_one, light_two],
            groups,
        )?)
    }

    fn plans(manifest: &SessionManifest) -> TestResult<(MasterPlan, LightCalibrationPlan)> {
        let master_plan = MasterPlan::from_manifest(
            manifest,
            MasterPlanOptions::new(FlatPedestalPolicy::RequireMatchedDark, 0.0, 0.0)?,
        )?;
        let light_plan = LightCalibrationPlan::from_manifest_and_master_plan(
            manifest,
            &master_plan,
            LightCalibrationPlanOptions::new(0.0)?,
        )?;
        Ok((master_plan, light_plan))
    }

    fn build_masters(
        root: &Path,
        output: &Path,
        manifest: &SessionManifest,
        master_plan: &MasterPlan,
    ) -> TestResult {
        let request = MasterPlanExecutionRequest::new(
            root.to_path_buf(),
            output.to_path_buf(),
            manifest.clone(),
            master_plan.clone(),
            FlatNormalizationParameters::new(3, 1.0e-12)?,
        )?
        .with_tile_shape(2, 1)?;
        run_master_plan(
            &request,
            &CancellationToken::new(),
            &MemoryBudget::new(1_048_576)?,
            |_| {},
        )?;
        Ok(())
    }

    #[test]
    fn calibrates_integrates_and_publishes_with_exact_provenance() -> TestResult {
        let directory = TestDirectory::new()?;
        let root = directory.path.join("session");
        let masters = directory.path.join("masters");
        let output = directory.path.join("lights");
        fs::create_dir(&root)?;
        fs::create_dir(&masters)?;
        fs::create_dir(&output)?;
        let manifest = grouped_manifest(&root)?;
        let (master_plan, light_plan) = plans(&manifest)?;
        build_masters(&root, &masters, &manifest, &master_plan)?;
        let request = LightPlanExecutionRequest::new(
            root,
            masters,
            output.clone(),
            manifest,
            master_plan,
            light_plan,
            CalibrationParameters::new(1.0e-12)?,
        )?
        .with_tile_shape(2, 1)?;
        let mut progress = Vec::new();

        let result = run_light_plan(
            &request,
            &CancellationToken::new(),
            &MemoryBudget::new(1_048_576)?,
            |event| progress.push(event),
        )?;

        assert_eq!(result.products().len(), 1);
        let product = &result.products()[0];
        assert_eq!(product.group_id(), "light");
        assert_eq!(product.dark_group_id(), "dark");
        assert_eq!(product.flat_group_id(), "flat");
        assert_eq!(product.tiles_processed(), 2);
        assert_eq!(product.tiles_reused(), 0);
        assert!(progress.iter().all(|event| {
            event.product_index() == 0 && event.product_count() == 1 && event.group_id() == "light"
        }));

        let mut reader = PrimaryImageReader::open(
            File::open(output.join("integrated-light-light.fits"))?,
            HeaderReadOptions::default(),
        )?;
        let header = reader.report().header();
        assert_eq!(header.string("AETHMAN"), Some(result.manifest_sha256()));
        assert_eq!(header.string("AETHPLN"), Some(result.light_plan_sha256()));
        assert_eq!(header.string("AETHGRP"), Some("light"));
        assert_eq!(header.string("AETHALG"), Some(STRICT_MEAN_ALGORITHM_ID));
        assert_eq!(header.integer("AETHSRC"), Some(2));
        let image = reader.read_region_image(ImageRegion::new(0, 0, 0, 3, 1))?;
        assert_eq!(
            image
                .pixels()
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>(),
            [5.0_f64; 3].map(f64::to_bits)
        );
        assert!(
            fs::read_dir(output)?
                .collect::<Result<Vec<_>, _>>()?
                .iter()
                .all(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        );
        Ok(())
    }

    #[test]
    fn stale_master_provenance_blocks_execution_without_output() -> TestResult {
        let directory = TestDirectory::new()?;
        let root = directory.path.join("session");
        let masters = directory.path.join("masters");
        let output = directory.path.join("lights");
        fs::create_dir(&root)?;
        fs::create_dir(&masters)?;
        fs::create_dir(&output)?;
        let manifest = grouped_manifest(&root)?;
        let (master_plan, light_plan) = plans(&manifest)?;
        build_masters(&root, &masters, &manifest, &master_plan)?;
        let stale_flat = masters.join("master-flat-flat.fits");
        fs::remove_file(&stale_flat)?;
        write_f64_primary_atomic_new(
            &stale_flat,
            &ScientificImage::from_pixels(Dimensions::new(3, 1, 1)?, vec![1.0; 3])?,
        )?;
        let request = LightPlanExecutionRequest::new(
            root,
            masters,
            output.clone(),
            manifest,
            master_plan,
            light_plan,
            CalibrationParameters::new(0.0)?,
        )?;

        let result = run_light_plan(
            &request,
            &CancellationToken::new(),
            &MemoryBudget::new(1_048_576)?,
            |_| {},
        );

        assert!(matches!(
            result,
            Err(LightPlanExecutionError::MasterProvenanceMismatch { group_id })
                if group_id == "flat"
        ));
        assert!(fs::read_dir(output)?.next().is_none());
        Ok(())
    }

    #[test]
    fn existing_destination_and_early_cancellation_never_publish() -> TestResult {
        let directory = TestDirectory::new()?;
        let root = directory.path.join("session");
        let masters = directory.path.join("masters");
        let output = directory.path.join("lights");
        fs::create_dir(&root)?;
        fs::create_dir(&masters)?;
        fs::create_dir(&output)?;
        let manifest = grouped_manifest(&root)?;
        let (master_plan, light_plan) = plans(&manifest)?;
        build_masters(&root, &masters, &manifest, &master_plan)?;
        let sentinel = output.join("integrated-light-light.fits");
        fs::write(&sentinel, b"keep")?;
        let request = LightPlanExecutionRequest::new(
            root,
            masters,
            output.clone(),
            manifest,
            master_plan,
            light_plan,
            CalibrationParameters::new(0.0)?,
        )?;
        let blocked = run_light_plan(
            &request,
            &CancellationToken::new(),
            &MemoryBudget::new(1_048_576)?,
            |_| {},
        );
        assert!(matches!(
            blocked,
            Err(LightPlanExecutionError::DestinationExists { .. })
        ));
        assert_eq!(fs::read(&sentinel)?, b"keep");

        fs::remove_file(&sentinel)?;
        let cancellation = CancellationToken::new();
        assert!(cancellation.cancel());
        let cancelled = run_light_plan(
            &request,
            &cancellation,
            &MemoryBudget::new(1_048_576)?,
            |_| {},
        );
        assert!(matches!(
            cancelled,
            Err(LightPlanExecutionError::Cancelled(_))
        ));
        assert!(fs::read_dir(output)?.next().is_none());
        Ok(())
    }

    #[test]
    fn request_rejects_a_structurally_valid_but_tampered_light_plan() -> TestResult {
        let directory = TestDirectory::new()?;
        let root = directory.path.join("session");
        let masters = directory.path.join("masters");
        let output = directory.path.join("lights");
        fs::create_dir(&root)?;
        fs::create_dir(&masters)?;
        fs::create_dir(&output)?;
        let manifest = grouped_manifest(&root)?;
        let (master_plan, light_plan) = plans(&manifest)?;
        let mut encoded: serde_json::Value = serde_json::from_slice(&light_plan.to_json_pretty()?)?;
        encoded["products"][0]["source_group_id"] = serde_json::json!("dark");
        let tampered = LightCalibrationPlan::from_json_slice(&serde_json::to_vec(&encoded)?)?;

        let result = LightPlanExecutionRequest::new(
            root,
            masters,
            output,
            manifest,
            master_plan,
            tampered,
            CalibrationParameters::new(0.0)?,
        );

        assert!(matches!(
            result,
            Err(LightPlanExecutionError::LightPlanMismatch)
        ));
        Ok(())
    }
}
