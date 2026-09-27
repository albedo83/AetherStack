//! Command-line bridge for one privacy-preserving FITS registration diagnostic.

use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use aether_register::diagnose_paths;

const USAGE: &str = "Usage: aether-register [--compact] <source.fits> <reference.fits>";

#[derive(Debug, Eq, PartialEq)]
struct Config {
    source: PathBuf,
    reference: PathBuf,
    compact: bool,
}

fn main() -> ExitCode {
    let config = match parse_args(env::args_os().skip(1)) {
        Ok(config) => config,
        Err(message) => {
            eprintln!("{message}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let report = match diagnose_paths(&config.source, &config.reference) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("registration diagnostic failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    let serialized = if config.compact {
        serde_json::to_string(&report)
    } else {
        serde_json::to_string_pretty(&report)
    };
    match serialized {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("registration diagnostic serialization failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn parse_args(arguments: impl IntoIterator<Item = OsString>) -> Result<Config, String> {
    let mut compact = false;
    let mut paths = Vec::new();
    for argument in arguments {
        if argument == "--compact" {
            compact = true;
        } else if argument == "--help" || argument == "-h" {
            return Err("A source and reference raw CFA FITS pair is required.".to_owned());
        } else if argument.to_string_lossy().starts_with('-') {
            return Err("Unknown option.".to_owned());
        } else {
            paths.push(PathBuf::from(argument));
        }
    }
    if paths.len() != 2 {
        return Err("Exactly two FITS inputs are required.".to_owned());
    }
    let reference = paths
        .pop()
        .ok_or_else(|| "Missing reference input.".to_owned())?;
    let source = paths
        .pop()
        .ok_or_else(|| "Missing source input.".to_owned())?;
    Ok(Config {
        source,
        reference,
        compact,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_two_inputs_and_compact_mode() {
        let config = parse_args(["--compact", "source.fits", "reference.fits"].map(OsString::from));
        assert_eq!(
            config,
            Ok(Config {
                source: PathBuf::from("source.fits"),
                reference: PathBuf::from("reference.fits"),
                compact: true,
            })
        );
    }

    #[test]
    fn rejects_options_and_wrong_arity() {
        assert!(parse_args([OsString::from("--unknown")]).is_err());
        assert!(parse_args([OsString::from("only-one.fits")]).is_err());
    }
}
