use aether_core::PixelFlags;
use aether_metadata::BayerPattern;
use aether_registration::ProjectiveTransform;

use crate::{CfaDrizzleDeposition, DrizzleError, DrizzleParameters, deposit_cfa_detector_pixel};

/// One original single-plane detector sample and its complete quality state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetectorSample {
    x: u32,
    y: u32,
    value: f64,
    flags: PixelFlags,
}

impl DetectorSample {
    /// Binds a physical value and quality flags to its detector coordinate.
    #[must_use]
    pub const fn new(x: u32, y: u32, value: f64, flags: PixelFlags) -> Self {
        Self { x, y, value, flags }
    }

    /// Horizontal detector coordinate.
    #[must_use]
    pub const fn x(self) -> u32 {
        self.x
    }

    /// Vertical detector coordinate.
    #[must_use]
    pub const fn y(self) -> u32 {
        self.y
    }

    /// Decoded scientific value, retained even when unusable.
    #[must_use]
    pub const fn value(self) -> f64 {
        self.value
    }

    /// Complete known and unknown quality bits.
    #[must_use]
    pub const fn flags(self) -> PixelFlags {
        self.flags
    }
}

/// Bounded output rectangle used by one footprint deposition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DrizzleOutputBounds {
    width: u32,
    height: u32,
    maximum_contributions: usize,
}

impl DrizzleOutputBounds {
    /// Validates nonzero dimensions and work ceiling before source inspection.
    pub fn new(
        width: u32,
        height: u32,
        maximum_contributions: usize,
    ) -> Result<Self, DrizzleError> {
        if width == 0 || height == 0 || maximum_contributions == 0 {
            return Err(DrizzleError::InvalidGeometry);
        }
        Ok(Self {
            width,
            height,
            maximum_contributions,
        })
    }

    /// Output width in pixels.
    #[must_use]
    pub const fn width(self) -> u32 {
        self.width
    }

    /// Output height in pixels.
    #[must_use]
    pub const fn height(self) -> u32 {
        self.height
    }

    /// Maximum output pixels one detector drop may visit.
    #[must_use]
    pub const fn maximum_contributions(self) -> usize {
        self.maximum_contributions
    }
}

/// Stable reason why one detector sample produced no Drizzle contribution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DrizzleSampleExclusion {
    /// At least one source quality flag was already present.
    Masked,
    /// An unflagged decoded value was NaN or infinite.
    NonFinite,
}

/// Auditable result for one attempted CFA detector sample.
#[derive(Clone, Debug, PartialEq)]
pub enum DrizzleSampleOutcome {
    /// A clear finite sample produced geometric contributions.
    Deposited(CfaDrizzleDeposition),
    /// No flux or weight was deposited; all quality bits are retained.
    Excluded {
        /// Stable exclusion category.
        reason: DrizzleSampleExclusion,
        /// Original flags plus `INVALID` for an unflagged non-finite value.
        flags: PixelFlags,
    },
}

/// Routes one CFA sample only when it is clear and finite.
///
/// Masked and non-finite samples return evidence without evaluating geometry or
/// allocating contribution storage. This ordering prevents defective pixels
/// from consuming work or leaking weight into contribution maps.
pub fn deposit_cfa_sample(
    sample: DetectorSample,
    transform: ProjectiveTransform,
    parameters: DrizzleParameters,
    pattern: &BayerPattern,
    frame_weight: f64,
    output: DrizzleOutputBounds,
) -> Result<DrizzleSampleOutcome, DrizzleError> {
    if !sample.flags.is_clear() {
        return Ok(DrizzleSampleOutcome::Excluded {
            reason: DrizzleSampleExclusion::Masked,
            flags: sample.flags,
        });
    }
    if !sample.value.is_finite() {
        return Ok(DrizzleSampleOutcome::Excluded {
            reason: DrizzleSampleExclusion::NonFinite,
            flags: sample.flags | PixelFlags::INVALID,
        });
    }
    let deposition = deposit_cfa_detector_pixel(
        sample.x,
        sample.y,
        transform,
        parameters,
        pattern,
        sample.value,
        frame_weight,
        output.width,
        output.height,
        output.maximum_contributions,
    )?;
    Ok(DrizzleSampleOutcome::Deposited(deposition))
}

#[cfg(test)]
mod tests {
    use aether_core::PixelFlags;
    use aether_metadata::BayerPattern;
    use aether_registration::ProjectiveTransform;

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn masked_samples_retain_all_bits_and_skip_invalid_geometry() -> TestResult {
        let flags = PixelFlags::HOT | PixelFlags::from_bits_retain(0b1000_0000);
        let outcome = deposit_cfa_sample(
            DetectorSample::new(0, 0, 120.0, flags),
            ProjectiveTransform::IDENTITY,
            DrizzleParameters::new(2, 1.0)?,
            &BayerPattern::Other("unsupported".to_owned()),
            f64::NAN,
            DrizzleOutputBounds::new(8, 8, 16)?,
        )?;
        assert_eq!(
            outcome,
            DrizzleSampleOutcome::Excluded {
                reason: DrizzleSampleExclusion::Masked,
                flags,
            }
        );
        Ok(())
    }

    #[test]
    fn unflagged_nonfinite_samples_gain_invalid_without_geometry() -> TestResult {
        let outcome = deposit_cfa_sample(
            DetectorSample::new(0, 0, f64::INFINITY, PixelFlags::CLEAR),
            ProjectiveTransform::IDENTITY,
            DrizzleParameters::new(2, 1.0)?,
            &BayerPattern::Other("unsupported".to_owned()),
            1.0,
            DrizzleOutputBounds::new(8, 8, 16)?,
        )?;
        assert_eq!(
            outcome,
            DrizzleSampleOutcome::Excluded {
                reason: DrizzleSampleExclusion::NonFinite,
                flags: PixelFlags::INVALID,
            }
        );
        Ok(())
    }

    #[test]
    fn clear_finite_sample_reaches_exactly_one_cfa_plane() -> TestResult {
        let outcome = deposit_cfa_sample(
            DetectorSample::new(0, 0, 120.0, PixelFlags::CLEAR),
            ProjectiveTransform::IDENTITY,
            DrizzleParameters::new(2, 1.0)?,
            &BayerPattern::Rggb,
            1.0,
            DrizzleOutputBounds::new(8, 8, 16)?,
        )?;
        let DrizzleSampleOutcome::Deposited(deposition) = outcome else {
            return Err("clear finite sample was not deposited".into());
        };
        assert_eq!(deposition.channel().plane(), 0);
        assert_eq!(deposition.deposition().contributions().len(), 4);
        Ok(())
    }
}
