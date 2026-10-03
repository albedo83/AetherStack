//! Opt-in sequential fingerprint throughput measurement for private FITS corpora.
//!
//! The command reports aggregate counts and timings only. It deliberately keeps
//! source paths, file names, fingerprints, and image contents out of its output.

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use aether_session::fingerprint_reader_with_progress;

const DEFAULT_PASSES: usize = 3;
const MAX_PASSES: usize = 20;
const PROGRESS_INTERVAL_BYTES: u64 = 8 * 1_024 * 1_024;
const USAGE: &str = "Usage: aether-fingerprint-bench [--passes N] [--limit N] <corpus-directory>";

#[derive(Debug, Eq, PartialEq)]
struct Config {
    root: PathBuf,
    passes: usize,
    limit: Option<usize>,
}

#[derive(Debug)]
struct Corpus {
    files: Vec<PathBuf>,
    skipped_symbolic_links: u64,
}

#[derive(Debug, Eq, PartialEq)]
struct Seal {
    byte_length: u64,
    sha256: String,
}

#[derive(Debug)]
struct PassMeasurement {
    byte_length: u64,
    elapsed: Duration,
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
    let corpus = collect_fits_files(&config.root, config.limit)
        .map_err(|error| format!("cannot enumerate the corpus: {error}"))?;
    if corpus.files.is_empty() {
        return Err("the corpus contains no regular FITS files".to_owned());
    }

    println!("AetherStack sequential fingerprint benchmark");
    println!("files: {}", corpus.files.len());
    println!("passes: {}", config.passes);
    println!("skipped_symbolic_links: {}", corpus.skipped_symbolic_links);
    println!("buffer_bytes: {}", aether_session::FINGERPRINT_BUFFER_BYTES);

    let mut reference = None;
    let mut measurements = Vec::with_capacity(config.passes);
    for pass in 0..config.passes {
        let (seals, measurement) = measure_pass(&corpus.files)?;
        if let Some(expected) = &reference {
            if expected != &seals {
                return Err(format!(
                    "source evidence changed between pass 1 and pass {}",
                    pass + 1
                ));
            }
        } else {
            reference = Some(seals);
        }
        println!(
            "pass_{}: bytes={} seconds={:.6} mib_per_second={:.2}",
            pass + 1,
            measurement.byte_length,
            measurement.elapsed.as_secs_f64(),
            mebibytes_per_second(&measurement)
        );
        measurements.push(measurement);
    }

    let mut throughput = measurements
        .iter()
        .map(mebibytes_per_second)
        .collect::<Vec<_>>();
    throughput.sort_by(f64::total_cmp);
    let Some(median) = throughput.get(throughput.len() / 2) else {
        return Err("no throughput measurement was produced".to_owned());
    };
    println!("median_mib_per_second: {median:.2}");
    println!("evidence_stable_across_passes: true");
    Ok(())
}

fn measure_pass(paths: &[PathBuf]) -> Result<(Vec<Seal>, PassMeasurement), String> {
    let started = Instant::now();
    let mut total_bytes = 0_u64;
    let mut seals = Vec::with_capacity(paths.len());

    for (index, path) in paths.iter().enumerate() {
        let metadata = fs::symlink_metadata(path)
            .map_err(|error| format!("cannot inspect source #{}: {error}", index + 1))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(format!("source #{} is no longer a regular file", index + 1));
        }
        let file = File::open(path)
            .map_err(|error| format!("cannot open source #{}: {error}", index + 1))?;
        let opened_metadata = file
            .metadata()
            .map_err(|error| format!("cannot inspect open source #{}: {error}", index + 1))?;
        if !opened_metadata.is_file() || opened_metadata.len() != metadata.len() {
            return Err(format!(
                "source #{} changed while it was being opened",
                index + 1
            ));
        }
        let Some(progress_interval) = NonZeroU64::new(PROGRESS_INTERVAL_BYTES) else {
            return Err("fingerprint progress interval must be non-zero".to_owned());
        };
        let mut reader = file;
        let fingerprint = fingerprint_reader_with_progress(&mut reader, progress_interval, |_| {})
            .map_err(|error| format!("cannot fingerprint source #{}: {error}", index + 1))?;
        if fingerprint.byte_length() != opened_metadata.len() {
            return Err(format!(
                "source #{} changed while it was being read",
                index + 1
            ));
        }
        total_bytes = total_bytes
            .checked_add(fingerprint.byte_length())
            .ok_or_else(|| "aggregate corpus size exceeds 64-bit accounting".to_owned())?;
        seals.push(Seal {
            byte_length: fingerprint.byte_length(),
            sha256: fingerprint.sha256().to_owned(),
        });
    }

    Ok((
        seals,
        PassMeasurement {
            byte_length: total_bytes,
            elapsed: started.elapsed(),
        },
    ))
}

fn collect_fits_files(root: &Path, limit: Option<usize>) -> io::Result<Corpus> {
    let metadata = fs::symlink_metadata(root)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the corpus root must not be a symbolic link",
        ));
    }
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the corpus root must be a directory",
        ));
    }

    let mut directories = vec![root.to_owned()];
    let mut files = Vec::new();
    let mut skipped_symbolic_links = 0_u64;
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                skipped_symbolic_links = skipped_symbolic_links.saturating_add(1);
            } else if file_type.is_dir() {
                directories.push(entry.path());
            } else if file_type.is_file() && is_fits_path(&entry.path()) {
                files.push(entry.path());
            }
        }
    }
    files.sort();
    if let Some(limit) = limit {
        files.truncate(limit);
    }

    Ok(Corpus {
        files,
        skipped_symbolic_links,
    })
}

fn is_fits_path(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("fits")
                || extension.eq_ignore_ascii_case("fit")
                || extension.eq_ignore_ascii_case("fts")
        })
}

fn mebibytes_per_second(measurement: &PassMeasurement) -> f64 {
    let seconds = measurement.elapsed.as_secs_f64();
    if seconds == 0.0 {
        return f64::INFINITY;
    }
    measurement.byte_length as f64 / (1024.0 * 1024.0) / seconds
}

fn parse_args(arguments: impl IntoIterator<Item = OsString>) -> Result<Config, String> {
    let mut root = None;
    let mut passes = DEFAULT_PASSES;
    let mut limit = None;
    let mut arguments = arguments.into_iter();

    while let Some(argument) = arguments.next() {
        if argument == "--passes" {
            passes = parse_positive_usize(arguments.next(), "--passes")?;
            if passes > MAX_PASSES {
                return Err(format!("--passes must not exceed {MAX_PASSES}"));
            }
        } else if argument == "--limit" {
            limit = Some(parse_positive_usize(arguments.next(), "--limit")?);
        } else if argument.to_string_lossy().starts_with('-') {
            return Err(format!("unknown option: {}", argument.to_string_lossy()));
        } else if root.replace(PathBuf::from(argument)).is_some() {
            return Err("exactly one corpus directory is required".to_owned());
        }
    }

    let root = root.ok_or_else(|| "a corpus directory is required".to_owned())?;
    Ok(Config {
        root,
        passes,
        limit,
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
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new() -> io::Result<Self> {
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = env::temp_dir().join(format!(
                "aether-fingerprint-bench-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path)?;
            Ok(Self { path })
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ignored = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn parses_bounded_benchmark_options() {
        let result = parse_args([
            OsString::from("--passes"),
            OsString::from("5"),
            OsString::from("--limit"),
            OsString::from("12"),
            OsString::from("corpus"),
        ]);

        assert_eq!(
            result,
            Ok(Config {
                root: PathBuf::from("corpus"),
                passes: 5,
                limit: Some(12),
            })
        );
    }

    #[test]
    fn rejects_missing_duplicate_and_unbounded_arguments() {
        assert!(parse_args([]).is_err());
        assert!(parse_args([OsString::from("a"), OsString::from("b")]).is_err());
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
                OsString::from("--limit"),
                OsString::from("0"),
                OsString::from("corpus"),
            ])
            .is_err()
        );
    }

    #[test]
    fn recognizes_fits_extensions_without_accepting_similar_names() {
        assert!(is_fits_path(Path::new("frame.FITS")));
        assert!(is_fits_path(Path::new("frame.fit")));
        assert!(is_fits_path(Path::new("frame.fts")));
        assert!(!is_fits_path(Path::new("frame.fits.json")));
        assert!(!is_fits_path(Path::new("frame.xisf")));
    }

    #[test]
    fn discovers_nested_regular_fits_in_deterministic_bounded_order() -> io::Result<()> {
        let directory = TestDirectory::new()?;
        let nested = directory.path.join("nested");
        fs::create_dir(&nested)?;
        fs::write(directory.path.join("z.fit"), b"z")?;
        fs::write(nested.join("a.FITS"), b"a")?;
        fs::write(directory.path.join("ignored.xisf"), b"ignored")?;

        let corpus = collect_fits_files(&directory.path, None)?;
        assert_eq!(corpus.files.len(), 2);
        assert!(corpus.files.windows(2).all(|pair| pair[0] < pair[1]));
        let limited = collect_fits_files(&directory.path, Some(1))?;
        assert_eq!(limited.files.len(), 1);
        assert_eq!(limited.files.first(), corpus.files.first());
        assert!(collect_fits_files(&directory.path.join("z.fit"), None).is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn counts_but_never_follows_symbolic_links() -> io::Result<()> {
        use std::os::unix::fs::symlink;

        let directory = TestDirectory::new()?;
        let outside = TestDirectory::new()?;
        fs::write(outside.path.join("private.fits"), b"private")?;
        symlink(&outside.path, directory.path.join("linked-corpus"))?;
        symlink(
            outside.path.join("private.fits"),
            directory.path.join("linked.fits"),
        )?;

        let corpus = collect_fits_files(&directory.path, None)?;
        assert!(corpus.files.is_empty());
        assert_eq!(corpus.skipped_symbolic_links, 2);
        Ok(())
    }
}
