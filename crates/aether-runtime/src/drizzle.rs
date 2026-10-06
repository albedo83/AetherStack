use std::error::Error;
use std::fmt::{Display, Formatter};
use std::io::{Read, Seek};
use std::mem::size_of;
use std::path::{Path, PathBuf};

use aether_core::{CompensatedSum, PixelFlags};
use aether_drizzle::{
    DrizzleError, DrizzleFrameError, DrizzleFrameEvidence, DrizzleOutputBounds, DrizzleParameters,
    DrizzleSourceWindow, DrizzleTileAccumulator, DrizzleTileBounds, DrizzleTileEvidence,
    DrizzleTileResult, accumulate_cfa_window, plan_drizzle_source_window,
};
use aether_fits::{
    AtomicF64PrimaryStreamWriter, AtomicFitsSetWriteError, AtomicFitsWriteError,
    FitsOutputProvenance, FitsProvenanceError, FitsWriteSummary, HeaderReadOptions, ImageReadError,
    ImageRegion, PrimaryImageReader, SampleStatus, publish_atomic_fits_set,
};
use aether_metadata::BayerPattern;
use aether_registration::ProjectiveTransform;

use crate::{CancellationToken, Cancelled, MemoryBudget, MemoryBudgetError};

/// Provenance identity of the normalized Drizzle science image.
pub const DRIZZLE_SCIENCE_ALGORITHM_ID: &str = "drizzle-science-v1";
/// Provenance identity of the Drizzle accumulated-weight map.
pub const DRIZZLE_WEIGHT_ALGORITHM_ID: &str = "drizzle-weight-v1";
/// Provenance identity of the exact Drizzle detector-support map.
pub const DRIZZLE_SUPPORT_ALGORITHM_ID: &str = "drizzle-support-v1";

const SUPPORT_CONVERSION_CHUNK: usize = 4_096;
const MAX_EXACT_BINARY64_INTEGER: u64 = 1_u64 << 53;

/// Evidence for one bounded FITS region accumulated into a Drizzle tile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DrizzleFitsWindowEvidence {
    window: DrizzleSourceWindow,
    frame: DrizzleFrameEvidence,
}

impl DrizzleFitsWindowEvidence {
    /// Detector rectangle read from the source FITS plane.
    #[must_use]
    pub const fn window(self) -> DrizzleSourceWindow {
        self.window
    }

    /// Sample accounting for the materialized detector rectangle.
    #[must_use]
    pub const fn frame(self) -> DrizzleFrameEvidence {
        self.frame
    }
}

/// Failure while planning, reading, or accumulating one bounded FITS region.
#[derive(Debug)]
pub enum DrizzleFitsAccumulationError {
    /// The primary array is not one non-empty, `u32`-addressable detector plane.
    InvalidSourceDimensions,
    /// Conservative source-window planning failed.
    Planning(DrizzleError),
    /// The planned FITS region could not be decoded.
    Read(ImageReadError),
    /// The decoded detector samples could not be accumulated.
    Accumulation(DrizzleFrameError),
}

impl Display for DrizzleFitsAccumulationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSourceDimensions => formatter.write_str(
                "Drizzle FITS source must be one non-empty u32-addressable detector plane",
            ),
            Self::Planning(error) => write!(formatter, "cannot plan Drizzle FITS region: {error}"),
            Self::Read(error) => write!(formatter, "cannot read Drizzle FITS region: {error}"),
            Self::Accumulation(error) => {
                write!(formatter, "cannot accumulate Drizzle FITS region: {error}")
            }
        }
    }
}

impl Error for DrizzleFitsAccumulationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidSourceDimensions => None,
            Self::Planning(error) => Some(error),
            Self::Read(error) => Some(error),
            Self::Accumulation(error) => Some(error),
        }
    }
}

/// Reads and accumulates only the detector rectangle needed by one output tile.
///
/// Header acceptance and source fingerprint checks remain responsibilities of
/// the caller that opened `reader`. A disjoint source returns `Ok(None)` without
/// touching its pixel array. Otherwise the exact planned region is materialized
/// as one plane and accumulated with global detector coordinates preserved.
#[allow(clippy::too_many_arguments)]
pub fn accumulate_fits_cfa_tile<R: Read + Seek>(
    accumulator: &mut DrizzleTileAccumulator,
    reader: &mut PrimaryImageReader<R>,
    transform: ProjectiveTransform,
    parameters: DrizzleParameters,
    pattern: &BayerPattern,
    frame_weight: f64,
    output: DrizzleOutputBounds,
) -> Result<Option<DrizzleFitsWindowEvidence>, DrizzleFitsAccumulationError> {
    let (source_width, source_height) = fits_detector_dimensions(reader)?;
    let Some(window) = plan_drizzle_source_window(
        source_width,
        source_height,
        transform,
        parameters,
        accumulator.bounds(),
        output,
    )
    .map_err(DrizzleFitsAccumulationError::Planning)?
    else {
        return Ok(None);
    };
    let region = ImageRegion::new(
        0,
        u64::from(window.x()),
        u64::from(window.y()),
        u64::from(window.width()),
        u64::from(window.height()),
    );
    let source = reader
        .read_region_image(region)
        .map_err(DrizzleFitsAccumulationError::Read)?;
    let frame = accumulate_cfa_window(
        accumulator,
        &source,
        window,
        transform,
        parameters,
        pattern,
        frame_weight,
        output,
    )
    .map_err(DrizzleFitsAccumulationError::Accumulation)?;
    Ok(Some(DrizzleFitsWindowEvidence { window, frame }))
}

fn fits_detector_dimensions<R: Read + Seek>(
    reader: &PrimaryImageReader<R>,
) -> Result<(u32, u32), DrizzleFitsAccumulationError> {
    match reader.descriptor().axes() {
        [width, height] | [width, height, 1] => Ok((
            u32::try_from(*width)
                .ok()
                .filter(|value| *value != 0)
                .ok_or(DrizzleFitsAccumulationError::InvalidSourceDimensions)?,
            u32::try_from(*height)
                .ok()
                .filter(|value| *value != 0)
                .ok_or(DrizzleFitsAccumulationError::InvalidSourceDimensions)?,
        )),
        _ => Err(DrizzleFitsAccumulationError::InvalidSourceDimensions),
    }
}

/// One opened CFA source and its immutable Drizzle controls.
pub struct DrizzleFitsFrame<R> {
    reader: PrimaryImageReader<R>,
    transform: ProjectiveTransform,
    pattern: BayerPattern,
    frame_weight: f64,
}

impl<R> DrizzleFitsFrame<R> {
    /// Binds an opened FITS source to its reviewed geometry and weight.
    #[must_use]
    pub fn new(
        reader: PrimaryImageReader<R>,
        transform: ProjectiveTransform,
        pattern: BayerPattern,
        frame_weight: f64,
    ) -> Self {
        Self {
            reader,
            transform,
            pattern,
            frame_weight,
        }
    }

    /// Reviewed source-to-reference projective transform.
    #[must_use]
    pub const fn transform(&self) -> ProjectiveTransform {
        self.transform
    }

    /// Physical mosaic pattern at detector coordinate `(0, 0)`.
    #[must_use]
    pub const fn pattern(&self) -> &BayerPattern {
        &self.pattern
    }

    /// Positive source weight supplied to geometric deposition.
    #[must_use]
    pub const fn frame_weight(&self) -> f64 {
        self.frame_weight
    }

    /// Returns the opened FITS reader after accumulation is complete.
    #[must_use]
    pub fn into_reader(self) -> PrimaryImageReader<R> {
        self.reader
    }
}

/// Heap-payload estimate for one bounded multi-source Drizzle tile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DrizzleTileMemoryEstimate {
    accumulator_bytes: usize,
    finalization_bytes: usize,
    maximum_region_bytes: usize,
    peak_bytes: usize,
}

impl DrizzleTileMemoryEstimate {
    /// Compensated flux, compensated weight, and support-count payloads.
    #[must_use]
    pub const fn accumulator_bytes(self) -> usize {
        self.accumulator_bytes
    }

    /// Science, weight, and flag payloads allocated during finalization.
    #[must_use]
    pub const fn finalization_bytes(self) -> usize {
        self.finalization_bytes
    }

    /// Largest decoded regional-image payload for any one source.
    #[must_use]
    pub const fn maximum_region_bytes(self) -> usize {
        self.maximum_region_bytes
    }

    /// Maximum concurrent payload reserved for execution of this tile.
    #[must_use]
    pub const fn peak_bytes(self) -> usize {
        self.peak_bytes
    }
}

/// Computes the allocation payload required to execute and finalize one tile.
///
/// FITS sources are decoded sequentially, so only the largest planned regional
/// image contributes to the peak. Vector control blocks, allocator metadata,
/// open-file buffering, and fixed stack scratch are deliberately excluded; the
/// returned value accounts exactly for heap element payloads owned by this path.
pub fn estimate_drizzle_fits_tile_memory<R: Read + Seek>(
    tile: DrizzleTileBounds,
    frames: &[DrizzleFitsFrame<R>],
    parameters: DrizzleParameters,
    output: DrizzleOutputBounds,
) -> Result<DrizzleTileMemoryEstimate, DrizzleFitsStackError> {
    let tile_elements = tile.dimensions().pixel_count();
    let accumulator_element_bytes = size_of::<CompensatedSum>()
        .checked_mul(2)
        .and_then(|value| value.checked_add(size_of::<u64>()))
        .ok_or(DrizzleFitsStackError::CounterOverflow)?;
    let finalization_element_bytes = size_of::<f64>()
        .checked_mul(2)
        .and_then(|value| value.checked_add(size_of::<PixelFlags>()))
        .ok_or(DrizzleFitsStackError::CounterOverflow)?;
    let region_element_bytes = size_of::<f64>()
        .checked_add(size_of::<PixelFlags>())
        .and_then(|value| value.checked_add(size_of::<SampleStatus>()))
        .ok_or(DrizzleFitsStackError::CounterOverflow)?;
    let accumulator_bytes = tile_elements
        .checked_mul(accumulator_element_bytes)
        .ok_or(DrizzleFitsStackError::CounterOverflow)?;
    let finalization_bytes = tile_elements
        .checked_mul(finalization_element_bytes)
        .ok_or(DrizzleFitsStackError::CounterOverflow)?;

    let mut maximum_region_bytes = 0_usize;
    for (index, frame) in frames.iter().enumerate() {
        let (source_width, source_height) = fits_detector_dimensions(&frame.reader)
            .map_err(|source| DrizzleFitsStackError::Source { index, source })?;
        let window = plan_drizzle_source_window(
            source_width,
            source_height,
            frame.transform,
            parameters,
            tile,
            output,
        )
        .map_err(|error| DrizzleFitsStackError::Source {
            index,
            source: DrizzleFitsAccumulationError::Planning(error),
        })?;
        if let Some(window) = window {
            let elements = usize::try_from(u64::from(window.width()) * u64::from(window.height()))
                .map_err(|_| DrizzleFitsStackError::CounterOverflow)?;
            let bytes = elements
                .checked_mul(region_element_bytes)
                .ok_or(DrizzleFitsStackError::CounterOverflow)?;
            maximum_region_bytes = maximum_region_bytes.max(bytes);
        }
    }
    let peak_bytes = accumulator_bytes
        .checked_add(finalization_bytes.max(maximum_region_bytes))
        .ok_or(DrizzleFitsStackError::CounterOverflow)?;
    Ok(DrizzleTileMemoryEstimate {
        accumulator_bytes,
        finalization_bytes,
        maximum_region_bytes,
        peak_bytes,
    })
}

/// Aggregate accounting for all FITS sources visited for one output tile.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DrizzleFitsTileEvidence {
    source_frames: u64,
    intersecting_frames: u64,
    source_samples: u64,
    masked_samples: u64,
    nonfinite_samples: u64,
    deposited_samples: u64,
    outside_output_samples: u64,
    geometric_contributions: u64,
}

impl DrizzleFitsTileEvidence {
    /// Number of sources examined in stable slice order.
    #[must_use]
    pub const fn source_frames(self) -> u64 {
        self.source_frames
    }

    /// Sources whose conservative detector window was non-empty.
    #[must_use]
    pub const fn intersecting_frames(self) -> u64 {
        self.intersecting_frames
    }

    /// Detector samples decoded across all intersecting windows.
    #[must_use]
    pub const fn source_samples(self) -> u64 {
        self.source_samples
    }

    /// Samples excluded by pre-existing quality flags.
    #[must_use]
    pub const fn masked_samples(self) -> u64 {
        self.masked_samples
    }

    /// Clear samples excluded because their values were non-finite.
    #[must_use]
    pub const fn nonfinite_samples(self) -> u64 {
        self.nonfinite_samples
    }

    /// Clear finite samples passed to geometric deposition.
    #[must_use]
    pub const fn deposited_samples(self) -> u64 {
        self.deposited_samples
    }

    /// Deposited samples whose footprints missed the global output.
    #[must_use]
    pub const fn outside_output_samples(self) -> u64 {
        self.outside_output_samples
    }

    /// Geometric contributions before tile ownership filtering.
    #[must_use]
    pub const fn geometric_contributions(self) -> u64 {
        self.geometric_contributions
    }

    fn add_frame(&mut self, frame: DrizzleFrameEvidence) -> Result<(), DrizzleFitsStackError> {
        self.intersecting_frames = checked_sum(self.intersecting_frames, 1)?;
        self.source_samples = checked_sum(self.source_samples, frame.source_samples())?;
        self.masked_samples = checked_sum(self.masked_samples, frame.masked_samples())?;
        self.nonfinite_samples = checked_sum(self.nonfinite_samples, frame.nonfinite_samples())?;
        self.deposited_samples = checked_sum(self.deposited_samples, frame.deposited_samples())?;
        self.outside_output_samples =
            checked_sum(self.outside_output_samples, frame.outside_output_samples())?;
        self.geometric_contributions = checked_sum(
            self.geometric_contributions,
            frame.geometric_contributions(),
        )?;
        Ok(())
    }
}

/// Failure while accumulating an ordered FITS source set into one tile.
#[derive(Debug)]
pub enum DrizzleFitsStackError {
    /// A specific source failed during planning, decoding, or accumulation.
    Source {
        /// Zero-based stable source-order index.
        index: usize,
        /// Underlying typed source failure.
        source: DrizzleFitsAccumulationError,
    },
    /// Aggregate evidence exceeded its representable integer domain.
    CounterOverflow,
}

impl Display for DrizzleFitsStackError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Source { index, source } => {
                write!(
                    formatter,
                    "cannot accumulate Drizzle FITS source {index}: {source}"
                )
            }
            Self::CounterOverflow => formatter.write_str("Drizzle FITS tile evidence overflowed"),
        }
    }
}

impl Error for DrizzleFitsStackError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Source { source, .. } => Some(source),
            Self::CounterOverflow => None,
        }
    }
}

/// Accumulates an ordered set of opened FITS sources into one bounded tile.
///
/// Each source is completely processed before the next source begins, fixing
/// floating-point reduction order independently of I/O scheduling. Callers must
/// discard the accumulator after an error because earlier sources can already
/// have contributed when a later source fails.
pub fn accumulate_fits_cfa_frames<R: Read + Seek>(
    accumulator: &mut DrizzleTileAccumulator,
    frames: &mut [DrizzleFitsFrame<R>],
    parameters: DrizzleParameters,
    output: DrizzleOutputBounds,
) -> Result<DrizzleFitsTileEvidence, DrizzleFitsStackError> {
    let mut evidence = DrizzleFitsTileEvidence {
        source_frames: u64::try_from(frames.len())
            .map_err(|_| DrizzleFitsStackError::CounterOverflow)?,
        ..DrizzleFitsTileEvidence::default()
    };
    for (index, frame) in frames.iter_mut().enumerate() {
        let result = accumulate_fits_cfa_tile(
            accumulator,
            &mut frame.reader,
            frame.transform,
            parameters,
            &frame.pattern,
            frame.frame_weight,
            output,
        )
        .map_err(|source| DrizzleFitsStackError::Source { index, source })?;
        if let Some(result) = result {
            evidence.add_frame(result.frame())?;
        }
    }
    Ok(evidence)
}

fn checked_sum(left: u64, right: u64) -> Result<u64, DrizzleFitsStackError> {
    left.checked_add(right)
        .ok_or(DrizzleFitsStackError::CounterOverflow)
}

/// Completed private tile together with its bounded-execution evidence.
#[derive(Clone, Debug, PartialEq)]
pub struct DrizzleTileExecutionResult {
    result: DrizzleTileResult,
    sources: DrizzleFitsTileEvidence,
    memory: DrizzleTileMemoryEstimate,
}

impl DrizzleTileExecutionResult {
    /// Finalized science, weight, support, and flag buffers for the tile.
    #[must_use]
    pub const fn result(&self) -> &DrizzleTileResult {
        &self.result
    }

    /// Aggregate ordered-source accounting.
    #[must_use]
    pub const fn source_evidence(&self) -> DrizzleFitsTileEvidence {
        self.sources
    }

    /// Memory estimate reserved for the complete operation.
    #[must_use]
    pub const fn memory_estimate(&self) -> DrizzleTileMemoryEstimate {
        self.memory
    }

    /// Transfers ownership of the finalized tile to the caller.
    #[must_use]
    pub fn into_result(self) -> DrizzleTileResult {
        self.result
    }
}

/// Failure while executing one cancellation-aware, memory-bounded tile.
#[derive(Debug)]
pub enum DrizzleTileExecutionError {
    /// Cancellation was observed at a bounded checkpoint.
    Cancelled(Cancelled),
    /// Tile preflight or one ordered FITS source failed.
    Sources(DrizzleFitsStackError),
    /// The planned peak could not be reserved.
    Memory(MemoryBudgetError),
    /// Tile allocation or finalization failed.
    Accumulation(aether_drizzle::DrizzleAccumulationError),
}

impl Display for DrizzleTileExecutionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled(error) => Display::fmt(error, formatter),
            Self::Sources(error) => Display::fmt(error, formatter),
            Self::Memory(error) => Display::fmt(error, formatter),
            Self::Accumulation(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for DrizzleTileExecutionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Cancelled(error) => Some(error),
            Self::Sources(error) => Some(error),
            Self::Memory(error) => Some(error),
            Self::Accumulation(error) => Some(error),
        }
    }
}

/// Executes one private Drizzle tile under cancellation and memory contracts.
///
/// Cancellation is checked before preflight, after reservation, before every
/// source, and before finalization. Any failure drops both the unpublished
/// accumulator and its reservation. The returned tile is complete but remains
/// private until a higher-level product transaction publishes it.
pub fn run_drizzle_fits_tile<R: Read + Seek>(
    tile: DrizzleTileBounds,
    frames: &mut [DrizzleFitsFrame<R>],
    parameters: DrizzleParameters,
    output: DrizzleOutputBounds,
    cancellation: &CancellationToken,
    memory: &MemoryBudget,
) -> Result<DrizzleTileExecutionResult, DrizzleTileExecutionError> {
    cancellation
        .checkpoint()
        .map_err(DrizzleTileExecutionError::Cancelled)?;
    let estimate = estimate_drizzle_fits_tile_memory(tile, frames, parameters, output)
        .map_err(DrizzleTileExecutionError::Sources)?;
    let _reservation = memory
        .try_reserve(estimate.peak_bytes())
        .map_err(DrizzleTileExecutionError::Memory)?;
    cancellation
        .checkpoint()
        .map_err(DrizzleTileExecutionError::Cancelled)?;
    let mut accumulator =
        DrizzleTileAccumulator::new(tile).map_err(DrizzleTileExecutionError::Accumulation)?;
    let mut sources = DrizzleFitsTileEvidence {
        source_frames: u64::try_from(frames.len()).map_err(|_| {
            DrizzleTileExecutionError::Sources(DrizzleFitsStackError::CounterOverflow)
        })?,
        ..DrizzleFitsTileEvidence::default()
    };
    for (index, frame) in frames.iter_mut().enumerate() {
        cancellation
            .checkpoint()
            .map_err(DrizzleTileExecutionError::Cancelled)?;
        let result = accumulate_fits_cfa_tile(
            &mut accumulator,
            &mut frame.reader,
            frame.transform,
            parameters,
            &frame.pattern,
            frame.frame_weight,
            output,
        )
        .map_err(|source| {
            DrizzleTileExecutionError::Sources(DrizzleFitsStackError::Source { index, source })
        })?;
        if let Some(result) = result {
            sources
                .add_frame(result.frame())
                .map_err(DrizzleTileExecutionError::Sources)?;
        }
    }
    cancellation
        .checkpoint()
        .map_err(DrizzleTileExecutionError::Cancelled)?;
    let result = accumulator
        .finish()
        .map_err(DrizzleTileExecutionError::Accumulation)?;
    Ok(DrizzleTileExecutionResult {
        result,
        sources,
        memory: estimate,
    })
}

/// One full-width horizontal output band and its exact payload estimate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DrizzlePlannedBand {
    bounds: DrizzleTileBounds,
    memory: DrizzleTileMemoryEstimate,
}

impl DrizzlePlannedBand {
    /// Half-open global output bounds owned by this band.
    #[must_use]
    pub const fn bounds(self) -> DrizzleTileBounds {
        self.bounds
    }

    /// Exact heap-element payload preflight for this band.
    #[must_use]
    pub const fn memory_estimate(self) -> DrizzleTileMemoryEstimate {
        self.memory
    }
}

/// Complete top-to-bottom plan of disjoint full-width Drizzle bands.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DrizzleBandPlan {
    bands: Vec<DrizzlePlannedBand>,
    peak_bytes: usize,
}

impl DrizzleBandPlan {
    /// Bands in deterministic top-to-bottom order.
    #[must_use]
    pub fn bands(&self) -> &[DrizzlePlannedBand] {
        &self.bands
    }

    /// Largest per-band heap-element payload in the plan.
    #[must_use]
    pub const fn peak_bytes(&self) -> usize {
        self.peak_bytes
    }
}

/// Failure while choosing memory-bounded full-width output bands.
#[derive(Debug)]
pub enum DrizzleBandPlanError {
    /// Maximum requested band height or memory limit is zero.
    InvalidLimit,
    /// Even a one-row full-width band exceeds the supplied payload ceiling.
    InsufficientMemory {
        /// First output row that cannot be planned.
        origin_y: u32,
        /// Exact bytes needed by the one-row candidate.
        required: usize,
        /// Caller-supplied payload ceiling.
        limit: usize,
    },
    /// Tile-bound construction failed.
    Bounds(aether_drizzle::DrizzleAccumulationError),
    /// FITS geometry or source-window preflight failed.
    Sources(DrizzleFitsStackError),
    /// Plan bookkeeping could not be represented or allocated.
    PlanCapacity,
}

impl Display for DrizzleBandPlanError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidLimit => {
                formatter.write_str("Drizzle band height and memory limit must be positive")
            }
            Self::InsufficientMemory {
                origin_y,
                required,
                limit,
            } => write!(
                formatter,
                "Drizzle output row {origin_y} needs {required} bytes within a {limit}-byte limit"
            ),
            Self::Bounds(error) => Display::fmt(error, formatter),
            Self::Sources(error) => Display::fmt(error, formatter),
            Self::PlanCapacity => formatter.write_str("cannot allocate Drizzle band plan"),
        }
    }
}

impl Error for DrizzleBandPlanError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Bounds(error) => Some(error),
            Self::Sources(error) => Some(error),
            Self::InvalidLimit | Self::InsufficientMemory { .. } | Self::PlanCapacity => None,
        }
    }
}

/// Plans the tallest full-width band that fits at each output row.
///
/// Candidate height is monotone at a fixed origin: both output storage and the
/// conservative inverse-mapped detector rectangle can only stay equal or grow.
/// Binary search therefore selects the largest fitting height without a linear
/// scan. Adjacent half-open bands cover every output row exactly once.
pub fn plan_drizzle_fits_bands<R: Read + Seek>(
    frames: &[DrizzleFitsFrame<R>],
    parameters: DrizzleParameters,
    output: DrizzleOutputBounds,
    maximum_band_height: u32,
    memory_limit: usize,
) -> Result<DrizzleBandPlan, DrizzleBandPlanError> {
    if maximum_band_height == 0 || memory_limit == 0 {
        return Err(DrizzleBandPlanError::InvalidLimit);
    }
    let estimated_bands = output
        .height()
        .checked_add(maximum_band_height - 1)
        .ok_or(DrizzleBandPlanError::PlanCapacity)?
        / maximum_band_height;
    let mut bands = Vec::new();
    bands
        .try_reserve_exact(
            usize::try_from(estimated_bands).map_err(|_| DrizzleBandPlanError::PlanCapacity)?,
        )
        .map_err(|_| DrizzleBandPlanError::PlanCapacity)?;
    let mut origin_y = 0_u32;
    let mut peak_bytes = 0_usize;
    while origin_y < output.height() {
        let remaining = output.height() - origin_y;
        let mut low = 1_u32;
        let mut high = maximum_band_height.min(remaining);
        let mut accepted = None;
        while low <= high {
            let height = low + (high - low) / 2;
            let bounds = DrizzleTileBounds::new(0, origin_y, output.width(), height, 3)
                .map_err(DrizzleBandPlanError::Bounds)?;
            let memory = estimate_drizzle_fits_tile_memory(bounds, frames, parameters, output)
                .map_err(DrizzleBandPlanError::Sources)?;
            if memory.peak_bytes() <= memory_limit {
                accepted = Some(DrizzlePlannedBand { bounds, memory });
                if height == high {
                    break;
                }
                low = height + 1;
            } else {
                if height == 1 {
                    high = 0;
                } else {
                    high = height - 1;
                }
            }
        }
        let Some(band) = accepted else {
            let bounds = DrizzleTileBounds::new(0, origin_y, output.width(), 1, 3)
                .map_err(DrizzleBandPlanError::Bounds)?;
            let required = estimate_drizzle_fits_tile_memory(bounds, frames, parameters, output)
                .map_err(DrizzleBandPlanError::Sources)?
                .peak_bytes();
            return Err(DrizzleBandPlanError::InsufficientMemory {
                origin_y,
                required,
                limit: memory_limit,
            });
        };
        peak_bytes = peak_bytes.max(band.memory.peak_bytes());
        origin_y = origin_y
            .checked_add(
                u32::try_from(band.bounds.dimensions().height())
                    .map_err(|_| DrizzleBandPlanError::PlanCapacity)?,
            )
            .ok_or(DrizzleBandPlanError::PlanCapacity)?;
        bands
            .try_reserve(1)
            .map_err(|_| DrizzleBandPlanError::PlanCapacity)?;
        bands.push(band);
    }
    Ok(DrizzleBandPlan { bands, peak_bytes })
}

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
    use std::io::{self, Cursor};
    use std::sync::atomic::{AtomicU64, Ordering};

    use aether_core::{Dimensions, ScientificImage};
    use aether_drizzle::{
        DrizzleOutputBounds, DrizzleParameters, DrizzleTileAccumulator, DrizzleTileBounds,
        accumulate_cfa_frame, deposit_detector_footprint, project_detector_footprint,
    };
    use aether_fits::{PrimaryImageReader, SampleStatus, write_f64_primary};
    use aether_metadata::BayerPattern;
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

    fn fits_bytes(image: &ScientificImage) -> Result<Vec<u8>, Box<dyn Error>> {
        let mut bytes = Vec::new();
        write_f64_primary(&mut bytes, image)?;
        Ok(bytes)
    }

    #[test]
    fn bounded_fits_read_matches_complete_frame_accumulation() -> TestResult {
        let width = 8_u32;
        let height = 6_u32;
        let image = ScientificImage::from_pixels(
            Dimensions::new(width as usize, height as usize, 1)?,
            (0..width * height)
                .map(|index| f64::from(index) + 0.125)
                .collect(),
        )?;
        let transform = ProjectiveTransform::new([
            [0.99, -0.03, 0.4],
            [0.02, 1.01, -0.2],
            [0.0005, -0.0003, 1.0],
        ])?;
        let parameters = DrizzleParameters::new(1, 0.8)?;
        let tile = DrizzleTileBounds::new(3, 2, 2, 2, 3)?;
        let output = DrizzleOutputBounds::new(width, height, 16)?;

        let mut complete = DrizzleTileAccumulator::new(tile)?;
        accumulate_cfa_frame(
            &mut complete,
            &image,
            transform,
            parameters,
            &BayerPattern::Rggb,
            1.25,
            output,
        )?;
        let complete = complete.finish()?;

        let mut reader = PrimaryImageReader::open(
            Cursor::new(fits_bytes(&image)?),
            HeaderReadOptions::default(),
        )?;
        let mut bounded = DrizzleTileAccumulator::new(tile)?;
        let evidence = accumulate_fits_cfa_tile(
            &mut bounded,
            &mut reader,
            transform,
            parameters,
            &BayerPattern::Rggb,
            1.25,
            output,
        )?
        .ok_or("test tile unexpectedly missed the detector")?;
        assert_eq!(
            evidence.frame().source_samples(),
            u64::from(evidence.window().width()) * u64::from(evidence.window().height())
        );
        assert!(evidence.window().width() < width || evidence.window().height() < height);
        let bounded = bounded.finish()?;

        assert_eq!(
            bounded
                .values()
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>(),
            complete
                .values()
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            bounded
                .weights()
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>(),
            complete
                .weights()
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            bounded.contribution_counts(),
            complete.contribution_counts()
        );
        assert_eq!(bounded.flags(), complete.flags());
        Ok(())
    }

    #[test]
    fn ordered_fits_sources_match_complete_frame_reduction() -> TestResult {
        let width = 8_u32;
        let height = 6_u32;
        let first = ScientificImage::from_pixels(
            Dimensions::new(width as usize, height as usize, 1)?,
            (0..width * height)
                .map(|index| f64::from(index) + 0.25)
                .collect(),
        )?;
        let second = ScientificImage::from_pixels(
            Dimensions::new(width as usize, height as usize, 1)?,
            (0..width * height)
                .map(|index| f64::from(index) * 1.5 + 4.0)
                .collect(),
        )?;
        let first_transform = ProjectiveTransform::new([
            [0.99, -0.03, 0.4],
            [0.02, 1.01, -0.2],
            [0.0005, -0.0003, 1.0],
        ])?;
        let second_transform = ProjectiveTransform::new([
            [1.01, 0.02, -0.3],
            [-0.01, 0.98, 0.25],
            [-0.0004, 0.0002, 1.0],
        ])?;
        let parameters = DrizzleParameters::new(1, 0.8)?;
        let tile = DrizzleTileBounds::new(3, 2, 2, 2, 3)?;
        let output = DrizzleOutputBounds::new(width, height, 16)?;

        let mut complete = DrizzleTileAccumulator::new(tile)?;
        accumulate_cfa_frame(
            &mut complete,
            &first,
            first_transform,
            parameters,
            &BayerPattern::Rggb,
            1.25,
            output,
        )?;
        accumulate_cfa_frame(
            &mut complete,
            &second,
            second_transform,
            parameters,
            &BayerPattern::Rggb,
            0.75,
            output,
        )?;
        let complete = complete.finish()?;

        let mut frames = [
            DrizzleFitsFrame::new(
                PrimaryImageReader::open(
                    Cursor::new(fits_bytes(&first)?),
                    HeaderReadOptions::default(),
                )?,
                first_transform,
                BayerPattern::Rggb,
                1.25,
            ),
            DrizzleFitsFrame::new(
                PrimaryImageReader::open(
                    Cursor::new(fits_bytes(&second)?),
                    HeaderReadOptions::default(),
                )?,
                second_transform,
                BayerPattern::Rggb,
                0.75,
            ),
        ];
        let mut bounded = DrizzleTileAccumulator::new(tile)?;
        let evidence = accumulate_fits_cfa_frames(&mut bounded, &mut frames, parameters, output)?;
        assert_eq!(evidence.source_frames(), 2);
        assert_eq!(evidence.intersecting_frames(), 2);
        assert_eq!(
            evidence.source_samples(),
            evidence
                .masked_samples()
                .checked_add(evidence.nonfinite_samples())
                .and_then(|value| value.checked_add(evidence.deposited_samples()))
                .ok_or("test evidence overflow")?
        );
        let bounded = bounded.finish()?;

        assert_eq!(
            bounded
                .values()
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>(),
            complete
                .values()
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            bounded
                .weights()
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>(),
            complete
                .weights()
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            bounded.contribution_counts(),
            complete.contribution_counts()
        );
        assert_eq!(bounded.flags(), complete.flags());
        Ok(())
    }

    #[test]
    fn tile_memory_preflight_uses_the_largest_region_not_their_sum() -> TestResult {
        let width = 8_u32;
        let height = 6_u32;
        let image = ScientificImage::from_pixels(
            Dimensions::new(width as usize, height as usize, 1)?,
            vec![1.0; width as usize * height as usize],
        )?;
        let bytes = fits_bytes(&image)?;
        let transform = ProjectiveTransform::new([
            [0.99, -0.03, 0.4],
            [0.02, 1.01, -0.2],
            [0.0005, -0.0003, 1.0],
        ])?;
        let parameters = DrizzleParameters::new(1, 0.8)?;
        let tile = DrizzleTileBounds::new(3, 2, 2, 2, 3)?;
        let output = DrizzleOutputBounds::new(width, height, 16)?;
        let frame = || -> Result<_, ImageReadError> {
            Ok(DrizzleFitsFrame::new(
                PrimaryImageReader::open(Cursor::new(bytes.clone()), HeaderReadOptions::default())?,
                transform,
                BayerPattern::Rggb,
                1.0,
            ))
        };
        let frames = [frame()?, frame()?];

        let estimate = estimate_drizzle_fits_tile_memory(tile, &frames, parameters, output)?;
        let tile_elements = tile.dimensions().pixel_count();
        assert_eq!(
            estimate.accumulator_bytes(),
            tile_elements * (size_of::<CompensatedSum>() * 2 + size_of::<u64>())
        );
        assert_eq!(
            estimate.finalization_bytes(),
            tile_elements * (size_of::<f64>() * 2 + size_of::<PixelFlags>())
        );
        let window =
            plan_drizzle_source_window(width, height, transform, parameters, tile, output)?
                .ok_or("test tile unexpectedly missed the detector")?;
        let region_elements = window.width() as usize * window.height() as usize;
        assert_eq!(
            estimate.maximum_region_bytes(),
            region_elements
                * (size_of::<f64>() + size_of::<PixelFlags>() + size_of::<SampleStatus>())
        );
        assert_eq!(
            estimate.peak_bytes(),
            estimate.accumulator_bytes()
                + estimate
                    .finalization_bytes()
                    .max(estimate.maximum_region_bytes())
        );
        let budget = crate::MemoryBudget::new(estimate.peak_bytes())?;
        let reservation = budget.try_reserve(estimate.peak_bytes())?;
        assert_eq!(reservation.bytes(), estimate.peak_bytes());
        drop(reservation);
        Ok(())
    }

    #[test]
    fn tile_execution_holds_and_releases_its_exact_memory_reservation() -> TestResult {
        let image = ScientificImage::from_pixels(
            Dimensions::new(4, 3, 1)?,
            (0..12).map(|value| f64::from(value) + 0.5).collect(),
        )?;
        let bytes = fits_bytes(&image)?;
        let build_frame = || -> Result<_, ImageReadError> {
            Ok(DrizzleFitsFrame::new(
                PrimaryImageReader::open(Cursor::new(bytes.clone()), HeaderReadOptions::default())?,
                ProjectiveTransform::IDENTITY,
                BayerPattern::Rggb,
                1.0,
            ))
        };
        let tile = DrizzleTileBounds::new(0, 0, 4, 3, 3)?;
        let parameters = DrizzleParameters::new(1, 1.0)?;
        let output = DrizzleOutputBounds::new(4, 3, 4)?;
        let estimate =
            estimate_drizzle_fits_tile_memory(tile, &[build_frame()?], parameters, output)?;
        let memory = crate::MemoryBudget::new(estimate.peak_bytes())?;
        let mut frames = [build_frame()?];

        let executed = run_drizzle_fits_tile(
            tile,
            &mut frames,
            parameters,
            output,
            &CancellationToken::new(),
            &memory,
        )?;

        assert_eq!(executed.memory_estimate(), estimate);
        assert_eq!(executed.source_evidence().source_frames(), 1);
        assert_eq!(executed.source_evidence().intersecting_frames(), 1);
        assert_eq!(memory.used(), 0);
        assert_eq!(memory.peak(), estimate.peak_bytes());
        assert_eq!(executed.result().bounds(), tile);
        Ok(())
    }

    #[test]
    fn cancelled_tile_stops_before_memory_or_pixel_work() -> TestResult {
        let image = ScientificImage::from_pixels(Dimensions::new(2, 2, 1)?, vec![1.0; 4])?;
        let mut frames = [DrizzleFitsFrame::new(
            PrimaryImageReader::open(
                Cursor::new(fits_bytes(&image)?),
                HeaderReadOptions::default(),
            )?,
            ProjectiveTransform::IDENTITY,
            BayerPattern::Rggb,
            1.0,
        )];
        let memory = crate::MemoryBudget::new(1)?;
        let cancellation = CancellationToken::new();
        assert!(cancellation.cancel());

        let result = run_drizzle_fits_tile(
            DrizzleTileBounds::new(0, 0, 2, 2, 3)?,
            &mut frames,
            DrizzleParameters::new(1, 1.0)?,
            DrizzleOutputBounds::new(2, 2, 4)?,
            &cancellation,
            &memory,
        );

        assert!(matches!(
            result,
            Err(DrizzleTileExecutionError::Cancelled(_))
        ));
        assert_eq!(memory.used(), 0);
        assert_eq!(memory.peak(), 0);
        Ok(())
    }

    #[test]
    fn adaptive_band_plan_covers_output_within_memory_limit() -> TestResult {
        let image = ScientificImage::from_pixels(Dimensions::new(4, 5, 1)?, vec![1.0; 20])?;
        let frame = DrizzleFitsFrame::new(
            PrimaryImageReader::open(
                Cursor::new(fits_bytes(&image)?),
                HeaderReadOptions::default(),
            )?,
            ProjectiveTransform::IDENTITY,
            BayerPattern::Rggb,
            1.0,
        );
        let frames = [frame];
        let parameters = DrizzleParameters::new(1, 1.0)?;
        let output = DrizzleOutputBounds::new(4, 5, 4)?;
        let mut one_row_limit = 0_usize;
        for origin_y in 0..output.height() {
            let bounds = DrizzleTileBounds::new(0, origin_y, output.width(), 1, 3)?;
            one_row_limit = one_row_limit.max(
                estimate_drizzle_fits_tile_memory(bounds, &frames, parameters, output)?
                    .peak_bytes(),
            );
        }

        let plan = plan_drizzle_fits_bands(&frames, parameters, output, 4, one_row_limit)?;
        assert!(!plan.bands().is_empty());
        assert!(plan.peak_bytes() <= one_row_limit);
        let mut expected_y = 0_u32;
        for band in plan.bands() {
            assert_eq!(band.bounds().origin_x(), 0);
            assert_eq!(band.bounds().origin_y(), expected_y);
            assert_eq!(band.bounds().dimensions().width(), 4);
            assert_eq!(band.bounds().dimensions().planes(), 3);
            assert!(band.memory_estimate().peak_bytes() <= one_row_limit);
            expected_y += u32::try_from(band.bounds().dimensions().height())?;
        }
        assert_eq!(expected_y, output.height());

        let first_row = DrizzleTileBounds::new(0, 0, output.width(), 1, 3)?;
        let required =
            estimate_drizzle_fits_tile_memory(first_row, &frames, parameters, output)?.peak_bytes();
        assert!(matches!(
            plan_drizzle_fits_bands(&frames, parameters, output, 4, required - 1),
            Err(DrizzleBandPlanError::InsufficientMemory {
                origin_y: 0,
                required: actual,
                limit
            }) if actual == required && limit == required - 1
        ));
        Ok(())
    }

    #[test]
    fn ordered_fits_failure_reports_the_stable_source_index() -> TestResult {
        let image = ScientificImage::from_pixels(Dimensions::new(2, 2, 1)?, vec![1.0; 4])?;
        let bytes = fits_bytes(&image)?;
        let mut frames = [
            DrizzleFitsFrame::new(
                PrimaryImageReader::open(Cursor::new(bytes.clone()), HeaderReadOptions::default())?,
                ProjectiveTransform::IDENTITY,
                BayerPattern::Rggb,
                1.0,
            ),
            DrizzleFitsFrame::new(
                PrimaryImageReader::open(Cursor::new(bytes), HeaderReadOptions::default())?,
                ProjectiveTransform::IDENTITY,
                BayerPattern::Other("unsupported".to_owned()),
                1.0,
            ),
        ];
        let mut accumulator = DrizzleTileAccumulator::new(DrizzleTileBounds::new(0, 0, 2, 2, 3)?)?;
        let error = accumulate_fits_cfa_frames(
            &mut accumulator,
            &mut frames,
            DrizzleParameters::new(1, 1.0)?,
            DrizzleOutputBounds::new(2, 2, 4)?,
        );

        assert!(matches!(
            error,
            Err(DrizzleFitsStackError::Source {
                index: 1,
                source: DrizzleFitsAccumulationError::Accumulation(DrizzleFrameError::Geometry(
                    DrizzleError::UnsupportedCfaPattern
                ))
            })
        ));
        Ok(())
    }

    #[test]
    fn disjoint_fits_source_requires_no_pixel_bytes() -> TestResult {
        let image = ScientificImage::from_pixels(Dimensions::new(2, 2, 1)?, vec![1.0; 4])?;
        let mut bytes = fits_bytes(&image)?;
        let header_reader =
            PrimaryImageReader::open(Cursor::new(bytes.clone()), HeaderReadOptions::default())?;
        bytes.truncate(usize::try_from(header_reader.descriptor().data_offset())?);
        let mut header_only =
            PrimaryImageReader::open(Cursor::new(bytes), HeaderReadOptions::default())?;
        let mut accumulator = DrizzleTileAccumulator::new(DrizzleTileBounds::new(0, 0, 2, 2, 3)?)?;
        let disjoint = accumulate_fits_cfa_tile(
            &mut accumulator,
            &mut header_only,
            ProjectiveTransform::new([[1.0, 0.0, 100.0], [0.0, 1.0, 100.0], [0.0, 0.0, 1.0]])?,
            DrizzleParameters::new(1, 1.0)?,
            &BayerPattern::Rggb,
            1.0,
            DrizzleOutputBounds::new(2, 2, 4)?,
        )?;

        assert_eq!(disjoint, None);
        let result = accumulator.finish()?;
        assert_eq!(result.evidence().depositions_seen(), 0);
        assert!(
            result
                .flags()
                .iter()
                .all(|flags| flags.contains(PixelFlags::MISSING))
        );
        Ok(())
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
