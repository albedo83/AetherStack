use crate::CARD_SIZE;

/// Interpreted value of a FITS card.
#[derive(Clone, Debug, PartialEq)]
pub enum FitsValue {
    /// FITS logical value `T` or `F`.
    Logical(bool),
    /// Signed integer representable in 64 bits.
    Integer(i64),
    /// Floating-point number, including FITS notation with a `D` exponent.
    Float(f64),
    /// FITS string delimited by single quotes.
    String(String),
    /// Empty value field.
    Undefined,
    /// Value retained as text when it is not recognized.
    Raw(String),
}

impl FitsValue {
    /// Returns the logical value when this value has that type.
    #[must_use]
    pub const fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Logical(value) => Some(*value),
            _ => None,
        }
    }

    /// Returns the integer value when this value has that type.
    #[must_use]
    pub const fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Integer(value) => Some(*value),
            _ => None,
        }
    }

    /// Returns a numeric value as `f64`.
    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Integer(value) => Some(*value as f64),
            Self::Float(value) => Some(*value),
            _ => None,
        }
    }

    /// Returns the string when this value has that type.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }
}

/// An 80-byte FITS card together with its interpretation.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    raw: [u8; CARD_SIZE],
    keyword: String,
    value: Option<FitsValue>,
    comment: Option<String>,
}

impl Card {
    pub(crate) fn parse(raw: [u8; CARD_SIZE]) -> Self {
        let keyword = String::from_utf8_lossy(&raw[..8]).trim().to_owned();
        let has_value_indicator = raw[8] == b'=';

        if has_value_indicator {
            let field = String::from_utf8_lossy(&raw[10..]);
            let (value_text, comment) = split_value_and_comment(&field);
            return Self {
                raw,
                keyword,
                value: Some(parse_value(&value_text)),
                comment,
            };
        }

        let text = String::from_utf8_lossy(&raw[8..]).trim_end().to_owned();
        Self {
            raw,
            keyword,
            value: None,
            comment: (!text.is_empty()).then_some(text),
        }
    }

    /// The original 80 bytes, without normalization.
    #[must_use]
    pub const fn raw(&self) -> &[u8; CARD_SIZE] {
        &self.raw
    }

    /// Keyword from the first eight bytes, without trailing spaces.
    #[must_use]
    pub fn keyword(&self) -> &str {
        &self.keyword
    }

    /// Interpreted value, when the card uses the `= ` indicator.
    #[must_use]
    pub const fn value(&self) -> Option<&FitsValue> {
        self.value.as_ref()
    }

    /// Comment after the `/` separator, or the text of a commentary card.
    #[must_use]
    pub fn comment(&self) -> Option<&str> {
        self.comment.as_deref()
    }

    pub(crate) fn has_standard_value_indicator(&self) -> bool {
        self.raw[8] == b'=' && self.raw[9] == b' '
    }

    pub(crate) fn has_fixed_value_ending_at_column_30(&self) -> bool {
        let value_field = &self.raw[10..30];
        value_field
            .iter()
            .rposition(|byte| *byte != b' ')
            .is_some_and(|position| position == value_field.len() - 1)
    }
}

fn split_value_and_comment(field: &str) -> (String, Option<String>) {
    let bytes = field.as_bytes();
    let mut in_string = false;
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'\'' if in_string && bytes.get(index + 1) == Some(&b'\'') => {
                index += 2;
                continue;
            }
            b'\'' => in_string = !in_string,
            b'/' if !in_string => {
                let value = field[..index].trim_end().to_owned();
                let comment = field[index + 1..].trim().to_owned();
                return (value, (!comment.is_empty()).then_some(comment));
            }
            _ => {}
        }
        index += 1;
    }

    (field.trim_end().to_owned(), None)
}

fn parse_value(value: &str) -> FitsValue {
    let value = value.trim();
    if value.is_empty() {
        return FitsValue::Undefined;
    }
    if value == "T" {
        return FitsValue::Logical(true);
    }
    if value == "F" {
        return FitsValue::Logical(false);
    }
    if value.starts_with('\'') {
        return FitsValue::String(parse_quoted_string(value));
    }
    if let Ok(integer) = value.parse::<i64>() {
        return FitsValue::Integer(integer);
    }

    let normalized_exponent = value.replace(['D', 'd'], "E");
    if let Ok(float) = normalized_exponent.parse::<f64>() {
        return FitsValue::Float(float);
    }

    FitsValue::Raw(value.to_owned())
}

fn parse_quoted_string(value: &str) -> String {
    let mut output = String::new();
    let mut characters = value.chars();
    if characters.next() != Some('\'') {
        return value.to_owned();
    }

    let mut pending_quote = false;
    for character in characters {
        if pending_quote {
            if character == '\'' {
                output.push('\'');
                pending_quote = false;
                continue;
            }
            break;
        }
        if character == '\'' {
            pending_quote = true;
        } else {
            output.push(character);
        }
    }

    output.trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_card(text: &str) -> [u8; CARD_SIZE] {
        let mut raw = [b' '; CARD_SIZE];
        let bytes = text.as_bytes();
        let length = bytes.len().min(CARD_SIZE);
        raw[..length].copy_from_slice(&bytes[..length]);
        raw
    }

    #[test]
    fn parses_integer_and_comment() {
        let card = Card::parse(raw_card(
            "BITPIX  =                   16 / number of bits per pixel",
        ));
        assert_eq!(card.keyword(), "BITPIX");
        assert_eq!(card.value(), Some(&FitsValue::Integer(16)));
        assert_eq!(card.comment(), Some("number of bits per pixel"));
    }

    #[test]
    fn slash_inside_string_is_not_a_comment() {
        let card = Card::parse(raw_card("OBJECT  = 'SH2-136 / Flying Bat' / target name"));
        assert_eq!(
            card.value(),
            Some(&FitsValue::String("SH2-136 / Flying Bat".to_owned()))
        );
        assert_eq!(card.comment(), Some("target name"));
    }

    #[test]
    fn parses_escaped_quote_in_string() {
        let card = Card::parse(raw_card("OBJECT  = 'Barnard''s Galaxy'"));
        assert_eq!(
            card.value(),
            Some(&FitsValue::String("Barnard's Galaxy".to_owned()))
        );
    }

    #[test]
    fn parses_fortran_double_exponent() {
        let card = Card::parse(raw_card("EXPTIME =              1.25D+02"));
        assert_eq!(card.value(), Some(&FitsValue::Float(125.0)));
    }

    #[test]
    fn preserves_raw_unrecognized_value() {
        let card = Card::parse(raw_card("CUSTOM  = COMPLEX-VALUE"));
        assert_eq!(
            card.value(),
            Some(&FitsValue::Raw("COMPLEX-VALUE".to_owned()))
        );
    }
}
