//! Bounded, path-private extraction of PixInsight XDRZ alignment geometry.

use std::env;
use std::ffi::OsString;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use serde::Serialize;

const MAX_XDRZ_BYTES: u64 = 8_u64 << 20;
const XDRZ_NAMESPACE: &str = "http://www.pixinsight.com/xdrz";
const USAGE: &str = "Usage: aether-xdrz-inspect [--compact] <alignment.xdrz>";

#[derive(Debug, Eq, PartialEq)]
struct Config {
    path: PathBuf,
    compact: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AlignmentGeometry {
    schema_version: u32,
    format_version: String,
    width: u64,
    height: u64,
    channels: u64,
    origin_x: f64,
    origin_y: f64,
    matrix: [[f64; 3]; 3],
    projective_terms_present: bool,
}

#[derive(Default)]
struct PartialGeometry {
    format_version: Option<String>,
    dimensions: Option<(u64, u64, u64)>,
    origin: Option<(f64, f64)>,
    matrix: Option<[[f64; 3]; 3]>,
    reading_matrix: bool,
}

fn main() -> ExitCode {
    let config = match parse_args(env::args_os().skip(1)) {
        Ok(config) => config,
        Err(message) => {
            eprintln!("{message}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let geometry = match inspect_path(&config.path) {
        Ok(geometry) => geometry,
        Err(message) => {
            eprintln!("XDRZ inspection failed: {message}");
            return ExitCode::FAILURE;
        }
    };
    let serialized = if config.compact {
        serde_json::to_string(&geometry)
    } else {
        serde_json::to_string_pretty(&geometry)
    };
    match serialized {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("XDRZ serialization failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn parse_args(arguments: impl IntoIterator<Item = OsString>) -> Result<Config, String> {
    let mut compact = false;
    let mut path = None;
    for argument in arguments {
        if argument == "--compact" {
            if compact {
                return Err("--compact was specified more than once.".to_owned());
            }
            compact = true;
        } else if argument == "--help" || argument == "-h" {
            return Err("One XDRZ alignment artifact is required.".to_owned());
        } else if argument.to_string_lossy().starts_with('-') {
            return Err("Unknown option.".to_owned());
        } else if path.replace(PathBuf::from(argument)).is_some() {
            return Err("Exactly one XDRZ input is required.".to_owned());
        }
    }
    Ok(Config {
        path: path.ok_or_else(|| "Missing XDRZ input.".to_owned())?,
        compact,
    })
}

fn inspect_path(path: &Path) -> Result<AlignmentGeometry, String> {
    let file = File::open(path).map_err(|error| format!("cannot open input: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect input metadata: {error}"))?;
    if !metadata.is_file() {
        return Err("input is not a regular file".to_owned());
    }
    let length = metadata.len();
    if length > MAX_XDRZ_BYTES {
        return Err(format!(
            "input exceeds the {MAX_XDRZ_BYTES}-byte safety limit"
        ));
    }
    let capacity = usize::try_from(length).map_err(|_| "input length exceeds usize".to_owned())?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(capacity)
        .map_err(|_| "input buffer allocation failed")?;
    BufReader::new(file)
        .take(MAX_XDRZ_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read input: {error}"))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_XDRZ_BYTES {
        return Err("input grew beyond the safety limit while reading".to_owned());
    }
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != length {
        return Err("input length changed while reading".to_owned());
    }
    parse_xdrz(&bytes)
}

fn parse_xdrz(bytes: &[u8]) -> Result<AlignmentGeometry, String> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut partial = PartialGeometry::default();
    let mut depth = 0_usize;
    let mut root_closed = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) if depth == 0 && element.name().as_ref() == "xdrz" => {
                if root_closed {
                    return Err("multiple XML roots".to_owned());
                }
                set_once(
                    &mut partial.format_version,
                    attribute(&element, "version")?,
                    "xdrz version",
                )?;
                if attribute(&element, "xmlns")? != XDRZ_NAMESPACE {
                    return Err("unsupported xdrz namespace".to_owned());
                }
                depth += 1;
            }
            Ok(Event::Start(element)) => {
                if depth == 0 {
                    return Err("the XML root must be xdrz".to_owned());
                }
                if partial.reading_matrix {
                    return Err("alignment matrix must contain text only".to_owned());
                }
                if element.name().as_ref() == "AlignmentMatrix" {
                    if depth != 1 {
                        return Err("alignment matrix must be a direct xdrz child".to_owned());
                    }
                    if partial.matrix.is_some() {
                        return Err("duplicate alignment matrix".to_owned());
                    }
                    partial.reading_matrix = true;
                }
                depth += 1;
            }
            Ok(Event::Empty(element))
                if depth == 1 && element.name().as_ref() == "ReferenceGeometry" =>
            {
                let dimensions = (
                    positive_integer(&element, "width")?,
                    positive_integer(&element, "height")?,
                    positive_integer(&element, "numberOfChannels")?,
                );
                set_once(&mut partial.dimensions, dimensions, "reference geometry")?;
            }
            Ok(Event::Empty(element))
                if depth == 1 && element.name().as_ref() == "AlignmentOrigin" =>
            {
                let origin = (finite_number(&element, "x")?, finite_number(&element, "y")?);
                set_once(&mut partial.origin, origin, "alignment origin")?;
            }
            Ok(Event::Empty(_)) if depth == 0 => {
                return Err("the XML root must be a non-empty xdrz element".to_owned());
            }
            Ok(Event::Text(text)) if partial.reading_matrix => {
                if partial.matrix.is_some() {
                    return Err("alignment matrix has multiple text segments".to_owned());
                }
                partial.matrix = Some(parse_matrix(text.as_ref().trim())?);
            }
            Ok(Event::End(element)) => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| "unexpected closing XML element".to_owned())?;
                if element.name().as_ref() == "AlignmentMatrix" {
                    if depth != 1 || !partial.reading_matrix || partial.matrix.is_none() {
                        return Err("empty or misplaced alignment matrix".to_owned());
                    }
                    partial.reading_matrix = false;
                } else if element.name().as_ref() == "xdrz" {
                    if depth != 0 {
                        return Err("nested xdrz element".to_owned());
                    }
                    root_closed = true;
                }
            }
            Ok(Event::DocType(_)) => return Err("document types are not accepted".to_owned()),
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(error) => return Err(format!("malformed XML: {error}")),
        }
    }

    if depth != 0 || !root_closed || partial.reading_matrix {
        return Err("incomplete xdrz document".to_owned());
    }

    let format_version = partial
        .format_version
        .ok_or_else(|| "missing xdrz version".to_owned())?;
    if format_version != "1.0" {
        return Err(format!("unsupported xdrz version {format_version}"));
    }
    let (width, height, channels) = partial
        .dimensions
        .ok_or_else(|| "missing reference geometry".to_owned())?;
    let (origin_x, origin_y) = partial
        .origin
        .ok_or_else(|| "missing alignment origin".to_owned())?;
    let matrix = partial
        .matrix
        .ok_or_else(|| "missing alignment matrix".to_owned())?;
    let determinant = matrix[0][0] * (matrix[1][1] * matrix[2][2] - matrix[1][2] * matrix[2][1])
        - matrix[0][1] * (matrix[1][0] * matrix[2][2] - matrix[1][2] * matrix[2][0])
        + matrix[0][2] * (matrix[1][0] * matrix[2][1] - matrix[1][1] * matrix[2][0]);
    if !determinant.is_finite() || determinant == 0.0 {
        return Err("alignment matrix is singular".to_owned());
    }
    Ok(AlignmentGeometry {
        schema_version: 1,
        format_version,
        width,
        height,
        channels,
        origin_x,
        origin_y,
        projective_terms_present: matrix[2][0] != 0.0 || matrix[2][1] != 0.0,
        matrix,
    })
}

fn attribute(element: &BytesStart<'_>, name: &str) -> Result<String, String> {
    let mut value = None;
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| format!("invalid XML attribute: {error}"))?;
        if attribute.key.as_ref() == name {
            if value.is_some() {
                return Err("duplicate XML attribute".to_owned());
            }
            value = Some(
                attribute
                    .normalized_value(XmlVersion::Implicit1_0)
                    .map_err(|error| format!("invalid XML attribute value: {error}"))?
                    .into_owned(),
            );
        }
    }
    value.ok_or_else(|| format!("missing {name} attribute"))
}

fn positive_integer(element: &BytesStart<'_>, name: &str) -> Result<u64, String> {
    let value = attribute(element, name)?;
    let parsed = value
        .parse::<u64>()
        .map_err(|_| format!("invalid {name} integer"))?;
    if parsed == 0 {
        return Err(format!("{name} must be positive"));
    }
    Ok(parsed)
}

fn finite_number(element: &BytesStart<'_>, name: &str) -> Result<f64, String> {
    let value = attribute(element, name)?;
    let parsed = value
        .parse::<f64>()
        .map_err(|_| format!("invalid {name} number"))?;
    if !parsed.is_finite() {
        return Err(format!("{name} must be finite"));
    }
    Ok(parsed)
}

fn parse_matrix(value: &str) -> Result<[[f64; 3]; 3], String> {
    let values = value
        .split(',')
        .map(str::trim)
        .map(|part| {
            let parsed = part
                .parse::<f64>()
                .map_err(|_| "alignment matrix contains an invalid number".to_owned())?;
            if !parsed.is_finite() {
                return Err("alignment matrix must contain finite numbers".to_owned());
            }
            Ok(parsed)
        })
        .collect::<Result<Vec<_>, String>>()?;
    let values: [f64; 9] = values
        .try_into()
        .map_err(|_| "alignment matrix must contain exactly nine values".to_owned())?;
    Ok([
        [values[0], values[1], values[2]],
        [values[3], values[4], values[5]],
        [values[6], values[7], values[8]],
    ])
}

fn set_once<T>(target: &mut Option<T>, value: T, label: &str) -> Result<(), String> {
    if target.replace(value).is_some() {
        Err(format!("duplicate {label}"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &[u8] = br#"<?xml version="1.0"?>
<xdrz version="1.0" xmlns="http://www.pixinsight.com/xdrz">
  <ReferenceGeometry width="4144" height="2822" numberOfChannels="3"/>
  <AlignmentOrigin x="0.5" y="0.5"/>
  <AlignmentMatrix>1,0,-2.5,0,1,3.25,1e-8,-2e-8,1</AlignmentMatrix>
</xdrz>"#;

    #[test]
    fn extracts_only_bounded_geometry_without_paths() -> Result<(), String> {
        let geometry = parse_xdrz(VALID)?;
        assert_eq!(
            (geometry.width, geometry.height, geometry.channels),
            (4144, 2822, 3)
        );
        assert!((geometry.origin_x - 0.5).abs() < f64::EPSILON);
        assert!((geometry.matrix[0][2] + 2.5).abs() < f64::EPSILON);
        assert!(geometry.projective_terms_present);
        let json = serde_json::to_string(&geometry).map_err(|error| error.to_string())?;
        assert!(!json.contains("SourceImage"));
        assert!(!json.contains('/'));
        Ok(())
    }

    #[test]
    fn rejects_missing_duplicate_nonfinite_singular_and_doctype_inputs() {
        assert!(parse_xdrz(b"<xdrz version=\"1.0\"/>").is_err());
        assert!(parse_xdrz(&[VALID, VALID].concat()).is_err());
        let valid = String::from_utf8_lossy(VALID);
        assert!(parse_xdrz(valid.replace(XDRZ_NAMESPACE, "urn:unexpected").as_bytes()).is_err());
        assert!(parse_xdrz(valid.replace("<xdrz ", "<wrapper><xdrz ").as_bytes()).is_err());
        assert!(parse_xdrz(valid.replace("1,0,-2.5", "NaN,0,-2.5").as_bytes()).is_err());
        assert!(
            parse_xdrz(
                valid
                    .replace("1,0,-2.5,0,1,3.25,1e-8,-2e-8,1", "1,0,0,0,0,0,0,0,1",)
                    .as_bytes()
            )
            .is_err()
        );
        assert!(parse_xdrz(b"<!DOCTYPE xdrz><xdrz version=\"1.0\"/>").is_err());
    }

    #[test]
    fn accepts_invertible_matrices_independent_of_homogeneous_scale() -> Result<(), String> {
        let valid = String::from_utf8_lossy(VALID);
        let scaled = valid.replace(
            "1,0,-2.5,0,1,3.25,1e-8,-2e-8,1",
            "1e-20,0,-2.5e-20,0,1e-20,3.25e-20,1e-28,-2e-28,1e-20",
        );
        let geometry = parse_xdrz(scaled.as_bytes())?;
        assert!(geometry.matrix[0][0] > 0.0);
        Ok(())
    }

    #[test]
    fn parses_exactly_one_input_and_optional_compact_output() {
        assert_eq!(
            parse_args(["--compact", "alignment.xdrz"].map(OsString::from)),
            Ok(Config {
                path: PathBuf::from("alignment.xdrz"),
                compact: true,
            })
        );
        assert!(parse_args(Vec::<OsString>::new()).is_err());
        assert!(parse_args(["first.xdrz", "second.xdrz"].map(OsString::from)).is_err());
        assert!(parse_args(["--compact", "--compact", "one.xdrz"].map(OsString::from)).is_err());
    }
}
