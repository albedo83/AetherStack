use std::io::Read;

use crate::{
    BLOCK_SIZE, CARD_SIZE, Card, Diagnostic, DiagnosticCode, FitsError, FitsValue, Header,
    HeaderReport, Severity,
};

/// Limits applied while reading a header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeaderReadOptions {
    /// Maximum number of 2,880-byte blocks before aborting.
    pub max_blocks: usize,
}

impl Default for HeaderReadOptions {
    fn default() -> Self {
        Self { max_blocks: 1_024 }
    }
}

/// Reads the primary header of a FITS stream without consuming the pixel array.
///
/// The stream is left immediately after the final header block. Interpretable
/// standard violations are returned as diagnostics; truncation, limits, and I/O
/// failures produce a [`FitsError`].
///
/// # Errors
///
/// Returns an error when a block is truncated, `END` is missing, the block limit
/// is exceeded, or the underlying reader fails.
pub fn read_primary_header<R: Read>(
    reader: &mut R,
    options: HeaderReadOptions,
) -> Result<HeaderReport, FitsError> {
    let mut cards = Vec::new();
    let mut diagnostics = Vec::new();

    for block_index in 0..options.max_blocks {
        let mut block = [0_u8; BLOCK_SIZE];
        if !read_block(reader, &mut block)? {
            return Err(FitsError::MissingEndCard {
                blocks_read: block_index,
            });
        }

        for (card_in_block, bytes) in block.chunks_exact(CARD_SIZE).enumerate() {
            let card_index = block_index * (BLOCK_SIZE / CARD_SIZE) + card_in_block;
            let mut raw = [0_u8; CARD_SIZE];
            raw.copy_from_slice(bytes);
            validate_card_bytes(&raw, card_index, &mut diagnostics);

            let card = Card::parse(raw);
            validate_keyword(&card, card_index, &mut diagnostics);
            let is_end = card.keyword() == "END";
            cards.push(card);

            if is_end {
                let padding_start = (card_in_block + 1) * CARD_SIZE;
                if block
                    .get(padding_start..)
                    .is_some_and(|padding| padding.iter().any(|byte| *byte != b' '))
                {
                    diagnostics.push(Diagnostic::new(
                        Severity::Error,
                        DiagnosticCode::NonBlankHeaderPadding,
                        Some(card_index),
                        Some("END".to_owned()),
                        "bytes following END in the current header block are not all spaces",
                    ));
                }

                validate_mandatory_cards(&cards, &mut diagnostics);
                return Ok(HeaderReport::new(
                    Header::new(cards, block_index + 1),
                    diagnostics,
                ));
            }
        }
    }

    Err(FitsError::HeaderBlockLimitExceeded {
        max_blocks: options.max_blocks,
    })
}

fn read_block<R: Read>(reader: &mut R, block: &mut [u8; BLOCK_SIZE]) -> Result<bool, FitsError> {
    let mut filled = 0;
    while filled < block.len() {
        match reader.read(&mut block[filled..]) {
            Ok(0) if filled == 0 => return Ok(false),
            Ok(0) => {
                return Err(FitsError::TruncatedHeader {
                    bytes_in_partial_block: filled,
                });
            }
            Ok(read) => filled += read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(FitsError::Io(error)),
        }
    }
    Ok(true)
}

fn validate_card_bytes(
    raw: &[u8; CARD_SIZE],
    card_index: usize,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if raw.iter().any(|byte| !(32..=126).contains(byte)) {
        diagnostics.push(Diagnostic::new(
            Severity::Error,
            DiagnosticCode::NonAsciiCard,
            Some(card_index),
            None,
            "header card contains bytes outside printable 7-bit ASCII",
        ));
    }
}

fn validate_keyword(card: &Card, card_index: usize, diagnostics: &mut Vec<Diagnostic>) {
    let keyword = card.keyword();
    if keyword.is_empty() {
        return;
    }
    let legal = keyword
        .bytes()
        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || b"-_".contains(&byte));
    if !legal {
        diagnostics.push(Diagnostic::new(
            Severity::Error,
            DiagnosticCode::IllegalKeyword,
            Some(card_index),
            Some(keyword.to_owned()),
            "standard FITS keyword contains an illegal character",
        ));
    }
}

fn validate_mandatory_cards(cards: &[Card], diagnostics: &mut Vec<Diagnostic>) {
    validate_position(cards, "SIMPLE", 0, diagnostics);
    validate_position(cards, "BITPIX", 1, diagnostics);
    validate_position(cards, "NAXIS", 2, diagnostics);

    validate_mandatory_value(
        cards,
        "SIMPLE",
        |value| matches!(value, FitsValue::Logical(true)),
        "SIMPLE must be the logical value T",
        diagnostics,
    );
    validate_mandatory_value(
        cards,
        "BITPIX",
        |value| matches!(value, FitsValue::Integer(8 | 16 | 32 | 64 | -32 | -64)),
        "BITPIX is not a supported standard image value",
        diagnostics,
    );
    validate_mandatory_value(
        cards,
        "NAXIS",
        |value| matches!(value, FitsValue::Integer(0..=999)),
        "NAXIS must be an integer between 0 and 999",
        diagnostics,
    );

    let axis_count = cards
        .iter()
        .find(|card| card.keyword() == "NAXIS")
        .and_then(Card::value)
        .and_then(FitsValue::as_i64)
        .filter(|value| (0..=999).contains(value))
        .and_then(|value| usize::try_from(value).ok());

    if let Some(axis_count) = axis_count {
        for axis in 1..=axis_count {
            let keyword = format!("NAXIS{axis}");
            validate_position(cards, &keyword, axis + 2, diagnostics);
            validate_mandatory_value(
                cards,
                &keyword,
                |value| matches!(value, FitsValue::Integer(0..)),
                "axis length must be a non-negative integer",
                diagnostics,
            );
        }
    }

    for (index, card) in cards.iter().enumerate() {
        if is_fixed_format_mandatory(card.keyword())
            && (!card.has_standard_value_indicator() || !card.has_fixed_value_ending_at_column_30())
        {
            diagnostics.push(Diagnostic::new(
                Severity::Error,
                DiagnosticCode::NonStandardFixedValue,
                Some(index),
                Some(card.keyword().to_owned()),
                "mandatory logical or integer value must end in column 30",
            ));
        }
    }
}

fn validate_position(
    cards: &[Card],
    keyword: &str,
    expected_index: usize,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match cards.iter().position(|card| card.keyword() == keyword) {
        None => diagnostics.push(Diagnostic::new(
            Severity::Error,
            DiagnosticCode::MissingMandatoryCard,
            None,
            Some(keyword.to_owned()),
            format!("mandatory card {keyword} is missing"),
        )),
        Some(actual_index) if actual_index != expected_index => diagnostics.push(Diagnostic::new(
            Severity::Error,
            DiagnosticCode::MandatoryCardOrder,
            Some(actual_index),
            Some(keyword.to_owned()),
            format!(
                "mandatory card {keyword} is at index {actual_index}, expected {expected_index}"
            ),
        )),
        Some(_) => {}
    }
}

fn validate_mandatory_value(
    cards: &[Card],
    keyword: &str,
    is_valid: impl FnOnce(&FitsValue) -> bool,
    message: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some((index, card)) = cards
        .iter()
        .enumerate()
        .find(|(_, card)| card.keyword() == keyword)
    else {
        return;
    };
    if !card.value().is_some_and(is_valid) {
        diagnostics.push(Diagnostic::new(
            Severity::Error,
            DiagnosticCode::InvalidMandatoryValue,
            Some(index),
            Some(keyword.to_owned()),
            message,
        ));
    }
}

fn is_fixed_format_mandatory(keyword: &str) -> bool {
    matches!(keyword, "SIMPLE" | "BITPIX" | "NAXIS")
        || keyword.strip_prefix("NAXIS").is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
        })
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::ValidationMode;

    fn fixed_card(keyword: &str, value: &str) -> [u8; CARD_SIZE] {
        text_card(&format!("{keyword:<8}= {value:>20}"))
    }

    fn text_card(text: &str) -> [u8; CARD_SIZE] {
        let mut card = [b' '; CARD_SIZE];
        let bytes = text.as_bytes();
        let length = bytes.len().min(CARD_SIZE);
        card[..length].copy_from_slice(&bytes[..length]);
        card
    }

    fn block_with(cards: &[[u8; CARD_SIZE]]) -> [u8; BLOCK_SIZE] {
        let mut block = [b' '; BLOCK_SIZE];
        for (index, card) in cards.iter().enumerate() {
            let start = index * CARD_SIZE;
            let end = start + CARD_SIZE;
            if let Some(destination) = block.get_mut(start..end) {
                destination.copy_from_slice(card);
            }
        }
        block
    }

    fn standard_header() -> [u8; BLOCK_SIZE] {
        block_with(&[
            fixed_card("SIMPLE", "T"),
            fixed_card("BITPIX", "16"),
            fixed_card("NAXIS", "2"),
            fixed_card("NAXIS1", "4144"),
            fixed_card("NAXIS2", "2822"),
            text_card("INSTRUME= 'ZWO ASI294MC Pro'"),
            text_card("END"),
        ])
    }

    #[test]
    fn reads_conformant_primary_header() {
        let mut input = Cursor::new(standard_header());
        let report = read_primary_header(&mut input, HeaderReadOptions::default());
        assert!(report.is_ok());
        let Some(report) = report.ok() else {
            return;
        };

        assert!(report.is_conformant());
        assert_eq!(report.header().integer("BITPIX"), Some(16));
        assert_eq!(report.header().integer("NAXIS1"), Some(4_144));
        assert_eq!(report.header().string("INSTRUME"), Some("ZWO ASI294MC Pro"));
        assert_eq!(report.header().blocks_read(), 1);
    }

    #[test]
    fn tolerant_mode_accepts_left_aligned_mandatory_values() {
        let block = block_with(&[
            text_card("SIMPLE  = T"),
            text_card("BITPIX  = 16"),
            text_card("NAXIS   = 2"),
            text_card("NAXIS1  = 3840"),
            text_card("NAXIS2  = 2160"),
            text_card("END"),
        ]);
        let mut input = Cursor::new(block);
        let report = read_primary_header(&mut input, HeaderReadOptions::default());
        assert!(report.is_ok());
        let Some(report) = report.ok() else {
            return;
        };

        assert!(!report.is_accepted(ValidationMode::Strict));
        assert!(report.is_accepted(ValidationMode::Tolerant));
        assert_eq!(report.header().integer("NAXIS1"), Some(3_840));
        assert_eq!(
            report
                .diagnostics()
                .iter()
                .filter(|diagnostic| { diagnostic.code() == DiagnosticCode::NonStandardFixedValue })
                .count(),
            5
        );
    }

    #[test]
    fn reports_non_blank_padding_after_end() {
        let mut block = standard_header();
        let Some(last_byte) = block.last_mut() else {
            return;
        };
        *last_byte = 0;
        let mut input = Cursor::new(block);
        let report = read_primary_header(&mut input, HeaderReadOptions::default());
        assert!(report.is_ok());
        let Some(report) = report.ok() else {
            return;
        };

        assert!(
            report
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code() == DiagnosticCode::NonBlankHeaderPadding)
        );
    }

    #[test]
    fn rejects_partial_header_block() {
        let mut input = Cursor::new([b' '; CARD_SIZE]);
        assert!(matches!(
            read_primary_header(&mut input, HeaderReadOptions::default()),
            Err(FitsError::TruncatedHeader {
                bytes_in_partial_block: CARD_SIZE
            })
        ));
    }

    #[test]
    fn reports_missing_end_on_block_boundary() {
        let mut input = Cursor::new([b' '; BLOCK_SIZE]);
        assert!(matches!(
            read_primary_header(&mut input, HeaderReadOptions::default()),
            Err(FitsError::MissingEndCard { blocks_read: 1 })
        ));
    }

    #[test]
    fn enforces_header_block_limit() {
        let mut input = Cursor::new([b' '; BLOCK_SIZE * 2]);
        assert!(matches!(
            read_primary_header(&mut input, HeaderReadOptions { max_blocks: 1 }),
            Err(FitsError::HeaderBlockLimitExceeded { max_blocks: 1 })
        ));
    }
}
