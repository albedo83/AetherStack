use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_core::{PixelFlags, ScientificImage};
use serde::Serialize;

const NORMAL_MAD_SCALE: f64 = 1.482_602_218_505_602;

/// Validated controls for iterative median/MAD background estimation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BackgroundParameters {
    clipping_sigma: f64,
    maximum_iterations: u32,
    minimum_samples: usize,
}

impl BackgroundParameters {
    /// Creates explicit robust-background controls.
    ///
    /// # Errors
    ///
    /// Returns a typed error unless the clipping threshold is finite and
    /// positive, and both iteration and support limits are non-zero.
    pub fn new(
        clipping_sigma: f64,
        maximum_iterations: u32,
        minimum_samples: usize,
    ) -> Result<Self, BackgroundError> {
        if !clipping_sigma.is_finite() || clipping_sigma <= 0.0 {
            return Err(BackgroundError::InvalidClippingSigma { clipping_sigma });
        }
        if maximum_iterations == 0 {
            return Err(BackgroundError::ZeroMaximumIterations);
        }
        if minimum_samples == 0 {
            return Err(BackgroundError::ZeroMinimumSamples);
        }
        Ok(Self {
            clipping_sigma,
            maximum_iterations,
            minimum_samples,
        })
    }

    /// Symmetric clipping threshold in robust standard deviations.
    #[must_use]
    pub const fn clipping_sigma(self) -> f64 {
        self.clipping_sigma
    }

    /// Maximum number of median/MAD estimation and clipping rounds.
    #[must_use]
    pub const fn maximum_iterations(self) -> u32 {
        self.maximum_iterations
    }

    /// Minimum number of clear finite samples required after every round.
    #[must_use]
    pub const fn minimum_samples(self) -> usize {
        self.minimum_samples
    }
}

/// Robust global background and noise estimate for one scientific image.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct BackgroundEstimate {
    location: f64,
    median_absolute_deviation: f64,
    noise_sigma: f64,
    total_samples: usize,
    initial_usable_samples: usize,
    retained_samples: usize,
    masked_samples: usize,
    non_finite_samples: usize,
    clipped_samples: usize,
    iterations: u32,
    converged: bool,
}

impl BackgroundEstimate {
    /// Median of the final retained background population.
    #[must_use]
    pub const fn location(self) -> f64 {
        self.location
    }

    /// Median absolute deviation of the final retained population.
    #[must_use]
    pub const fn median_absolute_deviation(self) -> f64 {
        self.median_absolute_deviation
    }

    /// Gaussian-consistent noise estimate, `MAD * 1.482602218505602`.
    #[must_use]
    pub const fn noise_sigma(self) -> f64 {
        self.noise_sigma
    }

    /// Total pixels inspected, including invalid pixels.
    #[must_use]
    pub const fn total_samples(self) -> usize {
        self.total_samples
    }

    /// Clear finite pixels available before clipping.
    #[must_use]
    pub const fn initial_usable_samples(self) -> usize {
        self.initial_usable_samples
    }

    /// Pixels retained in the final robust background population.
    #[must_use]
    pub const fn retained_samples(self) -> usize {
        self.retained_samples
    }

    /// Pixels excluded because their quality mask was non-clear.
    #[must_use]
    pub const fn masked_samples(self) -> usize {
        self.masked_samples
    }

    /// Clear pixels excluded because their value was NaN or infinite.
    #[must_use]
    pub const fn non_finite_samples(self) -> usize {
        self.non_finite_samples
    }

    /// Initially usable pixels rejected by robust clipping.
    #[must_use]
    pub const fn clipped_samples(self) -> usize {
        self.clipped_samples
    }

    /// Number of estimation rounds completed.
    #[must_use]
    pub const fn iterations(self) -> u32 {
        self.iterations
    }

    /// Whether a round retained every input from the preceding round.
    #[must_use]
    pub const fn converged(self) -> bool {
        self.converged
    }
}

/// Failure to calculate a robust background estimate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BackgroundError {
    /// The clipping threshold must be finite and positive.
    InvalidClippingSigma {
        /// Rejected threshold.
        clipping_sigma: f64,
    },
    /// At least one estimation round is required.
    ZeroMaximumIterations,
    /// At least one retained sample must be required.
    ZeroMinimumSamples,
    /// Requested plane is outside the image.
    PlaneOutOfBounds {
        /// Requested plane.
        plane: usize,
        /// Available planes.
        planes: usize,
    },
    /// The image does not contain enough clear finite pixels.
    InsufficientSamples {
        /// Clear finite samples available at the failure point.
        available: usize,
        /// Required support.
        required: usize,
        /// Samples excluded by a quality flag before clipping.
        masked: usize,
        /// Clear samples excluded because they were non-finite before clipping.
        non_finite: usize,
    },
    /// Scratch storage for exact order statistics could not be reserved.
    AllocationFailed {
        /// Number of `f64` elements requested.
        elements: usize,
    },
    /// A derived robust scale or clipping boundary was non-finite.
    NumericalOverflow,
}

impl Display for BackgroundError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidClippingSigma { clipping_sigma } => write!(
                formatter,
                "background clipping sigma must be finite and positive, received {clipping_sigma}"
            ),
            Self::ZeroMaximumIterations => {
                formatter.write_str("background estimation requires at least one iteration")
            }
            Self::ZeroMinimumSamples => {
                formatter.write_str("background estimation requires at least one sample")
            }
            Self::PlaneOutOfBounds { plane, planes } => write!(
                formatter,
                "background plane {plane} is outside {planes} available planes"
            ),
            Self::InsufficientSamples {
                available,
                required,
                masked,
                non_finite,
            } => write!(
                formatter,
                "background has {available} usable samples but requires {required}; {masked} masked and {non_finite} non-finite samples were excluded"
            ),
            Self::AllocationFailed { elements } => write!(
                formatter,
                "cannot reserve {elements} samples for robust background estimation"
            ),
            Self::NumericalOverflow => {
                formatter.write_str("robust background scale exceeds the finite f64 domain")
            }
        }
    }
}

impl Error for BackgroundError {}

/// Estimates global background and noise by exact iterative median/MAD clipping.
///
/// Only clear finite pixels enter the estimator. Each round calculates the exact
/// median and median absolute deviation, converts MAD to a Gaussian-consistent
/// standard deviation, then removes samples outside the symmetric configured
/// threshold. The retained population is deterministic but requires scratch
/// space proportional to the number of usable pixels.
///
/// A perfectly uniform population is valid and produces zero MAD and zero noise.
/// Star detection must reject that degenerate scale explicitly rather than
/// manufacturing a detection threshold.
///
/// # Errors
///
/// Returns a typed support, allocation, parameter, or finite-domain failure.
pub fn estimate_background(
    image: &ScientificImage,
    parameters: BackgroundParameters,
) -> Result<BackgroundEstimate, BackgroundError> {
    estimate_samples(image.pixels(), image.mask().as_slice(), parameters)
}

/// Estimates robust background and noise for one planar image channel.
///
/// # Errors
///
/// Returns [`BackgroundError::PlaneOutOfBounds`] for an unavailable plane, or
/// the same support, allocation, parameter, and numerical failures as
/// [`estimate_background`].
pub fn estimate_plane_background(
    image: &ScientificImage,
    plane: usize,
    parameters: BackgroundParameters,
) -> Result<BackgroundEstimate, BackgroundError> {
    let dimensions = image.dimensions();
    if plane >= dimensions.planes() {
        return Err(BackgroundError::PlaneOutOfBounds {
            plane,
            planes: dimensions.planes(),
        });
    }
    let plane_area = dimensions
        .width()
        .checked_mul(dimensions.height())
        .ok_or(BackgroundError::NumericalOverflow)?;
    let start = plane
        .checked_mul(plane_area)
        .ok_or(BackgroundError::NumericalOverflow)?;
    let end = start
        .checked_add(plane_area)
        .ok_or(BackgroundError::NumericalOverflow)?;
    let pixels = image
        .pixels()
        .get(start..end)
        .ok_or(BackgroundError::NumericalOverflow)?;
    let flags = image
        .mask()
        .as_slice()
        .get(start..end)
        .ok_or(BackgroundError::NumericalOverflow)?;
    estimate_samples(pixels, flags, parameters)
}

fn estimate_samples(
    pixels: &[f64],
    flags: &[PixelFlags],
    parameters: BackgroundParameters,
) -> Result<BackgroundEstimate, BackgroundError> {
    // Revalidate because future deserialization may construct this type through
    // a checked wire representation and this function is a public trust boundary.
    let parameters = BackgroundParameters::new(
        parameters.clipping_sigma,
        parameters.maximum_iterations,
        parameters.minimum_samples,
    )?;
    let total_samples = pixels.len();
    let mut masked_samples = 0_usize;
    let mut non_finite_samples = 0_usize;
    let usable_count = pixels
        .iter()
        .zip(flags)
        .filter(|(value, flags)| {
            if !flags.is_clear() {
                masked_samples += 1;
                false
            } else if !value.is_finite() {
                non_finite_samples += 1;
                false
            } else {
                true
            }
        })
        .count();
    if usable_count < parameters.minimum_samples {
        return Err(BackgroundError::InsufficientSamples {
            available: usable_count,
            required: parameters.minimum_samples,
            masked: masked_samples,
            non_finite: non_finite_samples,
        });
    }

    let mut retained = try_vec(usable_count)?;
    retained.extend(
        pixels
            .iter()
            .zip(flags)
            .filter_map(|(value, flags)| (flags.is_clear() && value.is_finite()).then_some(*value)),
    );
    let initial_usable_samples = retained.len();
    let mut deviations = try_vec(initial_usable_samples)?;
    let mut final_location = 0.0;
    let mut final_mad = 0.0;
    let mut final_noise = 0.0;
    let mut iterations = 0_u32;
    let mut converged = false;

    for _ in 0..parameters.maximum_iterations {
        iterations += 1;
        final_location = exact_median(&mut retained);
        deviations.clear();
        deviations.extend(retained.iter().map(|value| (*value - final_location).abs()));
        final_mad = exact_median(&mut deviations);
        final_noise = final_mad * NORMAL_MAD_SCALE;
        if !final_location.is_finite() || !final_mad.is_finite() || !final_noise.is_finite() {
            return Err(BackgroundError::NumericalOverflow);
        }

        if final_noise == 0.0 {
            converged = true;
            break;
        }
        let clipping_radius = parameters.clipping_sigma * final_noise;
        if !clipping_radius.is_finite() {
            return Err(BackgroundError::NumericalOverflow);
        }
        let before = retained.len();
        retained.retain(|value| (*value - final_location).abs() <= clipping_radius);
        if retained.len() < parameters.minimum_samples {
            return Err(BackgroundError::InsufficientSamples {
                available: retained.len(),
                required: parameters.minimum_samples,
                masked: masked_samples,
                non_finite: non_finite_samples,
            });
        }
        if retained.len() == before {
            converged = true;
            break;
        }
    }

    // When the final iteration clipped samples, its published estimate must
    // describe the retained population rather than the preceding population.
    if !converged {
        final_location = exact_median(&mut retained);
        deviations.clear();
        deviations.extend(retained.iter().map(|value| (*value - final_location).abs()));
        final_mad = exact_median(&mut deviations);
        final_noise = final_mad * NORMAL_MAD_SCALE;
        if !final_location.is_finite() || !final_mad.is_finite() || !final_noise.is_finite() {
            return Err(BackgroundError::NumericalOverflow);
        }
    }

    Ok(BackgroundEstimate {
        location: canonical_zero(final_location),
        median_absolute_deviation: canonical_zero(final_mad),
        noise_sigma: canonical_zero(final_noise),
        total_samples,
        initial_usable_samples,
        retained_samples: retained.len(),
        masked_samples,
        non_finite_samples,
        clipped_samples: initial_usable_samples - retained.len(),
        iterations,
        converged,
    })
}

fn try_vec(elements: usize) -> Result<Vec<f64>, BackgroundError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(elements)
        .map_err(|_| BackgroundError::AllocationFailed { elements })?;
    Ok(values)
}

pub(crate) fn exact_median(values: &mut [f64]) -> f64 {
    let length = values.len();
    let middle = length / 2;
    let (lower, upper, _) = values.select_nth_unstable_by(middle, f64::total_cmp);
    if length % 2 == 1 {
        *upper
    } else {
        let lower = lower
            .iter()
            .copied()
            .max_by(f64::total_cmp)
            .unwrap_or(*upper);
        lower.midpoint(*upper)
    }
}

fn canonical_zero(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use aether_core::{Dimensions, PixelFlags};

    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn StdError>>;

    fn image(values: Vec<f64>) -> TestResult<ScientificImage> {
        let dimensions = Dimensions::new(values.len(), 1, 1)?;
        Ok(ScientificImage::from_pixels(dimensions, values)?)
    }

    fn parameters() -> TestResult<BackgroundParameters> {
        Ok(BackgroundParameters::new(3.0, 8, 4)?)
    }

    #[test]
    fn rejects_invalid_parameters() {
        for invalid in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(matches!(
                BackgroundParameters::new(invalid, 1, 1),
                Err(BackgroundError::InvalidClippingSigma { .. })
            ));
        }
        assert_eq!(
            BackgroundParameters::new(3.0, 0, 1),
            Err(BackgroundError::ZeroMaximumIterations)
        );
        assert_eq!(
            BackgroundParameters::new(3.0, 1, 0),
            Err(BackgroundError::ZeroMinimumSamples)
        );
    }

    #[test]
    fn exact_clipping_rejects_bright_sources_without_biasing_background() -> TestResult {
        let mut values = Vec::new();
        for index in 0..1_000 {
            let noise = (index % 5) as f64 - 2.0;
            values.push(1_000.0 + noise);
        }
        values.extend([2_000.0, 5_000.0, 10_000.0]);
        let estimate = estimate_background(&image(values)?, parameters()?)?;

        assert_eq!(estimate.location().to_bits(), 1_000.0_f64.to_bits());
        assert_eq!(
            estimate.median_absolute_deviation().to_bits(),
            1.0_f64.to_bits()
        );
        assert!((estimate.noise_sigma() - NORMAL_MAD_SCALE).abs() <= f64::EPSILON);
        assert_eq!(estimate.initial_usable_samples(), 1_003);
        assert_eq!(estimate.retained_samples(), 1_000);
        assert_eq!(estimate.clipped_samples(), 3);
        assert!(estimate.converged());
        Ok(())
    }

    #[test]
    fn counts_masks_and_non_finite_values_before_clipping() -> TestResult {
        let mut input = image(vec![10.0, 10.0, 10.0, 10.0, 10.0, f64::NAN])?;
        input.mask_mut().as_mut_slice()[4] = PixelFlags::REJECTED;
        let estimate = estimate_background(&input, parameters()?)?;

        assert_eq!(estimate.total_samples(), 6);
        assert_eq!(estimate.initial_usable_samples(), 4);
        assert_eq!(estimate.masked_samples(), 1);
        assert_eq!(estimate.non_finite_samples(), 1);
        assert_eq!(estimate.noise_sigma().to_bits(), 0.0_f64.to_bits());
        assert!(estimate.converged());
        Ok(())
    }

    #[test]
    fn even_median_is_finite_across_the_full_binary64_range() -> TestResult {
        let mut values = [-f64::MAX, f64::MAX];
        assert_eq!(exact_median(&mut values).to_bits(), 0.0_f64.to_bits());

        let input = image(vec![-f64::MAX, f64::MAX, -f64::MAX, f64::MAX])?;
        assert!(matches!(
            estimate_background(&input, BackgroundParameters::new(3.0, 1, 4)?),
            Err(BackgroundError::NumericalOverflow)
        ));
        Ok(())
    }

    #[test]
    fn insufficient_support_reports_original_exclusions() -> TestResult {
        let result = estimate_background(
            &image(vec![1.0, 2.0, f64::NAN])?,
            BackgroundParameters::new(3.0, 2, 3)?,
        );
        assert!(matches!(
            result,
            Err(BackgroundError::InsufficientSamples {
                available: 2,
                required: 3,
                masked: 0,
                non_finite: 1,
            })
        ));
        Ok(())
    }

    #[test]
    fn plane_estimation_never_mixes_channel_populations() -> TestResult {
        let input = ScientificImage::from_pixels(
            Dimensions::new(4, 1, 2)?,
            vec![10.0, 10.0, 10.0, 10.0, 100.0, 100.0, 100.0, 100.0],
        )?;
        let first = estimate_plane_background(&input, 0, parameters()?)?;
        let second = estimate_plane_background(&input, 1, parameters()?)?;

        assert_eq!(first.location().to_bits(), 10.0_f64.to_bits());
        assert_eq!(second.location().to_bits(), 100.0_f64.to_bits());
        assert_eq!(first.total_samples(), 4);
        assert_eq!(second.total_samples(), 4);
        assert!(matches!(
            estimate_plane_background(&input, 2, parameters()?),
            Err(BackgroundError::PlaneOutOfBounds {
                plane: 2,
                planes: 2,
            })
        ));
        Ok(())
    }
}
