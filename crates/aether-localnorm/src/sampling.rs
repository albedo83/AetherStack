use std::collections::BinaryHeap;
use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_core::{PixelMask, ScientificImage};

use crate::{
    LocalAffineFit, LocalFitError, LocalFitParameters, NormalizationSample, fit_local_affine_iter,
};

/// Stable identity of deterministic bounded spatial sampling.
pub const CELL_SAMPLING_ALGORITHM_ID: &str = "local-cell-priority-sampling-f64-v1";

/// Explicit grid and memory limits for local-normalization sampling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SamplingGridParameters {
    cell_width: usize,
    cell_height: usize,
    maximum_samples_per_cell: usize,
    maximum_cells: usize,
}

impl SamplingGridParameters {
    /// Builds a non-empty bounded sampling grid.
    pub fn new(
        cell_width: usize,
        cell_height: usize,
        maximum_samples_per_cell: usize,
        maximum_cells: usize,
    ) -> Result<Self, SamplingError> {
        if cell_width == 0
            || cell_height == 0
            || maximum_samples_per_cell == 0
            || maximum_cells == 0
        {
            return Err(SamplingError::InvalidParameters);
        }
        Ok(Self {
            cell_width,
            cell_height,
            maximum_samples_per_cell,
            maximum_cells,
        })
    }

    /// Nominal cell width in pixels.
    #[must_use]
    pub const fn cell_width(self) -> usize {
        self.cell_width
    }

    /// Nominal cell height in pixels.
    #[must_use]
    pub const fn cell_height(self) -> usize {
        self.cell_height
    }

    /// Hard retained-sample limit for every cell.
    #[must_use]
    pub const fn maximum_samples_per_cell(self) -> usize {
        self.maximum_samples_per_cell
    }

    /// Hard cell-count limit checked before allocating the result.
    #[must_use]
    pub const fn maximum_cells(self) -> usize {
        self.maximum_cells
    }
}

/// Half-open image bounds of one edge-clipped grid cell.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CellBounds {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

impl CellBounds {
    /// Left coordinate.
    #[must_use]
    pub const fn x(self) -> usize {
        self.x
    }

    /// Top coordinate.
    #[must_use]
    pub const fn y(self) -> usize {
        self.y
    }

    /// Edge-clipped width.
    #[must_use]
    pub const fn width(self) -> usize {
        self.width
    }

    /// Edge-clipped height.
    #[must_use]
    pub const fn height(self) -> usize {
        self.height
    }

    /// Exact number of image pixels represented by the cell.
    #[must_use]
    pub const fn area(self) -> usize {
        self.width * self.height
    }
}

/// One retained source/reference pair and its registered image coordinate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpatialSample {
    x: usize,
    y: usize,
    pair: NormalizationSample,
}

impl SpatialSample {
    /// Horizontal image coordinate.
    #[must_use]
    pub const fn x(self) -> usize {
        self.x
    }

    /// Vertical image coordinate.
    #[must_use]
    pub const fn y(self) -> usize {
        self.y
    }

    /// Validated scientific sample pair.
    #[must_use]
    pub const fn pair(self) -> NormalizationSample {
        self.pair
    }
}

/// Complete mutually exclusive accounting for one cell scan.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CellSamplingEvidence {
    protected: usize,
    source_masked: usize,
    reference_masked: usize,
    non_finite: usize,
    eligible: usize,
    retained: usize,
}

impl CellSamplingEvidence {
    /// Pixels excluded by the explicit protected-region mask.
    #[must_use]
    pub const fn protected(self) -> usize {
        self.protected
    }

    /// Remaining pixels excluded by the source quality mask.
    #[must_use]
    pub const fn source_masked(self) -> usize {
        self.source_masked
    }

    /// Remaining pixels excluded by the reference quality mask.
    #[must_use]
    pub const fn reference_masked(self) -> usize {
        self.reference_masked
    }

    /// Remaining pixels excluded because either value is non-finite.
    #[must_use]
    pub const fn non_finite(self) -> usize {
        self.non_finite
    }

    /// Clear finite pairs considered by deterministic priority sampling.
    #[must_use]
    pub const fn eligible(self) -> usize {
        self.eligible
    }

    /// Pairs retained after enforcing the per-cell memory limit.
    #[must_use]
    pub const fn retained(self) -> usize {
        self.retained
    }

    /// Eligible pairs omitted only because the configured cap was reached.
    #[must_use]
    pub const fn downsampled(self) -> usize {
        self.eligible - self.retained
    }

    /// Pixels classified by the mutually exclusive scan categories.
    #[must_use]
    pub const fn classified_pixels(self) -> usize {
        self.protected
            + self.source_masked
            + self.reference_masked
            + self.non_finite
            + self.eligible
    }
}

/// Samples and evidence for one cell in canonical row-major grid order.
#[derive(Clone, Debug, PartialEq)]
pub struct LocalCellSamples {
    bounds: CellBounds,
    samples: Vec<SpatialSample>,
    evidence: CellSamplingEvidence,
}

impl LocalCellSamples {
    /// Image bounds covered by this cell.
    #[must_use]
    pub const fn bounds(&self) -> CellBounds {
        self.bounds
    }

    /// Retained samples sorted by image row then column.
    #[must_use]
    pub fn samples(&self) -> &[SpatialSample] {
        &self.samples
    }

    /// Complete scan accounting.
    #[must_use]
    pub const fn evidence(&self) -> CellSamplingEvidence {
        self.evidence
    }
}

/// Per-cell model or typed rejection in the original canonical grid order.
#[derive(Clone, Debug, PartialEq)]
pub struct LocalCellFit {
    bounds: CellBounds,
    sampling_evidence: CellSamplingEvidence,
    result: Result<LocalAffineFit, LocalFitError>,
}

impl LocalCellFit {
    /// Image bounds represented by this result.
    #[must_use]
    pub const fn bounds(&self) -> CellBounds {
        self.bounds
    }

    /// Sampling evidence retained even when fitting fails.
    #[must_use]
    pub const fn sampling_evidence(&self) -> CellSamplingEvidence {
        self.sampling_evidence
    }

    /// Fitted affine model or the exact cell-local failure.
    pub const fn result(&self) -> Result<LocalAffineFit, LocalFitError> {
        self.result
    }
}

/// Failure to validate or allocate a local sampling grid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SamplingError {
    /// A grid extent or hard resource limit is zero.
    InvalidParameters,
    /// Source and reference dimensions differ.
    ImageDimensionsMismatch,
    /// The optional protected-region mask does not match the images.
    ProtectedMaskDimensionsMismatch,
    /// The selected plane does not exist.
    PlaneOutOfBounds,
    /// The derived grid contains more cells than permitted.
    CellLimitExceeded,
    /// Checked grid arithmetic overflowed.
    ArithmeticOverflow,
    /// Scratch or result allocation failed.
    AllocationFailed,
    /// An already validated finite pair could not be reconstructed.
    InvalidRetainedSample,
}

impl Display for SamplingError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidParameters => "local sampling parameters must be non-zero",
            Self::ImageDimensionsMismatch => "local sampling images have different dimensions",
            Self::ProtectedMaskDimensionsMismatch => {
                "local sampling protected mask has different dimensions"
            }
            Self::PlaneOutOfBounds => "local sampling plane is outside the image",
            Self::CellLimitExceeded => "local sampling grid exceeds its cell limit",
            Self::ArithmeticOverflow => "local sampling grid arithmetic overflowed",
            Self::AllocationFailed => "local sampling allocation failed",
            Self::InvalidRetainedSample => "local sampling retained an invalid value pair",
        })
    }
}

impl Error for SamplingError {}

/// Extracts bounded, deterministic samples from matching registered images.
///
/// Any non-clear protected-mask entry wins over image-quality exclusions. The
/// source mask wins next, followed by the reference mask and non-finite values,
/// making every input pixel belong to exactly one evidence category. Eligible
/// pixels with the smallest stable coordinate priorities are retained.
pub fn sample_local_grid(
    source: &ScientificImage,
    reference: &ScientificImage,
    protected_regions: Option<&PixelMask>,
    plane: usize,
    parameters: SamplingGridParameters,
) -> Result<Vec<LocalCellSamples>, SamplingError> {
    let dimensions = source.dimensions();
    if reference.dimensions() != dimensions {
        return Err(SamplingError::ImageDimensionsMismatch);
    }
    if protected_regions.is_some_and(|mask| mask.dimensions() != dimensions) {
        return Err(SamplingError::ProtectedMaskDimensionsMismatch);
    }
    if plane >= dimensions.planes() {
        return Err(SamplingError::PlaneOutOfBounds);
    }

    let columns = ceil_div(dimensions.width(), parameters.cell_width);
    let rows = ceil_div(dimensions.height(), parameters.cell_height);
    let cell_count = columns
        .checked_mul(rows)
        .ok_or(SamplingError::ArithmeticOverflow)?;
    if cell_count > parameters.maximum_cells {
        return Err(SamplingError::CellLimitExceeded);
    }

    let mut cells = Vec::new();
    cells
        .try_reserve_exact(cell_count)
        .map_err(|_| SamplingError::AllocationFailed)?;
    let plane_offset = plane
        .checked_mul(
            dimensions
                .width()
                .checked_mul(dimensions.height())
                .ok_or(SamplingError::ArithmeticOverflow)?,
        )
        .ok_or(SamplingError::ArithmeticOverflow)?;

    for row in 0..rows {
        for column in 0..columns {
            let x = column
                .checked_mul(parameters.cell_width)
                .ok_or(SamplingError::ArithmeticOverflow)?;
            let y = row
                .checked_mul(parameters.cell_height)
                .ok_or(SamplingError::ArithmeticOverflow)?;
            let bounds = CellBounds {
                x,
                y,
                width: parameters.cell_width.min(dimensions.width() - x),
                height: parameters.cell_height.min(dimensions.height() - y),
            };
            cells.push(sample_cell(
                source,
                reference,
                protected_regions,
                plane_offset,
                bounds,
                parameters.maximum_samples_per_cell,
            )?);
        }
    }
    Ok(cells)
}

/// Fits every sampled cell without discarding failures from sparse regions.
///
/// A result entry is emitted for every input cell in the same order. Only
/// allocation of the result vector can fail the complete operation; scientific
/// failures stay attached to their cells for support diagnostics and later
/// surface-validity decisions.
pub fn fit_local_grid(
    cells: &[LocalCellSamples],
    parameters: LocalFitParameters,
) -> Result<Vec<LocalCellFit>, LocalFitError> {
    let mut fits = Vec::new();
    fits.try_reserve_exact(cells.len())
        .map_err(|_| LocalFitError::AllocationFailed)?;
    for cell in cells {
        let result = fit_local_affine_iter(
            cell.samples.iter().map(|sample| sample.pair),
            cell.samples.len(),
            parameters,
        );
        fits.push(LocalCellFit {
            bounds: cell.bounds,
            sampling_evidence: cell.evidence,
            result,
        });
    }
    Ok(fits)
}

fn sample_cell(
    source: &ScientificImage,
    reference: &ScientificImage,
    protected_regions: Option<&PixelMask>,
    plane_offset: usize,
    bounds: CellBounds,
    sample_limit: usize,
) -> Result<LocalCellSamples, SamplingError> {
    let width = source.dimensions().width();
    let capacity = sample_limit.min(bounds.area());
    let mut selected = BinaryHeap::<(u64, usize, usize)>::new();
    selected
        .try_reserve_exact(capacity)
        .map_err(|_| SamplingError::AllocationFailed)?;
    let mut evidence = CellSamplingEvidence::default();

    for y in bounds.y..bounds.y + bounds.height {
        for x in bounds.x..bounds.x + bounds.width {
            let index = plane_offset + y * width + x;
            if protected_regions.is_some_and(|mask| !mask.as_slice()[index].is_clear()) {
                evidence.protected += 1;
                continue;
            }
            if !source.mask().as_slice()[index].is_clear() {
                evidence.source_masked += 1;
                continue;
            }
            if !reference.mask().as_slice()[index].is_clear() {
                evidence.reference_masked += 1;
                continue;
            }
            let source_value = source.pixels()[index];
            let reference_value = reference.pixels()[index];
            if !source_value.is_finite() || !reference_value.is_finite() {
                evidence.non_finite += 1;
                continue;
            }
            evidence.eligible += 1;
            let candidate = (coordinate_priority(x, y), y, x);
            if selected.len() < sample_limit {
                selected.push(candidate);
            } else if selected.peek().is_some_and(|largest| candidate < *largest) {
                selected.pop();
                selected.push(candidate);
            }
        }
    }

    let mut coordinates = selected.into_vec();
    coordinates.sort_unstable_by_key(|&(_, y, x)| (y, x));
    let mut samples = Vec::new();
    samples
        .try_reserve_exact(coordinates.len())
        .map_err(|_| SamplingError::AllocationFailed)?;
    for (_, y, x) in coordinates {
        let index = plane_offset + y * width + x;
        let pair = NormalizationSample::new(source.pixels()[index], reference.pixels()[index])
            .map_err(map_retained_sample_error)?;
        samples.push(SpatialSample { x, y, pair });
    }
    evidence.retained = samples.len();
    Ok(LocalCellSamples {
        bounds,
        samples,
        evidence,
    })
}

fn map_retained_sample_error(_: LocalFitError) -> SamplingError {
    SamplingError::InvalidRetainedSample
}

const fn ceil_div(value: usize, divisor: usize) -> usize {
    1 + (value - 1) / divisor
}

const fn coordinate_priority(x: usize, y: usize) -> u64 {
    let mut value = (x as u64) ^ (y as u64).rotate_left(32) ^ 0x9e37_79b9_7f4a_7c15;
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::{Dimensions, PixelFlags};

    type TestResult = Result<(), Box<dyn Error>>;

    fn image(
        width: usize,
        height: usize,
        values: Vec<f64>,
    ) -> Result<ScientificImage, Box<dyn Error>> {
        Ok(ScientificImage::from_pixels(
            Dimensions::new(width, height, 1)?,
            values,
        )?)
    }

    fn parameters(sample_limit: usize) -> Result<SamplingGridParameters, SamplingError> {
        SamplingGridParameters::new(3, 2, sample_limit, 8)
    }

    #[test]
    fn partitions_edge_cells_and_accounts_for_every_pixel() -> TestResult {
        let source = image(5, 3, (0..15).map(f64::from).collect())?;
        let reference = image(5, 3, (0..15).map(|value| f64::from(value * 2)).collect())?;
        let cells = sample_local_grid(&source, &reference, None, 0, parameters(16)?)?;
        assert_eq!(cells.len(), 4);
        assert_eq!(
            cells[0].bounds(),
            CellBounds {
                x: 0,
                y: 0,
                width: 3,
                height: 2
            }
        );
        assert_eq!(
            cells[3].bounds(),
            CellBounds {
                x: 3,
                y: 2,
                width: 2,
                height: 1
            }
        );
        for cell in &cells {
            assert_eq!(cell.evidence().classified_pixels(), cell.bounds().area());
            assert_eq!(cell.evidence().retained(), cell.bounds().area());
            assert_eq!(cell.evidence().downsampled(), 0);
        }
        Ok(())
    }

    #[test]
    fn exclusion_precedence_and_sample_cap_are_explicit() -> TestResult {
        let mut source = image(3, 2, vec![0.0, 1.0, 2.0, 3.0, f64::NAN, 5.0])?;
        let mut reference = image(3, 2, vec![0.0, 2.0, 4.0, 6.0, 8.0, 10.0])?;
        let dimensions = source.dimensions();
        let mut protected = PixelMask::clear(dimensions)?;
        protected.set(0, 0, 0, PixelFlags::REJECTED)?;
        source.mark(0, 0, 0, PixelFlags::HOT)?;
        source.mark(1, 0, 0, PixelFlags::HOT)?;
        reference.mark(2, 0, 0, PixelFlags::SATURATED)?;

        let first = sample_local_grid(&source, &reference, Some(&protected), 0, parameters(1)?)?;
        let second = sample_local_grid(&source, &reference, Some(&protected), 0, parameters(1)?)?;
        assert_eq!(first, second);
        let evidence = first[0].evidence();
        assert_eq!(evidence.protected(), 1);
        assert_eq!(evidence.source_masked(), 1);
        assert_eq!(evidence.reference_masked(), 1);
        assert_eq!(evidence.non_finite(), 1);
        assert_eq!(evidence.eligible(), 2);
        assert_eq!(evidence.retained(), 1);
        assert_eq!(evidence.downsampled(), 1);
        assert_eq!(evidence.classified_pixels(), 6);
        assert_eq!(first[0].samples().len(), 1);
        Ok(())
    }

    #[test]
    fn validation_rejects_mismatches_planes_and_excessive_grids() -> TestResult {
        assert_eq!(
            SamplingGridParameters::new(0, 1, 1, 1),
            Err(SamplingError::InvalidParameters)
        );
        let source = image(2, 2, vec![0.0; 4])?;
        let other = image(3, 2, vec![0.0; 6])?;
        let controls = SamplingGridParameters::new(1, 1, 1, 4)?;
        assert_eq!(
            sample_local_grid(&source, &other, None, 0, controls),
            Err(SamplingError::ImageDimensionsMismatch)
        );
        assert_eq!(
            sample_local_grid(&source, &source, None, 1, controls),
            Err(SamplingError::PlaneOutOfBounds)
        );
        let too_few_cells = SamplingGridParameters::new(1, 1, 1, 3)?;
        assert_eq!(
            sample_local_grid(&source, &source, None, 0, too_few_cells),
            Err(SamplingError::CellLimitExceeded)
        );
        let wrong_mask = PixelMask::clear(other.dimensions())?;
        assert_eq!(
            sample_local_grid(&source, &source, Some(&wrong_mask), 0, controls),
            Err(SamplingError::ProtectedMaskDimensionsMismatch)
        );
        Ok(())
    }

    #[test]
    fn grid_fitting_preserves_successes_and_cell_local_failures() -> TestResult {
        let source = image(4, 2, vec![0.0, 1.0, 10.0, 11.0, 2.0, 3.0, 12.0, 13.0])?;
        let reference = image(4, 2, vec![1.0, 3.0, 21.0, 23.0, 5.0, 7.0, 25.0, 27.0])?;
        let sampling = SamplingGridParameters::new(2, 2, 4, 2)?;
        let cells = sample_local_grid(&source, &reference, None, 0, sampling)?;
        let fit_parameters = LocalFitParameters::new(4, 4, 6, 1.0e-12)?;
        let fits = fit_local_grid(&cells, fit_parameters)?;
        assert_eq!(fits.len(), 2);
        for fit in &fits {
            let model = fit.result()?;
            assert_eq!(model.scale().to_bits(), 2.0_f64.to_bits());
            assert_eq!(model.offset().to_bits(), 1.0_f64.to_bits());
            assert_eq!(fit.sampling_evidence().retained(), 4);
        }

        let sparse_sampling = SamplingGridParameters::new(2, 2, 3, 2)?;
        let sparse_cells = sample_local_grid(&source, &reference, None, 0, sparse_sampling)?;
        let sparse_fits = fit_local_grid(&sparse_cells, fit_parameters)?;
        assert!(sparse_fits.iter().all(|fit| {
            fit.result() == Err(LocalFitError::SampleCountOutsideBounds)
                && fit.sampling_evidence().retained() == 3
        }));
        Ok(())
    }
}
