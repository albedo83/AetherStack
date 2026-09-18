use aether_fits::Header;

use crate::{
    BayerPattern, Binning, CameraModel, CanonicalMetadata, CanonicalValue, Confidence, FrameType,
    MetadataIssue, MetadataIssueCode,
};

/// Produces a canonical view without modifying or hiding the source header.
#[must_use]
pub fn normalize_header(header: &Header) -> CanonicalMetadata {
    let mut metadata = CanonicalMetadata::default();

    metadata.camera = resolve_string(header, &["INSTRUME", "CAMERA"], true, &mut metadata.issues)
        .map(|(raw, keyword)| {
            let (camera, confidence) = normalize_camera(&raw);
            CanonicalValue::new(camera, keyword, confidence)
        });
    if metadata.camera.is_none() {
        metadata.issues.push(MetadataIssue::new(
            MetadataIssueCode::MissingInstrument,
            ["INSTRUME", "CAMERA"],
            "no camera identifier is present",
        ));
    }

    metadata.frame_type = resolve_string(
        header,
        &["IMAGETYP", "FRAME", "FRAMETYP"],
        true,
        &mut metadata.issues,
    )
    .map(|(raw, keyword)| {
        let (frame_type, confidence) = normalize_frame_type(&raw);
        CanonicalValue::new(frame_type, keyword, confidence)
    });

    metadata.exposure_seconds =
        resolve_number(header, &["EXPTIME", "EXPOSURE"], &mut metadata.issues);
    metadata.sensor_temperature_c = resolve_number(
        header,
        &["CCD-TEMP", "CCD_TEMP", "SENSORT"],
        &mut metadata.issues,
    );
    metadata.set_temperature_c =
        resolve_number(header, &["SET-TEMP", "SET_TEMP"], &mut metadata.issues);
    metadata.gain = resolve_number(header, &["GAIN"], &mut metadata.issues);
    metadata.offset = resolve_number(header, &["OFFSET"], &mut metadata.issues);

    let x_binning = resolve_positive_u32(header, "XBINNING", &mut metadata.issues);
    let y_binning = resolve_positive_u32(header, "YBINNING", &mut metadata.issues);
    if let (Some(x), Some(y)) = (x_binning, y_binning) {
        metadata.binning = Some(CanonicalValue::new(
            Binning { x, y },
            "XBINNING+YBINNING",
            Confidence::Exact,
        ));
    }

    metadata.filter = resolve_string(header, &["FILTER"], false, &mut metadata.issues)
        .map(|(value, keyword)| CanonicalValue::new(value, keyword, Confidence::Exact));
    metadata.bayer_pattern = resolve_string(
        header,
        &["BAYERPAT", "BAYERPATN"],
        true,
        &mut metadata.issues,
    )
    .map(|(raw, keyword)| {
        let (pattern, confidence) = normalize_bayer_pattern(&raw);
        CanonicalValue::new(pattern, keyword, confidence)
    });

    metadata
}

fn normalize_camera(raw: &str) -> (CameraModel, Confidence) {
    let trimmed = raw.trim();
    if trimmed == "ZWO ASI294MC Pro" {
        return (CameraModel::ZwoAsi294McPro, Confidence::Exact);
    }
    if trimmed.eq_ignore_ascii_case("ZWO ASI294MC Pro") {
        return (CameraModel::ZwoAsi294McPro, Confidence::Normalized);
    }
    if trimmed == "ATR585C" {
        return (CameraModel::TouptekAtr585C, Confidence::Exact);
    }
    if trimmed.eq_ignore_ascii_case("ToupTek ATR585C")
        || trimmed
            .to_ascii_uppercase()
            .strip_prefix("ATR585C(")
            .is_some_and(|suffix| suffix.ends_with(')'))
    {
        return (CameraModel::TouptekAtr585C, Confidence::Normalized);
    }
    (CameraModel::Other(trimmed.to_owned()), Confidence::Exact)
}

fn normalize_frame_type(raw: &str) -> (FrameType, Confidence) {
    let trimmed = raw.trim();
    let normalized = trimmed.to_ascii_uppercase();
    let frame_type = match normalized.as_str() {
        "BIAS" => FrameType::Bias,
        "DARK" => FrameType::Dark,
        "FLAT" => FrameType::Flat,
        "LIGHT" => FrameType::Light,
        _ => return (FrameType::Other(trimmed.to_owned()), Confidence::Exact),
    };
    let confidence = if trimmed == normalized {
        Confidence::Exact
    } else {
        Confidence::Normalized
    };
    (frame_type, confidence)
}

fn normalize_bayer_pattern(raw: &str) -> (BayerPattern, Confidence) {
    let trimmed = raw.trim();
    let normalized = trimmed.to_ascii_uppercase();
    let pattern = match normalized.as_str() {
        "RGGB" => BayerPattern::Rggb,
        "BGGR" => BayerPattern::Bggr,
        "GRBG" => BayerPattern::Grbg,
        "GBRG" => BayerPattern::Gbrg,
        _ => return (BayerPattern::Other(trimmed.to_owned()), Confidence::Exact),
    };
    let confidence = if trimmed == normalized {
        Confidence::Exact
    } else {
        Confidence::Normalized
    };
    (pattern, confidence)
}

fn resolve_string(
    header: &Header,
    keywords: &[&str],
    compare_case_insensitively: bool,
    issues: &mut Vec<MetadataIssue>,
) -> Option<(String, String)> {
    let mut values: Vec<(String, String)> = Vec::new();
    for keyword in keywords {
        if let Some(value) = header.string(keyword) {
            values.push((value.to_owned(), (*keyword).to_owned()));
        } else if header.card(keyword).is_some() {
            issues.push(MetadataIssue::new(
                MetadataIssueCode::InvalidValue,
                [*keyword],
                format!("{keyword} is present but is not a FITS string"),
            ));
        }
    }

    let first = values.first()?.clone();
    let conflicting = values.iter().skip(1).any(|(value, _)| {
        if compare_case_insensitively {
            !value.eq_ignore_ascii_case(&first.0)
        } else {
            value != &first.0
        }
    });
    if conflicting {
        issues.push(MetadataIssue::new(
            MetadataIssueCode::ConflictingValues,
            values.iter().map(|(_, keyword)| keyword),
            "equivalent string metadata keywords contain conflicting values",
        ));
    }
    Some(first)
}

fn resolve_number(
    header: &Header,
    keywords: &[&str],
    issues: &mut Vec<MetadataIssue>,
) -> Option<CanonicalValue<f64>> {
    let mut values: Vec<(f64, String)> = Vec::new();
    for keyword in keywords {
        if let Some(value) = header.number(keyword) {
            if value.is_finite() {
                values.push((value, (*keyword).to_owned()));
            } else {
                issues.push(MetadataIssue::new(
                    MetadataIssueCode::InvalidValue,
                    [*keyword],
                    format!("{keyword} must be finite"),
                ));
            }
        } else if header.card(keyword).is_some() {
            issues.push(MetadataIssue::new(
                MetadataIssueCode::InvalidValue,
                [*keyword],
                format!("{keyword} is present but is not numeric"),
            ));
        }
    }

    let first = values.first()?.clone();
    if values
        .iter()
        .skip(1)
        .any(|(value, _)| !nearly_equal(*value, first.0))
    {
        issues.push(MetadataIssue::new(
            MetadataIssueCode::ConflictingValues,
            values.iter().map(|(_, keyword)| keyword),
            "equivalent numeric metadata keywords contain conflicting values",
        ));
    }
    Some(CanonicalValue::new(first.0, first.1, Confidence::Exact))
}

fn resolve_positive_u32(
    header: &Header,
    keyword: &str,
    issues: &mut Vec<MetadataIssue>,
) -> Option<u32> {
    let Some(value) = header.integer(keyword) else {
        if header.card(keyword).is_some() {
            issues.push(MetadataIssue::new(
                MetadataIssueCode::InvalidValue,
                [keyword],
                format!("{keyword} is present but is not an integer"),
            ));
        }
        return None;
    };
    match u32::try_from(value).ok().filter(|value| *value > 0) {
        Some(value) => Some(value),
        None => {
            issues.push(MetadataIssue::new(
                MetadataIssueCode::InvalidValue,
                [keyword],
                format!("{keyword} must be a positive 32-bit integer"),
            ));
            None
        }
    }
}

fn nearly_equal(left: f64, right: f64) -> bool {
    let scale = left.abs().max(right.abs()).max(1.0);
    (left - right).abs() <= f64::EPSILON * 8.0 * scale
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use aether_fits::{BLOCK_SIZE, CARD_SIZE, HeaderReadOptions, read_primary_header};

    use super::*;

    fn header(extra_cards: &[String]) -> Option<aether_fits::Header> {
        let mut all_cards = vec![
            format!("{:<8}= {:>20}", "SIMPLE", "T"),
            format!("{:<8}= {:>20}", "BITPIX", "16"),
            format!("{:<8}= {:>20}", "NAXIS", "2"),
            format!("{:<8}= {:>20}", "NAXIS1", "4"),
            format!("{:<8}= {:>20}", "NAXIS2", "3"),
        ];
        all_cards.extend_from_slice(extra_cards);
        all_cards.push("END".to_owned());

        let mut block = [b' '; BLOCK_SIZE];
        for (index, text) in all_cards.iter().enumerate() {
            let start = index * CARD_SIZE;
            let end = start + CARD_SIZE;
            let destination = block.get_mut(start..end)?;
            let bytes = text.as_bytes();
            let length = bytes.len().min(CARD_SIZE);
            destination[..length].copy_from_slice(&bytes[..length]);
        }

        let mut cursor = Cursor::new(block);
        read_primary_header(&mut cursor, HeaderReadOptions::default())
            .ok()
            .map(|report| report.into_parts().0)
    }

    fn string_card(keyword: &str, value: &str) -> String {
        format!("{keyword:<8}= '{value}'")
    }

    fn number_card(keyword: &str, value: &str) -> String {
        format!("{keyword:<8}= {value:>20}")
    }

    #[test]
    fn normalizes_touptek_transport_suffix() {
        let Some(header) = header(&[
            string_card("INSTRUME", "ATR585C(USB2.0)"),
            string_card("BAYERPAT", "RGGB"),
        ]) else {
            return;
        };
        let metadata = normalize_header(&header);
        let Some(camera) = metadata.camera else {
            return;
        };

        assert_eq!(camera.value(), &CameraModel::TouptekAtr585C);
        assert_eq!(camera.confidence(), Confidence::Normalized);
        assert!(camera.value().is_priority_supported());
    }

    #[test]
    fn recognizes_zwo_priority_camera() {
        let Some(header) = header(&[string_card("INSTRUME", "ZWO ASI294MC Pro")]) else {
            return;
        };
        let metadata = normalize_header(&header);

        assert_eq!(
            metadata.camera.as_ref().map(CanonicalValue::value),
            Some(&CameraModel::ZwoAsi294McPro)
        );
    }

    #[test]
    fn normalizes_frame_type_case_without_changing_source() {
        let Some(header) = header(&[
            string_card("INSTRUME", "ATR585C"),
            string_card("IMAGETYP", "Dark"),
        ]) else {
            return;
        };
        let metadata = normalize_header(&header);
        let Some(frame_type) = metadata.frame_type else {
            return;
        };

        assert_eq!(frame_type.value(), &FrameType::Dark);
        assert_eq!(frame_type.source_keyword(), "IMAGETYP");
        assert_eq!(frame_type.confidence(), Confidence::Normalized);
    }

    #[test]
    fn reports_conflicting_exposure_keywords() {
        let Some(header) = header(&[
            string_card("INSTRUME", "ATR585C"),
            number_card("EXPTIME", "120.0"),
            number_card("EXPOSURE", "60.0"),
        ]) else {
            return;
        };
        let metadata = normalize_header(&header);

        assert_eq!(
            metadata
                .exposure_seconds
                .as_ref()
                .map(CanonicalValue::value),
            Some(&120.0)
        );
        assert!(
            metadata
                .issues
                .iter()
                .any(|issue| issue.code() == MetadataIssueCode::ConflictingValues)
        );
    }

    #[test]
    fn does_not_invent_missing_camera() {
        let Some(header) = header(&[]) else {
            return;
        };
        let metadata = normalize_header(&header);

        assert!(metadata.camera.is_none());
        assert!(
            metadata
                .issues
                .iter()
                .any(|issue| issue.code() == MetadataIssueCode::MissingInstrument)
        );
    }

    #[test]
    fn leaves_unprofiled_camera_unsupported() {
        let Some(header) = header(&[string_card("INSTRUME", "Canon EOS M50")]) else {
            return;
        };
        let metadata = normalize_header(&header);
        let Some(camera) = metadata.camera else {
            return;
        };

        assert_eq!(
            camera.value(),
            &CameraModel::Other("Canon EOS M50".to_owned())
        );
        assert!(!camera.value().is_priority_supported());
        assert_eq!(camera.value().sensor_kind(), crate::SensorKind::Unknown);
    }
}
