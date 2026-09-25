use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use aether_calibration::{
    CalibrationMasterKind, FlatNormalizationParameters, FlatNormalizationSupport,
};
use aether_core::ImageStatistics;
use aether_fits::{FitsOutputProvenance, FitsProvenanceError, FitsWriteSummary, HeaderReadOptions};
use aether_session::{
    FingerprintError, FlatPedestalAssociation, ManifestError, ManifestFile, ManifestGroup,
    MasterPlan, MasterPlanError, MasterProductKind, SessionManifest, fingerprint_reader,
};

use crate::{
    CancellationToken, Cancelled, MemoryBudget, PipelineSource, ProgressEvent,
    STRICT_FLAT_MASTER_ALGORITHM_ID, STRICT_MEAN_ALGORITHM_ID, StrictFlatMasterRequest,
    StrictPipelineError, run_strict_flat_master_pipeline, run_strict_master_pipeline,
};

const MAX_STAGING_DIRECTORY_ATTEMPTS: usize = 128;
static STAGING_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Complete, validated request for executing one canonical master plan.
#[derive(Clone, Debug)]
pub struct MasterPlanExecutionRequest {
    session_root: PathBuf,
    output_directory: PathBuf,
    manifest: Arc<SessionManifest>,
    plan: MasterPlan,
    flat_normalization: FlatNormalizationParameters,
    tile_width: usize,
    tile_height: usize,
}

impl MasterPlanExecutionRequest {
    /// Binds one manifest, its exact plan, runtime root, and output directory.
    ///
    /// Paths must be absolute. The plan digest binding is validated immediately;
    /// filesystem type and destination-collision checks are repeated at run time.
    /// The default tile shape is 256 by 256 pixels.
    ///
    /// # Errors
    ///
    /// Returns a typed error for relative paths, a plan built from another
    /// manifest, an unresolved flat association, or an invalid product graph.
    pub fn new(
        session_root: PathBuf,
        output_directory: PathBuf,
        manifest: SessionManifest,
        plan: MasterPlan,
        flat_normalization: FlatNormalizationParameters,
    ) -> Result<Self, MasterPlanExecutionError> {
        Self::new_shared(
            session_root,
            output_directory,
            Arc::new(manifest),
            plan,
            flat_normalization,
        )
    }

    /// Binds a shared manifest without copying its potentially large file set.
    ///
    /// This is the preferred desktop entry point once directory ingestion has
    /// moved the immutable manifest into native session state.
    ///
    /// # Errors
    ///
    /// Returns the same validation failures as [`Self::new`].
    pub fn new_shared(
        session_root: PathBuf,
        output_directory: PathBuf,
        manifest: Arc<SessionManifest>,
        plan: MasterPlan,
        flat_normalization: FlatNormalizationParameters,
    ) -> Result<Self, MasterPlanExecutionError> {
        if !session_root.is_absolute() {
            return Err(MasterPlanExecutionError::SessionRootNotAbsolute);
        }
        if !output_directory.is_absolute() {
            return Err(MasterPlanExecutionError::OutputDirectoryNotAbsolute);
        }
        validate_plan_graph(&manifest, &plan)?;
        Ok(Self {
            session_root,
            output_directory,
            manifest,
            plan,
            flat_normalization,
            tile_width: 256,
            tile_height: 256,
        })
    }

    /// Replaces the default tile shape used by every product.
    ///
    /// # Errors
    ///
    /// Returns an error when either extent is zero.
    pub fn with_tile_shape(
        mut self,
        width: usize,
        height: usize,
    ) -> Result<Self, MasterPlanExecutionError> {
        if width == 0 || height == 0 {
            return Err(MasterPlanExecutionError::ZeroTileExtent { width, height });
        }
        self.tile_width = width;
        self.tile_height = height;
        Ok(self)
    }

    /// Canonical plan being executed.
    #[must_use]
    pub const fn plan(&self) -> &MasterPlan {
        &self.plan
    }

    /// Absolute destination directory for the complete product set.
    #[must_use]
    pub fn output_directory(&self) -> &Path {
        &self.output_directory
    }
}

/// Product-local progress enriched with canonical plan identity.
#[derive(Clone, Debug, PartialEq)]
pub struct MasterPlanProgressEvent {
    product_index: usize,
    product_count: usize,
    group_id: String,
    kind: MasterProductKind,
    stage: ProgressEvent,
}

impl MasterPlanProgressEvent {
    /// Zero-based execution-order index. Bias and dark dependencies precede flats.
    #[must_use]
    pub const fn product_index(&self) -> usize {
        self.product_index
    }

    /// Total products in the canonical plan.
    #[must_use]
    pub const fn product_count(&self) -> usize {
        self.product_count
    }

    /// Exact manifest group represented by the active product.
    #[must_use]
    pub fn group_id(&self) -> &str {
        &self.group_id
    }

    /// Scientific role of the active product.
    #[must_use]
    pub const fn kind(&self) -> MasterProductKind {
        self.kind
    }

    /// Underlying stage lifecycle and work-unit event.
    #[must_use]
    pub const fn stage(&self) -> &ProgressEvent {
        &self.stage
    }
}

/// One atomically published master product.
#[derive(Clone, Debug, PartialEq)]
pub struct MasterProductExecutionResult {
    group_id: String,
    kind: MasterProductKind,
    output: PathBuf,
    statistics: ImageStatistics,
    write_summary: FitsWriteSummary,
    normalization: Option<f64>,
    normalization_support: Option<FlatNormalizationSupport>,
}

impl MasterProductExecutionResult {
    /// Exact source group represented by this product.
    #[must_use]
    pub fn group_id(&self) -> &str {
        &self.group_id
    }

    /// Scientific product role.
    #[must_use]
    pub const fn kind(&self) -> MasterProductKind {
        self.kind
    }

    /// Final public output path.
    #[must_use]
    pub fn output(&self) -> &Path {
        &self.output
    }

    /// Exact statistics measured by private FITS readback before publication.
    #[must_use]
    pub const fn statistics(&self) -> ImageStatistics {
        self.statistics
    }

    /// FITS sample, checksum, and byte accounting.
    #[must_use]
    pub const fn write_summary(&self) -> FitsWriteSummary {
        self.write_summary
    }

    /// Exact global flat normalization scalar; absent for bias and dark masters.
    #[must_use]
    pub const fn normalization(&self) -> Option<f64> {
        self.normalization
    }

    /// Flat normalization accounting; absent for bias and dark masters.
    #[must_use]
    pub const fn normalization_support(&self) -> Option<FlatNormalizationSupport> {
        self.normalization_support
    }
}

/// Complete canonically ordered result of one master-plan transaction.
#[derive(Clone, Debug, PartialEq)]
pub struct MasterPlanExecutionResult {
    manifest_sha256: String,
    plan_sha256: String,
    products: Vec<MasterProductExecutionResult>,
}

impl MasterPlanExecutionResult {
    /// SHA-256 of the canonical source manifest.
    #[must_use]
    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }

    /// SHA-256 of the exact canonical plan bytes.
    #[must_use]
    pub fn plan_sha256(&self) -> &str {
        &self.plan_sha256
    }

    /// Published products ordered by source group identifier.
    #[must_use]
    pub fn products(&self) -> &[MasterProductExecutionResult] {
        &self.products
    }
}

/// Failure while validating or executing an exact master-plan transaction.
#[derive(Debug)]
pub enum MasterPlanExecutionError {
    /// Session root must be an absolute runtime-only path.
    SessionRootNotAbsolute,
    /// Output directory must be an absolute runtime-only path.
    OutputDirectoryNotAbsolute,
    /// Tile extents must both be non-zero.
    ZeroTileExtent {
        /// Rejected width.
        width: usize,
        /// Rejected height.
        height: usize,
    },
    /// Canonical manifest encoding or hashing failed.
    Manifest(ManifestError),
    /// Canonical plan encoding or hashing failed.
    Plan(MasterPlanError),
    /// Plan was generated from different canonical manifest bytes.
    ManifestDigestMismatch,
    /// Product refers to a group absent from the bound manifest.
    MissingGroup {
        /// Missing group identifier.
        group_id: String,
    },
    /// Product role disagrees with the manifest grouping key.
    ProductKindMismatch {
        /// Incoherent group identifier.
        group_id: String,
    },
    /// A flat has no unique plan-selected pedestal.
    UnresolvedFlatPedestal {
        /// Blocked flat group.
        group_id: String,
    },
    /// Selected pedestal does not identify a bias or dark product.
    InvalidPedestalProduct {
        /// Flat group requiring the dependency.
        flat_group_id: String,
        /// Invalid dependency group.
        pedestal_group_id: String,
    },
    /// Manifest group refers to an absent manifest file record.
    MissingManifestFile {
        /// Portable missing path.
        relative_path: String,
    },
    /// Two portable product names collide on case-insensitive filesystems.
    DestinationNameCollision {
        /// Conflicting filename.
        file_name: String,
    },
    /// Session-root metadata could not be inspected.
    InspectSessionRoot(std::io::Error),
    /// Session root is not a physical directory.
    SessionRootNotDirectory,
    /// Output-directory metadata could not be inspected.
    InspectOutputDirectory(std::io::Error),
    /// Output path is not a physical directory.
    OutputDirectoryNotDirectory,
    /// A destination already exists before the transaction begins.
    DestinationExists {
        /// Existing destination path.
        path: PathBuf,
    },
    /// A private transaction directory could not be created.
    CreateStagingDirectory(std::io::Error),
    /// Runtime provenance validation failed.
    Provenance(FitsProvenanceError),
    /// A product pipeline failed before batch publication.
    ProductPipeline {
        /// Product group that failed.
        group_id: String,
        /// Typed executor failure.
        source: StrictPipelineError,
    },
    /// A generated pedestal master could not be reopened.
    OpenGeneratedPedestal {
        /// Pedestal group.
        group_id: String,
        /// Operating-system failure.
        source: std::io::Error,
    },
    /// A generated pedestal master could not be fingerprinted.
    FingerprintGeneratedPedestal {
        /// Pedestal group.
        group_id: String,
        /// Streaming fingerprint failure.
        source: FingerprintError,
    },
    /// Final create-new hard-link publication failed.
    PublishProduct {
        /// Product group that could not be published.
        group_id: String,
        /// Operating-system failure.
        source: std::io::Error,
    },
    /// The published directory entry set could not be durably synchronized.
    SyncOutputDirectory(std::io::Error),
    /// Rollback could not remove a link created by this transaction.
    RollbackPublication {
        /// Link that remains published.
        path: PathBuf,
        /// Operating-system failure.
        source: std::io::Error,
    },
    /// Execution stopped at a cooperative checkpoint.
    Cancelled(Cancelled),
    /// Bounded result or progress storage could not be reserved.
    AllocationFailed,
}

impl Display for MasterPlanExecutionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionRootNotAbsolute => formatter.write_str("session root must be absolute"),
            Self::OutputDirectoryNotAbsolute => {
                formatter.write_str("master output directory must be absolute")
            }
            Self::ZeroTileExtent { width, height } => {
                write!(
                    formatter,
                    "master tile extent must be non-zero, received {width}x{height}"
                )
            }
            Self::Manifest(error) => Display::fmt(error, formatter),
            Self::Plan(error) => Display::fmt(error, formatter),
            Self::ManifestDigestMismatch => {
                formatter.write_str("master plan does not match the canonical session manifest")
            }
            Self::MissingGroup { group_id } => {
                write!(formatter, "master product group `{group_id}` is absent")
            }
            Self::ProductKindMismatch { group_id } => {
                write!(
                    formatter,
                    "master product role disagrees for group `{group_id}`"
                )
            }
            Self::UnresolvedFlatPedestal { group_id } => {
                write!(formatter, "flat group `{group_id}` has no unique pedestal")
            }
            Self::InvalidPedestalProduct {
                flat_group_id,
                pedestal_group_id,
            } => write!(
                formatter,
                "flat `{flat_group_id}` selects invalid pedestal product `{pedestal_group_id}`"
            ),
            Self::MissingManifestFile { relative_path } => {
                write!(
                    formatter,
                    "manifest group references missing file `{relative_path}`"
                )
            }
            Self::DestinationNameCollision { file_name } => {
                write!(formatter, "master destination name collides: `{file_name}`")
            }
            Self::InspectSessionRoot(error) => {
                write!(formatter, "cannot inspect session root: {error}")
            }
            Self::SessionRootNotDirectory => {
                formatter.write_str("session root is not a physical directory")
            }
            Self::InspectOutputDirectory(error) => {
                write!(formatter, "cannot inspect master output directory: {error}")
            }
            Self::OutputDirectoryNotDirectory => {
                formatter.write_str("master output path is not a physical directory")
            }
            Self::DestinationExists { path } => {
                write!(
                    formatter,
                    "master destination already exists: {}",
                    path.display()
                )
            }
            Self::CreateStagingDirectory(error) => {
                write!(
                    formatter,
                    "cannot create private master staging directory: {error}"
                )
            }
            Self::Provenance(error) => Display::fmt(error, formatter),
            Self::ProductPipeline { group_id, source } => {
                write!(formatter, "master product `{group_id}` failed: {source}")
            }
            Self::OpenGeneratedPedestal { group_id, source } => {
                write!(
                    formatter,
                    "cannot open generated pedestal `{group_id}`: {source}"
                )
            }
            Self::FingerprintGeneratedPedestal { group_id, source } => {
                write!(
                    formatter,
                    "cannot fingerprint generated pedestal `{group_id}`: {source}"
                )
            }
            Self::PublishProduct { group_id, source } => {
                write!(
                    formatter,
                    "cannot publish master product `{group_id}`: {source}"
                )
            }
            Self::SyncOutputDirectory(error) => {
                write!(
                    formatter,
                    "cannot synchronize master output directory: {error}"
                )
            }
            Self::RollbackPublication { path, source } => write!(
                formatter,
                "cannot roll back master publication `{}`: {source}",
                path.display()
            ),
            Self::Cancelled(error) => Display::fmt(error, formatter),
            Self::AllocationFailed => formatter.write_str("cannot allocate master execution state"),
        }
    }
}

impl Error for MasterPlanExecutionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Manifest(error) => Some(error),
            Self::Plan(error) => Some(error),
            Self::InspectSessionRoot(error)
            | Self::InspectOutputDirectory(error)
            | Self::CreateStagingDirectory(error) => Some(error),
            Self::Provenance(error) => Some(error),
            Self::ProductPipeline { source, .. } => Some(source),
            Self::OpenGeneratedPedestal { source, .. }
            | Self::PublishProduct { source, .. }
            | Self::RollbackPublication { source, .. } => Some(source),
            Self::SyncOutputDirectory(error) => Some(error),
            Self::FingerprintGeneratedPedestal { source, .. } => Some(source),
            Self::Cancelled(error) => Some(error),
            Self::SessionRootNotAbsolute
            | Self::OutputDirectoryNotAbsolute
            | Self::ZeroTileExtent { .. }
            | Self::ManifestDigestMismatch
            | Self::MissingGroup { .. }
            | Self::ProductKindMismatch { .. }
            | Self::UnresolvedFlatPedestal { .. }
            | Self::InvalidPedestalProduct { .. }
            | Self::MissingManifestFile { .. }
            | Self::DestinationNameCollision { .. }
            | Self::SessionRootNotDirectory
            | Self::OutputDirectoryNotDirectory
            | Self::DestinationExists { .. }
            | Self::AllocationFailed => None,
        }
    }
}

/// Executes every product in a validated master plan as one publication set.
///
/// Bias and dark dependencies execute before flats, independently of lexical
/// group order. Products are first written to a private sibling directory. Only
/// after all calculations and source revalidations succeed are create-new hard
/// links published to the requested directory. A publication failure removes
/// only links created by this transaction; existing paths are never modified.
///
/// # Errors
///
/// Returns a typed graph, filesystem, provenance, pipeline, cancellation, or
/// rollback failure. No final product is visible before every calculation has
/// succeeded.
pub fn run_master_plan<F>(
    request: &MasterPlanExecutionRequest,
    cancellation: &CancellationToken,
    memory: &MemoryBudget,
    mut progress: F,
) -> Result<MasterPlanExecutionResult, MasterPlanExecutionError>
where
    F: FnMut(MasterPlanProgressEvent),
{
    validate_runtime_directories(request)?;
    let manifest_sha256 = request
        .manifest
        .canonical_sha256()
        .map_err(MasterPlanExecutionError::Manifest)?;
    let plan_sha256 = request
        .plan
        .canonical_sha256()
        .map_err(MasterPlanExecutionError::Plan)?;
    let destinations = planned_destinations(request)?;
    preflight_destinations(&destinations)?;
    cancellation
        .checkpoint()
        .map_err(MasterPlanExecutionError::Cancelled)?;

    let staging = StagingDirectory::create(&request.output_directory)?;
    let product_count = request.plan.products().len();
    let mut results = Vec::new();
    results
        .try_reserve_exact(product_count)
        .map_err(|_| MasterPlanExecutionError::AllocationFailed)?;
    let mut staged_by_group: BTreeMap<String, PathBuf> = BTreeMap::new();

    let execution_order = request
        .plan
        .products()
        .iter()
        .filter(|product| product.kind() != MasterProductKind::Flat)
        .chain(
            request
                .plan
                .products()
                .iter()
                .filter(|product| product.kind() == MasterProductKind::Flat),
        );
    for (product_index, product) in execution_order.enumerate() {
        cancellation
            .checkpoint()
            .map_err(MasterPlanExecutionError::Cancelled)?;
        let group = find_group(&request.manifest, product.source_group_id())?;
        let sources = group_sources(request, group)?;
        let staged_output = staging
            .path()
            .join(product_file_name(product.kind(), group.id()));
        let public_output = destinations.get(group.id()).cloned().ok_or_else(|| {
            MasterPlanExecutionError::MissingGroup {
                group_id: group.id().to_owned(),
            }
        })?;
        let provenance = product_provenance(
            &manifest_sha256,
            &plan_sha256,
            group.id(),
            product.kind(),
            sources.len(),
        )?;
        let emit = |stage| {
            progress(MasterPlanProgressEvent {
                product_index,
                product_count,
                group_id: group.id().to_owned(),
                kind: product.kind(),
                stage,
            });
        };

        let result = match product.kind() {
            MasterProductKind::Bias | MasterProductKind::Dark => {
                let kind = match product.kind() {
                    MasterProductKind::Bias => CalibrationMasterKind::Bias,
                    MasterProductKind::Dark => CalibrationMasterKind::Dark,
                    MasterProductKind::Flat => unreachable!("flat handled by separate arm"),
                };
                let pipeline = crate::StrictMasterRequest::new(
                    kind,
                    sources,
                    staged_output.clone(),
                    provenance,
                )
                .and_then(|request_builder| {
                    request_builder.with_tile_shape(request.tile_width, request.tile_height)
                })
                .map(|request_builder| {
                    request_builder.with_header_policy(
                        HeaderReadOptions::default(),
                        request.manifest.fits_validation_mode(),
                    )
                })
                .map_err(|source| MasterPlanExecutionError::ProductPipeline {
                    group_id: group.id().to_owned(),
                    source,
                })?;
                let completed = run_strict_master_pipeline(&pipeline, cancellation, memory, emit)
                    .map_err(|source| MasterPlanExecutionError::ProductPipeline {
                    group_id: group.id().to_owned(),
                    source,
                })?;
                MasterProductExecutionResult {
                    group_id: group.id().to_owned(),
                    kind: product.kind(),
                    output: public_output,
                    statistics: completed.statistics(),
                    write_summary: completed.write_summary(),
                    normalization: None,
                    normalization_support: None,
                }
            }
            MasterProductKind::Flat => {
                let pedestal_group_id = selected_pedestal_group(product).ok_or_else(|| {
                    MasterPlanExecutionError::UnresolvedFlatPedestal {
                        group_id: group.id().to_owned(),
                    }
                })?;
                let pedestal_path = staged_by_group.get(pedestal_group_id).ok_or_else(|| {
                    MasterPlanExecutionError::InvalidPedestalProduct {
                        flat_group_id: group.id().to_owned(),
                        pedestal_group_id: pedestal_group_id.to_owned(),
                    }
                })?;
                let pedestal = generated_source(pedestal_group_id, pedestal_path)?;
                let pipeline = StrictFlatMasterRequest::new(
                    sources,
                    pedestal,
                    staged_output.clone(),
                    provenance,
                    request.flat_normalization,
                )
                .and_then(|request_builder| {
                    request_builder.with_tile_shape(request.tile_width, request.tile_height)
                })
                .map(|request_builder| {
                    request_builder.with_header_policy(
                        HeaderReadOptions::default(),
                        request.manifest.fits_validation_mode(),
                    )
                })
                .map_err(|source| MasterPlanExecutionError::ProductPipeline {
                    group_id: group.id().to_owned(),
                    source,
                })?;
                let completed =
                    run_strict_flat_master_pipeline(&pipeline, cancellation, memory, emit)
                        .map_err(|source| MasterPlanExecutionError::ProductPipeline {
                            group_id: group.id().to_owned(),
                            source,
                        })?;
                let common = completed.pipeline();
                MasterProductExecutionResult {
                    group_id: group.id().to_owned(),
                    kind: product.kind(),
                    output: public_output,
                    statistics: common.statistics(),
                    write_summary: common.write_summary(),
                    normalization: Some(completed.normalization()),
                    normalization_support: Some(completed.support()),
                }
            }
        };
        staged_by_group.insert(group.id().to_owned(), staged_output);
        results.push(result);
    }

    cancellation
        .checkpoint()
        .map_err(MasterPlanExecutionError::Cancelled)?;
    publish_product_set(
        &staged_by_group,
        &destinations,
        &request.output_directory,
        cancellation,
    )?;
    results.sort_by(|left, right| left.group_id.cmp(&right.group_id));
    Ok(MasterPlanExecutionResult {
        manifest_sha256,
        plan_sha256,
        products: results,
    })
}

fn validate_plan_graph(
    manifest: &SessionManifest,
    plan: &MasterPlan,
) -> Result<(), MasterPlanExecutionError> {
    let manifest_sha256 = manifest
        .canonical_sha256()
        .map_err(MasterPlanExecutionError::Manifest)?;
    if plan.manifest_sha256() != manifest_sha256 {
        return Err(MasterPlanExecutionError::ManifestDigestMismatch);
    }
    for product in plan.products() {
        let group = find_group(manifest, product.source_group_id())?;
        if !product_matches_group(product.kind(), group) {
            return Err(MasterPlanExecutionError::ProductKindMismatch {
                group_id: group.id().to_owned(),
            });
        }
        if product.kind() == MasterProductKind::Flat {
            let pedestal_group_id = selected_pedestal_group(product).ok_or_else(|| {
                MasterPlanExecutionError::UnresolvedFlatPedestal {
                    group_id: group.id().to_owned(),
                }
            })?;
            let Some(pedestal_product) = plan
                .products()
                .iter()
                .find(|candidate| candidate.source_group_id() == pedestal_group_id)
            else {
                return Err(MasterPlanExecutionError::InvalidPedestalProduct {
                    flat_group_id: group.id().to_owned(),
                    pedestal_group_id: pedestal_group_id.to_owned(),
                });
            };
            if !matches!(
                pedestal_product.kind(),
                MasterProductKind::Bias | MasterProductKind::Dark
            ) {
                return Err(MasterPlanExecutionError::InvalidPedestalProduct {
                    flat_group_id: group.id().to_owned(),
                    pedestal_group_id: pedestal_group_id.to_owned(),
                });
            }
        }
        for relative_path in group.files() {
            if find_file(manifest, relative_path).is_none() {
                return Err(MasterPlanExecutionError::MissingManifestFile {
                    relative_path: relative_path.clone(),
                });
            }
        }
    }
    Ok(())
}

fn validate_runtime_directories(
    request: &MasterPlanExecutionRequest,
) -> Result<(), MasterPlanExecutionError> {
    let root = fs::symlink_metadata(&request.session_root)
        .map_err(MasterPlanExecutionError::InspectSessionRoot)?;
    if !root.file_type().is_dir() {
        return Err(MasterPlanExecutionError::SessionRootNotDirectory);
    }
    let output = fs::symlink_metadata(&request.output_directory)
        .map_err(MasterPlanExecutionError::InspectOutputDirectory)?;
    if !output.file_type().is_dir() {
        return Err(MasterPlanExecutionError::OutputDirectoryNotDirectory);
    }
    Ok(())
}

fn planned_destinations(
    request: &MasterPlanExecutionRequest,
) -> Result<BTreeMap<String, PathBuf>, MasterPlanExecutionError> {
    let mut destinations = BTreeMap::new();
    let mut portable_names = BTreeSet::new();
    for product in request.plan.products() {
        let file_name = product_file_name(product.kind(), product.source_group_id());
        if !portable_names.insert(file_name.to_ascii_lowercase()) {
            return Err(MasterPlanExecutionError::DestinationNameCollision { file_name });
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
) -> Result<(), MasterPlanExecutionError> {
    for path in destinations.values() {
        match fs::symlink_metadata(path) {
            Ok(_) => {
                return Err(MasterPlanExecutionError::DestinationExists { path: path.clone() });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(MasterPlanExecutionError::InspectOutputDirectory(error)),
        }
    }
    Ok(())
}

fn group_sources(
    request: &MasterPlanExecutionRequest,
    group: &ManifestGroup,
) -> Result<Vec<PipelineSource>, MasterPlanExecutionError> {
    let mut sources = Vec::new();
    sources
        .try_reserve_exact(group.files().len())
        .map_err(|_| MasterPlanExecutionError::AllocationFailed)?;
    for relative_path in group.files() {
        let file = find_file(&request.manifest, relative_path).ok_or_else(|| {
            MasterPlanExecutionError::MissingManifestFile {
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

fn generated_source(
    group_id: &str,
    path: &Path,
) -> Result<PipelineSource, MasterPlanExecutionError> {
    let mut file =
        File::open(path).map_err(|source| MasterPlanExecutionError::OpenGeneratedPedestal {
            group_id: group_id.to_owned(),
            source,
        })?;
    let fingerprint = fingerprint_reader(&mut file).map_err(|source| {
        MasterPlanExecutionError::FingerprintGeneratedPedestal {
            group_id: group_id.to_owned(),
            source,
        }
    })?;
    Ok(PipelineSource::new(path.to_owned(), fingerprint))
}

fn product_provenance(
    manifest_sha256: &str,
    plan_sha256: &str,
    group_id: &str,
    kind: MasterProductKind,
    source_count: usize,
) -> Result<FitsOutputProvenance, MasterPlanExecutionError> {
    let source_count =
        u32::try_from(source_count).map_err(|_| MasterPlanExecutionError::AllocationFailed)?;
    let algorithm = match kind {
        MasterProductKind::Bias | MasterProductKind::Dark => STRICT_MEAN_ALGORITHM_ID,
        MasterProductKind::Flat => STRICT_FLAT_MASTER_ALGORITHM_ID,
    };
    FitsOutputProvenance::new(manifest_sha256, group_id, algorithm, source_count)
        .and_then(|provenance| provenance.with_plan_sha256(plan_sha256))
        .map_err(MasterPlanExecutionError::Provenance)
}

fn find_group<'a>(
    manifest: &'a SessionManifest,
    group_id: &str,
) -> Result<&'a ManifestGroup, MasterPlanExecutionError> {
    manifest
        .groups()
        .iter()
        .find(|group| group.id() == group_id)
        .ok_or_else(|| MasterPlanExecutionError::MissingGroup {
            group_id: group_id.to_owned(),
        })
}

fn find_file<'a>(manifest: &'a SessionManifest, path: &str) -> Option<&'a ManifestFile> {
    manifest
        .files()
        .iter()
        .find(|file| file.relative_path() == path)
}

fn product_matches_group(kind: MasterProductKind, group: &ManifestGroup) -> bool {
    use aether_metadata::FrameType;

    matches!(
        (kind, group.key().frame_type()),
        (MasterProductKind::Bias, FrameType::Bias)
            | (MasterProductKind::Dark, FrameType::Dark)
            | (MasterProductKind::Flat, FrameType::Flat)
    )
}

fn selected_pedestal_group(product: &aether_session::MasterProductPlan) -> Option<&str> {
    match product.flat_pedestal()? {
        FlatPedestalAssociation::MatchedDark { group_id, .. }
        | FlatPedestalAssociation::Bias { group_id, .. } => Some(group_id),
        FlatPedestalAssociation::Unresolved { .. } => None,
    }
}

pub(crate) fn product_file_name(kind: MasterProductKind, group_id: &str) -> String {
    let role = match kind {
        MasterProductKind::Bias => "bias",
        MasterProductKind::Dark => "dark",
        MasterProductKind::Flat => "flat",
    };
    format!("master-{role}-{group_id}.fits")
}

fn publish_product_set(
    staged_by_group: &BTreeMap<String, PathBuf>,
    destinations: &BTreeMap<String, PathBuf>,
    output_directory: &Path,
    cancellation: &CancellationToken,
) -> Result<(), MasterPlanExecutionError> {
    let mut published = Vec::new();
    published
        .try_reserve_exact(staged_by_group.len())
        .map_err(|_| MasterPlanExecutionError::AllocationFailed)?;
    for (group_id, staged) in staged_by_group {
        if let Err(cancelled) = cancellation.checkpoint() {
            rollback_publications(&published)?;
            return Err(MasterPlanExecutionError::Cancelled(cancelled));
        }
        let destination = &destinations[group_id];
        if let Err(source) = fs::hard_link(staged, destination) {
            rollback_publications(&published)?;
            return Err(MasterPlanExecutionError::PublishProduct {
                group_id: group_id.clone(),
                source,
            });
        }
        published.push(destination.clone());
    }
    if let Err(source) = sync_output_directory(output_directory) {
        rollback_publications(&published)?;
        return Err(MasterPlanExecutionError::SyncOutputDirectory(source));
    }
    Ok(())
}

fn rollback_publications(paths: &[PathBuf]) -> Result<(), MasterPlanExecutionError> {
    for path in paths.iter().rev() {
        fs::remove_file(path).map_err(|source| MasterPlanExecutionError::RollbackPublication {
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
    fn create(output_directory: &Path) -> Result<Self, MasterPlanExecutionError> {
        for _attempt in 0..MAX_STAGING_DIRECTORY_ATTEMPTS {
            let sequence = STAGING_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = output_directory.join(format!(
                ".aetherstack-master-stage-{}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(MasterPlanExecutionError::CreateStagingDirectory(error));
                }
            }
        }
        Err(MasterPlanExecutionError::CreateStagingDirectory(
            std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "private master staging name space exhausted",
            ),
        ))
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for StagingDirectory {
    fn drop(&mut self) {
        if let Ok(entries) = fs::read_dir(&self.path) {
            for entry in entries.flatten() {
                let _ignored = fs::remove_file(entry.path());
            }
        }
        let _ignored = fs::remove_dir(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::sync::atomic::{AtomicU64, Ordering};

    use aether_core::{Dimensions, ScientificImage};
    use aether_fits::{
        HeaderReadOptions, ImageRegion, PrimaryImageReader, write_f64_primary_atomic_new,
    };
    use aether_metadata::{
        BayerPattern, Binning, CameraModel, CanonicalMetadata, CanonicalValue, Confidence,
        FrameType,
    };
    use aether_session::{
        ClassificationPolicy, FlatPedestalPolicy, ManifestFile, ManifestGroup, MasterPlanOptions,
        StrictGroupingKey, classify_frame,
    };

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
                "aether-runtime-master-plan-{}-{sequence}",
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

    fn metadata(frame_type: FrameType, exposure_seconds: f64) -> CanonicalMetadata {
        CanonicalMetadata {
            camera: Some(exact(CameraModel::ZwoAsi294McPro, "INSTRUME")),
            frame_type: Some(exact(frame_type, "IMAGETYP")),
            exposure_seconds: Some(exact(exposure_seconds, "EXPTIME")),
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
        exposure_seconds: f64,
    ) -> TestResult<ManifestFile> {
        let path = root.join(relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let dimensions = Dimensions::new(values.len(), 1, 1)?;
        write_f64_primary_atomic_new(&path, &ScientificImage::from_pixels(dimensions, values)?)?;
        let mut source = File::open(&path)?;
        let fingerprint = fingerprint_reader(&mut source)?;
        let metadata = metadata(frame_type, exposure_seconds);
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
        let dark = write_source(
            root,
            "darks/dark-1.fits",
            vec![10.0, 10.0, 10.0],
            FrameType::Dark,
            2.0,
        )?;
        let flat_one = write_source(
            root,
            "flats/flat-1.fits",
            vec![11.0, 12.0, 13.0],
            FrameType::Flat,
            2.0,
        )?;
        let flat_two = write_source(
            root,
            "flats/flat-2.fits",
            vec![13.0, 16.0, 19.0],
            FrameType::Flat,
            2.0,
        )?;
        let dark_key =
            StrictGroupingKey::from_metadata(FrameType::Dark, dark.metadata(), dark.axes())?;
        let flat_key = StrictGroupingKey::from_metadata(
            FrameType::Flat,
            flat_one.metadata(),
            flat_one.axes(),
        )?;
        let groups = vec![
            ManifestGroup::new(
                "dark",
                dark_key,
                vec![dark.relative_path().to_owned()],
                Vec::new(),
                None,
            )?,
            ManifestGroup::new(
                "flat",
                flat_key,
                vec![
                    flat_one.relative_path().to_owned(),
                    flat_two.relative_path().to_owned(),
                ],
                Vec::new(),
                None,
            )?,
        ];
        Ok(SessionManifest::new(
            ClassificationPolicy::RequireAgreement,
            vec![dark, flat_one, flat_two],
            groups,
        )?)
    }

    fn plan(manifest: &SessionManifest) -> TestResult<MasterPlan> {
        Ok(MasterPlan::from_manifest(
            manifest,
            MasterPlanOptions::new(FlatPedestalPolicy::PreferMatchedDarkThenBias, 0.01, 1.0)?,
        )?)
    }

    #[test]
    fn executes_dependencies_then_publishes_the_complete_product_set() -> TestResult {
        let directory = TestDirectory::new()?;
        let root = directory.path.join("session");
        let output = directory.path.join("masters");
        fs::create_dir(&root)?;
        fs::create_dir(&output)?;
        let manifest = grouped_manifest(&root)?;
        let plan = plan(&manifest)?;
        let request = MasterPlanExecutionRequest::new(
            root,
            output.clone(),
            manifest,
            plan,
            FlatNormalizationParameters::new(3, 1.0e-12)?,
        )?
        .with_tile_shape(2, 1)?;
        let memory = MemoryBudget::new(1_048_576)?;
        let mut progress = Vec::new();

        let result = run_master_plan(&request, &CancellationToken::new(), &memory, |event| {
            progress.push(event)
        })?;

        assert_eq!(result.products().len(), 2);
        assert_eq!(result.products()[0].group_id(), "dark");
        assert_eq!(result.products()[0].kind(), MasterProductKind::Dark);
        assert_eq!(result.products()[0].normalization(), None);
        assert_eq!(result.products()[1].group_id(), "flat");
        assert_eq!(result.products()[1].kind(), MasterProductKind::Flat);
        assert_eq!(result.products()[1].normalization(), Some(4.0));
        assert_eq!(
            result.products()[1]
                .normalization_support()
                .map(FlatNormalizationSupport::accepted),
            Some(3)
        );
        assert_eq!(memory.used(), 0);
        assert!(progress.iter().any(|event| {
            event.product_index() == 0
                && event.group_id() == "dark"
                && event.kind() == MasterProductKind::Dark
        }));
        assert!(progress.iter().any(|event| {
            event.product_index() == 1
                && event.group_id() == "flat"
                && event.kind() == MasterProductKind::Flat
        }));

        let dark_output = output.join("master-dark-dark.fits");
        let flat_output = output.join("master-flat-flat.fits");
        assert!(dark_output.is_file());
        assert!(flat_output.is_file());
        let mut reader =
            PrimaryImageReader::open(File::open(flat_output)?, HeaderReadOptions::default())?;
        assert_eq!(
            reader.report().header().string("AETHMAN"),
            Some(result.manifest_sha256())
        );
        assert_eq!(
            reader.report().header().string("AETHPLN"),
            Some(result.plan_sha256())
        );
        assert_eq!(
            reader.report().header().string("AETHALG"),
            Some(STRICT_FLAT_MASTER_ALGORITHM_ID)
        );
        let image = reader.read_region_image(ImageRegion::new(0, 0, 0, 3, 1))?;
        let expected = [0.5, 1.0, 1.5];
        assert_eq!(
            image
                .pixels()
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>(),
            expected.map(f64::to_bits)
        );
        assert!(
            fs::read_dir(&output)?
                .collect::<Result<Vec<_>, _>>()?
                .iter()
                .all(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        );
        Ok(())
    }

    #[test]
    fn later_product_failure_leaves_no_public_or_private_products() -> TestResult {
        let directory = TestDirectory::new()?;
        let root = directory.path.join("session");
        let output = directory.path.join("masters");
        fs::create_dir(&root)?;
        fs::create_dir(&output)?;
        let manifest = grouped_manifest(&root)?;
        let request = MasterPlanExecutionRequest::new(
            root,
            output.clone(),
            manifest.clone(),
            plan(&manifest)?,
            FlatNormalizationParameters::new(4, 0.0)?,
        )?;

        let result = run_master_plan(
            &request,
            &CancellationToken::new(),
            &MemoryBudget::new(1_048_576)?,
            |_| {},
        );

        assert!(matches!(
            result,
            Err(MasterPlanExecutionError::ProductPipeline {
                source: StrictPipelineError::FlatNormalization(_),
                ..
            })
        ));
        assert!(fs::read_dir(output)?.next().is_none());
        Ok(())
    }

    #[test]
    fn existing_destination_blocks_the_transaction_before_calculation() -> TestResult {
        let directory = TestDirectory::new()?;
        let root = directory.path.join("session");
        let output = directory.path.join("masters");
        fs::create_dir(&root)?;
        fs::create_dir(&output)?;
        let manifest = grouped_manifest(&root)?;
        let sentinel = output.join("master-dark-dark.fits");
        fs::write(&sentinel, b"keep")?;
        let request = MasterPlanExecutionRequest::new(
            root,
            output.clone(),
            manifest.clone(),
            plan(&manifest)?,
            FlatNormalizationParameters::new(3, 0.0)?,
        )?;

        let result = run_master_plan(
            &request,
            &CancellationToken::new(),
            &MemoryBudget::new(1_048_576)?,
            |_| {},
        );

        assert!(matches!(
            result,
            Err(MasterPlanExecutionError::DestinationExists { .. })
        ));
        assert_eq!(fs::read(sentinel)?, b"keep");
        assert_eq!(fs::read_dir(output)?.count(), 1);
        Ok(())
    }

    #[test]
    fn cancellation_before_staging_leaves_output_empty() -> TestResult {
        let directory = TestDirectory::new()?;
        let root = directory.path.join("session");
        let output = directory.path.join("masters");
        fs::create_dir(&root)?;
        fs::create_dir(&output)?;
        let manifest = grouped_manifest(&root)?;
        let request = MasterPlanExecutionRequest::new(
            root,
            output.clone(),
            manifest.clone(),
            plan(&manifest)?,
            FlatNormalizationParameters::new(3, 0.0)?,
        )?;
        let cancellation = CancellationToken::new();
        assert!(cancellation.cancel());

        let result = run_master_plan(
            &request,
            &cancellation,
            &MemoryBudget::new(1_048_576)?,
            |_| {},
        );

        assert!(matches!(
            result,
            Err(MasterPlanExecutionError::Cancelled(_))
        ));
        assert!(fs::read_dir(output)?.next().is_none());
        Ok(())
    }
}
