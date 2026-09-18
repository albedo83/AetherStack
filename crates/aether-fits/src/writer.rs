use std::error::Error;
use std::fmt::{Display, Formatter};
use std::io::{self, Write};

use aether_core::ScientificImage;

use crate::{BLOCK_SIZE, CARD_SIZE};

/// Canonical quiet-NaN payload used for unavailable floating FITS samples.
pub const CANONICAL_FITS_NAN_BITS: u64 = 0x7ff8_0000_0000_0000;
/// Version of the provenance cards emitted by this writer.
pub const FITS_OUTPUT_PROVENANCE_VERSION: u32 = 1;
/// Maximum byte length of a canonical output algorithm identifier.
pub const MAX_FITS_ALGORITHM_ID_BYTES: usize = 32;

/// Validated provenance attached to one processed FITS product.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FitsOutputProvenance {
    manifest_sha256: String,
    group_id: String,
    algorithm_id: String,
    source_count: u32,
}

impl FitsOutputProvenance {
    /// Builds provenance from canonical identifiers.
    ///
    /// The manifest digest and group identifier are full lowercase SHA-256 hex.
    /// The algorithm identifier starts with a lowercase ASCII letter or digit
    /// and may contain lowercase letters, digits, `.`, `_`, and `-`.
    ///
    /// # Errors
    ///
    /// Returns a typed error for a malformed identifier or zero source count.
    pub fn new(
        manifest_sha256: impl Into<String>,
        group_id: impl Into<String>,
        algorithm_id: impl Into<String>,
        source_count: u32,
    ) -> Result<Self, FitsProvenanceError> {
        let manifest_sha256 = manifest_sha256.into();
        let group_id = group_id.into();
        let algorithm_id = algorithm_id.into();
        if !is_lower_sha256(&manifest_sha256) {
            return Err(FitsProvenanceError::InvalidManifestSha256);
        }
        if !is_lower_sha256(&group_id) {
            return Err(FitsProvenanceError::InvalidGroupId);
        }
        if !is_algorithm_id(&algorithm_id) {
            return Err(FitsProvenanceError::InvalidAlgorithmId);
        }
        if source_count == 0 {
            return Err(FitsProvenanceError::ZeroSourceCount);
        }
        Ok(Self {
            manifest_sha256,
            group_id,
            algorithm_id,
            source_count,
        })
    }

    /// SHA-256 of the exact canonical session-manifest bytes.
    #[must_use]
    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }

    /// Exact session group identifier.
    #[must_use]
    pub fn group_id(&self) -> &str {
        &self.group_id
    }

    /// Versioned algorithm identifier.
    #[must_use]
    pub fn algorithm_id(&self) -> &str {
        &self.algorithm_id
    }

    /// Number of source images represented by the product.
    #[must_use]
    pub const fn source_count(&self) -> u32 {
        self.source_count
    }
}

/// Failure to construct canonical FITS output provenance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FitsProvenanceError {
    /// Manifest fingerprint is not 64 lowercase hexadecimal digits.
    InvalidManifestSha256,
    /// Group identifier is not 64 lowercase hexadecimal digits.
    InvalidGroupId,
    /// Algorithm identifier is empty, oversized, or non-canonical.
    InvalidAlgorithmId,
    /// A processed output must represent at least one source.
    ZeroSourceCount,
}

impl Display for FitsProvenanceError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidManifestSha256 => {
                formatter.write_str("manifest SHA-256 must be 64 lowercase hexadecimal digits")
            }
            Self::InvalidGroupId => {
                formatter.write_str("group identifier must be 64 lowercase hexadecimal digits")
            }
            Self::InvalidAlgorithmId => {
                formatter.write_str("algorithm identifier is not canonical")
            }
            Self::ZeroSourceCount => formatter.write_str("source count must be greater than zero"),
        }
    }
}

impl Error for FitsProvenanceError {}

/// Exact accounting returned after a successful primary-image write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FitsWriteSummary {
    samples_written: u64,
    substituted_samples: u64,
    bytes_written: u64,
}

impl FitsWriteSummary {
    /// Number of stored binary64 samples.
    #[must_use]
    pub const fn samples_written(self) -> u64 {
        self.samples_written
    }

    /// Samples replaced by canonical NaN because they were flagged or non-finite.
    #[must_use]
    pub const fn substituted_samples(self) -> u64 {
        self.substituted_samples
    }

    /// Complete FITS stream length including header and data padding.
    #[must_use]
    pub const fn bytes_written(self) -> u64 {
        self.bytes_written
    }
}

/// Failure while encoding a strict binary64 primary FITS image.
#[derive(Debug)]
pub enum FitsWriteError {
    /// A derived byte or sample count overflowed its representation.
    SizeOverflow,
    /// Image samples and mask entries violated their shared length invariant.
    ImageInvariant,
    /// An internally generated card did not fit the 80-byte FITS card width.
    CardTooLong {
        /// Keyword associated with the card.
        keyword: &'static str,
    },
    /// The destination rejected header, data, or padding bytes.
    Io(io::Error),
}

impl Display for FitsWriteError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SizeOverflow => formatter.write_str("FITS output size overflows"),
            Self::ImageInvariant => {
                formatter.write_str("image sample and mask lengths do not match")
            }
            Self::CardTooLong { keyword } => {
                write!(
                    formatter,
                    "generated FITS card `{keyword}` exceeds 80 bytes"
                )
            }
            Self::Io(error) => write!(formatter, "cannot write FITS output: {error}"),
        }
    }
}

impl Error for FitsWriteError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::SizeOverflow | Self::ImageInvariant | Self::CardTooLong { .. } => None,
        }
    }
}

/// Writes a standards-conformant binary64 primary FITS image.
///
/// Samples are emitted in the core image's planar order, which maps directly to
/// FITS axes `(x, y, plane)`. A one-plane image is encoded with two axes; more
/// planes use a third axis. Every clear finite value is preserved bit-for-bit in
/// big-endian form. A flagged, NaN, or infinite sample is replaced by one stable
/// quiet-NaN payload and counted in the returned summary.
///
/// The complete header and data unit are padded to 2,880-byte logical blocks.
/// This low-level function does not flush the destination and cannot make an
/// arbitrary stream atomic; filesystem publication is handled separately.
///
/// # Errors
///
/// Returns a typed error for size overflow, an internal image invariant failure,
/// an impossible generated card, or any destination I/O failure. The destination
/// may contain a valid prefix when an error is returned.
pub fn write_f64_primary<W: Write>(
    writer: &mut W,
    image: &ScientificImage,
) -> Result<FitsWriteSummary, FitsWriteError> {
    write_f64_primary_inner(writer, image, None)
}

/// Writes a binary64 primary FITS image with validated processing provenance.
///
/// In addition to [`write_f64_primary`]'s image contract, this emits `CREATOR`,
/// `AETHVER`, `AETHMAN`, `AETHGRP`, `AETHALG`, and `AETHSRC` cards. Identifiers
/// are validated by [`FitsOutputProvenance`] before any output is accepted.
///
/// # Errors
///
/// Returns the same stream and encoding errors as [`write_f64_primary`].
pub fn write_f64_primary_with_provenance<W: Write>(
    writer: &mut W,
    image: &ScientificImage,
    provenance: &FitsOutputProvenance,
) -> Result<FitsWriteSummary, FitsWriteError> {
    write_f64_primary_inner(writer, image, Some(provenance))
}

fn write_f64_primary_inner<W: Write>(
    writer: &mut W,
    image: &ScientificImage,
    provenance: Option<&FitsOutputProvenance>,
) -> Result<FitsWriteSummary, FitsWriteError> {
    if image.pixels().len() != image.mask().as_slice().len() {
        return Err(FitsWriteError::ImageInvariant);
    }
    let dimensions = image.dimensions();
    let axis_count = if dimensions.planes() == 1 { 2 } else { 3 };
    let mut header_bytes = 0_usize;

    write_fixed_card(writer, "SIMPLE", "T", &mut header_bytes)?;
    write_fixed_card(writer, "BITPIX", "-64", &mut header_bytes)?;
    write_fixed_card(writer, "NAXIS", &axis_count.to_string(), &mut header_bytes)?;
    write_fixed_card(
        writer,
        "NAXIS1",
        &dimensions.width().to_string(),
        &mut header_bytes,
    )?;
    write_fixed_card(
        writer,
        "NAXIS2",
        &dimensions.height().to_string(),
        &mut header_bytes,
    )?;
    if axis_count == 3 {
        write_fixed_card(
            writer,
            "NAXIS3",
            &dimensions.planes().to_string(),
            &mut header_bytes,
        )?;
    }
    write_fixed_card(writer, "EXTEND", "T", &mut header_bytes)?;
    if let Some(provenance) = provenance {
        write_string_card(
            writer,
            "CREATOR",
            &format!("AetherStack {}", env!("CARGO_PKG_VERSION")),
            &mut header_bytes,
        )?;
        write_fixed_card(
            writer,
            "AETHVER",
            &FITS_OUTPUT_PROVENANCE_VERSION.to_string(),
            &mut header_bytes,
        )?;
        write_string_card(
            writer,
            "AETHMAN",
            provenance.manifest_sha256(),
            &mut header_bytes,
        )?;
        write_string_card(writer, "AETHGRP", provenance.group_id(), &mut header_bytes)?;
        write_string_card(
            writer,
            "AETHALG",
            provenance.algorithm_id(),
            &mut header_bytes,
        )?;
        write_fixed_card(
            writer,
            "AETHSRC",
            &provenance.source_count().to_string(),
            &mut header_bytes,
        )?;
    }
    write_card(writer, "END", "END", &mut header_bytes)?;
    let header_padding = block_padding(header_bytes);
    write_padding(writer, b' ', header_padding)?;

    let mut substituted_samples = 0_u64;
    for (value, flags) in image.pixels().iter().zip(image.mask().as_slice()) {
        let stored = if flags.is_clear() && value.is_finite() {
            *value
        } else {
            substituted_samples = substituted_samples
                .checked_add(1)
                .ok_or(FitsWriteError::SizeOverflow)?;
            f64::from_bits(CANONICAL_FITS_NAN_BITS)
        };
        writer
            .write_all(&stored.to_be_bytes())
            .map_err(FitsWriteError::Io)?;
    }

    let data_bytes = checked_data_bytes(image.pixels().len())?;
    let data_padding = block_padding(data_bytes);
    write_padding(writer, 0, data_padding)?;
    let bytes_written = header_bytes
        .checked_add(header_padding)
        .and_then(|bytes| bytes.checked_add(data_bytes))
        .and_then(|bytes| bytes.checked_add(data_padding))
        .and_then(|bytes| u64::try_from(bytes).ok())
        .ok_or(FitsWriteError::SizeOverflow)?;
    let samples_written =
        u64::try_from(image.pixels().len()).map_err(|_| FitsWriteError::SizeOverflow)?;

    Ok(FitsWriteSummary {
        samples_written,
        substituted_samples,
        bytes_written,
    })
}

fn write_string_card<W: Write>(
    writer: &mut W,
    keyword: &'static str,
    value: &str,
    header_bytes: &mut usize,
) -> Result<(), FitsWriteError> {
    let escaped = value.replace('\'', "''");
    let text = format!("{keyword:<8}= '{escaped}'");
    write_card(writer, keyword, &text, header_bytes)
}

fn write_fixed_card<W: Write>(
    writer: &mut W,
    keyword: &'static str,
    value: &str,
    header_bytes: &mut usize,
) -> Result<(), FitsWriteError> {
    let text = format!("{keyword:<8}= {value:>20}");
    write_card(writer, keyword, &text, header_bytes)
}

fn write_card<W: Write>(
    writer: &mut W,
    keyword: &'static str,
    text: &str,
    header_bytes: &mut usize,
) -> Result<(), FitsWriteError> {
    let source = text.as_bytes();
    let Some(destination_length) = CARD_SIZE.checked_sub(source.len()) else {
        return Err(FitsWriteError::CardTooLong { keyword });
    };
    let mut card = [b' '; CARD_SIZE];
    let Some(destination) = card.get_mut(..source.len()) else {
        return Err(FitsWriteError::CardTooLong { keyword });
    };
    destination.copy_from_slice(source);
    writer.write_all(&card).map_err(FitsWriteError::Io)?;
    *header_bytes = header_bytes
        .checked_add(source.len())
        .and_then(|bytes| bytes.checked_add(destination_length))
        .ok_or(FitsWriteError::SizeOverflow)?;
    Ok(())
}

fn checked_data_bytes(samples: usize) -> Result<usize, FitsWriteError> {
    samples.checked_mul(8).ok_or(FitsWriteError::SizeOverflow)
}

fn block_padding(bytes: usize) -> usize {
    let remainder = bytes % BLOCK_SIZE;
    if remainder == 0 {
        0
    } else {
        BLOCK_SIZE - remainder
    }
}

fn write_padding<W: Write>(
    writer: &mut W,
    value: u8,
    mut bytes: usize,
) -> Result<(), FitsWriteError> {
    let block = [value; BLOCK_SIZE];
    while bytes > 0 {
        let length = bytes.min(BLOCK_SIZE);
        let Some(chunk) = block.get(..length) else {
            return Err(FitsWriteError::SizeOverflow);
        };
        writer.write_all(chunk).map_err(FitsWriteError::Io)?;
        bytes -= length;
    }
    Ok(())
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || matches!(*byte, b'a'..=b'f'))
}

fn is_algorithm_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes
        .first()
        .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && bytes.len() <= MAX_FITS_ALGORITHM_ID_BYTES
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(*byte, b'.' | b'_' | b'-')
        })
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use aether_core::{Dimensions, PixelFlags};

    use crate::{HeaderReadOptions, PrimaryImageReader, SampleStatus, StoredSampleFormat};

    use super::*;

    struct FailsAfter {
        remaining: usize,
    }

    impl Write for FailsAfter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            if self.remaining == 0 {
                return Err(io::Error::new(io::ErrorKind::WriteZero, "full"));
            }
            let written = buffer.len().min(self.remaining);
            self.remaining -= written;
            Ok(written)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn image() -> Result<ScientificImage, Box<dyn Error>> {
        let dimensions = Dimensions::new(2, 2, 1)?;
        Ok(ScientificImage::from_pixels(
            dimensions,
            vec![1.0, -2.5, 1.0 / 3.0, 4.0],
        )?)
    }

    #[test]
    fn writes_conformant_big_endian_binary64_and_round_trips() -> Result<(), Box<dyn Error>> {
        let image = image()?;
        let mut output = Vec::new();

        let summary = write_f64_primary(&mut output, &image)?;

        assert_eq!(summary.samples_written(), 4);
        assert_eq!(summary.substituted_samples(), 0);
        assert_eq!(summary.bytes_written(), (2 * BLOCK_SIZE) as u64);
        assert_eq!(output.len(), 2 * BLOCK_SIZE);
        assert_eq!(&output[BLOCK_SIZE..BLOCK_SIZE + 8], &1.0_f64.to_be_bytes());

        let mut reader =
            PrimaryImageReader::open(Cursor::new(output), HeaderReadOptions::default())?;
        assert!(reader.report().is_conformant());
        assert_eq!(
            reader.descriptor().sample_format(),
            StoredSampleFormat::Float64
        );
        assert_eq!(reader.descriptor().axes(), &[2, 2]);
        let mut values = [0.0; 4];
        let mut statuses = [SampleStatus::Undefined; 4];
        reader.read_physical_samples(0, &mut values, &mut statuses)?;
        let expected: Vec<u64> = image.pixels().iter().copied().map(f64::to_bits).collect();
        assert_eq!(values.map(f64::to_bits).as_slice(), expected.as_slice());
        assert_eq!(statuses, [SampleStatus::Valid; 4]);
        Ok(())
    }

    #[test]
    fn substitutes_masked_and_non_finite_values_with_one_nan_payload() -> Result<(), Box<dyn Error>>
    {
        let dimensions = Dimensions::new(3, 1, 1)?;
        let mut image = ScientificImage::from_pixels(
            dimensions,
            vec![42.0, f64::INFINITY, f64::from_bits(0x7ff8_1234_5678_9abc)],
        )?;
        image.mask_mut().as_mut_slice()[0] = PixelFlags::REJECTED;
        let mut output = Vec::new();

        let summary = write_f64_primary(&mut output, &image)?;

        assert_eq!(summary.substituted_samples(), 3);
        let canonical = CANONICAL_FITS_NAN_BITS.to_be_bytes();
        for sample in output[BLOCK_SIZE..BLOCK_SIZE + 24].chunks_exact(8) {
            assert_eq!(sample, canonical);
        }
        Ok(())
    }

    #[test]
    fn writes_a_third_axis_for_multi_plane_images() -> Result<(), Box<dyn Error>> {
        let dimensions = Dimensions::new(2, 1, 3)?;
        let image = ScientificImage::from_pixels(dimensions, vec![1.0; 6])?;
        let mut output = Vec::new();

        write_f64_primary(&mut output, &image)?;
        let reader = PrimaryImageReader::open(Cursor::new(output), HeaderReadOptions::default())?;

        assert_eq!(reader.descriptor().axes(), &[2, 1, 3]);
        Ok(())
    }

    #[test]
    fn validates_canonical_output_provenance() -> Result<(), Box<dyn Error>> {
        let provenance =
            FitsOutputProvenance::new("a".repeat(64), "b".repeat(64), "strict-mean-v1", 3)?;

        assert_eq!(provenance.manifest_sha256(), "a".repeat(64));
        assert_eq!(provenance.group_id(), "b".repeat(64));
        assert_eq!(provenance.algorithm_id(), "strict-mean-v1");
        assert_eq!(provenance.source_count(), 3);

        assert!(matches!(
            FitsOutputProvenance::new("A".repeat(64), "b".repeat(64), "strict-mean-v1", 3),
            Err(FitsProvenanceError::InvalidManifestSha256)
        ));
        assert!(matches!(
            FitsOutputProvenance::new("a".repeat(64), "b".repeat(63), "strict-mean-v1", 3),
            Err(FitsProvenanceError::InvalidGroupId)
        ));
        for invalid in [
            "",
            "Strict-mean-v1",
            "strict/mean",
            "-strict-mean",
            "an-algorithm-identifier-that-is-too-long",
        ] {
            assert!(matches!(
                FitsOutputProvenance::new("a".repeat(64), "b".repeat(64), invalid, 3),
                Err(FitsProvenanceError::InvalidAlgorithmId)
            ));
        }
        assert!(matches!(
            FitsOutputProvenance::new("a".repeat(64), "b".repeat(64), "strict-mean-v1", 0),
            Err(FitsProvenanceError::ZeroSourceCount)
        ));
        Ok(())
    }

    #[test]
    fn writes_validated_provenance_cards() -> Result<(), Box<dyn Error>> {
        let image = image()?;
        let provenance =
            FitsOutputProvenance::new("a".repeat(64), "b".repeat(64), "strict-mean-v1", 3)?;
        let mut output = Vec::new();

        write_f64_primary_with_provenance(&mut output, &image, &provenance)?;
        let reader = PrimaryImageReader::open(Cursor::new(output), HeaderReadOptions::default())?;
        let header = reader.report().header();

        assert!(reader.report().is_conformant());
        assert_eq!(
            header.string("CREATOR"),
            Some(concat!("AetherStack ", env!("CARGO_PKG_VERSION")))
        );
        assert_eq!(
            header.integer("AETHVER"),
            Some(i64::from(FITS_OUTPUT_PROVENANCE_VERSION))
        );
        assert_eq!(header.string("AETHMAN"), Some(provenance.manifest_sha256()));
        assert_eq!(header.string("AETHGRP"), Some(provenance.group_id()));
        assert_eq!(header.string("AETHALG"), Some(provenance.algorithm_id()));
        assert_eq!(
            header.integer("AETHSRC"),
            Some(i64::from(provenance.source_count()))
        );
        Ok(())
    }

    #[test]
    fn pads_unused_header_with_spaces_and_data_with_zeroes() -> Result<(), Box<dyn Error>> {
        let image = image()?;
        let mut output = Vec::new();
        write_f64_primary(&mut output, &image)?;

        let end_card = output[..BLOCK_SIZE]
            .chunks_exact(CARD_SIZE)
            .position(|card| card.starts_with(b"END"))
            .ok_or_else(|| io::Error::other("missing END card"))?;
        let after_end = (end_card + 1) * CARD_SIZE;
        assert!(
            output[after_end..BLOCK_SIZE]
                .iter()
                .all(|byte| *byte == b' ')
        );
        assert!(output[BLOCK_SIZE + 32..].iter().all(|byte| *byte == 0));
        Ok(())
    }

    #[test]
    fn propagates_destination_failures() -> Result<(), Box<dyn Error>> {
        let image = image()?;
        let mut writer = FailsAfter { remaining: 100 };

        assert!(matches!(
            write_f64_primary(&mut writer, &image),
            Err(FitsWriteError::Io(error)) if error.kind() == io::ErrorKind::WriteZero
        ));
        Ok(())
    }

    #[test]
    fn detects_impossible_data_size_before_multiplication_wraps() {
        assert!(matches!(
            checked_data_bytes(usize::MAX),
            Err(FitsWriteError::SizeOverflow)
        ));
    }
}
