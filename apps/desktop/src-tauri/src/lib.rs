//! Native desktop shell for AetherStack.
//!
//! Scientific and review behavior lives in the workspace crates. This crate is
//! deliberately limited to the operating-system window and typed IPC adapters,
//! preventing the web presenter from becoming a second processing engine.

use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs::File;
use std::io::{Read, Seek};
use std::path::PathBuf;

use aether_fits::{HeaderReadOptions, PrimaryImageReader};
use aether_preview::{
    FitsPreviewParameters, MissingPixelStyle, PreviewLimits, RgbaPreview, build_fits_preview,
    choose_reduction_level, render_grayscale_rgba8,
};
use aether_review::{DisplayTransform, TransferFunction};
use serde::{Deserialize, Serialize};
use tauri::ipc::Response;

const MAX_DESKTOP_PREVIEW_PIXELS: usize = 2 * 1_024 * 1_024;
const DESKTOP_PREVIEW_IO_CHUNK_SAMPLES: usize = 256 * 1_024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FitsPreviewRequest {
    path: PathBuf,
    plane: u64,
    maximum_width: usize,
    maximum_height: usize,
    black_point: f64,
    white_point: f64,
    midtone: f64,
    transfer: PreviewTransfer,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
enum PreviewTransfer {
    Linear,
    Midtones,
    Asinh { softness: f64 },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PreviewCommandError {
    code: &'static str,
    message: &'static str,
}

impl PreviewCommandError {
    const fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }
}

impl Display for PreviewCommandError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message)
    }
}

impl Error for PreviewCommandError {}

#[tauri::command]
fn render_fits_preview(request: FitsPreviewRequest) -> Result<Response, PreviewCommandError> {
    let file = File::open(&request.path).map_err(|_| {
        PreviewCommandError::new(
            "fits_open_failed",
            "The selected FITS file could not be opened.",
        )
    })?;
    let png = render_fits_preview_png(file, &request)?;
    Ok(Response::new(png))
}

fn render_fits_preview_png<R: Read + Seek>(
    input: R,
    request: &FitsPreviewRequest,
) -> Result<Vec<u8>, PreviewCommandError> {
    let requested_pixels = request
        .maximum_width
        .checked_mul(request.maximum_height)
        .ok_or_else(preview_bounds_error)?;
    let limits = PreviewLimits::new(
        request.maximum_width,
        request.maximum_height,
        requested_pixels.min(MAX_DESKTOP_PREVIEW_PIXELS),
    )
    .map_err(|_| preview_bounds_error())?;

    let mut reader =
        PrimaryImageReader::open(input, HeaderReadOptions::default()).map_err(|_| {
            PreviewCommandError::new(
                "fits_layout_unsupported",
                "The file does not contain a supported primary FITS image.",
            )
        })?;
    let axes = reader.descriptor().axes();
    let (source_width, source_height) = match axes {
        [width, height] | [width, height, _] => (*width, *height),
        _ => {
            return Err(PreviewCommandError::new(
                "fits_axes_unsupported",
                "The FITS preview requires a two- or three-axis primary image.",
            ));
        }
    };
    let reduction_level =
        choose_reduction_level(source_width, source_height, limits).map_err(|_| {
            PreviewCommandError::new(
                "preview_bounds_unsatisfied",
                "The image cannot be reduced within the preview safety limits.",
            )
        })?;
    let parameters = FitsPreviewParameters::new(
        request.plane,
        reduction_level,
        limits.maximum_pixels(),
        DESKTOP_PREVIEW_IO_CHUNK_SAMPLES,
    )
    .map_err(|_| preview_bounds_error())?;
    let scalar = build_fits_preview(&mut reader, parameters).map_err(|_| {
        PreviewCommandError::new(
            "preview_decode_failed",
            "The FITS pixels could not be decoded into a bounded preview.",
        )
    })?;
    let transfer_function = match request.transfer {
        PreviewTransfer::Linear => TransferFunction::Linear,
        PreviewTransfer::Midtones => TransferFunction::Midtones,
        PreviewTransfer::Asinh { softness } => TransferFunction::Asinh { softness },
    };
    let transform = DisplayTransform::new(
        request.black_point,
        request.white_point,
        request.midtone,
        transfer_function,
    )
    .map_err(|_| {
        PreviewCommandError::new(
            "display_transform_invalid",
            "The requested display transform is invalid.",
        )
    })?;
    let rgba = render_grayscale_rgba8(&scalar, transform, MissingPixelStyle::Checkerboard)
        .map_err(|_| {
            PreviewCommandError::new(
                "preview_mapping_failed",
                "The scalar preview could not be mapped for display.",
            )
        })?;
    encode_png(&rgba)
}

fn encode_png(preview: &RgbaPreview) -> Result<Vec<u8>, PreviewCommandError> {
    let width = u32::try_from(preview.width()).map_err(|_| preview_encoding_error())?;
    let height = u32::try_from(preview.height()).map_err(|_| preview_encoding_error())?;
    let mut output = Vec::new();
    let mut encoder = png::Encoder::new(&mut output, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|_| preview_encoding_error())?;
    writer
        .write_image_data(preview.pixels())
        .map_err(|_| preview_encoding_error())?;
    writer.finish().map_err(|_| preview_encoding_error())?;
    Ok(output)
}

const fn preview_bounds_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "preview_bounds_invalid",
        "The requested preview dimensions are outside the supported bounds.",
    )
}

const fn preview_encoding_error() -> PreviewCommandError {
    PreviewCommandError::new(
        "preview_encoding_failed",
        "The display preview could not be encoded.",
    )
}

/// Runs the native AetherStack application until its final window closes.
///
/// # Errors
///
/// Returns a Tauri runtime error if the desktop shell cannot be initialized or
/// the native event loop terminates abnormally.
pub fn run() -> Result<(), tauri::Error> {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![render_fits_preview])
        .run(tauri::generate_context!())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use aether_core::{Dimensions, ScientificImage};
    use aether_fits::write_f64_primary;

    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

    fn request(transfer: PreviewTransfer) -> FitsPreviewRequest {
        FitsPreviewRequest {
            path: PathBuf::from("unused-in-memory-test.fits"),
            plane: 0,
            maximum_width: 4,
            maximum_height: 2,
            black_point: 0.0,
            white_point: 8.0,
            midtone: 0.5,
            transfer,
        }
    }

    fn fits_bytes() -> TestResult<Vec<u8>> {
        let image = ScientificImage::from_pixels(
            Dimensions::new(4, 2, 1)?,
            (0..8).map(f64::from).collect(),
        )?;
        let mut bytes = Vec::new();
        write_f64_primary(&mut bytes, &image)?;
        Ok(bytes)
    }

    #[test]
    fn renders_a_png_with_the_bounded_preview_dimensions() -> TestResult {
        let encoded = render_fits_preview_png(
            Cursor::new(fits_bytes()?),
            &request(PreviewTransfer::Linear),
        )?;

        assert_eq!(&encoded[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&encoded[12..16], b"IHDR");
        assert_eq!(u32::from_be_bytes(encoded[16..20].try_into()?), 4);
        assert_eq!(u32::from_be_bytes(encoded[20..24].try_into()?), 2);
        Ok(())
    }

    #[test]
    fn rejects_an_invalid_display_transform_before_mapping() -> TestResult {
        let mut invalid = request(PreviewTransfer::Midtones);
        invalid.white_point = invalid.black_point;

        let Err(error) = render_fits_preview_png(Cursor::new(fits_bytes()?), &invalid) else {
            return Err("equal display points were accepted".into());
        };
        assert_eq!(error.code, "display_transform_invalid");
        Ok(())
    }

    #[test]
    fn rejects_zero_output_bounds() -> TestResult {
        let mut invalid = request(PreviewTransfer::Asinh { softness: 0.1 });
        invalid.maximum_width = 0;

        let Err(error) = render_fits_preview_png(Cursor::new(fits_bytes()?), &invalid) else {
            return Err("zero preview width was accepted".into());
        };
        assert_eq!(error.code, "preview_bounds_invalid");
        Ok(())
    }
}
