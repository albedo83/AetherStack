use aether_core::ScientificImage;
use aether_integration::{IntegrationError, MeanIntegration, PixelSupport, integrate_mean};

/// Scientific role assigned to a constructed calibration master.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CalibrationMasterKind {
    /// Electronic bias or offset master.
    Bias,
    /// Dark-current master, including a short-exposure dark master.
    Dark,
    /// Pedestal-corrected flat master before normalization.
    Flat,
}

/// Versioned master-integration algorithm.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MasterIntegrationAlgorithm {
    /// Deterministic unweighted arithmetic mean with compensated accumulation.
    StrictMeanV1,
}

impl MasterIntegrationAlgorithm {
    /// Stable identifier stored in future execution provenance and cache keys.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::StrictMeanV1 => "strict-mean-v1",
        }
    }
}

/// Role-tagged strict mean master with exact per-pixel support accounting.
#[derive(Clone, Debug, PartialEq)]
pub struct StrictMeanMaster {
    kind: CalibrationMasterKind,
    integration: MeanIntegration,
}

impl StrictMeanMaster {
    /// Scientific role selected by the versioned master plan.
    #[must_use]
    pub const fn kind(&self) -> CalibrationMasterKind {
        self.kind
    }

    /// Versioned numerical algorithm used to construct this master.
    #[must_use]
    pub const fn algorithm(&self) -> MasterIntegrationAlgorithm {
        MasterIntegrationAlgorithm::StrictMeanV1
    }

    /// Integrated master pixels and mask.
    #[must_use]
    pub const fn image(&self) -> &ScientificImage {
        self.integration.image()
    }

    /// Exact accepted, masked, and non-finite counts for every output pixel.
    #[must_use]
    pub fn support(&self) -> &[PixelSupport] {
        self.integration.support()
    }

    /// Consumes the master into its role, image, and support map.
    #[must_use]
    pub fn into_parts(self) -> (CalibrationMasterKind, ScientificImage, Vec<PixelSupport>) {
        let (image, support) = self.integration.into_parts();
        (self.kind, image, support)
    }
}

/// Constructs a role-tagged calibration master with the strict mean oracle.
///
/// Input order is scientifically significant and must follow canonical manifest
/// order. Bias and dark inputs are integrated directly. Flat inputs must already
/// have had the plan-selected bias or short-dark pedestal subtracted from each
/// frame; the resulting flat master is normalized separately with
/// [`crate::normalize_flat`]. No rejection or weighting is implied by this
/// versioned algorithm.
///
/// # Errors
///
/// Returns the strict integration error for empty input, excessive frame count,
/// dimension mismatch, invariant failure, or allocation failure.
pub fn construct_strict_mean_master(
    kind: CalibrationMasterKind,
    inputs: &[&ScientificImage],
) -> Result<StrictMeanMaster, IntegrationError> {
    Ok(StrictMeanMaster {
        kind,
        integration: integrate_mean(inputs)?,
    })
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use aether_core::{Dimensions, PixelFlags};

    use super::*;
    use crate::{FlatNormalizationParameters, normalize_flat, subtract_pedestal};

    type TestResult<T = ()> = Result<T, Box<dyn StdError>>;

    fn image(values: Vec<f64>) -> TestResult<ScientificImage> {
        let dimensions = Dimensions::new(values.len(), 1, 1)?;
        Ok(ScientificImage::from_pixels(dimensions, values)?)
    }

    #[test]
    fn retains_role_algorithm_and_exact_pixel_support() -> TestResult {
        let first = image(vec![1.0, 10.0])?;
        let mut second = image(vec![3.0, 20.0])?;
        second.mask_mut().as_mut_slice()[1] = PixelFlags::HOT;

        let master = construct_strict_mean_master(CalibrationMasterKind::Dark, &[&first, &second])?;

        assert_eq!(master.kind(), CalibrationMasterKind::Dark);
        assert_eq!(master.algorithm(), MasterIntegrationAlgorithm::StrictMeanV1);
        assert_eq!(master.algorithm().id(), "strict-mean-v1");
        assert_eq!(master.image().pixels(), &[2.0, 10.0]);
        assert_eq!(master.support()[0].accepted(), 2);
        assert_eq!(master.support()[1].accepted(), 1);
        assert_eq!(master.support()[1].masked(), 1);
        Ok(())
    }

    #[test]
    fn propagates_strict_integration_failures() -> TestResult {
        assert!(matches!(
            construct_strict_mean_master(CalibrationMasterKind::Bias, &[]),
            Err(IntegrationError::NoInputImages)
        ));

        let first = image(vec![1.0])?;
        let second = image(vec![1.0, 2.0])?;
        assert!(matches!(
            construct_strict_mean_master(CalibrationMasterKind::Bias, &[&first, &second]),
            Err(IntegrationError::DimensionMismatch { input_index: 1, .. })
        ));
        Ok(())
    }

    #[test]
    fn flat_equation_recovers_shape_across_subtract_integrate_normalize() -> TestResult {
        let pedestal = image(vec![10.0, 10.0, 10.0])?;
        let raw_first = image(vec![12.0, 14.0, 16.0])?;
        let raw_second = image(vec![14.0, 18.0, 22.0])?;
        let corrected_first = subtract_pedestal(&raw_first, &pedestal)?;
        let corrected_second = subtract_pedestal(&raw_second, &pedestal)?;

        let master = construct_strict_mean_master(
            CalibrationMasterKind::Flat,
            &[&corrected_first, &corrected_second],
        )?;
        let normalized = normalize_flat(
            master.image(),
            FlatNormalizationParameters::new(3, 1.0e-12)?,
        )?;

        assert_eq!(master.image().pixels(), &[3.0, 6.0, 9.0]);
        assert_eq!(normalized.normalization().to_bits(), 6.0_f64.to_bits());
        assert_eq!(normalized.image().pixels(), &[0.5, 1.0, 1.5]);
        assert_eq!(normalized.support().accepted(), 3);
        Ok(())
    }
}
