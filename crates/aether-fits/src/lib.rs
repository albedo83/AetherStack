//! Safe FITS header reading and validation.
//!
//! The reader deliberately handles headers only. It retains every 80-byte card
//! so higher layers can explain normalization decisions or diagnose a
//! non-conformant file without rewriting the source.

mod card;
mod diagnostic;
mod error;
mod header;
mod image_hdu;
mod image_reader;
mod reader;

pub use card::{Card, FitsValue};
pub use diagnostic::{Diagnostic, DiagnosticCode, Severity, ValidationMode};
pub use error::FitsError;
pub use header::{Header, HeaderReport};
pub use image_hdu::{ImageHduDescriptor, ImageHduError, StoredSampleFormat};
pub use image_reader::{ImageReadError, PrimaryImageReader, SampleStatus};
pub use reader::{HeaderReadOptions, read_primary_header};

/// Size of a FITS card in bytes.
pub const CARD_SIZE: usize = 80;

/// Size of a FITS logical block in bytes.
pub const BLOCK_SIZE: usize = 2_880;
