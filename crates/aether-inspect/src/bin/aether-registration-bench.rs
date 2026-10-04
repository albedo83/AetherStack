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
    AffineTransform, Lanczos3BandExecutor, ProjectiveLanczos3BandExecutor, ProjectiveTransform,
    resample_lanczos3, resample_lanczos3_projective,
};

const DEFAULT_WIDTH: usize = 1_024;
const DEFAULT_HEIGHT: usize = 768;
const DEFAULT_PLANES: usize = 1;
const DEFAULT_PASSES: usize = 3;
const DEFAULT_BAND_HEIGHT: usize = 64;
const MAX_AXIS: usize = 8_192;
const MAX_PLANES: usize = 3;
const MAX_PASSES: usize = 20;
const MAX_SAMPLES: usize = 100_000_000;
const USAGE: &str = "Usage: aether-registration-bench [--width N] [--height N] [--planes N] [--passes N] [--band-height N]";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Config {
    width: usize,
    height: usize,
    planes: usize,
    passes: usize,
    band_height: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
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
    println!("band_height: {}", config.band_height);
    println!("samples_per_pass: {}", dimensions.pixel_count());
    benchmark_affine(&source, config, affine)?;
    benchmark_projective(&source, config, projective)?;
    benchmark_affine_bands(&source, config, affine)?;
    benchmark_projective_bands(&source, config, projective)?;
    Ok(())
}

fn benchmark_affine_bands(
    source: &ScientificImage,
    config: Config,
    transform: AffineTransform,
) -> Result<(), String> {
    let (_, warmup_seal) = run_affine_bands(source, config, transform)?;
    let mut reference = Some(warmup_seal);
    let mut rates = Vec::with_capacity(config.passes);
    println!("affine_banded_warmup_complete: true");
    for pass in 1..=config.passes {
        let (elapsed, seal) = run_affine_bands(source, config, transform)?;
        rates.push(verify_seal_and_print(
            "affine_banded",
            pass,
            elapsed,
            config.width * config.height * config.planes,
            seal,
            &mut reference,
        )?);
    }
    println!(
        "affine_banded_median_megapixels_per_second: {:.3}",
        median(&mut rates)
    );
    println!("affine_banded_output_stable_across_passes: true");
    Ok(())
}

fn benchmark_projective_bands(
    source: &ScientificImage,
    config: Config,
    transform: ProjectiveTransform,
) -> Result<(), String> {
    let (_, warmup_seal) = run_projective_bands(source, config, transform)?;
    let mut reference = Some(warmup_seal);
    let mut rates = Vec::with_capacity(config.passes);
    println!("projective_banded_warmup_complete: true");
    for pass in 1..=config.passes {
        let (elapsed, seal) = run_projective_bands(source, config, transform)?;
        rates.push(verify_seal_and_print(
            "projective_banded",
            pass,
            elapsed,
            config.width * config.height * config.planes,
            seal,
            &mut reference,
        )?);
    }
    println!(
        "projective_banded_median_megapixels_per_second: {:.3}",
        median(&mut rates)
    );
    println!("projective_banded_output_stable_across_passes: true");
    Ok(())
}

fn run_affine_bands(
    source: &ScientificImage,
    config: Config,
    transform: AffineTransform,
) -> Result<(Duration, OutputSeal), String> {
    let mut executor = Lanczos3BandExecutor::new(
        source,
        config.width,
        config.height,
        config.band_height,
        transform,
    )
    .map_err(|error| error.to_string())?;
    let mut elapsed = Duration::ZERO;
    let mut seal = OutputSeal::default();
    loop {
        let started = Instant::now();
        let band = executor.next_band().map_err(|error| error.to_string())?;
        let Some(band) = band else { break };
        elapsed += started.elapsed();
        accumulate_output_seal(&mut seal, band.image());
    }
    if !executor.is_complete() {
        return Err("affine band traversal ended before the output was complete".to_owned());
    }
    Ok((elapsed, seal))
}

fn run_projective_bands(
    source: &ScientificImage,
    config: Config,
    transform: ProjectiveTransform,
) -> Result<(Duration, OutputSeal), String> {
    let mut executor = ProjectiveLanczos3BandExecutor::new(
        source,
        config.width,
        config.height,
        config.band_height,
        transform,
    )
    .map_err(|error| error.to_string())?;
    let mut elapsed = Duration::ZERO;
    let mut seal = OutputSeal::default();
    loop {
        let started = Instant::now();
        let band = executor.next_band().map_err(|error| error.to_string())?;
        let Some(band) = band else { break };
        elapsed += started.elapsed();
        accumulate_output_seal(&mut seal, band.image());
    }
    if !executor.is_complete() {
        return Err("projective band traversal ended before the output was complete".to_owned());
    }
    Ok((elapsed, seal))
}

fn benchmark_affine(
    source: &ScientificImage,
    config: Config,
    transform: AffineTransform,
) -> Result<(), String> {
    let warmup = resample_lanczos3(source, config.width, config.height, transform)
        .map_err(|error| error.to_string())?;
    let mut reference = Some(output_seal(warmup.image()));
    let mut rates = Vec::with_capacity(config.passes);
    println!("affine_warmup_complete: true");
    for pass in 1..=config.passes {
        let started = Instant::now();
        let output = resample_lanczos3(source, config.width, config.height, transform)
            .map_err(|error| error.to_string())?;
        let elapsed = started.elapsed();
        rates.push(verify_and_print(
            "affine",
            pass,
            elapsed,
            output.image(),
            &mut reference,
        )?);
    }
    println!(
        "affine_median_megapixels_per_second: {:.3}",
        median(&mut rates)
    );
    println!("affine_output_stable_across_passes: true");
    Ok(())
}

fn benchmark_projective(
    source: &ScientificImage,
    config: Config,
    transform: ProjectiveTransform,
) -> Result<(), String> {
    let warmup = resample_lanczos3_projective(source, config.width, config.height, transform)
        .map_err(|error| error.to_string())?;
    let mut reference = Some(output_seal(warmup.image()));
    let mut rates = Vec::with_capacity(config.passes);
    println!("projective_warmup_complete: true");
    for pass in 1..=config.passes {
        let started = Instant::now();
        let output = resample_lanczos3_projective(source, config.width, config.height, transform)
            .map_err(|error| error.to_string())?;
        let elapsed = started.elapsed();
        rates.push(verify_and_print(
            "projective",
            pass,
            elapsed,
            output.image(),
            &mut reference,
        )?);
    }
    println!(
        "projective_median_megapixels_per_second: {:.3}",
        median(&mut rates)
    );
    println!("projective_output_stable_across_passes: true");
    Ok(())
}

fn verify_and_print(
    geometry: &str,
    pass: usize,
    elapsed: Duration,
    image: &ScientificImage,
    reference: &mut Option<OutputSeal>,
) -> Result<f64, String> {
    let seal = output_seal(image);
    verify_seal_and_print(
        geometry,
        pass,
        elapsed,
        image.dimensions().pixel_count(),
        seal,
        reference,
    )
}

fn verify_seal_and_print(
    geometry: &str,
    pass: usize,
    elapsed: Duration,
    sample_count: usize,
    seal: OutputSeal,
    reference: &mut Option<OutputSeal>,
) -> Result<f64, String> {
    if reference.is_some_and(|expected| expected != seal) {
        return Err(format!("{geometry} output changed on pass {pass}"));
    }
    reference.get_or_insert(seal);
    let seconds = elapsed.as_secs_f64();
    let megapixels_per_second = sample_count as f64 / seconds / 1_000_000.0;
    println!("{geometry}_pass_{pass}_seconds: {seconds:.6}");
    println!("{geometry}_pass_{pass}_megapixels_per_second: {megapixels_per_second:.3}");
    Ok(megapixels_per_second)
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_unstable_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) * 0.5
    } else {
        values[middle]
    }
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
    let mut seal = OutputSeal::default();
    accumulate_output_seal(&mut seal, image);
    seal
}

fn accumulate_output_seal(seal: &mut OutputSeal, image: &ScientificImage) {
    seal.pixel_bits = image.pixels().iter().fold(seal.pixel_bits, |state, value| {
        state.rotate_left(7) ^ value.to_bits()
    });
    seal.mask_bits = image
        .mask()
        .as_slice()
        .iter()
        .fold(seal.mask_bits, |state, value| {
            state.rotate_left(3) ^ u64::from(value.bits())
        });
}

fn parse_args(arguments: impl IntoIterator<Item = OsString>) -> Result<Config, String> {
    let mut config = Config {
        width: DEFAULT_WIDTH,
        height: DEFAULT_HEIGHT,
        planes: DEFAULT_PLANES,
        passes: DEFAULT_PASSES,
        band_height: DEFAULT_BAND_HEIGHT,
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
            "--band-height" => config.band_height = parse_positive(arguments.next(), option)?,
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
    if config.band_height > MAX_AXIS {
        return Err(format!("--band-height must not exceed {MAX_AXIS}"));
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
                band_height: DEFAULT_BAND_HEIGHT,
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
                OsString::from("--band-height"),
                OsString::from("32"),
            ]),
            Ok(Config {
                width: 640,
                height: 480,
                planes: 3,
                passes: 5,
                band_height: 32,
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
                OsString::from("--band-height"),
                OsString::from((MAX_AXIS + 1).to_string()),
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

    #[test]
    fn median_is_order_independent_for_odd_and_even_pass_counts() {
        assert_eq!(median(&mut [8.0, 2.0, 5.0]).to_bits(), 5.0_f64.to_bits());
        assert_eq!(
            median(&mut [8.0, 2.0, 6.0, 4.0]).to_bits(),
            5.0_f64.to_bits()
        );
    }
}
