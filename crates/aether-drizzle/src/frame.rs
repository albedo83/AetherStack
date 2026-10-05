use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_core::ScientificImage;
use aether_metadata::BayerPattern;
use aether_registration::ProjectiveTransform;

use crate::{
    DetectorSample, DrizzleAccumulationError, DrizzleError, DrizzleOutputBounds, DrizzleParameters,
    DrizzleSampleExclusion, DrizzleSampleOutcome, DrizzleTileAccumulator, cfa_channel,
    deposit_cfa_sample,
};

/// Per-frame accounting before contributions are merged with other exposures.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DrizzleFrameEvidence {
    source_samples: u64,
    masked_samples: u64,
    nonfinite_samples: u64,
    deposited_samples: u64,
    outside_output_samples: u64,
    geometric_contributions: u64,
}

impl DrizzleFrameEvidence {
    /// Complete number of detector samples visited exactly once.
    #[must_use]
    pub const fn source_samples(self) -> u64 {
        self.source_samples
    }

    /// Samples excluded by any pre-existing quality bit.
    #[must_use]
    pub const fn masked_samples(self) -> u64 {
        self.masked_samples
    }

    /// Clear samples excluded because their value was NaN or infinite.
    #[must_use]
    pub const fn nonfinite_samples(self) -> u64 {
        self.nonfinite_samples
    }

    /// Clear finite samples that reached geometric deposition.
    #[must_use]
    pub const fn deposited_samples(self) -> u64 {
        self.deposited_samples
    }

    /// Deposited samples whose complete footprint missed the global output.
    #[must_use]
    pub const fn outside_output_samples(self) -> u64 {
        self.outside_output_samples
    }

    /// Geometric output-pixel contributions before tile ownership filtering.
    #[must_use]
    pub const fn geometric_contributions(self) -> u64 {
        self.geometric_contributions
    }
}

/// Failure while validating or accumulating one complete CFA detector frame.
#[derive(Debug)]
pub enum DrizzleFrameError {
    /// Input must contain exactly one detector plane with `u32`-addressable axes.
    InvalidSourceDimensions,
    /// The destination accumulator must be planar RGB and inside global output.
    InvalidTileBounds,
    /// Frame weight must be finite and strictly positive.
    InvalidFrameWeight,
    /// Evidence accounting exceeded its representable domain.
    CounterOverflow,
    /// CFA routing or footprint projection failed.
    Geometry(DrizzleError),
    /// A valid geometric deposition could not be accumulated.
    Accumulation(DrizzleAccumulationError),
}

impl Display for DrizzleFrameError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSourceDimensions => formatter.write_str(
                "Drizzle CFA source must be one plane with representable detector dimensions",
            ),
            Self::InvalidTileBounds => formatter
                .write_str("Drizzle CFA tile must be planar RGB and inside the global output"),
            Self::InvalidFrameWeight => {
                formatter.write_str("Drizzle frame weight must be finite and positive")
            }
            Self::CounterOverflow => formatter.write_str("Drizzle frame evidence overflowed"),
            Self::Geometry(error) => Display::fmt(error, formatter),
            Self::Accumulation(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for DrizzleFrameError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Geometry(error) => Some(error),
            Self::Accumulation(error) => Some(error),
            Self::InvalidSourceDimensions
            | Self::InvalidTileBounds
            | Self::InvalidFrameWeight
            | Self::CounterOverflow => None,
        }
    }
}

/// Accumulates one original CFA frame in deterministic detector row-major order.
///
/// Global controls, source shape, CFA support, frame weight, and tile containment
/// are validated before the first sample is inspected. Masked and non-finite
/// samples remain separately accounted and never reach geometry.
#[allow(clippy::too_many_arguments)]
pub fn accumulate_cfa_frame(
    accumulator: &mut DrizzleTileAccumulator,
    source: &ScientificImage,
    transform: ProjectiveTransform,
    parameters: DrizzleParameters,
    pattern: &BayerPattern,
    frame_weight: f64,
    output: DrizzleOutputBounds,
) -> Result<DrizzleFrameEvidence, DrizzleFrameError> {
    let source_dimensions = source.dimensions();
    if source_dimensions.planes() != 1
        || u32::try_from(source_dimensions.width()).is_err()
        || u32::try_from(source_dimensions.height()).is_err()
    {
        return Err(DrizzleFrameError::InvalidSourceDimensions);
    }
    if !frame_weight.is_finite() || frame_weight <= 0.0 {
        return Err(DrizzleFrameError::InvalidFrameWeight);
    }
    cfa_channel(pattern, 0, 0).map_err(DrizzleFrameError::Geometry)?;
    let tile = accumulator.bounds();
    let tile_dimensions = tile.dimensions();
    if tile_dimensions.planes() != 3
        || u64::from(tile.origin_x()) + tile_dimensions.width() as u64 > u64::from(output.width())
        || u64::from(tile.origin_y()) + tile_dimensions.height() as u64 > u64::from(output.height())
    {
        return Err(DrizzleFrameError::InvalidTileBounds);
    }

    let source_samples = u64::try_from(source_dimensions.pixel_count())
        .map_err(|_| DrizzleFrameError::CounterOverflow)?;
    let width = source_dimensions.width();
    let pixels = source.pixels();
    let flags = source.mask().as_slice();
    let mut evidence = DrizzleFrameEvidence {
        source_samples,
        ..DrizzleFrameEvidence::default()
    };
    for (index, (value, flags)) in pixels.iter().zip(flags).enumerate() {
        let x =
            u32::try_from(index % width).map_err(|_| DrizzleFrameError::InvalidSourceDimensions)?;
        let y =
            u32::try_from(index / width).map_err(|_| DrizzleFrameError::InvalidSourceDimensions)?;
        match deposit_cfa_sample(
            DetectorSample::new(x, y, *value, *flags),
            transform,
            parameters,
            pattern,
            frame_weight,
            output,
        )
        .map_err(DrizzleFrameError::Geometry)?
        {
            DrizzleSampleOutcome::Excluded { reason, flags: _ } => match reason {
                DrizzleSampleExclusion::Masked => increment(&mut evidence.masked_samples)?,
                DrizzleSampleExclusion::NonFinite => increment(&mut evidence.nonfinite_samples)?,
            },
            DrizzleSampleOutcome::Deposited(deposition) => {
                increment(&mut evidence.deposited_samples)?;
                let contribution_count =
                    u64::try_from(deposition.deposition().contributions().len())
                        .map_err(|_| DrizzleFrameError::CounterOverflow)?;
                if contribution_count == 0 {
                    increment(&mut evidence.outside_output_samples)?;
                }
                evidence.geometric_contributions = evidence
                    .geometric_contributions
                    .checked_add(contribution_count)
                    .ok_or(DrizzleFrameError::CounterOverflow)?;
                accumulator
                    .accumulate_cfa(&deposition)
                    .map_err(DrizzleFrameError::Accumulation)?;
            }
        }
    }
    let accounted = evidence
        .masked_samples
        .checked_add(evidence.nonfinite_samples)
        .and_then(|value| value.checked_add(evidence.deposited_samples))
        .ok_or(DrizzleFrameError::CounterOverflow)?;
    if accounted != source_samples {
        return Err(DrizzleFrameError::CounterOverflow);
    }
    Ok(evidence)
}

fn increment(value: &mut u64) -> Result<(), DrizzleFrameError> {
    *value = value
        .checked_add(1)
        .ok_or(DrizzleFrameError::CounterOverflow)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use aether_core::{Dimensions, PixelFlags, ScientificImage};
    use aether_metadata::BayerPattern;
    use aether_registration::ProjectiveTransform;

    use super::*;
    use crate::{DrizzleTileAccumulator, DrizzleTileBounds};

    type TestResult = Result<(), Box<dyn Error>>;

    fn image(values: Vec<f64>) -> Result<ScientificImage, Box<dyn Error>> {
        Ok(ScientificImage::from_pixels(
            Dimensions::new(2, 2, 1)?,
            values,
        )?)
    }

    #[test]
    fn accumulates_weighted_cfa_frames_without_inventing_channels() -> TestResult {
        let mut accumulator = DrizzleTileAccumulator::new(DrizzleTileBounds::new(0, 0, 2, 2, 3)?)?;
        let output = DrizzleOutputBounds::new(2, 2, 4)?;
        let first = accumulate_cfa_frame(
            &mut accumulator,
            &image(vec![10.0, 20.0, 30.0, 40.0])?,
            ProjectiveTransform::IDENTITY,
            DrizzleParameters::new(1, 1.0)?,
            &BayerPattern::Rggb,
            1.0,
            output,
        )?;
        let second = accumulate_cfa_frame(
            &mut accumulator,
            &image(vec![20.0, 40.0, 60.0, 80.0])?,
            ProjectiveTransform::IDENTITY,
            DrizzleParameters::new(1, 1.0)?,
            &BayerPattern::Rggb,
            3.0,
            output,
        )?;

        for evidence in [first, second] {
            assert_eq!(evidence.source_samples(), 4);
            assert_eq!(evidence.deposited_samples(), 4);
            assert_eq!(evidence.geometric_contributions(), 4);
            assert_eq!(evidence.masked_samples(), 0);
            assert_eq!(evidence.nonfinite_samples(), 0);
            assert_eq!(evidence.outside_output_samples(), 0);
        }
        let result = accumulator.finish()?;
        let expected = [17.5_f64, 35.0, 52.5, 70.0];
        for (index, expected_index) in [(0, 0), (5, 1), (6, 2), (11, 3)] {
            assert_eq!(
                result.values()[index].to_bits(),
                expected[expected_index].to_bits()
            );
            assert_eq!(result.weights()[index].to_bits(), 4.0_f64.to_bits());
            assert_eq!(result.contribution_counts()[index], 2);
            assert_eq!(result.flags()[index], PixelFlags::CLEAR);
        }
        assert_eq!(result.evidence().depositions_seen(), 8);
        assert_eq!(result.evidence().unsupported_pixels(), 8);
        Ok(())
    }

    #[test]
    fn accounts_masked_nonfinite_and_outside_samples_separately() -> TestResult {
        let mut source = image(vec![1.0, f64::NAN, 3.0, 4.0])?;
        source.mark(0, 0, 0, PixelFlags::HOT)?;
        let mut accumulator = DrizzleTileAccumulator::new(DrizzleTileBounds::new(0, 0, 1, 1, 3)?)?;
        let evidence = accumulate_cfa_frame(
            &mut accumulator,
            &source,
            ProjectiveTransform::new([[1.0, 0.0, 100.0], [0.0, 1.0, 100.0], [0.0, 0.0, 1.0]])?,
            DrizzleParameters::new(1, 1.0)?,
            &BayerPattern::Rggb,
            1.0,
            DrizzleOutputBounds::new(1, 1, 4)?,
        )?;

        assert_eq!(evidence.source_samples(), 4);
        assert_eq!(evidence.masked_samples(), 1);
        assert_eq!(evidence.nonfinite_samples(), 1);
        assert_eq!(evidence.deposited_samples(), 2);
        assert_eq!(evidence.outside_output_samples(), 2);
        assert_eq!(evidence.geometric_contributions(), 0);
        assert_eq!(accumulator.evidence().depositions_seen(), 2);
        assert_eq!(accumulator.evidence().contributions_seen(), 0);
        Ok(())
    }

    #[test]
    fn validates_global_frame_contract_before_mask_precedence() -> TestResult {
        let mut source = image(vec![1.0; 4])?;
        for flags in source.mask_mut().as_mut_slice() {
            *flags = PixelFlags::REJECTED;
        }
        let mut accumulator = DrizzleTileAccumulator::new(DrizzleTileBounds::new(0, 0, 2, 2, 3)?)?;
        let unsupported_pattern = accumulate_cfa_frame(
            &mut accumulator,
            &source,
            ProjectiveTransform::IDENTITY,
            DrizzleParameters::new(1, 1.0)?,
            &BayerPattern::Other("unknown".to_owned()),
            1.0,
            DrizzleOutputBounds::new(2, 2, 4)?,
        );
        assert!(matches!(
            unsupported_pattern,
            Err(DrizzleFrameError::Geometry(
                DrizzleError::UnsupportedCfaPattern
            ))
        ));
        assert!(matches!(
            accumulate_cfa_frame(
                &mut accumulator,
                &source,
                ProjectiveTransform::IDENTITY,
                DrizzleParameters::new(1, 1.0)?,
                &BayerPattern::Rggb,
                0.0,
                DrizzleOutputBounds::new(2, 2, 4)?,
            ),
            Err(DrizzleFrameError::InvalidFrameWeight)
        ));

        let mut outside = DrizzleTileAccumulator::new(DrizzleTileBounds::new(1, 1, 2, 2, 3)?)?;
        assert!(matches!(
            accumulate_cfa_frame(
                &mut outside,
                &source,
                ProjectiveTransform::IDENTITY,
                DrizzleParameters::new(1, 1.0)?,
                &BayerPattern::Rggb,
                1.0,
                DrizzleOutputBounds::new(2, 2, 4)?,
            ),
            Err(DrizzleFrameError::InvalidTileBounds)
        ));
        Ok(())
    }
}
