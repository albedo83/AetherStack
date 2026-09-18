use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};

/// Severity of a FITS conformance diagnostic.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Deviation that retains an unambiguous interpretation.
    Warning,
    /// Standard violation that must fail strict mode.
    Error,
}

/// Stable category of a conformance diagnostic.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCode {
    /// A card contains a byte forbidden in a FITS header.
    NonAsciiCard,
    /// A standard keyword name contains an illegal character.
    IllegalKeyword,
    /// A mandatory card is missing.
    MissingMandatoryCard,
    /// A mandatory card is not in its required position.
    MandatoryCardOrder,
    /// A mandatory card value has the wrong type or domain.
    InvalidMandatoryValue,
    /// A mandatory logical or integer value does not end in column 30.
    NonStandardFixedValue,
    /// Bytes following `END` in the current block are not all spaces.
    NonBlankHeaderPadding,
}

impl Display for DiagnosticCode {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::NonAsciiCard => "non_ascii_card",
            Self::IllegalKeyword => "illegal_keyword",
            Self::MissingMandatoryCard => "missing_mandatory_card",
            Self::MandatoryCardOrder => "mandatory_card_order",
            Self::InvalidMandatoryValue => "invalid_mandatory_value",
            Self::NonStandardFixedValue => "non_standard_fixed_value",
            Self::NonBlankHeaderPadding => "non_blank_header_padding",
        };
        formatter.write_str(value)
    }
}

/// Detailed diagnostic associated with a card or the complete header.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Diagnostic {
    severity: Severity,
    code: DiagnosticCode,
    card_index: Option<usize>,
    keyword: Option<String>,
    message: String,
}

impl Diagnostic {
    /// Builds a structured diagnostic.
    #[must_use]
    pub fn new(
        severity: Severity,
        code: DiagnosticCode,
        card_index: Option<usize>,
        keyword: Option<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity,
            code,
            card_index,
            keyword,
            message: message.into(),
        }
    }

    /// Diagnostic severity.
    #[must_use]
    pub const fn severity(&self) -> Severity {
        self.severity
    }

    /// Stable diagnostic category.
    #[must_use]
    pub const fn code(&self) -> DiagnosticCode {
        self.code
    }

    /// Zero-based index of the affected card, when applicable.
    #[must_use]
    pub const fn card_index(&self) -> Option<usize> {
        self.card_index
    }

    /// Affected keyword, when applicable.
    #[must_use]
    pub fn keyword(&self) -> Option<&str> {
        self.keyword.as_deref()
    }

    /// Human-readable explanation.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Policy for accepting deviations from the FITS standard.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationMode {
    /// Rejects every error-level diagnostic.
    Strict,
    /// Accepts interpretable headers and retains their diagnostics.
    Tolerant,
}
