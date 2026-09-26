//! Robust and explainable astronomical frame-quality measurements.
//!
//! This crate contains the strict CPU reference algorithms used by frame
//! selection and the interactive Blink reviewer. Display transforms never feed
//! these measurements; callers provide immutable linear scientific pixels.

mod background;
mod cfa;
mod rgb;
mod stars;

pub use background::{
    BackgroundError, BackgroundEstimate, BackgroundParameters, estimate_background,
    estimate_plane_background,
};
pub use cfa::{CFA_CELL_MEAN_ALGORITHM_ID, CfaDetectionError, prepare_cfa_cell_mean};
pub use rgb::{
    RGB_LUMINANCE_ALGORITHM_ID, RgbDetectionError, RgbLuminanceBuilder, RgbLuminanceChannel,
};
pub use stars::{
    FrameQuality, FrameQualityError, StarMeasurement, StarMeasurementParameters,
    measure_frame_quality,
};

/// Versioned identifier for the initial global robust-background estimator.
pub const GLOBAL_BACKGROUND_ALGORITHM_ID: &str = "global-mad-clip-v1";
/// Versioned identifier for the initial local-maximum stellar shape estimator.
pub const STAR_MEASUREMENT_ALGORITHM_ID: &str = "local-max-moments-v1";
