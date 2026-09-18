//! Deterministic, memory-bounded inventory of a FITS corpus.
//!
//! The binary walks files without following symbolic links, reads only the
//! primary header, and aggregates useful properties without exposing individual
//! paths in its default output.

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use aether_fits::{
    BLOCK_SIZE, DiagnosticCode, Header, HeaderReadOptions, ValidationMode, read_primary_header,
};
use aether_metadata::{FrameType, MetadataIssueCode, normalize_header};
use aether_session::{ClassificationSource, FrameClassification, classify_frame};

const USAGE: &str = "Usage: aether-inspect [--strict] [--limit N] [--examples N] <path>";

#[derive(Debug, Eq, PartialEq)]
struct Config {
    root: PathBuf,
    mode: ValidationMode,
    limit: Option<usize>,
    examples: usize,
}

#[derive(Debug, Default)]
struct Summary {
    files_discovered: u64,
    headers_read: u64,
    strict_conformant: u64,
    tolerated_nonconformant: u64,
    structural_errors: u64,
    traversal_errors: u64,
    missing_instrument: u64,
    bitpix: BTreeMap<String, u64>,
    dimensions: BTreeMap<String, u64>,
    instruments: BTreeMap<String, u64>,
    priority_cameras: BTreeMap<String, u64>,
    unsupported_cameras: BTreeMap<String, u64>,
    bayer_patterns: BTreeMap<String, u64>,
    software: BTreeMap<String, u64>,
    frame_types: BTreeMap<String, u64>,
    resolved_frame_types: BTreeMap<String, u64>,
    classification_conflicts: u64,
    classification_conflict_kinds: BTreeMap<String, u64>,
    unresolved_frame_types: u64,
    diagnostics: BTreeMap<DiagnosticCode, u64>,
    metadata_issues: BTreeMap<MetadataIssueCode, u64>,
    nonconformant_examples: Vec<PathBuf>,
    structural_error_examples: Vec<PathBuf>,
    conflict_examples: Vec<PathBuf>,
}

impl Summary {
    fn record_header(&mut self, path: &Path, report: &aether_fits::HeaderReport, config: &Config) {
        self.headers_read += 1;
        if report.is_conformant() {
            self.strict_conformant += 1;
        } else {
            self.tolerated_nonconformant += 1;
            push_example(&mut self.nonconformant_examples, path, config);
        }

        for diagnostic in report.diagnostics() {
            *self.diagnostics.entry(diagnostic.code()).or_default() += 1;
        }

        let header = report.header();
        let metadata = normalize_header(header);
        let classification = classify_frame(path, &metadata);
        if classification.has_conflict() {
            self.classification_conflicts += 1;
            push_example(&mut self.conflict_examples, path, config);
            *self
                .classification_conflict_kinds
                .entry(classification_key(&classification))
                .or_default() += 1;
        } else if let Some(frame_type) = classification.resolved() {
            *self
                .resolved_frame_types
                .entry(frame_type_key(frame_type))
                .or_default() += 1;
        } else {
            self.unresolved_frame_types += 1;
        }
        for issue in &metadata.issues {
            *self.metadata_issues.entry(issue.code()).or_default() += 1;
        }
        if let Some(camera) = &metadata.camera {
            let target = if camera.value().is_priority_supported() {
                &mut self.priority_cameras
            } else {
                &mut self.unsupported_cameras
            };
            *target
                .entry(camera.value().canonical_name().to_owned())
                .or_default() += 1;
        }

        record_optional_integer(&mut self.bitpix, header.integer("BITPIX"));
        *self.dimensions.entry(dimension_key(header)).or_default() += 1;

        if let Some(instrument) = header.string("INSTRUME") {
            *self.instruments.entry(instrument.to_owned()).or_default() += 1;
        } else if let Some(camera) = header.string("CAMERA") {
            *self.instruments.entry(camera.to_owned()).or_default() += 1;
        } else {
            self.missing_instrument += 1;
        }

        record_optional_string(&mut self.bayer_patterns, header.string("BAYERPAT"));
        record_first_string(
            &mut self.software,
            header,
            &["SWCREATE", "SOFTWARE", "CREATOR"],
        );
        record_first_string(
            &mut self.frame_types,
            header,
            &["IMAGETYP", "FRAME", "FRAMETYP"],
        );
    }

    fn print(&self, config: &Config) {
        println!("AetherStack FITS inventory");
        println!("root: {}", config.root.display());
        println!("mode: {:?}", config.mode);
        println!("files_discovered: {}", self.files_discovered);
        println!("headers_read: {}", self.headers_read);
        println!("strict_conformant: {}", self.strict_conformant);
        println!("tolerated_nonconformant: {}", self.tolerated_nonconformant);
        println!("structural_errors: {}", self.structural_errors);
        println!("traversal_errors: {}", self.traversal_errors);
        println!("missing_instrument: {}", self.missing_instrument);
        println!(
            "classification_conflicts: {}",
            self.classification_conflicts
        );
        println!("unresolved_frame_types: {}", self.unresolved_frame_types);
        print_map("bitpix", &self.bitpix);
        print_map("dimensions", &self.dimensions);
        print_map("instruments", &self.instruments);
        print_map("priority_cameras", &self.priority_cameras);
        print_map("unsupported_cameras", &self.unsupported_cameras);
        print_map("bayer_patterns", &self.bayer_patterns);
        print_map("software", &self.software);
        print_map("frame_types", &self.frame_types);
        print_map("resolved_frame_types", &self.resolved_frame_types);
        print_map(
            "classification_conflict_kinds",
            &self.classification_conflict_kinds,
        );
        print_diagnostics(&self.diagnostics);
        print_metadata_issues(&self.metadata_issues);
        print_examples("nonconformant_examples", &self.nonconformant_examples);
        print_examples("structural_error_examples", &self.structural_error_examples);
        print_examples("classification_conflict_examples", &self.conflict_examples);
    }
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

    let summary = match scan(&config) {
        Ok(summary) => summary,
        Err(error) => {
            eprintln!("cannot inspect {}: {error}", config.root.display());
            return ExitCode::FAILURE;
        }
    };
    summary.print(&config);

    if config.mode == ValidationMode::Strict
        && (summary.tolerated_nonconformant > 0 || summary.structural_errors > 0)
    {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn parse_args(arguments: impl IntoIterator<Item = OsString>) -> Result<Config, String> {
    let mut root = None;
    let mut mode = ValidationMode::Tolerant;
    let mut limit = None;
    let mut examples = 0;
    let mut arguments = arguments.into_iter();

    while let Some(argument) = arguments.next() {
        if argument == "--strict" {
            mode = ValidationMode::Strict;
        } else if argument == "--limit" {
            let Some(value) = arguments.next() else {
                return Err("--limit requires a positive integer".to_owned());
            };
            let Some(value) = value.to_str() else {
                return Err("--limit must be valid UTF-8".to_owned());
            };
            let parsed = value
                .parse::<usize>()
                .map_err(|_| "--limit requires a positive integer".to_owned())?;
            if parsed == 0 {
                return Err("--limit must be greater than zero".to_owned());
            }
            limit = Some(parsed);
        } else if argument == "--examples" {
            let Some(value) = arguments.next() else {
                return Err("--examples requires a non-negative integer".to_owned());
            };
            let Some(value) = value.to_str() else {
                return Err("--examples must be valid UTF-8".to_owned());
            };
            examples = value
                .parse::<usize>()
                .map_err(|_| "--examples requires a non-negative integer".to_owned())?;
        } else if argument == "--help" || argument == "-h" {
            return Err(USAGE.to_owned());
        } else if argument.to_string_lossy().starts_with('-') {
            return Err(format!("unknown option: {}", argument.to_string_lossy()));
        } else if root.replace(PathBuf::from(argument)).is_some() {
            return Err("only one input path can be inspected at a time".to_owned());
        }
    }

    let Some(root) = root else {
        return Err("missing FITS file or directory path".to_owned());
    };
    Ok(Config {
        root,
        mode,
        limit,
        examples,
    })
}

fn scan(config: &Config) -> io::Result<Summary> {
    let root_metadata = fs::symlink_metadata(&config.root)?;
    let mut summary = Summary::default();
    let mut pending = vec![(config.root.clone(), root_metadata.file_type())];

    while let Some((path, file_type)) = pending.pop() {
        if limit_reached(&summary, config.limit) {
            break;
        }
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            enqueue_directory(&path, &mut pending, &mut summary);
        } else if file_type.is_file() && is_fits_path(&path) {
            inspect_file(&path, &mut summary, config);
        }
    }

    Ok(summary)
}

fn enqueue_directory(
    directory: &Path,
    pending: &mut Vec<(PathBuf, fs::FileType)>,
    summary: &mut Summary,
) {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(_) => {
            summary.traversal_errors += 1;
            return;
        }
    };
    let mut children = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            summary.traversal_errors += 1;
            continue;
        };
        match entry.file_type() {
            Ok(file_type) => children.push((entry.path(), file_type)),
            Err(_) => summary.traversal_errors += 1,
        }
    }
    children.sort_by(|left, right| left.0.cmp(&right.0));
    pending.extend(children.into_iter().rev());
}

fn inspect_file(path: &Path, summary: &mut Summary, config: &Config) {
    summary.files_discovered += 1;
    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => {
            summary.structural_errors += 1;
            push_example(&mut summary.structural_error_examples, path, config);
            return;
        }
    };
    let mut reader = BufReader::with_capacity(BLOCK_SIZE, file);
    match read_primary_header(&mut reader, HeaderReadOptions::default()) {
        Ok(report) => summary.record_header(path, &report, config),
        Err(_) => {
            summary.structural_errors += 1;
            push_example(&mut summary.structural_error_examples, path, config);
        }
    }
}

fn limit_reached(summary: &Summary, limit: Option<usize>) -> bool {
    limit.is_some_and(|limit| {
        usize::try_from(summary.files_discovered).is_ok_and(|count| count >= limit)
    })
}

fn is_fits_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("fit") || extension.eq_ignore_ascii_case("fits")
        })
}

fn record_optional_integer(map: &mut BTreeMap<String, u64>, value: Option<i64>) {
    let key = value.map_or_else(|| "[missing]".to_owned(), |value| value.to_string());
    *map.entry(key).or_default() += 1;
}

fn record_optional_string(map: &mut BTreeMap<String, u64>, value: Option<&str>) {
    let key = value.unwrap_or("[missing]").to_owned();
    *map.entry(key).or_default() += 1;
}

fn record_first_string(map: &mut BTreeMap<String, u64>, header: &Header, keywords: &[&str]) {
    let value = keywords
        .iter()
        .find_map(|keyword| header.string(keyword))
        .unwrap_or("[missing]")
        .to_owned();
    *map.entry(value).or_default() += 1;
}

fn dimension_key(header: &Header) -> String {
    let Some(axis_count) = header
        .integer("NAXIS")
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value <= 9)
    else {
        return "[missing_or_unsupported]".to_owned();
    };
    if axis_count == 0 {
        return "0 axes".to_owned();
    }

    let dimensions = (1..=axis_count)
        .map(|axis| {
            header
                .integer(&format!("NAXIS{axis}"))
                .map_or_else(|| "?".to_owned(), |value| value.to_string())
        })
        .collect::<Vec<_>>()
        .join("x");
    format!("{dimensions} ({axis_count} axes)")
}

fn print_map(title: &str, values: &BTreeMap<String, u64>) {
    println!("{title}:");
    for (value, count) in values {
        println!("  {count:>8}  {value}");
    }
}

fn print_diagnostics(values: &BTreeMap<DiagnosticCode, u64>) {
    println!("diagnostics:");
    for (diagnostic, count) in values {
        println!("  {count:>8}  {diagnostic}");
    }
}

fn print_metadata_issues(values: &BTreeMap<MetadataIssueCode, u64>) {
    println!("metadata_issues:");
    for (issue, count) in values {
        println!("  {count:>8}  {issue:?}");
    }
}

fn push_example(examples: &mut Vec<PathBuf>, path: &Path, config: &Config) {
    if examples.len() >= config.examples {
        return;
    }
    let relative = path.strip_prefix(&config.root).unwrap_or(path);
    examples.push(relative.to_owned());
}

fn print_examples(title: &str, examples: &[PathBuf]) {
    if examples.is_empty() {
        return;
    }
    println!("{title}:");
    for path in examples {
        println!("  {}", path.display());
    }
}

fn frame_type_key(frame_type: &FrameType) -> String {
    match frame_type {
        FrameType::Bias => "Bias".to_owned(),
        FrameType::Dark => "Dark".to_owned(),
        FrameType::Flat => "Flat".to_owned(),
        FrameType::Light => "Light".to_owned(),
        FrameType::Other(value) => format!("Other({value})"),
    }
}

fn classification_key(classification: &FrameClassification) -> String {
    classification
        .evidence()
        .iter()
        .map(|evidence| {
            let source = match &evidence.source {
                ClassificationSource::HeaderKeyword(_) => "header",
                ClassificationSource::Directory(_) => "directory",
                ClassificationSource::FileName(_) => "filename",
            };
            format!("{source}:{}", frame_type_key(&evidence.frame_type))
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_strict_mode_and_limit() {
        let result = parse_args([
            OsString::from("--strict"),
            OsString::from("--limit"),
            OsString::from("25"),
            OsString::from("/data"),
        ]);
        assert_eq!(
            result,
            Ok(Config {
                root: PathBuf::from("/data"),
                mode: ValidationMode::Strict,
                limit: Some(25),
                examples: 0,
            })
        );
    }

    #[test]
    fn rejects_zero_limit() {
        let result = parse_args([
            OsString::from("--limit"),
            OsString::from("0"),
            OsString::from("/data"),
        ]);
        assert_eq!(result, Err("--limit must be greater than zero".to_owned()));
    }

    #[test]
    fn recognizes_fits_extensions_case_insensitively() {
        assert!(is_fits_path(Path::new("light.FITS")));
        assert!(is_fits_path(Path::new("dark.fit")));
        assert!(!is_fits_path(Path::new("image.xisf")));
    }
}
