//! Bounded deterministic FITS preview generation and display mapping.
//!
//! Preview pixels are derived artifacts: they never feed scientific processing.
//! Reduction reads bounded FITS regions and accumulates source samples in
//! canonical row-major order, making results independent of I/O chunk size.
//! Display mapping consumes one explicit transform shared by Blink frames.

use std::error::Error;
use std::fmt::{Display, Formatter};
use std::io::{Read, Seek};

use aether_core::{CompensatedSum, PixelFlags};
use aether_fits::{ImageReadError, ImageRegion, PrimaryImageReader, SampleStatus};
use aether_review::{DisplayTransform, TransferFunction};

/// Maximum reduced pixels in one scalar preview (4 MiPixels).
pub const MAX_PREVIEW_PIXELS: usize = 4 * 1_024 * 1_024;
/// Maximum decoded source samples in one I/O chunk (1 MiSample).
pub const MAX_PREVIEW_IO_CHUNK_SAMPLES: usize = 1_024 * 1_024;
/// Highest power-of-two reduction level accepted by this implementation.
///
/// Level 15 has at most `2^30` source samples in an interior square block, so
/// the per-pixel `u32` support counters cannot overflow for a valid block.
pub const MAX_REDUCTION_LEVEL: u8 = 15;
/// Stable identifier for the first robust display-only stretch estimator.
pub const AUTO_STRETCH_ALGORITHM_ID: &str = "aether-preview-auto-stretch-v1";

const AUTO_STRETCH_SHADOW_SIGMA: f64 = 2.8;
const AUTO_STRETCH_TARGET_BACKGROUND: f64 = 0.25;
const AUTO_STRETCH_HIGH_QUANTILE: f64 = 0.9995;
const NORMALIZED_BACKGROUND_EPSILON: f64 = 1.0e-6;
const LUMINANCE_RED: f64 = 0.2126;
const LUMINANCE_GREEN: f64 = 0.7152;
const LUMINANCE_BLUE: f64 = 0.0722;

/// Inspectable evidence behind one automatic display transform.
///
/// Automatic stretching changes only the presentation. The median, scaled
/// median absolute deviation, high quantile, and exact supporting-pixel count
/// are retained so a caller can explain why a preview looks the way it does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutomaticDisplayTransform {
    transform: DisplayTransform,
    finite_samples: usize,
    median: f64,
    scaled_mad: f64,
    high_quantile: f64,
}

impl AutomaticDisplayTransform {
    /// Explicit transform suitable for [`render_grayscale_rgba8`].
    #[must_use]
    pub const fn transform(self) -> DisplayTransform {
        self.transform
    }

    /// Number of reduced pixels supporting the estimator.
    #[must_use]
    pub const fn finite_samples(self) -> usize {
        self.finite_samples
    }

    /// Median of the finite reduced pixels.
    #[must_use]
    pub const fn median(self) -> f64 {
        self.median
    }

    /// Gaussian-consistent median absolute deviation (`MAD * 1.4826`).
    #[must_use]
    pub const fn scaled_mad(self) -> f64 {
        self.scaled_mad
    }

    /// Upper order statistic used as the display white point candidate.
    #[must_use]
    pub const fn high_quantile(self) -> f64 {
        self.high_quantile
    }
}

/// Validated output constraints used to select a preview-pyramid level.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreviewLimits {
    maximum_width: usize,
    maximum_height: usize,
    maximum_pixels: usize,
}

impl PreviewLimits {
    /// Creates positive bounded output constraints.
    ///
    /// # Errors
    ///
    /// Returns a typed error for a zero dimension or pixel limit, or a pixel
    /// limit above [`MAX_PREVIEW_PIXELS`].
    pub fn new(
        maximum_width: usize,
        maximum_height: usize,
        maximum_pixels: usize,
    ) -> Result<Self, PreviewError> {
        if maximum_width == 0 || maximum_height == 0 {
            return Err(PreviewError::InvalidOutputDimensions);
        }
        if maximum_pixels == 0 || maximum_pixels > MAX_PREVIEW_PIXELS {
            return Err(PreviewError::InvalidOutputPixelLimit {
                maximum: MAX_PREVIEW_PIXELS,
            });
        }
        Ok(Self {
            maximum_width,
            maximum_height,
            maximum_pixels,
        })
    }

    /// Maximum reduced width.
    #[must_use]
    pub const fn maximum_width(self) -> usize {
        self.maximum_width
    }

    /// Maximum reduced height.
    #[must_use]
    pub const fn maximum_height(self) -> usize {
        self.maximum_height
    }

    /// Maximum reduced pixel count.
    #[must_use]
    pub const fn maximum_pixels(self) -> usize {
        self.maximum_pixels
    }
}

/// Validated controls for one scalar FITS preview.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FitsPreviewParameters {
    plane: u64,
    reduction_level: u8,
    maximum_output_pixels: usize,
    io_chunk_samples: usize,
}

impl FitsPreviewParameters {
    /// Creates explicit bounded preview controls.
    ///
    /// Reduction level `n` averages non-overlapping `2^n` square source blocks.
    /// Edge blocks retain their actual support rather than being padded.
    ///
    /// # Errors
    ///
    /// Returns a typed error for an unsupported level, zero or excessive output
    /// bound, or zero or excessive I/O chunk size.
    pub fn new(
        plane: u64,
        reduction_level: u8,
        maximum_output_pixels: usize,
        io_chunk_samples: usize,
    ) -> Result<Self, PreviewError> {
        if reduction_level > MAX_REDUCTION_LEVEL {
            return Err(PreviewError::InvalidReductionLevel {
                maximum: MAX_REDUCTION_LEVEL,
            });
        }
        if maximum_output_pixels == 0 || maximum_output_pixels > MAX_PREVIEW_PIXELS {
            return Err(PreviewError::InvalidOutputPixelLimit {
                maximum: MAX_PREVIEW_PIXELS,
            });
        }
        if io_chunk_samples == 0 || io_chunk_samples > MAX_PREVIEW_IO_CHUNK_SAMPLES {
            return Err(PreviewError::InvalidIoChunkSize {
                maximum: MAX_PREVIEW_IO_CHUNK_SAMPLES,
            });
        }
        Ok(Self {
            plane,
            reduction_level,
            maximum_output_pixels,
            io_chunk_samples,
        })
    }

    /// Zero-based source plane.
    #[must_use]
    pub const fn plane(self) -> u64 {
        self.plane
    }

    /// Power-of-two reduction level.
    #[must_use]
    pub const fn reduction_level(self) -> u8 {
        self.reduction_level
    }

    /// Hard bound on reduced output pixels.
    #[must_use]
    pub const fn maximum_output_pixels(self) -> usize {
        self.maximum_output_pixels
    }

    /// Hard bound on decoded source samples held for one read.
    #[must_use]
    pub const fn io_chunk_samples(self) -> usize {
        self.io_chunk_samples
    }
}

/// One immutable reduced scalar preview.
#[derive(Clone, Debug, PartialEq)]
pub struct ScalarPreview {
    source_width: u64,
    source_height: u64,
    plane: u64,
    reduction_level: u8,
    reduction_factor: u64,
    width: usize,
    height: usize,
    values: Vec<f64>,
    valid_support: Vec<u32>,
    excluded_support: Vec<u32>,
    excluded_flags: Vec<PixelFlags>,
}

impl ScalarPreview {
    /// Original source width.
    #[must_use]
    pub const fn source_width(&self) -> u64 {
        self.source_width
    }

    /// Original source height.
    #[must_use]
    pub const fn source_height(&self) -> u64 {
        self.source_height
    }

    /// Selected zero-based source plane.
    #[must_use]
    pub const fn plane(&self) -> u64 {
        self.plane
    }

    /// Power-of-two reduction level.
    #[must_use]
    pub const fn reduction_level(&self) -> u8 {
        self.reduction_level
    }

    /// Source-pixel width and height represented by an interior output pixel.
    #[must_use]
    pub const fn reduction_factor(&self) -> u64 {
        self.reduction_factor
    }

    /// Reduced preview width.
    #[must_use]
    pub const fn width(&self) -> usize {
        self.width
    }

    /// Reduced preview height.
    #[must_use]
    pub const fn height(&self) -> usize {
        self.height
    }

    /// Reduced values in row-major order.
    ///
    /// Pixels with zero valid support contain NaN and are identified explicitly
    /// by [`Self::valid_support`].
    #[must_use]
    pub fn values(&self) -> &[f64] {
        &self.values
    }

    /// Count of finite unmasked source samples contributing to every value.
    #[must_use]
    pub fn valid_support(&self) -> &[u32] {
        &self.valid_support
    }

    /// Count of undefined or non-finite source samples in every reduction block.
    #[must_use]
    pub fn excluded_support(&self) -> &[u32] {
        &self.excluded_support
    }

    /// Union of reasons excluded from every reduction block.
    #[must_use]
    pub fn excluded_flags(&self) -> &[PixelFlags] {
        &self.excluded_flags
    }

    /// Reads one reduced sample and its complete support accounting.
    ///
    /// # Errors
    ///
    /// Returns [`PreviewError::CoordinateOutOfBounds`] outside the preview.
    pub fn sample(&self, x: usize, y: usize) -> Result<PreviewSample, PreviewError> {
        let index = y
            .checked_mul(self.width)
            .and_then(|row| row.checked_add(x))
            .filter(|_| x < self.width && y < self.height)
            .ok_or(PreviewError::CoordinateOutOfBounds {
                x,
                y,
                width: self.width,
                height: self.height,
            })?;
        Ok(PreviewSample {
            value: self.values[index],
            valid_support: self.valid_support[index],
            excluded_support: self.excluded_support[index],
            excluded_flags: self.excluded_flags[index],
        })
    }
}

/// One reduced sample with inspectable validity evidence.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreviewSample {
    value: f64,
    valid_support: u32,
    excluded_support: u32,
    excluded_flags: PixelFlags,
}

impl PreviewSample {
    /// Deterministic mean of valid source samples, or NaN when none were valid.
    #[must_use]
    pub const fn value(self) -> f64 {
        self.value
    }

    /// Finite usable source-sample count.
    #[must_use]
    pub const fn valid_support(self) -> u32 {
        self.valid_support
    }

    /// Excluded source-sample count.
    #[must_use]
    pub const fn excluded_support(self) -> u32 {
        self.excluded_support
    }

    /// Union of excluded source-sample reasons.
    #[must_use]
    pub const fn excluded_flags(self) -> PixelFlags {
        self.excluded_flags
    }
}

/// Presentation of reduced pixels lacking any valid source support.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MissingPixelStyle {
    /// Fully transparent black, suitable when the UI draws a separate overlay.
    Transparent,
    /// Opaque alternating light/dark magenta cells, preserving a non-solid cue.
    Checkerboard,
}

/// Packed display-only grayscale preview.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RgbaPreview {
    width: usize,
    height: usize,
    display_transform_version: u32,
    pixels: Vec<u8>,
}

/// Selects the finest power-of-two level satisfying all output constraints.
///
/// # Errors
///
/// Returns a typed error for zero source dimensions, derived-size overflow, or
/// when no implemented reduction level can satisfy the limits.
pub fn choose_reduction_level(
    source_width: u64,
    source_height: u64,
    limits: PreviewLimits,
) -> Result<u8, PreviewError> {
    if source_width == 0 || source_height == 0 {
        return Err(PreviewError::InvalidSourceDimensions);
    }
    // Revalidate the value type at this public trust boundary.
    let limits = PreviewLimits::new(
        limits.maximum_width,
        limits.maximum_height,
        limits.maximum_pixels,
    )?;
    let maximum_width = u64::try_from(limits.maximum_width).unwrap_or(u64::MAX);
    let maximum_height = u64::try_from(limits.maximum_height).unwrap_or(u64::MAX);
    let maximum_pixels =
        u64::try_from(limits.maximum_pixels).map_err(|_| PreviewError::SizeOverflow)?;
    for level in 0..=MAX_REDUCTION_LEVEL {
        let factor = 1_u64
            .checked_shl(u32::from(level))
            .ok_or(PreviewError::SizeOverflow)?;
        let width = ceil_div(source_width, factor)?;
        let height = ceil_div(source_height, factor)?;
        let pixels = width
            .checked_mul(height)
            .ok_or(PreviewError::SizeOverflow)?;
        if width <= maximum_width && height <= maximum_height && pixels <= maximum_pixels {
            return Ok(level);
        }
    }
    Err(PreviewError::NoReductionLevelFits)
}

impl RgbaPreview {
    /// Output width.
    #[must_use]
    pub const fn width(&self) -> usize {
        self.width
    }

    /// Output height.
    #[must_use]
    pub const fn height(&self) -> usize {
        self.height
    }

    /// Version of the transform schema used to map the pixels.
    #[must_use]
    pub const fn display_transform_version(&self) -> u32 {
        self.display_transform_version
    }

    /// Packed row-major RGBA8 bytes.
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }
}

/// Builds one deterministic scalar preview directly from a seekable FITS reader.
///
/// Source samples are visited in canonical plane-row-column order even when the
/// I/O buffer is narrower than a source row. Only fixed I/O storage and arrays
/// proportional to the bounded reduced output are allocated.
///
/// # Errors
///
/// Returns a typed layout, plane, bounds, allocation, support-overflow, or FITS
/// decoding failure. No partial preview is returned.
pub fn build_fits_preview<R: Read + Seek>(
    reader: &mut PrimaryImageReader<R>,
    parameters: FitsPreviewParameters,
) -> Result<ScalarPreview, PreviewError> {
    // Revalidate because this public boundary may later receive a checked wire
    // representation constructed outside this crate.
    let parameters = FitsPreviewParameters::new(
        parameters.plane,
        parameters.reduction_level,
        parameters.maximum_output_pixels,
        parameters.io_chunk_samples,
    )?;
    let (source_width, source_height, plane_count) = match reader.descriptor().axes() {
        [width, height] => (*width, *height, 1_u64),
        [width, height, planes] => (*width, *height, *planes),
        axes => return Err(PreviewError::UnsupportedAxisCount { axes: axes.len() }),
    };
    if parameters.plane >= plane_count {
        return Err(PreviewError::PlaneOutOfBounds {
            plane: parameters.plane,
            planes: plane_count,
        });
    }
    let reduction_factor = 1_u64
        .checked_shl(u32::from(parameters.reduction_level))
        .ok_or(PreviewError::SizeOverflow)?;
    let reduced_width_u64 = ceil_div(source_width, reduction_factor)?;
    let reduced_height_u64 = ceil_div(source_height, reduction_factor)?;
    let width = usize::try_from(reduced_width_u64).map_err(|_| PreviewError::SizeOverflow)?;
    let height = usize::try_from(reduced_height_u64).map_err(|_| PreviewError::SizeOverflow)?;
    let output_pixels = width
        .checked_mul(height)
        .ok_or(PreviewError::SizeOverflow)?;
    if output_pixels > parameters.maximum_output_pixels {
        return Err(PreviewError::OutputPixelLimitExceeded {
            required: output_pixels,
            maximum: parameters.maximum_output_pixels,
        });
    }

    let mut sums = try_filled_vec(output_pixels, CompensatedSum::new())?;
    let mut valid_support = try_filled_vec(output_pixels, 0_u32)?;
    let mut excluded_support = try_filled_vec(output_pixels, 0_u32)?;
    let mut excluded_flags = try_filled_vec(output_pixels, PixelFlags::CLEAR)?;

    let source_width_usize =
        usize::try_from(source_width).map_err(|_| PreviewError::SizeOverflow)?;
    let chunk_width = source_width_usize.min(parameters.io_chunk_samples);
    let chunk_height = if chunk_width == source_width_usize {
        parameters.io_chunk_samples / chunk_width
    } else {
        1
    }
    .max(1);
    let buffer_samples = chunk_width
        .checked_mul(chunk_height)
        .ok_or(PreviewError::SizeOverflow)?;
    let mut values = try_filled_vec(buffer_samples, 0.0_f64)?;
    let mut statuses = try_filled_vec(buffer_samples, SampleStatus::Valid)?;

    let mut source_y = 0_u64;
    while source_y < source_height {
        let height_this_read = if chunk_width == source_width_usize {
            u64::try_from(chunk_height)
                .map_err(|_| PreviewError::SizeOverflow)?
                .min(source_height - source_y)
        } else {
            1
        };
        let mut source_x = 0_u64;
        while source_x < source_width {
            let width_this_read = u64::try_from(chunk_width)
                .map_err(|_| PreviewError::SizeOverflow)?
                .min(source_width - source_x);
            let samples_this_read_u64 = width_this_read
                .checked_mul(height_this_read)
                .ok_or(PreviewError::SizeOverflow)?;
            let samples_this_read =
                usize::try_from(samples_this_read_u64).map_err(|_| PreviewError::SizeOverflow)?;
            reader
                .read_physical_region(
                    ImageRegion::new(
                        parameters.plane,
                        source_x,
                        source_y,
                        width_this_read,
                        height_this_read,
                    ),
                    &mut values[..samples_this_read],
                    &mut statuses[..samples_this_read],
                )
                .map_err(PreviewError::Fits)?;
            accumulate_region(
                &values[..samples_this_read],
                &statuses[..samples_this_read],
                source_x,
                source_y,
                width_this_read,
                reduction_factor,
                width,
                &mut sums,
                &mut valid_support,
                &mut excluded_support,
                &mut excluded_flags,
            )?;
            source_x = source_x
                .checked_add(width_this_read)
                .ok_or(PreviewError::SizeOverflow)?;
        }
        source_y = source_y
            .checked_add(height_this_read)
            .ok_or(PreviewError::SizeOverflow)?;
    }

    let mut reduced_values = try_filled_vec(output_pixels, 0.0_f64)?;
    for index in 0..output_pixels {
        if valid_support[index] == 0 {
            reduced_values[index] = f64::NAN;
            excluded_flags[index] |= PixelFlags::MISSING;
        } else {
            let mean = sums[index].total() / f64::from(valid_support[index]);
            if !mean.is_finite() {
                return Err(PreviewError::NumericalOverflow);
            }
            reduced_values[index] = canonical_zero(mean);
        }
    }

    Ok(ScalarPreview {
        source_width,
        source_height,
        plane: parameters.plane,
        reduction_level: parameters.reduction_level,
        reduction_factor,
        width,
        height,
        values: reduced_values,
        valid_support,
        excluded_support,
        excluded_flags,
    })
}

/// Estimates a robust, deterministic display-only transform from one preview.
///
/// The estimator ignores pixels without valid support, places the black point
/// 2.8 scaled-MAD units below the median, and limits the white point to the
/// 99.95th percentile so a single hot pixel cannot collapse the useful display
/// range. A midtones transfer maps the measured background median to 25% gray.
/// Constant images receive a small finite symmetric range instead of failing or
/// inventing scientific variation.
///
/// The complete valid sample set is sorted with [`f64::total_cmp`], making the
/// result independent of platform, I/O chunking, and thread scheduling.
///
/// # Errors
///
/// Returns a typed error when the preview arrays are inconsistent, no finite
/// supported samples exist, allocation fails, or derived arithmetic leaves the
/// finite numerical domain.
pub fn estimate_display_transform(
    preview: &ScalarPreview,
) -> Result<AutomaticDisplayTransform, PreviewError> {
    let pixel_count = preview
        .width
        .checked_mul(preview.height)
        .ok_or(PreviewError::SizeOverflow)?;
    if preview.values.len() != pixel_count || preview.valid_support.len() != pixel_count {
        return Err(PreviewError::PreviewInvariant);
    }

    let mut samples = Vec::new();
    samples
        .try_reserve_exact(pixel_count)
        .map_err(|_| PreviewError::AllocationFailed {
            elements: pixel_count,
        })?;
    samples.extend(
        preview
            .values
            .iter()
            .zip(&preview.valid_support)
            .filter_map(|(value, support)| (*support > 0 && value.is_finite()).then_some(*value)),
    );
    if samples.is_empty() {
        return Err(PreviewError::NoValidSamples);
    }

    estimate_transform_from_samples(samples)
}

/// Estimates one linked RGB display transform from linear-light luminance.
///
/// Only pixels supported in all three channels contribute. Luminance uses the
/// standard linear Rec. 709 coefficients, while the returned transform is
/// applied unchanged to red, green, and blue. A linked transform preserves
/// relative channel ratios and prevents automatic per-channel neutralization of
/// genuine astronomical color.
///
/// # Errors
///
/// Returns a typed error when the channel previews are not congruent, contain
/// inconsistent arrays, provide no common finite support, cannot allocate the
/// bounded estimator storage, or exceed the finite numerical domain.
pub fn estimate_rgb_display_transform(
    red: &ScalarPreview,
    green: &ScalarPreview,
    blue: &ScalarPreview,
) -> Result<AutomaticDisplayTransform, PreviewError> {
    let pixel_count = validate_rgb_previews(red, green, blue)?;
    let mut samples = Vec::new();
    samples
        .try_reserve_exact(pixel_count)
        .map_err(|_| PreviewError::AllocationFailed {
            elements: pixel_count,
        })?;
    for index in 0..pixel_count {
        if red.valid_support[index] == 0
            || green.valid_support[index] == 0
            || blue.valid_support[index] == 0
        {
            continue;
        }
        let luminance = LUMINANCE_RED * red.values[index]
            + LUMINANCE_GREEN * green.values[index]
            + LUMINANCE_BLUE * blue.values[index];
        if luminance.is_finite() {
            samples.push(luminance);
        }
    }
    if samples.is_empty() {
        return Err(PreviewError::NoValidSamples);
    }
    estimate_transform_from_samples(samples)
}

fn estimate_transform_from_samples(
    mut samples: Vec<f64>,
) -> Result<AutomaticDisplayTransform, PreviewError> {
    let finite_samples = samples.len();

    samples.sort_by(f64::total_cmp);
    let median = median_of_sorted(&samples)?;
    let high_index = ((samples.len() - 1) as f64 * AUTO_STRETCH_HIGH_QUANTILE).floor() as usize;
    let high_quantile = samples[high_index];

    for value in &mut samples {
        *value = (*value - median).abs();
        if !value.is_finite() {
            return Err(PreviewError::NumericalOverflow);
        }
    }
    samples.sort_by(f64::total_cmp);
    let mad = median_of_sorted(&samples)?;
    let scaled_mad = mad * 1.4826;
    if !scaled_mad.is_finite() {
        return Err(PreviewError::NumericalOverflow);
    }

    let fallback_scale = median.abs().max(1.0) * 1.0e-6;
    let black_point = if scaled_mad > 0.0 {
        median - AUTO_STRETCH_SHADOW_SIGMA * scaled_mad
    } else {
        median - fallback_scale
    };
    let mut white_point = high_quantile;
    if white_point <= black_point || white_point <= median {
        white_point = median + fallback_scale;
    }
    if !black_point.is_finite() || !white_point.is_finite() || black_point >= white_point {
        return Err(PreviewError::NumericalOverflow);
    }

    let normalized_background = ((median - black_point) / (white_point - black_point)).clamp(
        NORMALIZED_BACKGROUND_EPSILON,
        1.0 - NORMALIZED_BACKGROUND_EPSILON,
    );
    let midtone = solve_midtone(normalized_background, AUTO_STRETCH_TARGET_BACKGROUND)?;
    let transform = DisplayTransform::new(
        black_point,
        white_point,
        midtone,
        TransferFunction::Midtones,
    )
    .map_err(|_| PreviewError::InvalidDisplayTransform)?;

    Ok(AutomaticDisplayTransform {
        transform,
        finite_samples,
        median,
        scaled_mad,
        high_quantile,
    })
}

/// Maps one scalar preview to packed RGBA8 with an explicit display transform.
///
/// The transform never changes the scalar preview. Missing pixels use the
/// selected non-scientific presentation, while partially supported pixels retain
/// their measured grayscale value and remain inspectable through support arrays.
///
/// # Errors
///
/// Returns a typed overflow, allocation, or non-finite mapping failure.
pub fn render_grayscale_rgba8(
    preview: &ScalarPreview,
    transform: DisplayTransform,
    missing_style: MissingPixelStyle,
) -> Result<RgbaPreview, PreviewError> {
    let pixel_count = preview
        .width
        .checked_mul(preview.height)
        .ok_or(PreviewError::SizeOverflow)?;
    if preview.values.len() != pixel_count
        || preview.valid_support.len() != pixel_count
        || preview.excluded_support.len() != pixel_count
        || preview.excluded_flags.len() != pixel_count
    {
        return Err(PreviewError::PreviewInvariant);
    }
    let byte_count = pixel_count
        .checked_mul(4)
        .ok_or(PreviewError::SizeOverflow)?;
    let mut pixels = try_filled_vec(byte_count, 0_u8)?;
    let scale = transform.white_point() - transform.black_point();
    if !scale.is_finite() || scale <= 0.0 {
        return Err(PreviewError::InvalidDisplayTransform);
    }

    for index in 0..pixel_count {
        let byte_index = index.checked_mul(4).ok_or(PreviewError::SizeOverflow)?;
        let target = pixels
            .get_mut(byte_index..byte_index + 4)
            .ok_or(PreviewError::PreviewInvariant)?;
        if preview.valid_support[index] == 0 {
            target.copy_from_slice(&missing_rgba(index, preview.width, missing_style));
            continue;
        }
        let normalized =
            ((preview.values[index] - transform.black_point()) / scale).clamp(0.0, 1.0);
        let mapped = map_transfer(normalized, transform)?;
        if !mapped.is_finite() {
            return Err(PreviewError::NumericalOverflow);
        }
        let gray = (mapped * 255.0).round().clamp(0.0, 255.0) as u8;
        target.copy_from_slice(&[gray, gray, gray, 255]);
    }

    Ok(RgbaPreview {
        width: preview.width,
        height: preview.height,
        display_transform_version: transform.version(),
        pixels,
    })
}

/// Maps three congruent linear previews to packed RGBA8 with one linked stretch.
///
/// A pixel is displayed only when all three channels have valid support. This
/// conservative rule prevents an invalid channel from appearing as a plausible
/// false color. The scalar previews remain available for exact support and mask
/// inspection; this function creates display-only bytes.
///
/// # Errors
///
/// Returns a typed invariant, overflow, allocation, transform, or non-finite
/// mapping failure.
pub fn render_rgb_rgba8(
    red: &ScalarPreview,
    green: &ScalarPreview,
    blue: &ScalarPreview,
    transform: DisplayTransform,
    missing_style: MissingPixelStyle,
) -> Result<RgbaPreview, PreviewError> {
    let pixel_count = validate_rgb_previews(red, green, blue)?;
    let byte_count = pixel_count
        .checked_mul(4)
        .ok_or(PreviewError::SizeOverflow)?;
    let mut pixels = try_filled_vec(byte_count, 0_u8)?;
    let scale = transform.white_point() - transform.black_point();
    if !scale.is_finite() || scale <= 0.0 {
        return Err(PreviewError::InvalidDisplayTransform);
    }

    for index in 0..pixel_count {
        let byte_index = index.checked_mul(4).ok_or(PreviewError::SizeOverflow)?;
        let target = pixels
            .get_mut(byte_index..byte_index + 4)
            .ok_or(PreviewError::PreviewInvariant)?;
        if red.valid_support[index] == 0
            || green.valid_support[index] == 0
            || blue.valid_support[index] == 0
        {
            target.copy_from_slice(&missing_rgba(index, red.width, missing_style));
            continue;
        }
        let mapped = [red.values[index], green.values[index], blue.values[index]].map(|value| {
            let normalized = ((value - transform.black_point()) / scale).clamp(0.0, 1.0);
            map_transfer(normalized, transform)
        });
        let [red_value, green_value, blue_value] = mapped;
        let rgb = [red_value?, green_value?, blue_value?]
            .map(|value| (value * 255.0).round().clamp(0.0, 255.0) as u8);
        target.copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
    }

    Ok(RgbaPreview {
        width: red.width,
        height: red.height,
        display_transform_version: transform.version(),
        pixels,
    })
}

/// Failure to generate or map a bounded preview.
#[derive(Debug)]
pub enum PreviewError {
    /// Reduction level exceeded the implemented power-of-two range.
    InvalidReductionLevel {
        /// Maximum supported level.
        maximum: u8,
    },
    /// Output-pixel safety limit was zero or above the global bound.
    InvalidOutputPixelLimit {
        /// Global maximum.
        maximum: usize,
    },
    /// Output width or height constraint was zero.
    InvalidOutputDimensions,
    /// Source image width or height was zero.
    InvalidSourceDimensions,
    /// I/O chunk size was zero or above the global bound.
    InvalidIoChunkSize {
        /// Global maximum.
        maximum: usize,
    },
    /// FITS dimensionality was not a supported 2D or 3D image.
    UnsupportedAxisCount {
        /// Axis count encountered.
        axes: usize,
    },
    /// Requested source plane did not exist.
    PlaneOutOfBounds {
        /// Requested plane.
        plane: u64,
        /// Available plane count.
        planes: u64,
    },
    /// Reduced output exceeded the caller's explicit bound.
    OutputPixelLimitExceeded {
        /// Required output pixels.
        required: usize,
        /// Allowed output pixels.
        maximum: usize,
    },
    /// No implemented power-of-two level satisfied all constraints.
    NoReductionLevelFits,
    /// A reduced preview coordinate was outside the output.
    CoordinateOutOfBounds {
        /// Requested column.
        x: usize,
        /// Requested row.
        y: usize,
        /// Output width.
        width: usize,
        /// Output height.
        height: usize,
    },
    /// Source support or derived dimensions overflowed their representation.
    SizeOverflow,
    /// Compensated reduction produced a non-finite mean from finite samples.
    NumericalOverflow,
    /// Output storage could not be reserved.
    AllocationFailed {
        /// Number of elements requested.
        elements: usize,
    },
    /// FITS region reading failed.
    Fits(ImageReadError),
    /// Caller supplied a transform that violated its documented invariant.
    InvalidDisplayTransform,
    /// Internal scalar-preview arrays did not share one exact length.
    PreviewInvariant,
    /// No finite pixel with valid source support was available for estimation.
    NoValidSamples,
}

impl Display for PreviewError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidReductionLevel { maximum } => {
                write!(
                    formatter,
                    "preview reduction level exceeds maximum {maximum}"
                )
            }
            Self::InvalidOutputPixelLimit { maximum } => write!(
                formatter,
                "preview output limit must be between 1 and {maximum} pixels"
            ),
            Self::InvalidOutputDimensions => {
                formatter.write_str("preview output width and height limits must be positive")
            }
            Self::InvalidSourceDimensions => {
                formatter.write_str("preview source width and height must be positive")
            }
            Self::InvalidIoChunkSize { maximum } => write!(
                formatter,
                "preview I/O chunk must be between 1 and {maximum} samples"
            ),
            Self::UnsupportedAxisCount { axes } => {
                write!(
                    formatter,
                    "preview does not support a FITS image with {axes} axes"
                )
            }
            Self::PlaneOutOfBounds { plane, planes } => write!(
                formatter,
                "preview plane {plane} is outside {planes} available planes"
            ),
            Self::OutputPixelLimitExceeded { required, maximum } => write!(
                formatter,
                "preview requires {required} pixels but the configured maximum is {maximum}"
            ),
            Self::NoReductionLevelFits => formatter.write_str(
                "no implemented preview reduction level satisfies the output constraints",
            ),
            Self::CoordinateOutOfBounds {
                x,
                y,
                width,
                height,
            } => write!(
                formatter,
                "preview coordinate ({x}, {y}) is outside {width}x{height}"
            ),
            Self::SizeOverflow => formatter.write_str("preview dimensions or support overflow"),
            Self::NumericalOverflow => {
                formatter.write_str("preview reduction exceeded the finite numerical domain")
            }
            Self::AllocationFailed { elements } => {
                write!(formatter, "cannot reserve {elements} preview elements")
            }
            Self::Fits(error) => write!(formatter, "cannot read FITS preview pixels: {error}"),
            Self::InvalidDisplayTransform => {
                formatter.write_str("display transform is internally invalid")
            }
            Self::PreviewInvariant => {
                formatter.write_str("preview arrays do not share one pixel count")
            }
            Self::NoValidSamples => {
                formatter.write_str("preview contains no finite pixel with valid support")
            }
        }
    }
}

impl Error for PreviewError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Fits(error) => Some(error),
            _ => None,
        }
    }
}

fn validate_rgb_previews(
    red: &ScalarPreview,
    green: &ScalarPreview,
    blue: &ScalarPreview,
) -> Result<usize, PreviewError> {
    let pixel_count = red
        .width
        .checked_mul(red.height)
        .ok_or(PreviewError::SizeOverflow)?;
    let congruent = [green, blue].iter().all(|preview| {
        preview.source_width == red.source_width
            && preview.source_height == red.source_height
            && preview.reduction_level == red.reduction_level
            && preview.reduction_factor == red.reduction_factor
            && preview.width == red.width
            && preview.height == red.height
    });
    let arrays_valid = [red, green, blue].iter().all(|preview| {
        preview.values.len() == pixel_count
            && preview.valid_support.len() == pixel_count
            && preview.excluded_support.len() == pixel_count
            && preview.excluded_flags.len() == pixel_count
    });
    if !congruent || !arrays_valid {
        return Err(PreviewError::PreviewInvariant);
    }
    Ok(pixel_count)
}

#[allow(clippy::too_many_arguments)]
fn accumulate_region(
    values: &[f64],
    statuses: &[SampleStatus],
    source_x: u64,
    source_y: u64,
    region_width: u64,
    reduction_factor: u64,
    output_width: usize,
    sums: &mut [CompensatedSum],
    valid_support: &mut [u32],
    excluded_support: &mut [u32],
    excluded_flags: &mut [PixelFlags],
) -> Result<(), PreviewError> {
    if values.len() != statuses.len()
        || sums.len() != valid_support.len()
        || sums.len() != excluded_support.len()
        || sums.len() != excluded_flags.len()
    {
        return Err(PreviewError::PreviewInvariant);
    }
    let region_width_usize =
        usize::try_from(region_width).map_err(|_| PreviewError::SizeOverflow)?;
    for (local_index, (value, status)) in values.iter().zip(statuses).enumerate() {
        let local_x = local_index % region_width_usize;
        let local_y = local_index / region_width_usize;
        let global_x = source_x
            .checked_add(u64::try_from(local_x).map_err(|_| PreviewError::SizeOverflow)?)
            .ok_or(PreviewError::SizeOverflow)?;
        let global_y = source_y
            .checked_add(u64::try_from(local_y).map_err(|_| PreviewError::SizeOverflow)?)
            .ok_or(PreviewError::SizeOverflow)?;
        let output_x =
            usize::try_from(global_x / reduction_factor).map_err(|_| PreviewError::SizeOverflow)?;
        let output_y =
            usize::try_from(global_y / reduction_factor).map_err(|_| PreviewError::SizeOverflow)?;
        let output_index = output_y
            .checked_mul(output_width)
            .and_then(|row| row.checked_add(output_x))
            .ok_or(PreviewError::SizeOverflow)?;
        let sum = sums
            .get_mut(output_index)
            .ok_or(PreviewError::PreviewInvariant)?;
        let valid = valid_support
            .get_mut(output_index)
            .ok_or(PreviewError::PreviewInvariant)?;
        let excluded = excluded_support
            .get_mut(output_index)
            .ok_or(PreviewError::PreviewInvariant)?;
        let flags = excluded_flags
            .get_mut(output_index)
            .ok_or(PreviewError::PreviewInvariant)?;
        let usable = *status == SampleStatus::Valid && value.is_finite();
        if usable {
            *valid = valid.checked_add(1).ok_or(PreviewError::SizeOverflow)?;
            sum.add(*value);
        } else {
            *excluded = excluded.checked_add(1).ok_or(PreviewError::SizeOverflow)?;
            *flags |= match status {
                SampleStatus::Undefined => PixelFlags::MISSING,
                SampleStatus::NonFinite => PixelFlags::INVALID,
                SampleStatus::Valid => PixelFlags::INVALID,
            };
        }
    }
    Ok(())
}

fn map_transfer(value: f64, transform: DisplayTransform) -> Result<f64, PreviewError> {
    let mapped = match transform.transfer_function() {
        TransferFunction::Linear => value,
        TransferFunction::Midtones => {
            if value <= 0.0 || value >= 1.0 {
                value
            } else {
                let midtone = transform.midtone();
                ((midtone - 1.0) * value) / ((2.0 * midtone - 1.0) * value - midtone)
            }
        }
        TransferFunction::Asinh { softness } => {
            (value / softness).asinh() / (1.0 / softness).asinh()
        }
    };
    if mapped.is_finite() {
        Ok(mapped.clamp(0.0, 1.0))
    } else {
        Err(PreviewError::NumericalOverflow)
    }
}

fn median_of_sorted(values: &[f64]) -> Result<f64, PreviewError> {
    let middle = values.len() / 2;
    let median = if values.len().is_multiple_of(2) {
        values[middle - 1] * 0.5 + values[middle] * 0.5
    } else {
        values[middle]
    };
    if median.is_finite() {
        Ok(canonical_zero(median))
    } else {
        Err(PreviewError::NumericalOverflow)
    }
}

fn solve_midtone(input: f64, output: f64) -> Result<f64, PreviewError> {
    let denominator = input + output - 2.0 * input * output;
    let midtone = input * (1.0 - output) / denominator;
    if midtone.is_finite() && (0.0..1.0).contains(&midtone) {
        Ok(midtone)
    } else {
        Err(PreviewError::NumericalOverflow)
    }
}

fn missing_rgba(index: usize, width: usize, style: MissingPixelStyle) -> [u8; 4] {
    match style {
        MissingPixelStyle::Transparent => [0, 0, 0, 0],
        MissingPixelStyle::Checkerboard => {
            let x = index % width;
            let y = index / width;
            if (x + y).is_multiple_of(2) {
                [188, 64, 188, 255]
            } else {
                [54, 22, 54, 255]
            }
        }
    }
}

fn ceil_div(value: u64, divisor: u64) -> Result<u64, PreviewError> {
    value
        .checked_add(divisor - 1)
        .map(|value| value / divisor)
        .ok_or(PreviewError::SizeOverflow)
}

fn canonical_zero(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}

fn try_filled_vec<T: Clone>(elements: usize, value: T) -> Result<Vec<T>, PreviewError> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(elements)
        .map_err(|_| PreviewError::AllocationFailed { elements })?;
    output.resize(elements, value);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use aether_core::{Dimensions, ScientificImage};
    use aether_fits::{HeaderReadOptions, PrimaryImageReader, write_f64_primary};
    use aether_review::DISPLAY_TRANSFORM_VERSION;

    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn Error>>;

    fn fits_reader(
        width: usize,
        height: usize,
        values: Vec<f64>,
    ) -> TestResult<PrimaryImageReader<Cursor<Vec<u8>>>> {
        let image = ScientificImage::from_pixels(Dimensions::new(width, height, 1)?, values)?;
        fits_reader_from_image(&image)
    }

    fn fits_reader_from_image(
        image: &ScientificImage,
    ) -> TestResult<PrimaryImageReader<Cursor<Vec<u8>>>> {
        let mut bytes = Vec::new();
        write_f64_primary(&mut bytes, image)?;
        Ok(PrimaryImageReader::open(
            Cursor::new(bytes),
            HeaderReadOptions::default(),
        )?)
    }

    fn parameters(level: u8, chunk: usize) -> TestResult<FitsPreviewParameters> {
        Ok(FitsPreviewParameters::new(0, level, 1_024, chunk)?)
    }

    #[test]
    fn averages_power_of_two_blocks_with_exact_edge_support() -> TestResult {
        let values = (1..=15).map(f64::from).collect();
        let mut reader = fits_reader(5, 3, values)?;
        let preview = build_fits_preview(&mut reader, parameters(1, 15)?)?;

        assert_eq!((preview.width(), preview.height()), (3, 2));
        assert_eq!(preview.values(), &[4.0, 6.0, 7.5, 11.5, 13.5, 15.0]);
        assert_eq!(preview.valid_support(), &[4, 4, 2, 2, 2, 1]);
        assert_eq!(preview.excluded_support(), &[0; 6]);
        assert_eq!(preview.sample(2, 1)?.value().to_bits(), 15.0_f64.to_bits());
        Ok(())
    }

    #[test]
    fn io_chunk_size_does_not_change_reduction_bits() -> TestResult {
        let values: Vec<_> = (0..63).map(|value| f64::from(value) - 20.0).collect();
        let mut narrow = fits_reader(9, 7, values.clone())?;
        let mut broad = fits_reader(9, 7, values)?;
        let narrow = build_fits_preview(&mut narrow, parameters(2, 3)?)?;
        let broad = build_fits_preview(&mut broad, parameters(2, 63)?)?;

        assert_eq!(narrow.values(), broad.values());
        assert_eq!(narrow.valid_support(), broad.valid_support());
        assert_eq!(narrow.excluded_flags(), broad.excluded_flags());
        Ok(())
    }

    #[test]
    fn excludes_non_finite_source_and_retains_complete_accounting() -> TestResult {
        let mut reader = fits_reader(2, 2, vec![1.0, f64::NAN, 3.0, 4.0])?;
        let preview = build_fits_preview(&mut reader, parameters(1, 2)?)?;
        let sample = preview.sample(0, 0)?;

        assert_eq!(sample.value().to_bits(), (8.0_f64 / 3.0).to_bits());
        assert_eq!(sample.valid_support(), 3);
        assert_eq!(sample.excluded_support(), 1);
        assert!(sample.excluded_flags().contains(PixelFlags::INVALID));
        Ok(())
    }

    #[test]
    fn all_invalid_block_is_explicitly_missing() -> TestResult {
        let mut reader = fits_reader(2, 1, vec![f64::NAN, f64::INFINITY])?;
        let preview = build_fits_preview(&mut reader, parameters(1, 1)?)?;
        let sample = preview.sample(0, 0)?;

        assert!(sample.value().is_nan());
        assert_eq!(sample.valid_support(), 0);
        assert_eq!(sample.excluded_support(), 2);
        assert!(sample.excluded_flags().contains(PixelFlags::INVALID));
        assert!(sample.excluded_flags().contains(PixelFlags::MISSING));
        Ok(())
    }

    #[test]
    fn enforces_parameters_plane_output_and_coordinates() -> TestResult {
        assert!(matches!(
            FitsPreviewParameters::new(0, MAX_REDUCTION_LEVEL + 1, 1, 1),
            Err(PreviewError::InvalidReductionLevel { .. })
        ));
        assert!(matches!(
            FitsPreviewParameters::new(0, 0, 0, 1),
            Err(PreviewError::InvalidOutputPixelLimit { .. })
        ));
        assert!(matches!(
            FitsPreviewParameters::new(0, 0, 1, 0),
            Err(PreviewError::InvalidIoChunkSize { .. })
        ));

        let mut reader = fits_reader(4, 4, vec![1.0; 16])?;
        assert!(matches!(
            build_fits_preview(&mut reader, FitsPreviewParameters::new(1, 0, 16, 4)?,),
            Err(PreviewError::PlaneOutOfBounds { .. })
        ));
        assert!(matches!(
            build_fits_preview(&mut reader, FitsPreviewParameters::new(0, 0, 15, 4)?,),
            Err(PreviewError::OutputPixelLimitExceeded {
                required: 16,
                maximum: 15,
            })
        ));
        let preview = build_fits_preview(&mut reader, parameters(0, 4)?)?;
        assert!(matches!(
            preview.sample(4, 0),
            Err(PreviewError::CoordinateOutOfBounds { .. })
        ));
        Ok(())
    }

    #[test]
    fn selects_the_finest_level_that_satisfies_every_limit() -> TestResult {
        let limits = PreviewLimits::new(1_600, 1_200, 2_000_000)?;
        assert_eq!(choose_reduction_level(4_144, 2_822, limits)?, 2);

        let pixel_limited = PreviewLimits::new(10_000, 10_000, 500_000)?;
        assert_eq!(choose_reduction_level(4_144, 2_822, pixel_limited)?, 3);
        assert!(matches!(
            choose_reduction_level(0, 2_822, limits),
            Err(PreviewError::InvalidSourceDimensions)
        ));
        assert!(matches!(
            PreviewLimits::new(0, 1_200, 1),
            Err(PreviewError::InvalidOutputDimensions)
        ));
        assert!(matches!(
            choose_reduction_level(1_u64 << 40, 1, PreviewLimits::new(1, 1, 1)?),
            Err(PreviewError::NoReductionLevelFits)
        ));
        Ok(())
    }

    #[test]
    fn linear_mapping_produces_exact_grayscale_endpoints() -> TestResult {
        let mut reader = fits_reader(3, 1, vec![0.0, 0.5, 1.0])?;
        let preview = build_fits_preview(&mut reader, parameters(0, 3)?)?;
        let transform = DisplayTransform::new(0.0, 1.0, 0.5, TransferFunction::Linear)?;
        let rgba = render_grayscale_rgba8(&preview, transform, MissingPixelStyle::Transparent)?;

        assert_eq!(rgba.display_transform_version(), DISPLAY_TRANSFORM_VERSION);
        assert_eq!(
            rgba.pixels(),
            &[0, 0, 0, 255, 128, 128, 128, 255, 255, 255, 255, 255]
        );
        Ok(())
    }

    #[test]
    fn linked_rgb_mapping_preserves_channel_order_and_ratios() -> TestResult {
        let image = ScientificImage::from_pixels(
            Dimensions::new(2, 1, 3)?,
            vec![0.0, 1.0, 0.25, 0.75, 0.5, 0.125],
        )?;
        let mut red_reader = fits_reader_from_image(&image)?;
        let mut green_reader = fits_reader_from_image(&image)?;
        let mut blue_reader = fits_reader_from_image(&image)?;
        let red = build_fits_preview(&mut red_reader, FitsPreviewParameters::new(0, 0, 2, 2)?)?;
        let green = build_fits_preview(&mut green_reader, FitsPreviewParameters::new(1, 0, 2, 2)?)?;
        let blue = build_fits_preview(&mut blue_reader, FitsPreviewParameters::new(2, 0, 2, 2)?)?;
        let transform = DisplayTransform::new(0.0, 1.0, 0.5, TransferFunction::Linear)?;

        let rgba = render_rgb_rgba8(
            &red,
            &green,
            &blue,
            transform,
            MissingPixelStyle::Transparent,
        )?;

        assert_eq!(rgba.pixels(), &[0, 64, 128, 255, 255, 191, 32, 255]);
        Ok(())
    }

    #[test]
    fn linked_rgb_stretch_uses_luminance_and_common_support() -> TestResult {
        let image = ScientificImage::from_pixels(
            Dimensions::new(3, 1, 3)?,
            vec![10.0, 20.0, 30.0, 20.0, 40.0, 60.0, 30.0, 60.0, f64::NAN],
        )?;
        let mut readers = [
            fits_reader_from_image(&image)?,
            fits_reader_from_image(&image)?,
            fits_reader_from_image(&image)?,
        ];
        let red = build_fits_preview(&mut readers[0], FitsPreviewParameters::new(0, 0, 3, 3)?)?;
        let green = build_fits_preview(&mut readers[1], FitsPreviewParameters::new(1, 0, 3, 3)?)?;
        let blue = build_fits_preview(&mut readers[2], FitsPreviewParameters::new(2, 0, 3, 3)?)?;

        let estimate = estimate_rgb_display_transform(&red, &green, &blue)?;
        let first_luminance = LUMINANCE_RED * 10.0 + LUMINANCE_GREEN * 20.0 + LUMINANCE_BLUE * 30.0;
        let second_luminance =
            LUMINANCE_RED * 20.0 + LUMINANCE_GREEN * 40.0 + LUMINANCE_BLUE * 60.0;

        assert_eq!(estimate.finite_samples(), 2);
        assert_eq!(
            estimate.median().to_bits(),
            (first_luminance * 0.5 + second_luminance * 0.5).to_bits()
        );
        let rgba = render_rgb_rgba8(
            &red,
            &green,
            &blue,
            estimate.transform(),
            MissingPixelStyle::Checkerboard,
        )?;
        assert_eq!(&rgba.pixels()[8..12], &[188, 64, 188, 255]);
        Ok(())
    }

    #[test]
    fn missing_style_is_visible_or_transparent_without_changing_scalar_data() -> TestResult {
        let mut reader = fits_reader(2, 1, vec![f64::NAN, f64::NAN])?;
        let preview = build_fits_preview(&mut reader, parameters(0, 2)?)?;
        let transform = DisplayTransform::new(0.0, 1.0, 0.5, TransferFunction::Linear)?;
        let transparent =
            render_grayscale_rgba8(&preview, transform, MissingPixelStyle::Transparent)?;
        let checker = render_grayscale_rgba8(&preview, transform, MissingPixelStyle::Checkerboard)?;

        assert_eq!(transparent.pixels(), &[0; 8]);
        assert_eq!(checker.pixels(), &[188, 64, 188, 255, 54, 22, 54, 255]);
        assert!(preview.values().iter().all(|value| value.is_nan()));
        Ok(())
    }

    #[test]
    fn automatic_stretch_is_robust_to_one_extreme_hot_pixel() -> TestResult {
        let mut values: Vec<_> = (0..1_000)
            .map(|index| 1_000.0 + f64::from(index % 11) - 5.0)
            .collect();
        values.push(65_535.0);
        let mut reader = fits_reader(values.len(), 1, values)?;
        let preview =
            build_fits_preview(&mut reader, FitsPreviewParameters::new(0, 0, 2_000, 257)?)?;

        let estimate = estimate_display_transform(&preview)?;

        assert_eq!(estimate.finite_samples(), 1_001);
        assert!(estimate.median() >= 999.0 && estimate.median() <= 1_001.0);
        assert!(estimate.scaled_mad() > 0.0);
        assert!(estimate.high_quantile() < 65_535.0);
        assert_eq!(
            estimate.transform().white_point().to_bits(),
            estimate.high_quantile().to_bits()
        );
        assert_eq!(
            estimate.transform().transfer_function(),
            TransferFunction::Midtones
        );
        Ok(())
    }

    #[test]
    fn automatic_stretch_gives_a_constant_image_a_finite_range() -> TestResult {
        let mut reader = fits_reader(8, 1, vec![42.0; 8])?;
        let preview = build_fits_preview(&mut reader, parameters(0, 8)?)?;

        let estimate = estimate_display_transform(&preview)?;
        let transform = estimate.transform();

        assert_eq!(estimate.median().to_bits(), 42.0_f64.to_bits());
        assert_eq!(estimate.scaled_mad().to_bits(), 0.0_f64.to_bits());
        assert!(transform.black_point() < 42.0);
        assert!(transform.white_point() > 42.0);
        assert!((0.0..1.0).contains(&transform.midtone()));
        Ok(())
    }

    #[test]
    fn automatic_stretch_rejects_a_preview_without_valid_support() -> TestResult {
        let mut reader = fits_reader(2, 1, vec![f64::NAN, f64::INFINITY])?;
        let preview = build_fits_preview(&mut reader, parameters(0, 2)?)?;

        assert!(matches!(
            estimate_display_transform(&preview),
            Err(PreviewError::NoValidSamples)
        ));
        Ok(())
    }

    #[test]
    fn nonlinear_transfers_keep_black_white_and_midpoint_finite() -> TestResult {
        let midtones = DisplayTransform::new(0.0, 1.0, 0.5, TransferFunction::Midtones)?;
        assert_eq!(map_transfer(0.0, midtones)?.to_bits(), 0.0_f64.to_bits());
        assert_eq!(map_transfer(0.5, midtones)?.to_bits(), 0.5_f64.to_bits());
        assert_eq!(map_transfer(1.0, midtones)?.to_bits(), 1.0_f64.to_bits());

        let asinh =
            DisplayTransform::new(0.0, 1.0, 0.5, TransferFunction::Asinh { softness: 0.1 })?;
        assert_eq!(map_transfer(0.0, asinh)?.to_bits(), 0.0_f64.to_bits());
        assert!((0.0..1.0).contains(&map_transfer(0.5, asinh)?));
        assert_eq!(map_transfer(1.0, asinh)?.to_bits(), 1.0_f64.to_bits());
        Ok(())
    }
}
