use crate::{Card, Diagnostic, FitsValue, Severity, ValidationMode};

/// FITS header retaining every card in physical order.
#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    cards: Vec<Card>,
    blocks_read: usize,
}

impl Header {
    pub(crate) const fn new(cards: Vec<Card>, blocks_read: usize) -> Self {
        Self { cards, blocks_read }
    }

    /// Cards in physical order, including the `END` card.
    #[must_use]
    pub fn cards(&self) -> &[Card] {
        &self.cards
    }

    /// Number of 2,880-byte blocks consumed by the header.
    #[must_use]
    pub const fn blocks_read(&self) -> usize {
        self.blocks_read
    }

    /// First card that exactly matches the requested keyword.
    #[must_use]
    pub fn card(&self, keyword: &str) -> Option<&Card> {
        self.cards.iter().find(|card| card.keyword() == keyword)
    }

    /// All cards carrying the requested keyword.
    pub fn cards_named<'a>(&'a self, keyword: &'a str) -> impl Iterator<Item = &'a Card> {
        self.cards
            .iter()
            .filter(move |card| card.keyword() == keyword)
    }

    /// First value matching the requested keyword.
    #[must_use]
    pub fn value(&self, keyword: &str) -> Option<&FitsValue> {
        self.card(keyword).and_then(Card::value)
    }

    /// First integer value matching the requested keyword.
    #[must_use]
    pub fn integer(&self, keyword: &str) -> Option<i64> {
        self.value(keyword).and_then(FitsValue::as_i64)
    }

    /// First numeric value matching the requested keyword.
    #[must_use]
    pub fn number(&self, keyword: &str) -> Option<f64> {
        self.value(keyword).and_then(FitsValue::as_f64)
    }

    /// First string matching the requested keyword.
    #[must_use]
    pub fn string(&self, keyword: &str) -> Option<&str> {
        self.value(keyword).and_then(FitsValue::as_str)
    }

    /// First logical value matching the requested keyword.
    #[must_use]
    pub fn logical(&self, keyword: &str) -> Option<bool> {
        self.value(keyword).and_then(FitsValue::as_bool)
    }
}

/// Parsed header together with its conformance diagnostics.
#[derive(Clone, Debug, PartialEq)]
pub struct HeaderReport {
    header: Header,
    diagnostics: Vec<Diagnostic>,
}

impl HeaderReport {
    pub(crate) const fn new(header: Header, diagnostics: Vec<Diagnostic>) -> Self {
        Self {
            header,
            diagnostics,
        }
    }

    /// Interpreted header, including when diagnostics are present.
    #[must_use]
    pub const fn header(&self) -> &Header {
        &self.header
    }

    /// Diagnostics in detection order.
    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Returns whether the report contains no conformance error.
    #[must_use]
    pub fn is_conformant(&self) -> bool {
        self.diagnostics
            .iter()
            .all(|diagnostic| diagnostic.severity() != Severity::Error)
    }

    /// Applies an acceptance policy without discarding diagnostics.
    #[must_use]
    pub fn is_accepted(&self, mode: ValidationMode) -> bool {
        match mode {
            ValidationMode::Strict => self.is_conformant(),
            ValidationMode::Tolerant => true,
        }
    }

    /// Splits the report into its header and diagnostics.
    #[must_use]
    pub fn into_parts(self) -> (Header, Vec<Diagnostic>) {
        (self.header, self.diagnostics)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_mode_keeps_diagnostics_available() {
        let report = HeaderReport::new(
            Header::new(Vec::new(), 1),
            vec![Diagnostic::new(
                Severity::Error,
                crate::DiagnosticCode::MissingMandatoryCard,
                None,
                Some("SIMPLE".to_owned()),
                "missing SIMPLE",
            )],
        );

        assert!(!report.is_accepted(ValidationMode::Strict));
        assert!(report.is_accepted(ValidationMode::Tolerant));
        assert_eq!(report.diagnostics().len(), 1);
    }
}
