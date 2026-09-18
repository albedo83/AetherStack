//! Safe FITS header validation and bounded primary-image reading.
//!
//! The parser retains every 80-byte card so higher layers can explain
//! normalization decisions or diagnose a non-conformant file without rewriting
//! the source. Pixel access is seek-based and can materialize bounded regions in
//! the shared scientific image representation.

mod atomic_writer;
mod card;
mod diagnostic;
mod error;
mod header;
mod image_hdu;
mod image_reader;
mod reader;
mod writer;

pub use atomic_writer::{
    AtomicFitsWriteError, write_f64_primary_atomic_new,
    write_f64_primary_atomic_new_with_provenance,
};
pub use card::{Card, FitsValue};
pub use diagnostic::{Diagnostic, DiagnosticCode, Severity, ValidationMode};
pub use error::FitsError;
pub use header::{Header, HeaderReport};
pub use image_hdu::{ImageHduDescriptor, ImageHduError, ImageHduErrorCode, StoredSampleFormat};
pub use image_reader::{ImageReadError, ImageRegion, PrimaryImageReader, SampleStatus};
pub use reader::{HeaderReadOptions, read_primary_header};
pub use writer::{
    CANONICAL_FITS_NAN_BITS, FITS_OUTPUT_PROVENANCE_VERSION, FitsOutputProvenance,
    FitsProvenanceError, FitsWriteError, FitsWriteSummary, MAX_FITS_ALGORITHM_ID_BYTES,
    MAX_FITS_GROUP_ID_BYTES, write_f64_primary, write_f64_primary_with_provenance,
};

/// Size of a FITS card in bytes.
pub const CARD_SIZE: usize = 80;

/// Size of a FITS logical block in bytes.
pub const BLOCK_SIZE: usize = 2_880;
