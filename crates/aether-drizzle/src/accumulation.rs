use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_core::{CompensatedSum, Dimensions, PixelFlags};

use crate::{CfaDrizzleDeposition, DrizzleDeposition};

/// Half-open output rectangle owned by one bounded Drizzle accumulator.
///
/// Coordinates are global output coordinates. The right and bottom edges are
/// excluded, so adjacent tiles cannot own the same contribution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DrizzleTileBounds {
    origin_x: u32,
    origin_y: u32,
    dimensions: Dimensions,
}

impl DrizzleTileBounds {
    /// Builds a nonempty monochrome or planar-RGB tile.
    pub fn new(
        origin_x: u32,
        origin_y: u32,
        width: u32,
        height: u32,
        planes: usize,
    ) -> Result<Self, DrizzleAccumulationError> {
        if width == 0 || height == 0 || !matches!(planes, 1 | 3) {
            return Err(DrizzleAccumulationError::InvalidBounds);
        }
        let width = usize::try_from(width).map_err(|_| DrizzleAccumulationError::InvalidBounds)?;
        let height =
            usize::try_from(height).map_err(|_| DrizzleAccumulationError::InvalidBounds)?;
        let dimensions = Dimensions::new(width, height, planes)
            .map_err(|_| DrizzleAccumulationError::InvalidBounds)?;
        Ok(Self {
            origin_x,
            origin_y,
            dimensions,
        })
    }

    /// Global column of the tile's first pixel.
    #[must_use]
    pub const fn origin_x(self) -> u32 {
        self.origin_x
    }

    /// Global row of the tile's first pixel.
    #[must_use]
    pub const fn origin_y(self) -> u32 {
        self.origin_y
    }

    /// Local planar dimensions.
    #[must_use]
    pub const fn dimensions(self) -> Dimensions {
        self.dimensions
    }

    fn local_index(self, x: u32, y: u32, plane: usize) -> Option<usize> {
        if plane >= self.dimensions.planes()
            || x < self.origin_x
            || y < self.origin_y
            || u64::from(x) >= u64::from(self.origin_x) + self.dimensions.width() as u64
            || u64::from(y) >= u64::from(self.origin_y) + self.dimensions.height() as u64
        {
            return None;
        }
        let local_x = usize::try_from(x - self.origin_x).ok()?;
        let local_y = usize::try_from(y - self.origin_y).ok()?;
        self.dimensions.linear_index(local_x, local_y, plane).ok()
    }
}

/// Checked failure while allocating, accumulating, or finalizing a tile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DrizzleAccumulationError {
    /// Tile dimensions or plane count are outside the strict profile.
    InvalidBounds,
    /// A requested output plane does not belong to the tile.
    InvalidPlane,
    /// Required storage could not be reserved.
    AllocationFailed,
    /// An evidence or per-pixel contribution counter overflowed.
    CounterOverflow,
    /// A contribution or finalized compensated sum was not finite and valid.
    InvalidContribution,
}

impl Display for DrizzleAccumulationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidBounds => "Drizzle tile bounds are invalid or unrepresentable",
            Self::InvalidPlane => "Drizzle destination plane is outside the tile",
            Self::AllocationFailed => "Drizzle tile allocation failed",
            Self::CounterOverflow => "Drizzle accumulation counter overflowed",
            Self::InvalidContribution => "Drizzle contribution is invalid or non-finite",
        })
    }
}

impl Error for DrizzleAccumulationError {}

/// Auditable counters for all contributions presented to one tile.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DrizzleTileEvidence {
    depositions_seen: u64,
    contributions_seen: u64,
    contributions_accumulated: u64,
    contributions_outside: u64,
    unsupported_pixels: u64,
}

impl DrizzleTileEvidence {
    /// Number of source depositions presented to the tile.
    #[must_use]
    pub const fn depositions_seen(self) -> u64 {
        self.depositions_seen
    }

    /// Number of individual geometric contributions inspected.
    #[must_use]
    pub const fn contributions_seen(self) -> u64 {
        self.contributions_seen
    }

    /// Number of contributions owned and accumulated by the tile.
    #[must_use]
    pub const fn contributions_accumulated(self) -> u64 {
        self.contributions_accumulated
    }

    /// Number of valid contributions owned by another tile.
    #[must_use]
    pub const fn contributions_outside(self) -> u64 {
        self.contributions_outside
    }

    /// Number of finalized output samples with no weight.
    #[must_use]
    pub const fn unsupported_pixels(self) -> u64 {
        self.unsupported_pixels
    }

    /// Checked sum of independent tile-execution counters.
    ///
    /// This is intended for aggregating disjoint tiles. Counters describing
    /// inspected geometry remain execution totals, while accumulated support
    /// and unsupported output pixels are disjoint by tile ownership.
    pub fn checked_add(self, other: Self) -> Result<Self, DrizzleAccumulationError> {
        Ok(Self {
            depositions_seen: self
                .depositions_seen
                .checked_add(other.depositions_seen)
                .ok_or(DrizzleAccumulationError::CounterOverflow)?,
            contributions_seen: self
                .contributions_seen
                .checked_add(other.contributions_seen)
                .ok_or(DrizzleAccumulationError::CounterOverflow)?,
            contributions_accumulated: self
                .contributions_accumulated
                .checked_add(other.contributions_accumulated)
                .ok_or(DrizzleAccumulationError::CounterOverflow)?,
            contributions_outside: self
                .contributions_outside
                .checked_add(other.contributions_outside)
                .ok_or(DrizzleAccumulationError::CounterOverflow)?,
            unsupported_pixels: self
                .unsupported_pixels
                .checked_add(other.unsupported_pixels)
                .ok_or(DrizzleAccumulationError::CounterOverflow)?,
        })
    }
}

/// Bounded deterministic accumulator for one output tile.
#[derive(Clone, Debug)]
pub struct DrizzleTileAccumulator {
    bounds: DrizzleTileBounds,
    weighted_flux: Vec<CompensatedSum>,
    weights: Vec<CompensatedSum>,
    contribution_counts: Vec<u64>,
    evidence: DrizzleTileEvidence,
}

impl DrizzleTileAccumulator {
    /// Allocates zeroed accumulation maps for one validated tile.
    pub fn new(bounds: DrizzleTileBounds) -> Result<Self, DrizzleAccumulationError> {
        let elements = bounds.dimensions.pixel_count();
        Ok(Self {
            bounds,
            weighted_flux: zeroed_vec(elements)?,
            weights: zeroed_vec(elements)?,
            contribution_counts: zeroed_vec(elements)?,
            evidence: DrizzleTileEvidence::default(),
        })
    }

    /// Tile owned by this accumulator.
    #[must_use]
    pub const fn bounds(&self) -> DrizzleTileBounds {
        self.bounds
    }

    /// Current evidence, before unsupported output pixels are counted.
    #[must_use]
    pub const fn evidence(&self) -> DrizzleTileEvidence {
        self.evidence
    }

    /// Accumulates one monochrome deposition into an explicit destination plane.
    ///
    /// The operation validates all counters and contributions before mutating
    /// any sum, so an error cannot leave a partially accumulated deposition.
    pub fn accumulate(
        &mut self,
        deposition: &DrizzleDeposition,
        plane: usize,
    ) -> Result<(), DrizzleAccumulationError> {
        if plane >= self.bounds.dimensions.planes() {
            return Err(DrizzleAccumulationError::InvalidPlane);
        }
        let mut owned = 0_u64;
        for contribution in deposition.contributions() {
            if !contribution.weighted_flux().is_finite()
                || !contribution.weight().is_finite()
                || contribution.weight() <= 0.0
            {
                return Err(DrizzleAccumulationError::InvalidContribution);
            }
            if let Some(index) = self
                .bounds
                .local_index(contribution.x(), contribution.y(), plane)
            {
                self.contribution_counts[index]
                    .checked_add(1)
                    .ok_or(DrizzleAccumulationError::CounterOverflow)?;
                owned = owned
                    .checked_add(1)
                    .ok_or(DrizzleAccumulationError::CounterOverflow)?;
            }
        }
        let seen = u64::try_from(deposition.contributions().len())
            .map_err(|_| DrizzleAccumulationError::CounterOverflow)?;
        let outside = seen
            .checked_sub(owned)
            .ok_or(DrizzleAccumulationError::CounterOverflow)?;
        let next_evidence = DrizzleTileEvidence {
            depositions_seen: self
                .evidence
                .depositions_seen
                .checked_add(1)
                .ok_or(DrizzleAccumulationError::CounterOverflow)?,
            contributions_seen: self
                .evidence
                .contributions_seen
                .checked_add(seen)
                .ok_or(DrizzleAccumulationError::CounterOverflow)?,
            contributions_accumulated: self
                .evidence
                .contributions_accumulated
                .checked_add(owned)
                .ok_or(DrizzleAccumulationError::CounterOverflow)?,
            contributions_outside: self
                .evidence
                .contributions_outside
                .checked_add(outside)
                .ok_or(DrizzleAccumulationError::CounterOverflow)?,
            unsupported_pixels: 0,
        };

        for contribution in deposition.contributions() {
            let Some(index) = self
                .bounds
                .local_index(contribution.x(), contribution.y(), plane)
            else {
                continue;
            };
            self.weighted_flux[index].add(contribution.weighted_flux());
            self.weights[index].add(contribution.weight());
            self.contribution_counts[index] += 1;
        }
        self.evidence = next_evidence;
        Ok(())
    }

    /// Accumulates a CFA deposition into its physically measured RGB plane.
    pub fn accumulate_cfa(
        &mut self,
        deposition: &CfaDrizzleDeposition,
    ) -> Result<(), DrizzleAccumulationError> {
        if self.bounds.dimensions.planes() != 3 {
            return Err(DrizzleAccumulationError::InvalidPlane);
        }
        self.accumulate(deposition.deposition(), deposition.channel().plane())
    }

    /// Finalizes normalized values, weights, support counts, flags, and evidence.
    pub fn finish(self) -> Result<DrizzleTileResult, DrizzleAccumulationError> {
        let elements = self.bounds.dimensions.pixel_count();
        let mut values = reserved_vec(elements)?;
        let mut weights = reserved_vec(elements)?;
        let mut flags = reserved_vec(elements)?;
        let mut unsupported_pixels = 0_u64;
        for index in 0..elements {
            let weight = self.weights[index].total();
            let weighted_flux = self.weighted_flux[index].total();
            if !weight.is_finite() || !weighted_flux.is_finite() || weight < 0.0 {
                return Err(DrizzleAccumulationError::InvalidContribution);
            }
            if weight == 0.0 {
                values.push(f64::NAN);
                weights.push(0.0);
                flags.push(PixelFlags::MISSING);
                unsupported_pixels = unsupported_pixels
                    .checked_add(1)
                    .ok_or(DrizzleAccumulationError::CounterOverflow)?;
            } else {
                let value = weighted_flux / weight;
                if !value.is_finite() {
                    return Err(DrizzleAccumulationError::InvalidContribution);
                }
                values.push(value);
                weights.push(weight);
                flags.push(PixelFlags::CLEAR);
            }
        }
        let mut evidence = self.evidence;
        evidence.unsupported_pixels = unsupported_pixels;
        Ok(DrizzleTileResult {
            bounds: self.bounds,
            values,
            weights,
            contribution_counts: self.contribution_counts,
            flags,
            evidence,
        })
    }
}

/// Final normalized science and support maps for one tile.
#[derive(Clone, Debug, PartialEq)]
pub struct DrizzleTileResult {
    bounds: DrizzleTileBounds,
    values: Vec<f64>,
    weights: Vec<f64>,
    contribution_counts: Vec<u64>,
    flags: Vec<PixelFlags>,
    evidence: DrizzleTileEvidence,
}

impl DrizzleTileResult {
    /// Global bounds and local planar dimensions.
    #[must_use]
    pub const fn bounds(&self) -> DrizzleTileBounds {
        self.bounds
    }

    /// Normalized planar science values; unsupported samples are NaN.
    #[must_use]
    pub fn values(&self) -> &[f64] {
        &self.values
    }

    /// Compensated planar accumulated weights.
    #[must_use]
    pub fn weights(&self) -> &[f64] {
        &self.weights
    }

    /// Number of detector contributions supporting each planar sample.
    #[must_use]
    pub fn contribution_counts(&self) -> &[u64] {
        &self.contribution_counts
    }

    /// Output quality flags; zero-support samples contain `MISSING`.
    #[must_use]
    pub fn flags(&self) -> &[PixelFlags] {
        &self.flags
    }

    /// Complete accumulation and finalization evidence.
    #[must_use]
    pub const fn evidence(&self) -> DrizzleTileEvidence {
        self.evidence
    }
}

fn zeroed_vec<T: Clone + Default>(length: usize) -> Result<Vec<T>, DrizzleAccumulationError> {
    let mut values = reserved_vec(length)?;
    values.resize(length, T::default());
    Ok(values)
}

fn reserved_vec<T>(capacity: usize) -> Result<Vec<T>, DrizzleAccumulationError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(capacity)
        .map_err(|_| DrizzleAccumulationError::AllocationFailed)?;
    Ok(values)
}

#[cfg(test)]
mod tests {
    use aether_metadata::BayerPattern;
    use aether_registration::ProjectiveTransform;

    use super::*;
    use crate::{
        DrizzleParameters, deposit_cfa_detector_pixel, deposit_detector_footprint,
        project_detector_footprint,
    };

    type TestResult = Result<(), Box<dyn Error>>;

    fn identity_deposition(
        source_x: u32,
        source_y: u32,
        value: f64,
        weight: f64,
        output_width: u32,
        output_height: u32,
    ) -> Result<DrizzleDeposition, Box<dyn Error>> {
        let footprint = project_detector_footprint(
            source_x,
            source_y,
            ProjectiveTransform::IDENTITY,
            DrizzleParameters::new(1, 1.0)?,
        )?;
        Ok(deposit_detector_footprint(
            footprint,
            value,
            weight,
            output_width,
            output_height,
            4,
        )?)
    }

    #[test]
    fn reconstructs_weighted_values_and_support_maps() -> TestResult {
        let bounds = DrizzleTileBounds::new(0, 0, 2, 1, 1)?;
        let mut accumulator = DrizzleTileAccumulator::new(bounds)?;
        accumulator.accumulate(&identity_deposition(0, 0, 10.0, 2.0, 2, 1)?, 0)?;
        accumulator.accumulate(&identity_deposition(0, 0, 20.0, 1.0, 2, 1)?, 0)?;
        accumulator.accumulate(&identity_deposition(1, 0, -5.0, 4.0, 2, 1)?, 0)?;

        let result = accumulator.finish()?;
        assert_eq!(result.values(), &[40.0 / 3.0, -5.0]);
        assert_eq!(result.weights(), &[3.0, 4.0]);
        assert_eq!(result.contribution_counts(), &[2, 1]);
        assert_eq!(result.flags(), &[PixelFlags::CLEAR, PixelFlags::CLEAR]);
        assert_eq!(result.evidence().depositions_seen(), 3);
        assert_eq!(result.evidence().contributions_seen(), 3);
        assert_eq!(result.evidence().contributions_accumulated(), 3);
        assert_eq!(result.evidence().contributions_outside(), 0);
        assert_eq!(result.evidence().unsupported_pixels(), 0);
        Ok(())
    }

    #[test]
    fn marks_zero_support_as_missing_nan() -> TestResult {
        let result =
            DrizzleTileAccumulator::new(DrizzleTileBounds::new(4, 8, 2, 1, 1)?)?.finish()?;
        assert!(result.values().iter().all(|value| value.is_nan()));
        assert_eq!(result.weights(), &[0.0, 0.0]);
        assert_eq!(result.contribution_counts(), &[0, 0]);
        assert_eq!(result.flags(), &[PixelFlags::MISSING, PixelFlags::MISSING]);
        assert_eq!(result.evidence().unsupported_pixels(), 2);
        Ok(())
    }

    #[test]
    fn adjacent_tiles_own_every_contribution_exactly_once() -> TestResult {
        let deposition = identity_deposition(1, 0, 42.0, 3.0, 3, 1)?;
        let mut left = DrizzleTileAccumulator::new(DrizzleTileBounds::new(0, 0, 1, 1, 1)?)?;
        let mut right = DrizzleTileAccumulator::new(DrizzleTileBounds::new(1, 0, 2, 1, 1)?)?;
        left.accumulate(&deposition, 0)?;
        right.accumulate(&deposition, 0)?;
        let left = left.finish()?;
        let right = right.finish()?;
        assert_eq!(left.evidence().contributions_accumulated(), 0);
        assert_eq!(right.evidence().contributions_accumulated(), 1);
        assert_eq!(
            left.evidence().contributions_accumulated()
                + right.evidence().contributions_accumulated(),
            1
        );
        assert_eq!(right.values()[0].to_bits(), 42.0_f64.to_bits());
        Ok(())
    }

    #[test]
    fn split_tiles_are_bit_identical_to_one_complete_tile() -> TestResult {
        let depositions = [
            identity_deposition(0, 0, 1.0e16, 1.0, 4, 1)?,
            identity_deposition(0, 0, 1.0, 1.0, 4, 1)?,
            identity_deposition(0, 0, -1.0e16, 1.0, 4, 1)?,
            identity_deposition(1, 0, 11.0, 2.0, 4, 1)?,
            identity_deposition(2, 0, 12.0, 3.0, 4, 1)?,
            identity_deposition(3, 0, 13.0, 4.0, 4, 1)?,
        ];
        let mut complete = DrizzleTileAccumulator::new(DrizzleTileBounds::new(0, 0, 4, 1, 1)?)?;
        let mut left = DrizzleTileAccumulator::new(DrizzleTileBounds::new(0, 0, 2, 1, 1)?)?;
        let mut right = DrizzleTileAccumulator::new(DrizzleTileBounds::new(2, 0, 2, 1, 1)?)?;
        for deposition in &depositions {
            complete.accumulate(deposition, 0)?;
            left.accumulate(deposition, 0)?;
            right.accumulate(deposition, 0)?;
        }
        let complete = complete.finish()?;
        let split = [left.finish()?, right.finish()?];
        for (index, expected) in complete.values().iter().enumerate() {
            let tile = usize::from(index >= 2);
            let local = index % 2;
            assert_eq!(split[tile].values()[local].to_bits(), expected.to_bits());
            assert_eq!(
                split[tile].weights()[local].to_bits(),
                complete.weights()[index].to_bits()
            );
            assert_eq!(
                split[tile].contribution_counts()[local],
                complete.contribution_counts()[index]
            );
            assert_eq!(split[tile].flags()[local], complete.flags()[index]);
        }
        assert_eq!(complete.values()[0].to_bits(), (1.0 / 3.0_f64).to_bits());
        Ok(())
    }

    #[test]
    fn cfa_samples_remain_in_their_physical_planes() -> TestResult {
        let mut accumulator = DrizzleTileAccumulator::new(DrizzleTileBounds::new(0, 0, 2, 2, 3)?)?;
        for (x, y, value) in [(0, 0, 10.0), (1, 0, 20.0), (0, 1, 30.0), (1, 1, 40.0)] {
            let deposition = deposit_cfa_detector_pixel(
                x,
                y,
                ProjectiveTransform::IDENTITY,
                DrizzleParameters::new(1, 1.0)?,
                &BayerPattern::Rggb,
                value,
                1.0,
                2,
                2,
                4,
            )?;
            accumulator.accumulate_cfa(&deposition)?;
        }
        let result = accumulator.finish()?;
        let plane_stride = 4;
        assert_eq!(result.values()[0].to_bits(), 10.0_f64.to_bits());
        assert_eq!(
            result.values()[plane_stride + 1].to_bits(),
            20.0_f64.to_bits()
        );
        assert_eq!(
            result.values()[plane_stride + 2].to_bits(),
            30.0_f64.to_bits()
        );
        assert_eq!(
            result.values()[2 * plane_stride + 3].to_bits(),
            40.0_f64.to_bits()
        );
        assert_eq!(result.evidence().contributions_accumulated(), 4);
        assert_eq!(result.evidence().unsupported_pixels(), 8);
        Ok(())
    }

    #[test]
    fn rejects_invalid_bounds_and_planes() -> TestResult {
        assert_eq!(
            DrizzleTileBounds::new(0, 0, 0, 1, 1),
            Err(DrizzleAccumulationError::InvalidBounds)
        );
        assert_eq!(
            DrizzleTileBounds::new(0, 0, 1, 1, 2),
            Err(DrizzleAccumulationError::InvalidBounds)
        );
        let deposition = identity_deposition(0, 0, 1.0, 1.0, 1, 1)?;
        let mut mono = DrizzleTileAccumulator::new(DrizzleTileBounds::new(0, 0, 1, 1, 1)?)?;
        assert_eq!(
            mono.accumulate(&deposition, 1),
            Err(DrizzleAccumulationError::InvalidPlane)
        );
        Ok(())
    }
}
