//! Reproducible in-memory profiling for strict affine and projective resampling.
//!
//! The benchmark generates deterministic pixels, performs no filesystem I/O,
//! and verifies that every pass produces the same output seal.

use std::env;
use std::ffi::OsString;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use aether_core::{Dimensions, ScientificImage};
use aether_registration::{
    AffineTransform, ProjectiveTransform, resample_lanczos3, resample_lanczos3_projective,
};

const DEFAULT_WIDTH: usize = 1_024;
const DEFAULT_HEIGHT: usize = 768;
const DEFAULT_PLANES: usize = 1;
const DEFAULT_PASSES: usize = 3;
const MAX_AXIS: usize = 8_192;
const MAX_PLANES: usize = 3;
const MAX_PASSES: usize = 20;
const MAX_SAMPLES: usize = 100_000_000;
const USAGE: &str =
    "Usage: aether-registration-bench [--width N] [--height N] [--planes N] [--passes N]";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Config {
    width: usize,
    height: usize,
    planes: usize,
    passes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OutputSeal {
    pixel_bits: u64,
    mask_bits: u64,
}

fn main() -> ExitCode {
    let config = match parse_args(env::args_os().skip(1)) {
        Ok(config) => config,
        Err(message) => {
            eprintln!("{message}");
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("benchmark failed: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(config: Config) -> Result<(), String> {
    let dimensions = Dimensions::new(config.width, config.height, config.planes)
        .map_err(|error| error.to_string())?;
    let pixels = deterministic_pixels(dimensions.pixel_count())?;
    let source =
        ScientificImage::from_pixels(dimensions, pixels).map_err(|error| error.to_string())?;
    let affine = AffineTransform::new(0.9998, -0.0015, 0.0015, 0.9998, 2.25, -1.75)
        .map_err(|error| error.to_string())?;
    let projective = ProjectiveTransform::new([
        [0.9998, -0.0015, 2.25],
        [0.0015, 0.9998, -1.75],
        [8.0e-7, -6.0e-7, 1.0],
    ])
    .map_err(|error| error.to_string())?;

    println!("AetherStack registration benchmark");
    println!("width: {}", config.width);
    println!("height: {}", config.height);
    println!("planes: {}", config.planes);
    println!("passes: {}", config.passes);
    println!("samples_per_pass: {}", dimensions.pixel_count());
    benchmark_affine(&source, config, affine)?;
    benchmark_projective(&source, config, projective)?;
    Ok(())
}

fn benchmark_affine(
    source: &ScientificImage,
    config: Config,
    transform: AffineTransform,
) -> Result<(), String> {
    let mut reference = None;
    for pass in 1..=config.passes {
        let started = Instant::now();
        let output = resample_lanczos3(source, config.width, config.height, transform)
            .map_err(|error| error.to_string())?;
        let elapsed = started.elapsed();
        verify_and_print("affine", pass, elapsed, output.image(), &mut reference)?;
    }
    println!("affine_output_stable_across_passes: true");
    Ok(())
}

fn benchmark_projective(
    source: &ScientificImage,
    config: Config,
    transform: ProjectiveTransform,
) -> Result<(), String> {
    let mut reference = None;
    for pass in 1..=config.passes {
        let started = Instant::now();
        let output = resample_lanczos3_projective(source, config.width, config.height, transform)
            .map_err(|error| error.to_string())?;
        let elapsed = started.elapsed();
        verify_and_print("projective", pass, elapsed, output.image(), &mut reference)?;
    }
    println!("projective_output_stable_across_passes: true");
    Ok(())
}

fn verify_and_print(
    geometry: &str,
    pass: usize,
    elapsed: Duration,
    image: &ScientificImage,
    reference: &mut Option<OutputSeal>,
) -> Result<(), String> {
    let seal = output_seal(image);
    if reference.is_some_and(|expected| expected != seal) {
        return Err(format!("{geometry} output changed on pass {pass}"));
    }
    reference.get_or_insert(seal);
    let seconds = elapsed.as_secs_f64();
    let megapixels_per_second = image.dimensions().pixel_count() as f64 / seconds / 1_000_000.0;
    println!("{geometry}_pass_{pass}_seconds: {seconds:.6}");
    println!("{geometry}_pass_{pass}_megapixels_per_second: {megapixels_per_second:.3}");
    Ok(())
}

fn deterministic_pixels(count: usize) -> Result<Vec<f64>, String> {
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(count)
        .map_err(|_| "source allocation failed".to_owned())?;
    for index in 0..count {
        let coarse = (index % 4_093) as f64 * 0.125;
        let fine = ((index.wrapping_mul(2_654_435_761)) & 0xffff) as f64 / 65_536.0;
        pixels.push(coarse + fine);
    }
    Ok(pixels)
}

fn output_seal(image: &ScientificImage) -> OutputSeal {
    let pixel_bits = image
        .pixels()
        .iter()
        .fold(0_u64, |seal, value| seal.rotate_left(7) ^ value.to_bits());
    let mask_bits = image.mask().as_slice().iter().fold(0_u64, |seal, value| {
        seal.rotate_left(3) ^ u64::from(value.bits())
    });
    OutputSeal {
        pixel_bits,
        mask_bits,
    }
}

fn parse_args(arguments: impl IntoIterator<Item = OsString>) -> Result<Config, String> {
    let mut config = Config {
        width: DEFAULT_WIDTH,
        height: DEFAULT_HEIGHT,
        planes: DEFAULT_PLANES,
        passes: DEFAULT_PASSES,
    };
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        let option = argument
            .to_str()
            .ok_or_else(|| "options must be valid UTF-8".to_owned())?;
        match option {
            "--width" => config.width = parse_positive(arguments.next(), option)?,
            "--height" => config.height = parse_positive(arguments.next(), option)?,
            "--planes" => config.planes = parse_positive(arguments.next(), option)?,
            "--passes" => config.passes = parse_positive(arguments.next(), option)?,
            _ => return Err(format!("unknown option: {option}")),
        }
    }
    if config.width > MAX_AXIS || config.height > MAX_AXIS {
        return Err(format!("width and height must not exceed {MAX_AXIS}"));
    }
    if config.planes > MAX_PLANES {
        return Err(format!("--planes must not exceed {MAX_PLANES}"));
    }
    if config.passes > MAX_PASSES {
        return Err(format!("--passes must not exceed {MAX_PASSES}"));
    }
    let samples = config
        .width
        .checked_mul(config.height)
        .and_then(|value| value.checked_mul(config.planes))
        .ok_or_else(|| "sample count overflowed".to_owned())?;
    if samples > MAX_SAMPLES {
        return Err(format!("sample count must not exceed {MAX_SAMPLES}"));
    }
    Ok(config)
}

fn parse_positive(value: Option<OsString>, option: &str) -> Result<usize, String> {
    let value = value.ok_or_else(|| format!("{option} requires a positive integer"))?;
    let parsed = value
        .to_str()
        .ok_or_else(|| format!("{option} must be valid UTF-8"))?
        .parse::<usize>()
        .map_err(|_| format!("{option} requires a positive integer"))?;
    if parsed == 0 {
        return Err(format!("{option} must be greater than zero"));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_defaults_and_bounded_overrides() {
        assert_eq!(
            parse_args([]),
            Ok(Config {
                width: DEFAULT_WIDTH,
                height: DEFAULT_HEIGHT,
                planes: DEFAULT_PLANES,
                passes: DEFAULT_PASSES,
            })
        );
        assert_eq!(
            parse_args([
                OsString::from("--width"),
                OsString::from("640"),
                OsString::from("--height"),
                OsString::from("480"),
                OsString::from("--planes"),
                OsString::from("3"),
                OsString::from("--passes"),
                OsString::from("5"),
            ]),
            Ok(Config {
                width: 640,
                height: 480,
                planes: 3,
                passes: 5,
            })
        );
    }

    #[test]
    fn rejects_unknown_zero_and_unbounded_inputs() {
        assert!(parse_args([OsString::from("--unknown")]).is_err());
        assert!(parse_args([OsString::from("--width"), OsString::from("0")]).is_err());
        assert!(
            parse_args([
                OsString::from("--planes"),
                OsString::from((MAX_PLANES + 1).to_string()),
            ])
            .is_err()
        );
        assert!(
            parse_args([
                OsString::from("--passes"),
                OsString::from((MAX_PASSES + 1).to_string()),
            ])
            .is_err()
        );
        assert!(
            parse_args([
                OsString::from("--width"),
                OsString::from(MAX_AXIS.to_string()),
                OsString::from("--height"),
                OsString::from(MAX_AXIS.to_string()),
                OsString::from("--planes"),
                OsString::from(MAX_PLANES.to_string()),
            ])
            .is_err()
        );
    }

    #[test]
    fn deterministic_source_and_seal_detect_changes() -> Result<(), Box<dyn std::error::Error>> {
        let pixels = deterministic_pixels(8)?;
        assert_eq!(pixels, deterministic_pixels(8)?);
        let first = ScientificImage::from_pixels(Dimensions::new(2, 2, 2)?, pixels)?;
        let mut changed = first.clone();
        changed.pixels_mut()[0] += 1.0;
        assert_ne!(output_seal(&first), output_seal(&changed));
        Ok(())
    }
}
