use std::error::Error;
use std::fmt::{Display, Formatter};
use std::io::{self, Write};

use aether_core::ScientificImage;

use crate::{BLOCK_SIZE, CARD_SIZE};

/// Canonical quiet-NaN payload used for unavailable floating FITS samples.
pub const CANONICAL_FITS_NAN_BITS: u64 = 0x7ff8_0000_0000_0000;

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
