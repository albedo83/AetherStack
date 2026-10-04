//! Opt-in end-to-end session-import profiling for private FITS corpora.
//!
//! Output contains aggregate counts and timings only. Source paths, file names,
//! fingerprints, normalized metadata, and image contents are never printed.

use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use aether_session::{
    ClassificationPolicy, DirectoryManifestOptions, DirectoryScanTimings,
    generate_manifest_from_directory,
};

const DEFAULT_PASSES: usize = 3;
const MAX_PASSES: usize = 20;
const USAGE: &str = "Usage: aether-import-bench [--passes N] <corpus-directory>";

#[derive(Debug, Eq, PartialEq)]
struct Config {
    root: PathBuf,
    passes: usize,
}

#[derive(Debug, Eq, PartialEq)]
struct ImportSeal {
    manifest_sha256: String,
    source_files: usize,
    source_bytes: u64,
    failures: usize,
    unassigned_sources: usize,
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
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("benchmark failed: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(config: &Config) -> Result<(), String> {
    println!("AetherStack session import benchmark");
    println!("passes: {}", config.passes);
    println!("classification_policy: prefer_directory");

    let mut reference = None;
    for pass in 1..=config.passes {
        let options = DirectoryManifestOptions {
            classification_policy: ClassificationPolicy::PreferDirectory,
            ..DirectoryManifestOptions::default()
        };
        let report = generate_manifest_from_directory(&config.root, options)
            .map_err(|error| format!("import pass {pass} failed: {error}"))?;
        let seal = ImportSeal {
            manifest_sha256: report
                .manifest()
                .canonical_sha256()
                .map_err(|error| format!("cannot seal import pass {pass}: {error}"))?,
            source_files: report.manifest().files().len(),
            source_bytes: report.total_source_bytes(),
            failures: report.failures().len(),
            unassigned_sources: report.unassigned_sources().len(),
        };
        if let Some(expected) = &reference {
            if expected != &seal {
                return Err(format!(
                    "aggregate import evidence changed between pass 1 and pass {pass}"
                ));
            }
        } else {
            println!("source_files: {}", seal.source_files);
            println!("source_bytes: {}", seal.source_bytes);
            println!("failures: {}", seal.failures);
            println!("unassigned_sources: {}", seal.unassigned_sources);
            reference = Some(seal);
        }
        print_timings(pass, report.timings());
    }

    println!("evidence_stable_across_passes: true");
    Ok(())
}

fn print_timings(pass: usize, timings: DirectoryScanTimings) {
    println!("pass_{pass}_total_seconds: {:.6}", seconds(timings.total()));
    println!(
        "pass_{pass}_filesystem_and_overhead_seconds: {:.6}",
        seconds(timings.filesystem_and_overhead())
    );
    println!(
        "pass_{pass}_initial_headers_seconds: {:.6}",
        seconds(timings.initial_headers())
    );
    println!(
        "pass_{pass}_fingerprints_seconds: {:.6}",
        seconds(timings.fingerprints())
    );
    println!(
        "pass_{pass}_verification_headers_seconds: {:.6}",
        seconds(timings.verification_headers())
    );
    println!(
        "pass_{pass}_source_finalization_seconds: {:.6}",
        seconds(timings.source_finalization())
    );
    println!(
        "pass_{pass}_manifest_assembly_seconds: {:.6}",
        seconds(timings.manifest_assembly())
    );
}

fn seconds(duration: Duration) -> f64 {
    duration.as_secs_f64()
}

fn parse_args(arguments: impl IntoIterator<Item = OsString>) -> Result<Config, String> {
    let mut root = None;
    let mut passes = DEFAULT_PASSES;
    let mut arguments = arguments.into_iter();

    while let Some(argument) = arguments.next() {
        if argument == "--passes" {
            passes = parse_positive_usize(arguments.next(), "--passes")?;
            if passes > MAX_PASSES {
                return Err(format!("--passes must not exceed {MAX_PASSES}"));
            }
        } else if argument.to_string_lossy().starts_with('-') {
            return Err(format!("unknown option: {}", argument.to_string_lossy()));
        } else if root.replace(PathBuf::from(argument)).is_some() {
            return Err("exactly one corpus directory is required".to_owned());
        }
    }

    Ok(Config {
        root: root.ok_or_else(|| "a corpus directory is required".to_owned())?,
        passes,
    })
}

fn parse_positive_usize(value: Option<OsString>, option: &str) -> Result<usize, String> {
    let value = value.ok_or_else(|| format!("{option} requires a positive integer"))?;
    let value = value
        .to_str()
        .ok_or_else(|| format!("{option} must be valid UTF-8"))?;
    let parsed = value
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
    fn parses_bounded_options() {
        assert_eq!(
            parse_args([
                OsString::from("--passes"),
                OsString::from("5"),
                OsString::from("corpus"),
            ]),
            Ok(Config {
                root: PathBuf::from("corpus"),
                passes: 5,
            })
        );
    }

    #[test]
    fn rejects_missing_duplicate_unknown_and_unbounded_arguments() {
        assert!(parse_args([]).is_err());
        assert!(parse_args([OsString::from("a"), OsString::from("b")]).is_err());
        assert!(parse_args([OsString::from("--unknown"), OsString::from("a")]).is_err());
        assert!(
            parse_args([
                OsString::from("--passes"),
                OsString::from("21"),
                OsString::from("corpus"),
            ])
            .is_err()
        );
        assert!(
            parse_args([
                OsString::from("--passes"),
                OsString::from("0"),
                OsString::from("corpus"),
            ])
            .is_err()
        );
    }
}
