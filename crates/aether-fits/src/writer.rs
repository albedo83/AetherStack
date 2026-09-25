use std::error::Error;
use std::fmt::{Display, Formatter};
use std::io::{self, Seek, SeekFrom, Write};

use aether_core::{Dimensions, PixelFlags, ScientificImage};

use crate::{
    BLOCK_SIZE, CARD_SIZE, FitsChecksum, checksum_aligned, combine_checksums, encode_checksum,
    is_negative_zero,
};

const INITIAL_CHECKSUM_VALUE: &str = "0000000000000000";

/// Canonical quiet-NaN payload used for unavailable floating FITS samples.
pub const CANONICAL_FITS_NAN_BITS: u64 = 0x7ff8_0000_0000_0000;
/// Version of the provenance cards emitted by this writer.
pub const FITS_OUTPUT_PROVENANCE_VERSION: u32 = 3;
/// Maximum byte length of a canonical output algorithm identifier.
pub const MAX_FITS_ALGORITHM_ID_BYTES: usize = 32;
/// Maximum byte length of a portable session group identifier.
pub const MAX_FITS_GROUP_ID_BYTES: usize = 64;

/// Validated provenance attached to one processed FITS product.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FitsOutputProvenance {
    manifest_sha256: String,
    plan_sha256: Option<String>,
    group_id: String,
    algorithm_id: String,
    source_count: u32,
    source_sha256: Option<String>,
}

impl FitsOutputProvenance {
    /// Builds provenance from canonical identifiers.
    ///
    /// The manifest digest is full lowercase SHA-256 hex. The group identifier
    /// uses the session manifest's portable ASCII form. The algorithm identifier
    /// starts with a lowercase ASCII letter or digit and may contain lowercase
    /// letters, digits, `.`, `_`, and `-`.
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
        if !is_group_id(&group_id) {
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
            plan_sha256: None,
            group_id,
            algorithm_id,
            source_count,
            source_sha256: None,
        })
    }

    /// SHA-256 of the exact canonical session-manifest bytes.
    #[must_use]
    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }

    /// Adds the SHA-256 of the exact canonical master-plan bytes.
    ///
    /// This binding is optional because products that do not depend on a master
    /// plan, such as a direct light integration, still use the same provenance
    /// type. Master products should always attach it.
    ///
    /// # Errors
    ///
    /// Returns [`FitsProvenanceError::InvalidPlanSha256`] unless the value is
    /// exactly 64 lowercase hexadecimal digits.
    pub fn with_plan_sha256(
        mut self,
        plan_sha256: impl Into<String>,
    ) -> Result<Self, FitsProvenanceError> {
        let plan_sha256 = plan_sha256.into();
        if !is_lower_sha256(&plan_sha256) {
            return Err(FitsProvenanceError::InvalidPlanSha256);
        }
        self.plan_sha256 = Some(plan_sha256);
        Ok(self)
    }

    /// SHA-256 of the exact canonical master-plan bytes, when applicable.
    #[must_use]
    pub fn plan_sha256(&self) -> Option<&str> {
        self.plan_sha256.as_deref()
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

    /// Binds a single-frame product to the SHA-256 of its exact input bytes.
    pub fn with_source_sha256(
        mut self,
        source_sha256: impl Into<String>,
    ) -> Result<Self, FitsProvenanceError> {
        if self.source_count != 1 {
            return Err(FitsProvenanceError::SourceDigestRequiresSingleSource);
        }
        let source_sha256 = source_sha256.into();
        if !is_lower_sha256(&source_sha256) {
            return Err(FitsProvenanceError::InvalidSourceSha256);
        }
        self.source_sha256 = Some(source_sha256);
        Ok(self)
    }

    /// Exact input digest for a single-frame product, when present.
    #[must_use]
    pub fn source_sha256(&self) -> Option<&str> {
        self.source_sha256.as_deref()
    }
}

/// Failure to construct canonical FITS output provenance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FitsProvenanceError {
    /// Manifest fingerprint is not 64 lowercase hexadecimal digits.
    InvalidManifestSha256,
    /// Master-plan fingerprint is not 64 lowercase hexadecimal digits.
    InvalidPlanSha256,
    /// Group identifier is empty, oversized, or non-portable.
    InvalidGroupId,
    /// Algorithm identifier is empty, oversized, or non-canonical.
    InvalidAlgorithmId,
    /// A processed output must represent at least one source.
    ZeroSourceCount,
    /// Per-source identity is not full lowercase SHA-256 hex.
    InvalidSourceSha256,
    /// Per-source identity cannot describe a multi-source product.
    SourceDigestRequiresSingleSource,
}

impl Display for FitsProvenanceError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidManifestSha256 => {
                formatter.write_str("manifest SHA-256 must be 64 lowercase hexadecimal digits")
            }
            Self::InvalidPlanSha256 => {
                formatter.write_str("master-plan SHA-256 must be 64 lowercase hexadecimal digits")
            }
            Self::InvalidGroupId => formatter.write_str("group identifier is not portable"),
            Self::InvalidAlgorithmId => {
                formatter.write_str("algorithm identifier is not canonical")
            }
            Self::ZeroSourceCount => formatter.write_str("source count must be greater than zero"),
            Self::InvalidSourceSha256 => {
                formatter.write_str("source SHA-256 must be 64 lowercase hexadecimal digits")
            }
            Self::SourceDigestRequiresSingleSource => {
                formatter.write_str("source SHA-256 requires exactly one represented source")
            }
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
    data_checksum: Option<u32>,
    encoded_checksum: Option<[u8; 16]>,
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

    /// One's-complement checksum stored by `DATASUM`, when generated.
    #[must_use]
    pub const fn data_checksum(self) -> Option<u32> {
        self.data_checksum
    }

    /// Recommended 16-byte `CHECKSUM` value, when generated.
    #[must_use]
    pub const fn encoded_checksum(self) -> Option<[u8; 16]> {
        self.encoded_checksum
    }
}

/// Failure while encoding a strict binary64 primary FITS image.
#[derive(Debug)]
pub enum FitsWriteError {
    /// A derived byte or sample count overflowed its representation.
    SizeOverflow,
    /// The bounded primary-header buffer could not be allocated.
    HeaderAllocationFailed {
        /// Exact capacity requested for the generated header.
        bytes: usize,
    },
    /// Image samples and mask entries violated their shared length invariant.
    ImageInvariant,
    /// A stream chunk would exceed the declared primary-image sample count.
    TooManySamples {
        /// Samples declared by the output dimensions.
        expected: usize,
        /// Samples that would have been written after accepting the chunk.
        attempted: usize,
    },
    /// Stream finalization was attempted before every declared sample arrived.
    IncompleteSamples {
        /// Samples declared by the output dimensions.
        expected: usize,
        /// Samples accepted by the writer.
        written: usize,
    },
    /// An internally generated card did not fit the 80-byte FITS card width.
    CardTooLong {
        /// Keyword associated with the card.
        keyword: &'static str,
    },
    /// Internal checksum patching did not produce FITS negative zero.
    ChecksumInvariant,
    /// The destination rejected header, data, or padding bytes.
    Io(io::Error),
}

impl Display for FitsWriteError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SizeOverflow => formatter.write_str("FITS output size overflows"),
            Self::HeaderAllocationFailed { bytes } => {
                write!(formatter, "cannot allocate {bytes}-byte FITS header buffer")
            }
            Self::ImageInvariant => {
                formatter.write_str("image sample and mask lengths do not match")
            }
            Self::TooManySamples {
                expected,
                attempted,
            } => write!(
                formatter,
                "FITS stream would contain {attempted} samples; expected {expected}"
            ),
            Self::IncompleteSamples { expected, written } => write!(
                formatter,
                "FITS stream contains {written} samples; expected {expected}"
            ),
            Self::CardTooLong { keyword } => {
                write!(
                    formatter,
                    "generated FITS card `{keyword}` exceeds 80 bytes"
                )
            }
            Self::ChecksumInvariant => {
                formatter.write_str("generated FITS checksum does not verify as negative zero")
            }
            Self::Io(error) => write!(formatter, "cannot write FITS output: {error}"),
        }
    }
}

impl Error for FitsWriteError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::SizeOverflow
            | Self::HeaderAllocationFailed { .. }
            | Self::ImageInvariant
            | Self::TooManySamples { .. }
            | Self::IncompleteSamples { .. }
            | Self::CardTooLong { .. }
            | Self::ChecksumInvariant => None,
        }
    }
}

/// Incremental binary64 primary-HDU encoder with exact sample accounting.
///
/// The caller supplies consecutive chunks in FITS planar order. Chunks may be
/// scan lines, tile-row bands, or a complete image; their boundaries do not
/// appear in the output stream. Masked and non-finite values use the same
/// canonical NaN encoding as the complete-image convenience functions.
pub struct F64PrimaryStreamWriter<W: Write> {
    writer: W,
    expected_samples: usize,
    written_samples: usize,
    substituted_samples: u64,
    padded_header_bytes: usize,
    checksum_header: Option<ChecksumHeader>,
    data_checksum: Option<FitsChecksum>,
}

struct ChecksumHeader {
    bytes: Vec<u8>,
    datasum_card_offset: usize,
    checksum_card_offset: usize,
}

struct PrimaryHeader {
    padded_bytes: usize,
    checksum: Option<ChecksumHeader>,
}

struct FinishedDataUnit<W> {
    writer: W,
    summary: FitsWriteSummary,
    checksum: Option<(ChecksumHeader, u32)>,
}

impl<W: Write> F64PrimaryStreamWriter<W> {
    /// Starts an incremental standards-conformant binary64 primary FITS image.
    ///
    /// # Errors
    ///
    /// Returns a typed size, card-encoding, or destination I/O error before any
    /// sample chunks are accepted. The destination may contain a valid header
    /// prefix when an error is returned.
    pub fn new(writer: W, dimensions: Dimensions) -> Result<Self, FitsWriteError> {
        Self::new_inner(writer, dimensions, None, false)
    }

    /// Starts an incremental binary64 primary FITS image with provenance.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::new`].
    pub fn new_with_provenance(
        writer: W,
        dimensions: Dimensions,
        provenance: &FitsOutputProvenance,
    ) -> Result<Self, FitsWriteError> {
        Self::new_inner(writer, dimensions, Some(provenance), false)
    }

    pub(crate) fn new_checksummed(
        writer: W,
        dimensions: Dimensions,
        provenance: Option<&FitsOutputProvenance>,
    ) -> Result<Self, FitsWriteError> {
        Self::new_inner(writer, dimensions, provenance, true)
    }

    fn new_inner(
        mut writer: W,
        dimensions: Dimensions,
        provenance: Option<&FitsOutputProvenance>,
        include_checksums: bool,
    ) -> Result<Self, FitsWriteError> {
        let header = write_primary_header(&mut writer, dimensions, provenance, include_checksums)?;
        Ok(Self {
            writer,
            expected_samples: dimensions.pixel_count(),
            written_samples: 0,
            substituted_samples: 0,
            padded_header_bytes: header.padded_bytes,
            checksum_header: header.checksum,
            data_checksum: include_checksums.then(FitsChecksum::new),
        })
    }

    /// Appends one consecutive sample chunk in canonical FITS order.
    ///
    /// # Errors
    ///
    /// Returns an invariant error when slice lengths differ, a count error when
    /// the chunk exceeds the dimensions declared in the header, or an I/O error
    /// if the destination rejects bytes.
    pub fn write_samples(
        &mut self,
        pixels: &[f64],
        flags: &[PixelFlags],
    ) -> Result<(), FitsWriteError> {
        if pixels.len() != flags.len() {
            return Err(FitsWriteError::ImageInvariant);
        }
        let attempted = self
            .written_samples
            .checked_add(pixels.len())
            .ok_or(FitsWriteError::SizeOverflow)?;
        if attempted > self.expected_samples {
            return Err(FitsWriteError::TooManySamples {
                expected: self.expected_samples,
                attempted,
            });
        }

        for (value, flags) in pixels.iter().zip(flags) {
            let stored = if flags.is_clear() && value.is_finite() {
                *value
            } else {
                self.substituted_samples = self
                    .substituted_samples
                    .checked_add(1)
                    .ok_or(FitsWriteError::SizeOverflow)?;
                f64::from_bits(CANONICAL_FITS_NAN_BITS)
            };
            let bytes = stored.to_be_bytes();
            self.writer.write_all(&bytes).map_err(FitsWriteError::Io)?;
            if let Some(checksum) = &mut self.data_checksum {
                checksum.update(&bytes);
            }
        }
        self.written_samples = attempted;
        Ok(())
    }

    /// Appends all samples from one image-shaped consecutive chunk.
    ///
    /// The chunk's own dimensions are intentionally not encoded; only its
    /// canonical pixel and mask order contributes to the enclosing image.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::write_samples`].
    pub fn write_image_chunk(&mut self, image: &ScientificImage) -> Result<(), FitsWriteError> {
        self.write_samples(image.pixels(), image.mask().as_slice())
    }

    /// Completes data padding and returns the underlying destination.
    ///
    /// This method does not flush the destination. Filesystem wrappers must
    /// flush and synchronize before publication.
    ///
    /// # Errors
    ///
    /// Returns [`FitsWriteError::IncompleteSamples`] unless every declared
    /// sample was supplied, or an I/O error while writing final zero padding.
    pub fn finish(self) -> Result<(W, FitsWriteSummary), FitsWriteError> {
        let finished = self.finish_data_unit()?;
        if finished.checksum.is_some() {
            return Err(FitsWriteError::ChecksumInvariant);
        }
        Ok((finished.writer, finished.summary))
    }

    fn finish_data_unit(mut self) -> Result<FinishedDataUnit<W>, FitsWriteError> {
        if self.written_samples != self.expected_samples {
            return Err(FitsWriteError::IncompleteSamples {
                expected: self.expected_samples,
                written: self.written_samples,
            });
        }
        let data_bytes = checked_data_bytes(self.expected_samples)?;
        let data_padding = block_padding(data_bytes);
        write_padding(&mut self.writer, 0, data_padding)?;
        if let Some(checksum) = &mut self.data_checksum {
            update_zero_padding(checksum, data_padding);
        }
        let bytes_written = self
            .padded_header_bytes
            .checked_add(data_bytes)
            .and_then(|bytes| bytes.checked_add(data_padding))
            .and_then(|bytes| u64::try_from(bytes).ok())
            .ok_or(FitsWriteError::SizeOverflow)?;
        let samples_written =
            u64::try_from(self.written_samples).map_err(|_| FitsWriteError::SizeOverflow)?;
        Ok(FinishedDataUnit {
            writer: self.writer,
            summary: FitsWriteSummary {
                samples_written,
                substituted_samples: self.substituted_samples,
                bytes_written,
                data_checksum: None,
                encoded_checksum: None,
            },
            checksum: self
                .checksum_header
                .zip(self.data_checksum.map(FitsChecksum::finish_zero_padded)),
        })
    }
}

impl<W: Write + Seek> F64PrimaryStreamWriter<W> {
    pub(crate) fn finish_with_checksums(self) -> Result<(W, FitsWriteSummary), FitsWriteError> {
        let finished = self.finish_data_unit()?;
        let mut writer = finished.writer;
        let mut summary = finished.summary;
        let Some((mut header, data_checksum)) = finished.checksum else {
            return Err(FitsWriteError::ChecksumInvariant);
        };

        replace_string_card(
            &mut header.bytes,
            header.datasum_card_offset,
            "DATASUM",
            &data_checksum.to_string(),
        )?;
        let initial_header_checksum =
            checksum_aligned(&header.bytes).map_err(|_| FitsWriteError::ChecksumInvariant)?;
        let hdu_checksum = combine_checksums(initial_header_checksum, data_checksum);
        let encoded_checksum = encode_checksum(hdu_checksum);
        let encoded_text: String = encoded_checksum.iter().copied().map(char::from).collect();
        replace_string_card(
            &mut header.bytes,
            header.checksum_card_offset,
            "CHECKSUM",
            &encoded_text,
        )?;

        let final_header_checksum =
            checksum_aligned(&header.bytes).map_err(|_| FitsWriteError::ChecksumInvariant)?;
        if !is_negative_zero(combine_checksums(final_header_checksum, data_checksum)) {
            return Err(FitsWriteError::ChecksumInvariant);
        }

        writer
            .seek(SeekFrom::Start(0))
            .and_then(|_| writer.write_all(&header.bytes))
            .and_then(|_| writer.seek(SeekFrom::End(0)).map(|_| ()))
            .map_err(FitsWriteError::Io)?;
        summary.data_checksum = Some(data_checksum);
        summary.encoded_checksum = Some(encoded_checksum);
        Ok((writer, summary))
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
/// `AETHVER`, `AETHMAN`, optional `AETHPLN`, `AETHGRP`, `AETHALG`, `AETHSRC`,
/// and optional single-source `AETHINP`
/// cards. Identifiers are validated by [`FitsOutputProvenance`] before any
/// output is accepted.
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
    let mut stream = match provenance {
        Some(provenance) => {
            F64PrimaryStreamWriter::new_with_provenance(writer, image.dimensions(), provenance)?
        }
        None => F64PrimaryStreamWriter::new(writer, image.dimensions())?,
    };
    stream.write_image_chunk(image)?;
    let (_writer, summary) = stream.finish()?;
    Ok(summary)
}

fn write_primary_header<W: Write>(
    writer: &mut W,
    dimensions: Dimensions,
    provenance: Option<&FitsOutputProvenance>,
    include_checksums: bool,
) -> Result<PrimaryHeader, FitsWriteError> {
    let axis_count = if dimensions.planes() == 1 { 2 } else { 3 };
    let mut header = Vec::new();
    header
        .try_reserve_exact(BLOCK_SIZE)
        .map_err(|_| FitsWriteError::HeaderAllocationFailed { bytes: BLOCK_SIZE })?;
    let mut header_bytes = 0_usize;

    write_fixed_card(&mut header, "SIMPLE", "T", &mut header_bytes)?;
    write_fixed_card(&mut header, "BITPIX", "-64", &mut header_bytes)?;
    write_fixed_card(
        &mut header,
        "NAXIS",
        &axis_count.to_string(),
        &mut header_bytes,
    )?;
    write_fixed_card(
        &mut header,
        "NAXIS1",
        &dimensions.width().to_string(),
        &mut header_bytes,
    )?;
    write_fixed_card(
        &mut header,
        "NAXIS2",
        &dimensions.height().to_string(),
        &mut header_bytes,
    )?;
    if axis_count == 3 {
        write_fixed_card(
            &mut header,
            "NAXIS3",
            &dimensions.planes().to_string(),
            &mut header_bytes,
        )?;
    }
    write_fixed_card(&mut header, "EXTEND", "T", &mut header_bytes)?;
    if let Some(provenance) = provenance {
        write_string_card(
            &mut header,
            "CREATOR",
            &format!("AetherStack {}", env!("CARGO_PKG_VERSION")),
            &mut header_bytes,
        )?;
        write_fixed_card(
            &mut header,
            "AETHVER",
            &FITS_OUTPUT_PROVENANCE_VERSION.to_string(),
            &mut header_bytes,
        )?;
        write_string_card(
            &mut header,
            "AETHMAN",
            provenance.manifest_sha256(),
            &mut header_bytes,
        )?;
        if let Some(plan_sha256) = provenance.plan_sha256() {
            write_string_card(&mut header, "AETHPLN", plan_sha256, &mut header_bytes)?;
        }
        write_string_card(
            &mut header,
            "AETHGRP",
            provenance.group_id(),
            &mut header_bytes,
        )?;
        write_string_card(
            &mut header,
            "AETHALG",
            provenance.algorithm_id(),
            &mut header_bytes,
        )?;
        write_fixed_card(
            &mut header,
            "AETHSRC",
            &provenance.source_count().to_string(),
            &mut header_bytes,
        )?;
        if let Some(source_sha256) = provenance.source_sha256() {
            write_string_card(&mut header, "AETHINP", source_sha256, &mut header_bytes)?;
        }
    }
    let checksum_offsets = if include_checksums {
        let datasum_card_offset = header_bytes;
        write_string_card(&mut header, "DATASUM", "0", &mut header_bytes)?;
        let checksum_card_offset = header_bytes;
        write_string_card(
            &mut header,
            "CHECKSUM",
            INITIAL_CHECKSUM_VALUE,
            &mut header_bytes,
        )?;
        Some((datasum_card_offset, checksum_card_offset))
    } else {
        None
    };
    write_card(&mut header, "END", "END", &mut header_bytes)?;
    let header_padding = block_padding(header_bytes);
    write_padding(&mut header, b' ', header_padding)?;
    let padded_bytes = header_bytes
        .checked_add(header_padding)
        .ok_or(FitsWriteError::SizeOverflow)?;
    if header.len() != padded_bytes {
        return Err(FitsWriteError::SizeOverflow);
    }
    writer.write_all(&header).map_err(FitsWriteError::Io)?;

    Ok(PrimaryHeader {
        padded_bytes,
        checksum: checksum_offsets.map(|(datasum_card_offset, checksum_card_offset)| {
            ChecksumHeader {
                bytes: header,
                datasum_card_offset,
                checksum_card_offset,
            }
        }),
    })
}

fn replace_string_card(
    header: &mut [u8],
    offset: usize,
    keyword: &'static str,
    value: &str,
) -> Result<(), FitsWriteError> {
    let card = format_string_card(keyword, value)?;
    let end = offset
        .checked_add(CARD_SIZE)
        .ok_or(FitsWriteError::SizeOverflow)?;
    let destination = header
        .get_mut(offset..end)
        .ok_or(FitsWriteError::ChecksumInvariant)?;
    destination.copy_from_slice(&card);
    Ok(())
}

fn format_string_card(
    keyword: &'static str,
    value: &str,
) -> Result<[u8; CARD_SIZE], FitsWriteError> {
    let escaped = value.replace('\'', "''");
    let text = format!("{keyword:<8}= '{escaped}'");
    format_card(keyword, &text)
}

fn write_string_card<W: Write>(
    writer: &mut W,
    keyword: &'static str,
    value: &str,
    header_bytes: &mut usize,
) -> Result<(), FitsWriteError> {
    let card = format_string_card(keyword, value)?;
    write_formatted_card(writer, &card, header_bytes)
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
    let card = format_card(keyword, text)?;
    write_formatted_card(writer, &card, header_bytes)
}

fn format_card(keyword: &'static str, text: &str) -> Result<[u8; CARD_SIZE], FitsWriteError> {
    let source = text.as_bytes();
    if source.len() > CARD_SIZE {
        return Err(FitsWriteError::CardTooLong { keyword });
    }
    let mut card = [b' '; CARD_SIZE];
    let Some(destination) = card.get_mut(..source.len()) else {
        return Err(FitsWriteError::CardTooLong { keyword });
    };
    destination.copy_from_slice(source);
    Ok(card)
}

fn write_formatted_card<W: Write>(
    writer: &mut W,
    card: &[u8; CARD_SIZE],
    header_bytes: &mut usize,
) -> Result<(), FitsWriteError> {
    writer.write_all(card).map_err(FitsWriteError::Io)?;
    *header_bytes = header_bytes
        .checked_add(CARD_SIZE)
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

fn update_zero_padding(checksum: &mut FitsChecksum, mut bytes: usize) {
    let block = [0_u8; BLOCK_SIZE];
    while bytes > 0 {
        let length = bytes.min(BLOCK_SIZE);
        checksum.update(&block[..length]);
        bytes -= length;
    }
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

fn is_group_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_FITS_GROUP_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
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
    fn incremental_chunks_are_byte_identical_to_complete_image_encoding()
    -> Result<(), Box<dyn Error>> {
        let dimensions = Dimensions::new(5, 1, 1)?;
        let mut image = ScientificImage::from_pixels(
            dimensions,
            vec![1.0, -2.5, f64::INFINITY, 4.0, f64::NAN],
        )?;
        image.mask_mut().as_mut_slice()[1] = PixelFlags::REJECTED;
        let provenance =
            FitsOutputProvenance::new("a".repeat(64), "light-001", "strict-mean-v1", 2)?;
        let mut complete = Vec::new();
        let complete_summary =
            write_f64_primary_with_provenance(&mut complete, &image, &provenance)?;

        let mut stream =
            F64PrimaryStreamWriter::new_with_provenance(Vec::new(), dimensions, &provenance)?;
        stream.write_samples(&image.pixels()[..2], &image.mask().as_slice()[..2])?;
        stream.write_samples(&image.pixels()[2..], &image.mask().as_slice()[2..])?;
        let (incremental, incremental_summary) = stream.finish()?;

        assert_eq!(incremental_summary, complete_summary);
        assert_eq!(incremental, complete);
        Ok(())
    }

    #[test]
    fn incremental_writer_rejects_incoherent_sample_counts() -> Result<(), Box<dyn Error>> {
        let dimensions = Dimensions::new(2, 1, 1)?;
        let mut mismatched = F64PrimaryStreamWriter::new(Vec::new(), dimensions)?;
        assert!(matches!(
            mismatched.write_samples(&[1.0], &[]),
            Err(FitsWriteError::ImageInvariant)
        ));

        let mut excessive = F64PrimaryStreamWriter::new(Vec::new(), dimensions)?;
        assert!(matches!(
            excessive.write_samples(
                &[1.0, 2.0, 3.0],
                &[PixelFlags::CLEAR, PixelFlags::CLEAR, PixelFlags::CLEAR]
            ),
            Err(FitsWriteError::TooManySamples {
                expected: 2,
                attempted: 3,
            })
        ));

        let mut incomplete = F64PrimaryStreamWriter::new(Vec::new(), dimensions)?;
        incomplete.write_samples(&[1.0], &[PixelFlags::CLEAR])?;
        assert!(matches!(
            incomplete.finish(),
            Err(FitsWriteError::IncompleteSamples {
                expected: 2,
                written: 1,
            })
        ));
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
        assert_eq!(provenance.plan_sha256(), None);
        assert_eq!(provenance.group_id(), "b".repeat(64));
        assert_eq!(provenance.algorithm_id(), "strict-mean-v1");
        assert_eq!(provenance.source_count(), 3);
        assert_eq!(provenance.source_sha256(), None);

        let explicit_group =
            FitsOutputProvenance::new("a".repeat(64), "light-001", "strict-mean-v1", 3)?;
        assert_eq!(explicit_group.group_id(), "light-001");

        assert!(matches!(
            FitsOutputProvenance::new("A".repeat(64), "b".repeat(64), "strict-mean-v1", 3),
            Err(FitsProvenanceError::InvalidManifestSha256)
        ));
        for invalid in ["", "bad/group", &"b".repeat(65)] {
            assert!(matches!(
                FitsOutputProvenance::new("a".repeat(64), invalid, "strict-mean-v1", 3),
                Err(FitsProvenanceError::InvalidGroupId)
            ));
        }
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

        let plan_digest = "c".repeat(64);
        let with_plan = provenance.clone().with_plan_sha256(plan_digest.clone())?;
        assert_eq!(with_plan.plan_sha256(), Some(plan_digest.as_str()));
        for invalid in ["", &"C".repeat(64), &"c".repeat(63), &"g".repeat(64)] {
            assert!(matches!(
                provenance.clone().with_plan_sha256(invalid),
                Err(FitsProvenanceError::InvalidPlanSha256)
            ));
        }
        assert!(matches!(
            provenance.clone().with_source_sha256("d".repeat(64)),
            Err(FitsProvenanceError::SourceDigestRequiresSingleSource)
        ));
        let single = FitsOutputProvenance::new(
            "a".repeat(64),
            "light-001",
            "strict-calibrated-light-v1",
            1,
        )?;
        assert!(matches!(
            single.clone().with_source_sha256("D".repeat(64)),
            Err(FitsProvenanceError::InvalidSourceSha256)
        ));
        assert_eq!(
            single.with_source_sha256("d".repeat(64))?.source_sha256(),
            Some("dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd")
        );
        Ok(())
    }

    #[test]
    fn writes_validated_provenance_cards() -> Result<(), Box<dyn Error>> {
        let image = image()?;
        let provenance = FitsOutputProvenance::new(
            "a".repeat(64),
            "b".repeat(64),
            "strict-calibrated-light-v1",
            1,
        )?
        .with_plan_sha256("c".repeat(64))?
        .with_source_sha256("d".repeat(64))?;
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
        assert_eq!(header.string("AETHPLN"), provenance.plan_sha256());
        assert_eq!(header.string("AETHGRP"), Some(provenance.group_id()));
        assert_eq!(header.string("AETHALG"), Some(provenance.algorithm_id()));
        assert_eq!(
            header.integer("AETHSRC"),
            Some(i64::from(provenance.source_count()))
        );
        assert_eq!(header.string("AETHINP"), provenance.source_sha256());
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
