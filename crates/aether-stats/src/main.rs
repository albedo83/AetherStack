//! Bounded-memory pixel statistics for FITS files and directory trees.
//!
//! The command emits either a compact table or newline-delimited JSON. The JSON
//! form is intended as a stable bridge for frame-review tooling while the
//! desktop application is developed. It never loads a complete image.

use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use aether_fits::{
    DEFAULT_STATISTICS_CHUNK_SAMPLES, HeaderReadOptions, PrimaryImageReader, StoredSampleFormat,
    ValidationMode, primary_image_statistics,
};
use serde::Serialize;

const USAGE: &str = "Usage: aether-stats [--strict] [--jsonl] [--chunk-samples N] [--limit N] <file-or-directory>...";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OutputFormat {
    Human,
    JsonLines,
}

#[derive(Debug, Eq, PartialEq)]
struct Config {
    roots: Vec<PathBuf>,
    validation_mode: ValidationMode,
    output_format: OutputFormat,
    chunk_samples: NonZeroUsize,
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
struct FileStatistics {
    schema_version: u32,
    path: String,
    axes: Vec<u64>,
    stored_format: String,
    header_conformant: bool,
    header_diagnostics: usize,
    total_samples: usize,
    usable_samples: usize,
    undefined_samples: usize,
    non_finite_samples: usize,
    minimum: f64,
    maximum: f64,
    mean: f64,
    population_standard_deviation: f64,
    sample_standard_deviation: Option<f64>,
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

    match run(&config) {
        Ok(0) => ExitCode::SUCCESS,
        Ok(failures) => {
            eprintln!("{failures} FITS input(s) could not be measured");
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("statistics scan failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(config: &Config) -> Result<usize, Box<dyn Error>> {
    let stdout = io::stdout();
    let mut output = BufWriter::new(stdout.lock());
    if config.output_format == OutputFormat::Human {
        writeln!(
            output,
            "path\taxes\tformat\tconformant\tdiagnostics\ttotal\tusable\tundefined\tnon_finite\tminimum\tmaximum\tmean\tpopulation_stddev\tsample_stddev"
        )?;
    }

    let mut processed = 0_usize;
    let mut failures = 0_usize;
    for root in &config.roots {
        scan_root(root, config, &mut processed, &mut failures, &mut output)?;
        if limit_reached(processed, config.limit) {
            break;
        }
    }
    output.flush()?;
    Ok(failures)
}

fn scan_root<W: Write>(
    root: &Path,
    config: &Config,
    processed: &mut usize,
    failures: &mut usize,
    output: &mut W,
) -> Result<(), Box<dyn Error>> {
    let metadata = fs::symlink_metadata(root)?;
    let mut pending = vec![(root.to_owned(), metadata.file_type())];
    while let Some((path, file_type)) = pending.pop() {
        if limit_reached(*processed, config.limit) {
            break;
        }
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            let entries = match fs::read_dir(&path) {
                Ok(entries) => entries,
                Err(error) => {
                    increment(failures, "failure count")?;
                    eprintln!("{}: {error}", path.display());
                    continue;
                }
            };
            let mut children = Vec::new();
            for entry in entries {
                match entry.and_then(|entry| {
                    let file_type = entry.file_type()?;
                    Ok((entry.path(), file_type))
                }) {
                    Ok(child) => children.push(child),
                    Err(error) => {
                        increment(failures, "failure count")?;
                        eprintln!("{}: {error}", path.display());
                    }
                }
            }
            children.sort_by(|left, right| left.0.cmp(&right.0));
            pending.extend(children.into_iter().rev());
        } else if file_type.is_file() && is_fits_path(&path) {
            *processed = processed
                .checked_add(1)
                .ok_or_else(|| io::Error::other("processed-file count overflow"))?;
            match analyze_file(&path, config) {
                Ok(report) => write_report(output, &report, config.output_format)?,
                Err(error) => {
                    increment(failures, "failure count")?;
                    eprintln!("{}: {error}", path.display());
                }
            }
        }
    }
    Ok(())
}

fn analyze_file(path: &Path, config: &Config) -> Result<FileStatistics, Box<dyn Error>> {
    let file = File::open(path)?;
    let mut reader = PrimaryImageReader::open(file, HeaderReadOptions::default())?;
    if !reader.report().is_accepted(config.validation_mode) {
        return Err("FITS header is not accepted in strict mode".into());
    }

    let axes = reader.descriptor().axes().to_vec();
    let stored_format = stored_format_name(reader.descriptor().sample_format()).to_owned();
    let header_conformant = reader.report().is_conformant();
    let header_diagnostics = reader.report().diagnostics().len();
    let statistics = primary_image_statistics(&mut reader, config.chunk_samples)?;
    let moments = statistics.moments();

    Ok(FileStatistics {
        schema_version: 1,
        path: path
            .to_str()
            .ok_or("FITS path is not valid UTF-8")?
            .to_owned(),
        axes,
        stored_format,
        header_conformant,
        header_diagnostics,
        total_samples: moments.total_samples(),
        usable_samples: moments.usable_samples(),
        undefined_samples: statistics.undefined_samples(),
        non_finite_samples: statistics.non_finite_samples(),
        minimum: moments.minimum(),
        maximum: moments.maximum(),
        mean: moments.mean(),
        population_standard_deviation: moments.population_standard_deviation(),
        sample_standard_deviation: moments.sample_standard_deviation(),
    })
}

fn write_report<W: Write>(
    output: &mut W,
    report: &FileStatistics,
    format: OutputFormat,
) -> Result<(), Box<dyn Error>> {
    match format {
        OutputFormat::Human => {
            let axes = report
                .axes
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join("x");
            let sample_deviation = report
                .sample_standard_deviation
                .map_or_else(|| "undefined".to_owned(), |value| value.to_string());
            writeln!(
                output,
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                report.path,
                axes,
                report.stored_format,
                report.header_conformant,
                report.header_diagnostics,
                report.total_samples,
                report.usable_samples,
                report.undefined_samples,
                report.non_finite_samples,
                report.minimum,
                report.maximum,
                report.mean,
                report.population_standard_deviation,
                sample_deviation,
            )?;
        }
        OutputFormat::JsonLines => {
            serde_json::to_writer(&mut *output, report)?;
            output.write_all(b"\n")?;
        }
    }
    Ok(())
}

fn parse_args(arguments: impl IntoIterator<Item = OsString>) -> Result<Config, String> {
    let mut roots = Vec::new();
    let mut validation_mode = ValidationMode::Tolerant;
    let mut output_format = OutputFormat::Human;
    let mut chunk_samples = NonZeroUsize::new(DEFAULT_STATISTICS_CHUNK_SAMPLES)
        .ok_or_else(|| "internal default chunk size is zero".to_owned())?;
    let mut limit = None;
    let mut arguments = arguments.into_iter();

    while let Some(argument) = arguments.next() {
        if argument == "--strict" {
            validation_mode = ValidationMode::Strict;
        } else if argument == "--jsonl" {
            output_format = OutputFormat::JsonLines;
        } else if argument == "--chunk-samples" {
            chunk_samples = parse_positive(&mut arguments, "--chunk-samples")?;
        } else if argument == "--limit" {
            limit = Some(parse_positive(&mut arguments, "--limit")?.get());
        } else if argument == "--help" || argument == "-h" {
            return Err(USAGE.to_owned());
        } else if argument.to_string_lossy().starts_with('-') {
            return Err(format!("unknown option: {}", argument.to_string_lossy()));
        } else {
            roots.push(PathBuf::from(argument));
        }
    }

    if roots.is_empty() {
        return Err("missing FITS file or directory path".to_owned());
    }
    Ok(Config {
        roots,
        validation_mode,
        output_format,
        chunk_samples,
        limit,
    })
}

fn parse_positive(
    arguments: &mut impl Iterator<Item = OsString>,
    option: &str,
) -> Result<NonZeroUsize, String> {
    let value = arguments
        .next()
        .ok_or_else(|| format!("{option} requires a positive integer"))?;
    let value = value
        .to_str()
        .ok_or_else(|| format!("{option} must be valid UTF-8"))?;
    let parsed = value
        .parse::<usize>()
        .ok()
        .and_then(NonZeroUsize::new)
        .ok_or_else(|| format!("{option} requires a positive integer"))?;
    Ok(parsed)
}

fn limit_reached(processed: usize, limit: Option<usize>) -> bool {
    limit.is_some_and(|limit| processed >= limit)
}

fn increment(counter: &mut usize, label: &str) -> io::Result<()> {
    *counter = counter
        .checked_add(1)
        .ok_or_else(|| io::Error::other(format!("{label} overflow")))?;
    Ok(())
}

const fn stored_format_name(format: StoredSampleFormat) -> &'static str {
    match format {
        StoredSampleFormat::Unsigned8 => "unsigned8",
        StoredSampleFormat::Signed16 => "signed16",
        StoredSampleFormat::Signed32 => "signed32",
        StoredSampleFormat::Signed64 => "signed64",
        StoredSampleFormat::Float32 => "float32",
        StoredSampleFormat::Float64 => "float64",
    }
}

fn is_fits_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("fit")
                || extension.eq_ignore_ascii_case("fits")
                || extension.eq_ignore_ascii_case("fts")
        })
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use super::*;

    #[test]
    fn parses_batch_and_machine_readable_options() -> Result<(), String> {
        let config = parse_args([
            OsString::from("--strict"),
            OsString::from("--jsonl"),
            OsString::from("--chunk-samples"),
            OsString::from("1024"),
            OsString::from("--limit"),
            OsString::from("5"),
            OsString::from("lights"),
            OsString::from("flats"),
        ])?;

        assert_eq!(
            config,
            Config {
                roots: vec![PathBuf::from("lights"), PathBuf::from("flats")],
                validation_mode: ValidationMode::Strict,
                output_format: OutputFormat::JsonLines,
                chunk_samples: NonZeroUsize::new(1024)
                    .ok_or_else(|| "test chunk is zero".to_owned())?,
                limit: Some(5),
            }
        );
        Ok(())
    }

    #[test]
    fn rejects_zero_and_missing_numeric_values() {
        assert!(parse_args([OsString::from("--limit"), OsString::from("0")]).is_err());
        assert!(parse_args([OsString::from("--chunk-samples")]).is_err());
    }

    #[test]
    fn accepts_common_fits_extensions_case_insensitively() {
        for path in ["frame.fit", "frame.FITS", "frame.FtS"] {
            assert!(is_fits_path(Path::new(path)));
        }
        assert!(!is_fits_path(Path::new("frame.xisf")));
    }

    #[test]
    fn json_lines_report_is_versioned_and_newline_terminated() -> Result<(), Box<dyn StdError>> {
        let report = FileStatistics {
            schema_version: 1,
            path: "frame.fits".to_owned(),
            axes: vec![4, 3],
            stored_format: "signed16".to_owned(),
            header_conformant: true,
            header_diagnostics: 0,
            total_samples: 12,
            usable_samples: 12,
            undefined_samples: 0,
            non_finite_samples: 0,
            minimum: 1.0,
            maximum: 12.0,
            mean: 6.5,
            population_standard_deviation: 3.452_052_529_534_663,
            sample_standard_deviation: Some(3.605_551_275_463_989),
        };
        let mut encoded = Vec::new();

        write_report(&mut encoded, &report, OutputFormat::JsonLines)?;
        assert_eq!(encoded.last(), Some(&b'\n'));
        let value: serde_json::Value = serde_json::from_slice(&encoded)?;
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["path"], "frame.fits");
        assert_eq!(value["axes"], serde_json::json!([4, 3]));
        assert_eq!(value["stored_format"], "signed16");
        assert_eq!(value["usable_samples"], 12);
        Ok(())
    }
}
