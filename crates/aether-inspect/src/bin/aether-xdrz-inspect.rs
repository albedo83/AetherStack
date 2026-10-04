//! Bounded, path-private extraction of PixInsight XDRZ alignment geometry.

use std::env;
use std::ffi::OsString;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use serde::{Deserialize, Serialize};

const MAX_XDRZ_BYTES: u64 = 8_u64 << 20;
const MAX_AETHER_REPORT_BYTES: u64 = 1_u64 << 20;
const XDRZ_NAMESPACE: &str = "http://www.pixinsight.com/xdrz";
const COMPARISON_GRID_DIVISIONS: u32 = 4;
const USAGE: &str =
    "Usage: aether-xdrz-inspect [--compact] [--aether-report report.json] <alignment.xdrz>";

#[derive(Debug, Eq, PartialEq)]
struct Config {
    path: PathBuf,
    aether_report: Option<PathBuf>,
    compact: bool,
}

#[derive(Clone, Debug, Serialize)]
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AetherRegistrationReport {
    schema_version: u32,
    diagnostic_only: bool,
    accepted_plan: Option<AetherAcceptedPlan>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AetherAcceptedPlan {
    transform_coefficients_source_pixels: [f64; 6],
    reference_width: u64,
    reference_height: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeometryComparison {
    schema_version: u32,
    coordinate_convention: &'static str,
    sample_count: u32,
    root_mean_square_difference_pixels: f64,
    maximum_difference_pixels: f64,
    center_difference_pixels: f64,
    xdrz_projective_terms_present: bool,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum Output {
    Geometry(AlignmentGeometry),
    Comparison(GeometryComparison),
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
    let output = if let Some(report_path) = &config.aether_report {
        match compare_with_aether_report(&geometry, report_path) {
            Ok(comparison) => Output::Comparison(comparison),
            Err(message) => {
                eprintln!("XDRZ comparison failed: {message}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        Output::Geometry(geometry)
    };
    let serialized = if config.compact {
        serde_json::to_string(&output)
    } else {
        serde_json::to_string_pretty(&output)
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
    let mut aether_report = None;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        if argument == "--compact" {
            if compact {
                return Err("--compact was specified more than once.".to_owned());
            }
            compact = true;
        } else if argument == "--aether-report" {
            if aether_report.is_some() {
                return Err("--aether-report was specified more than once.".to_owned());
            }
            let value = arguments
                .next()
                .ok_or_else(|| "--aether-report requires a path.".to_owned())?;
            if value.to_string_lossy().starts_with('-') {
                return Err("--aether-report requires a path.".to_owned());
            }
            aether_report = Some(PathBuf::from(value));
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
        aether_report,
        compact,
    })
}

fn inspect_path(path: &Path) -> Result<AlignmentGeometry, String> {
    let bytes = read_bounded_regular_file(path, MAX_XDRZ_BYTES, "XDRZ input")?;
    parse_xdrz(&bytes)
}

fn read_bounded_regular_file(path: &Path, limit: u64, label: &str) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|error| format!("cannot open input: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect input metadata: {error}"))?;
    if !metadata.is_file() {
        return Err("input is not a regular file".to_owned());
    }
    let length = metadata.len();
    if length > limit {
        return Err(format!("{label} exceeds the {limit}-byte safety limit"));
    }
    let capacity = usize::try_from(length).map_err(|_| "input length exceeds usize".to_owned())?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(capacity)
        .map_err(|_| "input buffer allocation failed")?;
    BufReader::new(file)
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read input: {error}"))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
        return Err(format!(
            "{label} grew beyond the safety limit while reading"
        ));
    }
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != length {
        return Err("input length changed while reading".to_owned());
    }
    Ok(bytes)
}

fn compare_with_aether_report(
    geometry: &AlignmentGeometry,
    report_path: &Path,
) -> Result<GeometryComparison, String> {
    let bytes = read_bounded_regular_file(
        report_path,
        MAX_AETHER_REPORT_BYTES,
        "Aether registration report",
    )?;
    let plan = parse_aether_report(&bytes)?;
    compare_geometry_with_plan(geometry, &plan)
}

fn parse_aether_report(bytes: &[u8]) -> Result<AetherAcceptedPlan, String> {
    let report: AetherRegistrationReport = serde_json::from_slice(bytes)
        .map_err(|error| format!("invalid Aether registration report: {error}"))?;
    if report.schema_version != 2 || !report.diagnostic_only {
        return Err("unsupported Aether registration report".to_owned());
    }
    report
        .accepted_plan
        .ok_or_else(|| "Aether registration report has no accepted plan".to_owned())
}

fn compare_geometry_with_plan(
    geometry: &AlignmentGeometry,
    plan: &AetherAcceptedPlan,
) -> Result<GeometryComparison, String> {
    if plan.reference_width != geometry.width || plan.reference_height != geometry.height {
        return Err("reference dimensions disagree".to_owned());
    }
    validate_affine(plan.transform_coefficients_source_pixels)?;
    if geometry.width > (1_u64 << 53) || geometry.height > (1_u64 << 53) {
        return Err("reference dimensions exceed exact binary64 coordinates".to_owned());
    }

    let inverse = invert_homography(geometry.matrix)?;
    let maximum_x = (geometry.width - 1) as f64;
    let maximum_y = (geometry.height - 1) as f64;
    let mut squared_sum = CompensatedAccumulator::default();
    let mut maximum = 0.0_f64;
    let mut center = None;
    let mut count = 0_u32;
    for yi in 0..=COMPARISON_GRID_DIVISIONS {
        let y = maximum_y * f64::from(yi) / f64::from(COMPARISON_GRID_DIVISIONS);
        for xi in 0..=COMPARISON_GRID_DIVISIONS {
            let x = maximum_x * f64::from(xi) / f64::from(COMPARISON_GRID_DIVISIONS);
            let xdrz = apply_xdrz_source_to_reference(inverse, geometry, x, y)?;
            let aether = apply_affine(plan.transform_coefficients_source_pixels, x, y)?;
            let difference = (xdrz.0 - aether.0).hypot(xdrz.1 - aether.1);
            if !difference.is_finite() {
                return Err("comparison produced a non-finite difference".to_owned());
            }
            squared_sum.add(difference * difference)?;
            maximum = maximum.max(difference);
            if xi * 2 == COMPARISON_GRID_DIVISIONS && yi * 2 == COMPARISON_GRID_DIVISIONS {
                center = Some(difference);
            }
            count = count
                .checked_add(1)
                .ok_or_else(|| "comparison sample count overflow".to_owned())?;
        }
    }
    let mean_square = squared_sum.total()? / f64::from(count);
    let root_mean_square = mean_square.sqrt();
    if !root_mean_square.is_finite() {
        return Err("comparison RMS is not finite".to_owned());
    }
    Ok(GeometryComparison {
        schema_version: 1,
        coordinate_convention: "xdrz-reference-to-source-inverted-about-alignment-origin-v1",
        sample_count: count,
        root_mean_square_difference_pixels: root_mean_square,
        maximum_difference_pixels: maximum,
        center_difference_pixels: center
            .ok_or_else(|| "comparison center is missing".to_owned())?,
        xdrz_projective_terms_present: geometry.projective_terms_present,
    })
}

fn validate_affine(coefficients: [f64; 6]) -> Result<(), String> {
    if coefficients.iter().any(|value| !value.is_finite()) {
        return Err("Aether transform contains a non-finite coefficient".to_owned());
    }
    let determinant =
        coefficients[0].mul_add(coefficients[3], -(coefficients[1] * coefficients[2]));
    if !determinant.is_finite() || determinant == 0.0 {
        return Err("Aether transform is singular".to_owned());
    }
    Ok(())
}

fn apply_affine(coefficients: [f64; 6], x: f64, y: f64) -> Result<(f64, f64), String> {
    let mapped_x = coefficients[0].mul_add(x, coefficients[1].mul_add(y, coefficients[4]));
    let mapped_y = coefficients[2].mul_add(x, coefficients[3].mul_add(y, coefficients[5]));
    if !mapped_x.is_finite() || !mapped_y.is_finite() {
        return Err("Aether transform overflow".to_owned());
    }
    Ok((mapped_x, mapped_y))
}

fn apply_xdrz_source_to_reference(
    inverse: [[f64; 3]; 3],
    geometry: &AlignmentGeometry,
    x: f64,
    y: f64,
) -> Result<(f64, f64), String> {
    // XDRZ stores the output-reference to input-source sampling map in image
    // coordinates. AetherStack stores the opposite direction and places the
    // first pixel center at zero, so apply H^-1 between explicit origin shifts.
    let shifted_x = x + geometry.origin_x;
    let shifted_y = y + geometry.origin_y;
    let denominator =
        inverse[2][0].mul_add(shifted_x, inverse[2][1].mul_add(shifted_y, inverse[2][2]));
    if !denominator.is_finite() || denominator == 0.0 {
        return Err("XDRZ homography is undefined on the comparison grid".to_owned());
    }
    let mapped_x = inverse[0][0]
        .mul_add(shifted_x, inverse[0][1].mul_add(shifted_y, inverse[0][2]))
        / denominator
        - geometry.origin_x;
    let mapped_y = inverse[1][0]
        .mul_add(shifted_x, inverse[1][1].mul_add(shifted_y, inverse[1][2]))
        / denominator
        - geometry.origin_y;
    if !mapped_x.is_finite() || !mapped_y.is_finite() {
        return Err("XDRZ homography overflow".to_owned());
    }
    Ok((mapped_x, mapped_y))
}

fn invert_homography(matrix: [[f64; 3]; 3]) -> Result<[[f64; 3]; 3], String> {
    // Homographies are invariant to a common nonzero scale. Normalizing first
    // prevents otherwise valid small or large representations from underflowing
    // or overflowing during the cofactor expansion.
    let scale = matrix
        .iter()
        .flatten()
        .map(|value| value.abs())
        .fold(0.0_f64, f64::max);
    if !scale.is_finite() || scale == 0.0 {
        return Err("alignment matrix has invalid scale".to_owned());
    }
    let mut m = matrix;
    for value in m.iter_mut().flatten() {
        *value /= scale;
    }
    let c00 = m[1][1].mul_add(m[2][2], -(m[1][2] * m[2][1]));
    let c01 = -(m[1][0].mul_add(m[2][2], -(m[1][2] * m[2][0])));
    let c02 = m[1][0].mul_add(m[2][1], -(m[1][1] * m[2][0]));
    let determinant = m[0][0].mul_add(c00, m[0][1].mul_add(c01, m[0][2] * c02));
    if !determinant.is_finite() || determinant == 0.0 {
        return Err("alignment matrix is singular".to_owned());
    }
    let inverse_determinant = determinant.recip();
    let inverse = [
        [
            c00 * inverse_determinant,
            -(m[0][1].mul_add(m[2][2], -(m[0][2] * m[2][1]))) * inverse_determinant,
            m[0][1].mul_add(m[1][2], -(m[0][2] * m[1][1])) * inverse_determinant,
        ],
        [
            c01 * inverse_determinant,
            m[0][0].mul_add(m[2][2], -(m[0][2] * m[2][0])) * inverse_determinant,
            -(m[0][0].mul_add(m[1][2], -(m[0][2] * m[1][0]))) * inverse_determinant,
        ],
        [
            c02 * inverse_determinant,
            -(m[0][0].mul_add(m[2][1], -(m[0][1] * m[2][0]))) * inverse_determinant,
            m[0][0].mul_add(m[1][1], -(m[0][1] * m[1][0])) * inverse_determinant,
        ],
    ];
    if inverse.iter().flatten().any(|value| !value.is_finite()) {
        return Err("alignment matrix inverse overflow".to_owned());
    }
    Ok(inverse)
}

#[derive(Default)]
struct CompensatedAccumulator {
    sum: f64,
    correction: f64,
}

impl CompensatedAccumulator {
    fn add(&mut self, value: f64) -> Result<(), String> {
        // Neumaier compensation retains small squared residuals when field-edge
        // model differences are substantially larger than central differences.
        let next = self.sum + value;
        if self.sum.abs() >= value.abs() {
            self.correction += (self.sum - next) + value;
        } else {
            self.correction += (value - next) + self.sum;
        }
        self.sum = next;
        if !self.sum.is_finite() || !self.correction.is_finite() {
            return Err("comparison accumulation overflow".to_owned());
        }
        Ok(())
    }

    fn total(self) -> Result<f64, String> {
        let total = self.sum + self.correction;
        if total.is_finite() {
            Ok(total)
        } else {
            Err("comparison accumulation overflow".to_owned())
        }
    }
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
    invert_homography(matrix)?;
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
                aether_report: None,
                compact: true,
            })
        );
        assert_eq!(
            parse_args(["--aether-report", "report.json", "alignment.xdrz"].map(OsString::from)),
            Ok(Config {
                path: PathBuf::from("alignment.xdrz"),
                aether_report: Some(PathBuf::from("report.json")),
                compact: false,
            })
        );
        assert!(parse_args(Vec::<OsString>::new()).is_err());
        assert!(parse_args(["first.xdrz", "second.xdrz"].map(OsString::from)).is_err());
        assert!(parse_args(["--compact", "--compact", "one.xdrz"].map(OsString::from)).is_err());
        assert!(parse_args(["--aether-report", "one.xdrz"].map(OsString::from)).is_err());
    }

    #[test]
    fn comparison_inverts_xdrz_and_applies_alignment_origin() -> Result<(), String> {
        let geometry = AlignmentGeometry {
            schema_version: 1,
            format_version: "1.0".to_owned(),
            width: 101,
            height: 81,
            channels: 1,
            origin_x: 0.5,
            origin_y: 0.5,
            matrix: [[1.0, 0.0, 2.0], [0.0, 1.0, -3.0], [0.0, 0.0, 1.0]],
            projective_terms_present: false,
        };
        let plan = AetherAcceptedPlan {
            transform_coefficients_source_pixels: [1.0, 0.0, 0.0, 1.0, -2.0, 3.0],
            reference_width: 101,
            reference_height: 81,
        };
        let comparison = compare_geometry_with_plan(&geometry, &plan)?;
        assert_eq!(comparison.sample_count, 25);
        assert!(comparison.root_mean_square_difference_pixels < 1e-13);
        assert!(comparison.maximum_difference_pixels < 1e-13);
        assert!(comparison.center_difference_pixels < 1e-13);
        Ok(())
    }

    #[test]
    fn comparison_rejects_bad_aether_evidence_and_detects_projective_gap() -> Result<(), String> {
        let geometry = parse_xdrz(VALID)?;
        let wrong_dimensions = AetherAcceptedPlan {
            transform_coefficients_source_pixels: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            reference_width: 1,
            reference_height: geometry.height,
        };
        assert!(compare_geometry_with_plan(&geometry, &wrong_dimensions).is_err());
        let singular = AetherAcceptedPlan {
            transform_coefficients_source_pixels: [0.0; 6],
            reference_width: geometry.width,
            reference_height: geometry.height,
        };
        assert!(compare_geometry_with_plan(&geometry, &singular).is_err());
        let identity = AetherAcceptedPlan {
            transform_coefficients_source_pixels: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            reference_width: geometry.width,
            reference_height: geometry.height,
        };
        let comparison = compare_geometry_with_plan(&geometry, &identity)?;
        assert!(comparison.maximum_difference_pixels > 1.0);
        Ok(())
    }

    #[test]
    fn projective_inverse_is_scale_invariant_and_round_trips() -> Result<(), String> {
        let matrix = [[1.1, 0.02, 7.0], [-0.03, 0.9, -4.0], [2e-5, -3e-5, 2.0]];
        let inverse = invert_homography(matrix)?;
        let scaled = matrix.map(|row| row.map(|value| value * 1e150));
        let scaled_inverse = invert_homography(scaled)?;
        for row in 0..3 {
            for column in 0..3 {
                let product = (0..3)
                    .map(|index| matrix[row][index] * inverse[index][column])
                    .sum::<f64>();
                let expected = if row == column { 7.0 } else { 0.0 };
                assert!((product - expected).abs() < 1e-12);
                assert!((inverse[row][column] - scaled_inverse[row][column]).abs() < 1e-12);
            }
        }
        Ok(())
    }

    #[test]
    fn aether_report_decoder_requires_supported_accepted_diagnostic_evidence() -> Result<(), String>
    {
        let valid = br#"{
            "schemaVersion": 2,
            "diagnosticOnly": true,
            "acceptedPlan": {
                "transformCoefficientsSourcePixels": [1, 0, 0, 1, -2, 3],
                "referenceWidth": 4144,
                "referenceHeight": 2822
            },
            "additionalDiagnosticField": "ignored"
        }"#;
        let plan = parse_aether_report(valid)?;
        assert_eq!(plan.reference_width, 4144);
        let unsupported = String::from_utf8_lossy(valid).replacen(
            "\"schemaVersion\": 2",
            "\"schemaVersion\": 3",
            1,
        );
        assert!(parse_aether_report(unsupported.as_bytes()).is_err());
        assert!(parse_aether_report(br#"{"schemaVersion":2,"diagnosticOnly":true}"#).is_err());
        assert!(parse_aether_report(b"not json").is_err());
        Ok(())
    }
}
