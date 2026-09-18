use std::error::Error;
use std::fmt::{Display, Formatter};

use crate::{BLOCK_SIZE, Header};

/// Primitive representation used to store one FITS image sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoredSampleFormat {
    /// Unsigned 8-bit integer (`BITPIX=8`).
    Unsigned8,
    /// Signed 16-bit integer (`BITPIX=16`).
    Signed16,
    /// Signed 32-bit integer (`BITPIX=32`).
    Signed32,
    /// Signed 64-bit integer (`BITPIX=64`).
    Signed64,
    /// IEEE 754 binary32 (`BITPIX=-32`).
    Float32,
    /// IEEE 754 binary64 (`BITPIX=-64`).
    Float64,
}

impl StoredSampleFormat {
    fn from_bitpix(bitpix: i64) -> Result<Self, ImageHduError> {
        match bitpix {
            8 => Ok(Self::Unsigned8),
            16 => Ok(Self::Signed16),
            32 => Ok(Self::Signed32),
            64 => Ok(Self::Signed64),
            -32 => Ok(Self::Float32),
            -64 => Ok(Self::Float64),
            value => Err(ImageHduError::UnsupportedBitpix { value }),
        }
    }

    /// Number of bytes occupied by one stored sample.
    #[must_use]
    pub const fn byte_width(self) -> u64 {
        match self {
            Self::Unsigned8 => 1,
            Self::Signed16 => 2,
            Self::Signed32 | Self::Float32 => 4,
            Self::Signed64 | Self::Float64 => 8,
        }
    }

    /// Returns whether the stored representation is integral.
    #[must_use]
    pub const fn is_integer(self) -> bool {
        matches!(
            self,
            Self::Unsigned8 | Self::Signed16 | Self::Signed32 | Self::Signed64
        )
    }

    const fn accepts_blank(self, value: i64) -> bool {
        match self {
            Self::Unsigned8 => value >= u8::MIN as i64 && value <= u8::MAX as i64,
            Self::Signed16 => value >= i16::MIN as i64 && value <= i16::MAX as i64,
            Self::Signed32 => value >= i32::MIN as i64 && value <= i32::MAX as i64,
            Self::Signed64 => true,
            Self::Float32 | Self::Float64 => false,
        }
    }
}

/// Checked layout and scaling metadata for a FITS image HDU.
///
/// All sizes are computed before pixel allocation. Offsets refer to the start of
/// the FITS stream containing the parsed header.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageHduDescriptor {
    sample_format: StoredSampleFormat,
    axes: Vec<u64>,
    pixel_count: u64,
    data_offset: u64,
    data_bytes: u64,
    padded_data_bytes: u64,
    padded_data_end: u64,
    bscale: f64,
    bzero: f64,
    blank: Option<i64>,
}

impl ImageHduDescriptor {
    /// Builds a checked descriptor from a parsed FITS header.
    ///
    /// # Errors
    ///
    /// Returns an error when a required keyword is missing or has the wrong
    /// type, an axis is invalid, scaling is non-finite, `BLANK` is used with a
    /// floating-point representation, or a derived size overflows `u64`.
    pub fn from_header(header: &Header) -> Result<Self, ImageHduError> {
        let bitpix = required_integer(header, "BITPIX")?;
        let sample_format = StoredSampleFormat::from_bitpix(bitpix)?;
        let axis_count_value = required_integer(header, "NAXIS")?;
        let axis_count = usize::try_from(axis_count_value)
            .ok()
            .filter(|value| *value <= 999)
            .ok_or(ImageHduError::InvalidAxisCount {
                value: axis_count_value,
            })?;

        let mut axes = Vec::with_capacity(axis_count);
        for axis in 1..=axis_count {
            let keyword = format!("NAXIS{axis}");
            let value = required_integer(header, &keyword)?;
            let length = u64::try_from(value).map_err(|_| ImageHduError::InvalidAxisLength {
                keyword: keyword.clone(),
                value,
            })?;
            axes.push(length);
        }

        let pixel_count = checked_pixel_count(&axes)?;
        let data_bytes = pixel_count
            .checked_mul(sample_format.byte_width())
            .ok_or(ImageHduError::DataSizeOverflow)?;
        let padded_data_bytes = padded_block_size(data_bytes)?;
        let data_offset = u64::try_from(header.blocks_read())
            .ok()
            .and_then(|blocks| blocks.checked_mul(BLOCK_SIZE as u64))
            .ok_or(ImageHduError::DataSizeOverflow)?;
        let padded_data_end = data_offset
            .checked_add(padded_data_bytes)
            .ok_or(ImageHduError::DataSizeOverflow)?;

        let bscale = optional_finite_number(header, "BSCALE")?.unwrap_or(1.0);
        let bzero = optional_finite_number(header, "BZERO")?.unwrap_or(0.0);
        let blank = optional_integer(header, "BLANK")?;
        if blank.is_some() && !sample_format.is_integer() {
            return Err(ImageHduError::BlankOnFloatingPointImage);
        }
        if let Some(value) = blank
            && !sample_format.accepts_blank(value)
        {
            return Err(ImageHduError::BlankOutOfRange {
                value,
                sample_format,
            });
        }

        Ok(Self {
            sample_format,
            axes,
            pixel_count,
            data_offset,
            data_bytes,
            padded_data_bytes,
            padded_data_end,
            bscale,
            bzero,
            blank,
        })
    }

    /// Stored primitive representation.
    #[must_use]
    pub const fn sample_format(&self) -> StoredSampleFormat {
        self.sample_format
    }

    /// Axis lengths in FITS order (`NAXIS1`, `NAXIS2`, and so on).
    #[must_use]
    pub fn axes(&self) -> &[u64] {
        &self.axes
    }

    /// Total number of stored samples, or zero for a header-only HDU.
    #[must_use]
    pub const fn pixel_count(&self) -> u64 {
        self.pixel_count
    }

    /// Byte offset of the first stored sample.
    #[must_use]
    pub const fn data_offset(&self) -> u64 {
        self.data_offset
    }

    /// Number of meaningful image-data bytes before FITS block padding.
    #[must_use]
    pub const fn data_bytes(&self) -> u64 {
        self.data_bytes
    }

    /// Number of bytes occupied by image data including FITS block padding.
    #[must_use]
    pub const fn padded_data_bytes(&self) -> u64 {
        self.padded_data_bytes
    }

    /// Minimum stream length covering the padded primary image data.
    ///
    /// A larger stream may contain extension HDUs and is therefore valid.
    #[must_use]
    pub const fn padded_data_end(&self) -> u64 {
        self.padded_data_end
    }

    /// Multiplicative physical-value scale, defaulting to one.
    #[must_use]
    pub const fn bscale(&self) -> f64 {
        self.bscale
    }

    /// Additive physical-value offset, defaulting to zero.
    #[must_use]
    pub const fn bzero(&self) -> f64 {
        self.bzero
    }

    /// Integer sentinel representing an undefined stored sample.
    #[must_use]
    pub const fn blank(&self) -> Option<i64> {
        self.blank
    }
}

/// Error raised while deriving a checked FITS image layout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImageHduError {
    /// A required keyword is absent.
    MissingKeyword {
        /// Missing FITS keyword.
        keyword: String,
    },
    /// A keyword expected to contain an integer has another type.
    KeywordNotInteger {
        /// Invalid FITS keyword.
        keyword: String,
    },
    /// A scaling keyword is not a finite number.
    KeywordNotFiniteNumber {
        /// Invalid FITS keyword.
        keyword: String,
    },
    /// `BITPIX` does not describe a standard image representation.
    UnsupportedBitpix {
        /// Unsupported `BITPIX` value.
        value: i64,
    },
    /// `NAXIS` is outside the standard range from zero through 999.
    InvalidAxisCount {
        /// Invalid `NAXIS` value.
        value: i64,
    },
    /// An axis length is negative.
    InvalidAxisLength {
        /// Axis keyword.
        keyword: String,
        /// Invalid signed value.
        value: i64,
    },
    /// A sample count, byte count, padding calculation, or offset overflowed.
    DataSizeOverflow,
    /// `BLANK` is only defined for integer arrays.
    BlankOnFloatingPointImage,
    /// `BLANK` cannot be represented by the stored integer type.
    BlankOutOfRange {
        /// Invalid sentinel value.
        value: i64,
        /// Stored integer representation.
        sample_format: StoredSampleFormat,
    },
}

impl ImageHduError {
    /// Stable category suitable for aggregate diagnostics.
    #[must_use]
    pub const fn code(&self) -> ImageHduErrorCode {
        match self {
            Self::MissingKeyword { .. } => ImageHduErrorCode::MissingKeyword,
            Self::KeywordNotInteger { .. } => ImageHduErrorCode::KeywordNotInteger,
            Self::KeywordNotFiniteNumber { .. } => ImageHduErrorCode::KeywordNotFiniteNumber,
            Self::UnsupportedBitpix { .. } => ImageHduErrorCode::UnsupportedBitpix,
            Self::InvalidAxisCount { .. } => ImageHduErrorCode::InvalidAxisCount,
            Self::InvalidAxisLength { .. } => ImageHduErrorCode::InvalidAxisLength,
            Self::DataSizeOverflow => ImageHduErrorCode::DataSizeOverflow,
            Self::BlankOnFloatingPointImage => ImageHduErrorCode::BlankOnFloatingPointImage,
            Self::BlankOutOfRange { .. } => ImageHduErrorCode::BlankOutOfRange,
        }
    }
}

/// Stable category of an image-HDU layout error.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ImageHduErrorCode {
    /// A required keyword is absent.
    MissingKeyword,
    /// An integer keyword has another value type.
    KeywordNotInteger,
    /// A scaling keyword is not finite and numeric.
    KeywordNotFiniteNumber,
    /// `BITPIX` is not a supported standard representation.
    UnsupportedBitpix,
    /// `NAXIS` lies outside its standard domain.
    InvalidAxisCount,
    /// An axis length is negative.
    InvalidAxisLength,
    /// A derived size or offset overflowed.
    DataSizeOverflow,
    /// `BLANK` was attached to a floating-point array.
    BlankOnFloatingPointImage,
    /// `BLANK` lies outside the stored integer domain.
    BlankOutOfRange,
}

impl Display for ImageHduErrorCode {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::MissingKeyword => "missing_keyword",
            Self::KeywordNotInteger => "keyword_not_integer",
            Self::KeywordNotFiniteNumber => "keyword_not_finite_number",
            Self::UnsupportedBitpix => "unsupported_bitpix",
            Self::InvalidAxisCount => "invalid_axis_count",
            Self::InvalidAxisLength => "invalid_axis_length",
            Self::DataSizeOverflow => "data_size_overflow",
            Self::BlankOnFloatingPointImage => "blank_on_floating_point_image",
            Self::BlankOutOfRange => "blank_out_of_range",
        };
        formatter.write_str(value)
    }
}

impl Display for ImageHduError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingKeyword { keyword } => {
                write!(formatter, "required FITS keyword {keyword} is missing")
            }
            Self::KeywordNotInteger { keyword } => {
                write!(formatter, "FITS keyword {keyword} must be an integer")
            }
            Self::KeywordNotFiniteNumber { keyword } => {
                write!(formatter, "FITS keyword {keyword} must be a finite number")
            }
            Self::UnsupportedBitpix { value } => {
                write!(formatter, "unsupported FITS BITPIX value {value}")
            }
            Self::InvalidAxisCount { value } => {
                write!(
                    formatter,
                    "FITS NAXIS must be between 0 and 999, received {value}"
                )
            }
            Self::InvalidAxisLength { keyword, value } => {
                write!(
                    formatter,
                    "FITS {keyword} must be non-negative, received {value}"
                )
            }
            Self::DataSizeOverflow => formatter.write_str("FITS image data size overflows u64"),
            Self::BlankOnFloatingPointImage => {
                formatter.write_str("FITS BLANK is not valid for a floating-point image")
            }
            Self::BlankOutOfRange {
                value,
                sample_format,
            } => write!(
                formatter,
                "FITS BLANK value {value} is outside the range of {sample_format:?}"
            ),
        }
    }
}

impl Error for ImageHduError {}

fn required_integer(header: &Header, keyword: &str) -> Result<i64, ImageHduError> {
    if let Some(value) = header.integer(keyword) {
        return Ok(value);
    }
    if header.card(keyword).is_some() {
        Err(ImageHduError::KeywordNotInteger {
            keyword: keyword.to_owned(),
        })
    } else {
        Err(ImageHduError::MissingKeyword {
            keyword: keyword.to_owned(),
        })
    }
}

fn optional_integer(header: &Header, keyword: &str) -> Result<Option<i64>, ImageHduError> {
    if let Some(value) = header.integer(keyword) {
        return Ok(Some(value));
    }
    if header.card(keyword).is_some() {
        Err(ImageHduError::KeywordNotInteger {
            keyword: keyword.to_owned(),
        })
    } else {
        Ok(None)
    }
}

fn optional_finite_number(header: &Header, keyword: &str) -> Result<Option<f64>, ImageHduError> {
    match header.number(keyword) {
        Some(value) if value.is_finite() => Ok(Some(value)),
        Some(_) => Err(ImageHduError::KeywordNotFiniteNumber {
            keyword: keyword.to_owned(),
        }),
        None if header.card(keyword).is_some() => Err(ImageHduError::KeywordNotFiniteNumber {
            keyword: keyword.to_owned(),
        }),
        None => Ok(None),
    }
}

fn checked_pixel_count(axes: &[u64]) -> Result<u64, ImageHduError> {
    if axes.is_empty() {
        return Ok(0);
    }
    axes.iter().try_fold(1_u64, |count, axis| {
        count
            .checked_mul(*axis)
            .ok_or(ImageHduError::DataSizeOverflow)
    })
}

fn padded_block_size(data_bytes: u64) -> Result<u64, ImageHduError> {
    if data_bytes == 0 {
        return Ok(0);
    }
    let block_size = BLOCK_SIZE as u64;
    let blocks = data_bytes
        .checked_add(block_size - 1)
        .ok_or(ImageHduError::DataSizeOverflow)?
        / block_size;
    blocks
        .checked_mul(block_size)
        .ok_or(ImageHduError::DataSizeOverflow)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use crate::{CARD_SIZE, HeaderReadOptions, read_primary_header};

    use super::*;

    fn header(cards: &[String]) -> Option<Header> {
        let mut block = [b' '; BLOCK_SIZE];
        for (index, text) in cards
            .iter()
            .chain(std::iter::once(&"END".to_owned()))
            .enumerate()
        {
            let destination = block.get_mut(index * CARD_SIZE..(index + 1) * CARD_SIZE)?;
            let bytes = text.as_bytes();
            let length = bytes.len().min(CARD_SIZE);
            destination[..length].copy_from_slice(&bytes[..length]);
        }
        read_primary_header(&mut Cursor::new(block), HeaderReadOptions::default())
            .ok()
            .map(|report| report.into_parts().0)
    }

    fn integer_card(keyword: &str, value: i64) -> String {
        format!("{keyword:<8}= {value:>20}")
    }

    fn float_card(keyword: &str, value: &str) -> String {
        format!("{keyword:<8}= {value:>20}")
    }

    fn image_cards(bitpix: i64, axes: &[i64]) -> Vec<String> {
        let mut cards = vec![
            format!("{:<8}= {:>20}", "SIMPLE", "T"),
            integer_card("BITPIX", bitpix),
            integer_card("NAXIS", axes.len() as i64),
        ];
        cards.extend(
            axes.iter()
                .enumerate()
                .map(|(index, value)| integer_card(&format!("NAXIS{}", index + 1), *value)),
        );
        cards
    }

    #[test]
    fn describes_unsigned_convention_without_allocating_pixels() {
        let mut cards = image_cards(16, &[4_144, 2_822]);
        cards.push(float_card("BSCALE", "1.0"));
        cards.push(float_card("BZERO", "32768.0"));
        cards.push(integer_card("BLANK", -32_768));
        let Some(header) = header(&cards) else {
            return;
        };

        let result = ImageHduDescriptor::from_header(&header);
        assert!(result.is_ok());
        let Some(descriptor) = result.ok() else {
            return;
        };
        assert_eq!(descriptor.sample_format(), StoredSampleFormat::Signed16);
        assert_eq!(descriptor.axes(), &[4_144, 2_822]);
        assert_eq!(descriptor.pixel_count(), 11_694_368);
        assert_eq!(descriptor.data_offset(), 2_880);
        assert_eq!(descriptor.data_bytes(), 23_388_736);
        assert_eq!(descriptor.padded_data_bytes() % 2_880, 0);
        assert_eq!(
            descriptor.padded_data_end(),
            descriptor.data_offset() + descriptor.padded_data_bytes()
        );
        assert_eq!(descriptor.bscale().to_bits(), 1.0_f64.to_bits());
        assert_eq!(descriptor.bzero().to_bits(), 32_768.0_f64.to_bits());
        assert_eq!(descriptor.blank(), Some(-32_768));
    }

    #[test]
    fn header_only_hdu_has_no_pixel_bytes() {
        let Some(header) = header(&image_cards(8, &[])) else {
            return;
        };
        let result = ImageHduDescriptor::from_header(&header);
        assert!(result.is_ok());
        let Some(descriptor) = result.ok() else {
            return;
        };

        assert_eq!(descriptor.pixel_count(), 0);
        assert_eq!(descriptor.data_bytes(), 0);
        assert_eq!(descriptor.padded_data_bytes(), 0);
    }

    #[test]
    fn rejects_missing_axis_card() {
        let cards = vec![
            format!("{:<8}= {:>20}", "SIMPLE", "T"),
            integer_card("BITPIX", 16),
            integer_card("NAXIS", 2),
            integer_card("NAXIS1", 16),
        ];
        let Some(header) = header(&cards) else {
            return;
        };

        assert_eq!(
            ImageHduDescriptor::from_header(&header),
            Err(ImageHduError::MissingKeyword {
                keyword: "NAXIS2".to_owned()
            })
        );
    }

    #[test]
    fn rejects_negative_axis_length() {
        let Some(header) = header(&image_cards(16, &[32, -1])) else {
            return;
        };

        assert_eq!(
            ImageHduDescriptor::from_header(&header),
            Err(ImageHduError::InvalidAxisLength {
                keyword: "NAXIS2".to_owned(),
                value: -1
            })
        );
    }

    #[test]
    fn rejects_sample_count_overflow() {
        let Some(header) = header(&image_cards(64, &[i64::MAX, 3])) else {
            return;
        };

        assert_eq!(
            ImageHduDescriptor::from_header(&header),
            Err(ImageHduError::DataSizeOverflow)
        );
    }

    #[test]
    fn rejects_blank_for_floating_point_pixels() {
        let mut cards = image_cards(-32, &[10, 10]);
        cards.push(integer_card("BLANK", 0));
        let Some(header) = header(&cards) else {
            return;
        };

        assert_eq!(
            ImageHduDescriptor::from_header(&header),
            Err(ImageHduError::BlankOnFloatingPointImage)
        );
    }

    #[test]
    fn rejects_blank_outside_stored_integer_range() {
        let mut cards = image_cards(16, &[10, 10]);
        cards.push(integer_card("BLANK", 32_768));
        let Some(header) = header(&cards) else {
            return;
        };

        assert_eq!(
            ImageHduDescriptor::from_header(&header),
            Err(ImageHduError::BlankOutOfRange {
                value: 32_768,
                sample_format: StoredSampleFormat::Signed16
            })
        );
    }

    #[test]
    fn rejects_nonstandard_bitpix() {
        let Some(header) = header(&image_cards(24, &[10, 10])) else {
            return;
        };

        assert_eq!(
            ImageHduDescriptor::from_header(&header),
            Err(ImageHduError::UnsupportedBitpix { value: 24 })
        );
    }

    #[test]
    fn exposes_stable_error_codes() {
        let error = ImageHduError::InvalidAxisLength {
            keyword: "NAXIS2".to_owned(),
            value: -1,
        };

        assert_eq!(error.code(), ImageHduErrorCode::InvalidAxisLength);
        assert_eq!(error.code().to_string(), "invalid_axis_length");
    }

    #[test]
    fn rejects_non_finite_scaling() {
        let mut cards = image_cards(-32, &[10, 10]);
        cards.push(float_card("BSCALE", "NaN"));
        let Some(header) = header(&cards) else {
            return;
        };

        assert_eq!(
            ImageHduDescriptor::from_header(&header),
            Err(ImageHduError::KeywordNotFiniteNumber {
                keyword: "BSCALE".to_owned()
            })
        );
    }
}
