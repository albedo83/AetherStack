use std::error::Error;
use std::fmt::{Display, Formatter};
use std::io::{Read, Seek, SeekFrom};

use crate::{
    FitsError, HeaderReadOptions, HeaderReport, ImageHduDescriptor, ImageHduError,
    StoredSampleFormat, read_primary_header,
};

const SCRATCH_BYTES: usize = 8 * 1_024;

/// Validity state assigned while decoding a stored FITS sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SampleStatus {
    /// The physical value is finite and usable.
    Valid,
    /// The stored integer equals the HDU's `BLANK` sentinel.
    Undefined,
    /// The stored or scaled floating-point value is NaN or infinite.
    NonFinite,
}

/// Rectangular region within one FITS image plane.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageRegion {
    plane: u64,
    x: u64,
    y: u64,
    width: u64,
    height: u64,
}

impl ImageRegion {
    /// Builds a region. Bounds are checked against an image when it is read.
    #[must_use]
    pub const fn new(plane: u64, x: u64, y: u64, width: u64, height: u64) -> Self {
        Self {
            plane,
            x,
            y,
            width,
            height,
        }
    }

    /// Zero-based image plane.
    #[must_use]
    pub const fn plane(self) -> u64 {
        self.plane
    }

    /// First source column.
    #[must_use]
    pub const fn x(self) -> u64 {
        self.x
    }

    /// First source row.
    #[must_use]
    pub const fn y(self) -> u64 {
        self.y
    }

    /// Region width.
    #[must_use]
    pub const fn width(self) -> u64 {
        self.width
    }

    /// Region height.
    #[must_use]
    pub const fn height(self) -> u64 {
        self.height
    }
}

/// Seekable reader for the pixel array of a primary FITS image.
///
/// The reader owns the stream and keeps the full header report available. The
/// current vertical slice decodes the two representations present in the
/// priority corpus: signed 16-bit integers and IEEE 754 binary32 values. Other
/// standard `BITPIX` values remain describable but return an explicit error when
/// pixel decoding is requested.
pub struct PrimaryImageReader<R> {
    reader: R,
    report: HeaderReport,
    descriptor: ImageHduDescriptor,
}

impl<R: Read + Seek> PrimaryImageReader<R> {
    /// Reads the primary header and prepares checked random access to its pixels.
    ///
    /// Header conformance diagnostics remain available through [`Self::report`].
    /// Callers choose strict or tolerant acceptance before processing pixels.
    ///
    /// # Errors
    ///
    /// Returns an error when the header cannot be read or its image layout is
    /// invalid.
    pub fn open(mut reader: R, options: HeaderReadOptions) -> Result<Self, ImageReadError> {
        let report = read_primary_header(&mut reader, options).map_err(ImageReadError::Header)?;
        let descriptor =
            ImageHduDescriptor::from_header(report.header()).map_err(ImageReadError::Descriptor)?;
        Ok(Self {
            reader,
            report,
            descriptor,
        })
    }

    /// Parsed header and all of its conformance diagnostics.
    #[must_use]
    pub const fn report(&self) -> &HeaderReport {
        &self.report
    }

    /// Checked image layout and scaling metadata.
    #[must_use]
    pub const fn descriptor(&self) -> &ImageHduDescriptor {
        &self.descriptor
    }

    /// Returns the owned input stream.
    #[must_use]
    pub fn into_inner(self) -> R {
        self.reader
    }

    /// Reads a contiguous sample range and converts it to physical `f64` values.
    ///
    /// Integer `BLANK` samples are written as NaN with
    /// [`SampleStatus::Undefined`]. Stored or scaled NaNs and infinities retain
    /// their IEEE value and receive [`SampleStatus::NonFinite`]. Scaling uses one
    /// explicit fused multiply-add in double precision:
    /// `physical = stored * BSCALE + BZERO`.
    ///
    /// The method uses fixed-size scratch storage regardless of the requested
    /// range length. If an I/O error occurs, a prefix of the output slices may
    /// already have been updated.
    ///
    /// # Errors
    ///
    /// Returns an error when output lengths differ, the requested range lies
    /// outside the image, byte offsets overflow, the stored representation is not
    /// implemented by this vertical slice, seeking fails, or data is truncated.
    pub fn read_physical_samples(
        &mut self,
        start_sample: u64,
        values: &mut [f64],
        statuses: &mut [SampleStatus],
    ) -> Result<(), ImageReadError> {
        if values.len() != statuses.len() {
            return Err(ImageReadError::OutputLengthMismatch {
                values: values.len(),
                statuses: statuses.len(),
            });
        }

        let sample_count =
            u64::try_from(values.len()).map_err(|_| ImageReadError::OffsetOverflow)?;
        let end_sample = start_sample
            .checked_add(sample_count)
            .ok_or(ImageReadError::OffsetOverflow)?;
        if end_sample > self.descriptor.pixel_count() {
            return Err(ImageReadError::SampleRangeOutOfBounds {
                start: start_sample,
                count: sample_count,
                available: self.descriptor.pixel_count(),
            });
        }

        let format = self.descriptor.sample_format();
        if !matches!(
            format,
            StoredSampleFormat::Signed16 | StoredSampleFormat::Float32
        ) {
            return Err(ImageReadError::UnsupportedSampleFormat { format });
        }

        let relative_offset = start_sample
            .checked_mul(format.byte_width())
            .ok_or(ImageReadError::OffsetOverflow)?;
        let absolute_offset = self
            .descriptor
            .data_offset()
            .checked_add(relative_offset)
            .ok_or(ImageReadError::OffsetOverflow)?;
        self.reader
            .seek(SeekFrom::Start(absolute_offset))
            .map_err(ImageReadError::Io)?;

        let byte_width =
            usize::try_from(format.byte_width()).map_err(|_| ImageReadError::OffsetOverflow)?;
        let samples_per_chunk = SCRATCH_BYTES / byte_width;
        let mut scratch = [0_u8; SCRATCH_BYTES];

        for (value_chunk, status_chunk) in values
            .chunks_mut(samples_per_chunk)
            .zip(statuses.chunks_mut(samples_per_chunk))
        {
            let byte_count = value_chunk
                .len()
                .checked_mul(byte_width)
                .ok_or(ImageReadError::OffsetOverflow)?;
            let Some(byte_chunk) = scratch.get_mut(..byte_count) else {
                return Err(ImageReadError::OffsetOverflow);
            };
            self.reader
                .read_exact(byte_chunk)
                .map_err(ImageReadError::Io)?;

            for ((bytes, value), status) in byte_chunk
                .chunks_exact(byte_width)
                .zip(value_chunk.iter_mut())
                .zip(status_chunk.iter_mut())
            {
                let stored = decode_supported_sample(format, bytes)?;
                let (physical, sample_status) = self.to_physical(stored);
                *value = physical;
                *status = sample_status;
            }
        }

        Ok(())
    }

    /// Reads a rectangular region from a two- or three-dimensional image.
    ///
    /// `plane` selects the third FITS axis; it must be zero for a 2D image.
    /// Output is tightly packed in row-major order even though source rows may
    /// contain pixels outside the requested region. Empty regions are accepted
    /// when their origin lies on or inside the corresponding image edge.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported axis counts, invalid coordinates, output
    /// lengths that do not exactly match `width * height`, arithmetic overflow,
    /// or any error reported by [`Self::read_physical_samples`].
    pub fn read_physical_region(
        &mut self,
        region: ImageRegion,
        values: &mut [f64],
        statuses: &mut [SampleStatus],
    ) -> Result<(), ImageReadError> {
        let plane = region.plane();
        let x = region.x();
        let y = region.y();
        let width = region.width();
        let height = region.height();
        let (image_width, image_height, plane_count) = match self.descriptor.axes() {
            [image_width, image_height] => (*image_width, *image_height, 1),
            [image_width, image_height, plane_count] => (*image_width, *image_height, *plane_count),
            axes => {
                return Err(ImageReadError::UnsupportedAxisCount { axes: axes.len() });
            }
        };

        if plane >= plane_count {
            return Err(ImageReadError::PlaneOutOfBounds { plane, plane_count });
        }
        let right = x.checked_add(width);
        let bottom = y.checked_add(height);
        if right.is_none_or(|right| right > image_width)
            || bottom.is_none_or(|bottom| bottom > image_height)
        {
            return Err(ImageReadError::RegionOutOfBounds {
                x,
                y,
                width,
                height,
                image_width,
                image_height,
            });
        }

        let expected_u64 = width
            .checked_mul(height)
            .ok_or(ImageReadError::OffsetOverflow)?;
        let expected = usize::try_from(expected_u64).map_err(|_| ImageReadError::OffsetOverflow)?;
        if values.len() != expected || statuses.len() != expected {
            return Err(ImageReadError::RegionOutputLengthMismatch {
                expected,
                values: values.len(),
                statuses: statuses.len(),
            });
        }

        let row_width = usize::try_from(width).map_err(|_| ImageReadError::OffsetOverflow)?;
        let row_count = usize::try_from(height).map_err(|_| ImageReadError::OffsetOverflow)?;
        let plane_start = plane
            .checked_mul(image_height)
            .and_then(|row| row.checked_mul(image_width))
            .ok_or(ImageReadError::OffsetOverflow)?;

        for row in 0..row_count {
            let output_start = row
                .checked_mul(row_width)
                .ok_or(ImageReadError::OffsetOverflow)?;
            let output_end = output_start
                .checked_add(row_width)
                .ok_or(ImageReadError::OffsetOverflow)?;
            let Some(value_row) = values.get_mut(output_start..output_end) else {
                return Err(ImageReadError::OffsetOverflow);
            };
            let Some(status_row) = statuses.get_mut(output_start..output_end) else {
                return Err(ImageReadError::OffsetOverflow);
            };
            let source_y = y
                .checked_add(u64::try_from(row).map_err(|_| ImageReadError::OffsetOverflow)?)
                .ok_or(ImageReadError::OffsetOverflow)?;
            let source_start = source_y
                .checked_mul(image_width)
                .and_then(|offset| plane_start.checked_add(offset))
                .and_then(|offset| offset.checked_add(x))
                .ok_or(ImageReadError::OffsetOverflow)?;
            self.read_physical_samples(source_start, value_row, status_row)?;
        }

        Ok(())
    }

    fn to_physical(&self, stored: StoredSample) -> (f64, SampleStatus) {
        if let StoredSample::Integer(value) = stored
            && self.descriptor.blank() == Some(value)
        {
            return (f64::NAN, SampleStatus::Undefined);
        }

        let stored_value = match stored {
            StoredSample::Integer(value) => value as f64,
            StoredSample::Float(value) => value,
        };
        let physical = stored_value.mul_add(self.descriptor.bscale(), self.descriptor.bzero());
        let status = if physical.is_finite() {
            SampleStatus::Valid
        } else {
            SampleStatus::NonFinite
        };
        (physical, status)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum StoredSample {
    Integer(i64),
    Float(f64),
}

fn decode_supported_sample(
    format: StoredSampleFormat,
    bytes: &[u8],
) -> Result<StoredSample, ImageReadError> {
    match (format, bytes) {
        (StoredSampleFormat::Signed16, [first, second]) => {
            Ok(StoredSample::Integer(i64::from(i16::from_be_bytes([
                *first, *second,
            ]))))
        }
        (StoredSampleFormat::Float32, [first, second, third, fourth]) => {
            Ok(StoredSample::Float(f64::from(f32::from_be_bytes([
                *first, *second, *third, *fourth,
            ]))))
        }
        (format, _)
            if !matches!(
                format,
                StoredSampleFormat::Signed16 | StoredSampleFormat::Float32
            ) =>
        {
            Err(ImageReadError::UnsupportedSampleFormat { format })
        }
        _ => Err(ImageReadError::InvalidStoredSampleWidth),
    }
}

/// Error raised while opening or reading a FITS primary image.
#[derive(Debug)]
pub enum ImageReadError {
    /// Primary-header parsing failed.
    Header(FitsError),
    /// Image layout derivation failed.
    Descriptor(ImageHduError),
    /// Seeking or reading pixel bytes failed.
    Io(std::io::Error),
    /// The value and status output buffers have different lengths.
    OutputLengthMismatch {
        /// Number of physical-value slots.
        values: usize,
        /// Number of status slots.
        statuses: usize,
    },
    /// The requested sample range is outside the image.
    SampleRangeOutOfBounds {
        /// First requested sample.
        start: u64,
        /// Requested sample count.
        count: u64,
        /// Total available sample count.
        available: u64,
    },
    /// A rectangular read was requested from an unsupported number of axes.
    UnsupportedAxisCount {
        /// Actual number of axes.
        axes: usize,
    },
    /// The selected plane does not exist.
    PlaneOutOfBounds {
        /// Requested zero-based plane.
        plane: u64,
        /// Available plane count.
        plane_count: u64,
    },
    /// A rectangular region extends beyond the image.
    RegionOutOfBounds {
        /// Requested first column.
        x: u64,
        /// Requested first row.
        y: u64,
        /// Requested width.
        width: u64,
        /// Requested height.
        height: u64,
        /// Available image width.
        image_width: u64,
        /// Available image height.
        image_height: u64,
    },
    /// Region output slices do not match the requested area.
    RegionOutputLengthMismatch {
        /// Required sample count.
        expected: usize,
        /// Number of physical-value slots.
        values: usize,
        /// Number of status slots.
        statuses: usize,
    },
    /// A byte or sample offset cannot be represented safely.
    OffsetOverflow,
    /// Pixel decoding for the stored representation is not implemented yet.
    UnsupportedSampleFormat {
        /// Unsupported stored representation.
        format: StoredSampleFormat,
    },
    /// An internal decoding chunk did not match the representation width.
    InvalidStoredSampleWidth,
}

impl Display for ImageReadError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Header(error) => write!(formatter, "cannot read FITS primary header: {error}"),
            Self::Descriptor(error) => write!(formatter, "invalid FITS image layout: {error}"),
            Self::Io(error) => write!(formatter, "cannot read FITS image data: {error}"),
            Self::OutputLengthMismatch { values, statuses } => write!(
                formatter,
                "output length mismatch: {values} values and {statuses} statuses"
            ),
            Self::SampleRangeOutOfBounds {
                start,
                count,
                available,
            } => write!(
                formatter,
                "sample range starting at {start} with length {count} exceeds {available} samples"
            ),
            Self::UnsupportedAxisCount { axes } => write!(
                formatter,
                "rectangular reads require 2 or 3 FITS axes, received {axes}"
            ),
            Self::PlaneOutOfBounds { plane, plane_count } => write!(
                formatter,
                "plane {plane} is outside the available plane count {plane_count}"
            ),
            Self::RegionOutOfBounds {
                x,
                y,
                width,
                height,
                image_width,
                image_height,
            } => write!(
                formatter,
                "region ({x}, {y}, {width}, {height}) exceeds image {image_width}x{image_height}"
            ),
            Self::RegionOutputLengthMismatch {
                expected,
                values,
                statuses,
            } => write!(
                formatter,
                "region requires {expected} outputs, received {values} values and {statuses} statuses"
            ),
            Self::OffsetOverflow => formatter.write_str("FITS image byte offset overflows"),
            Self::UnsupportedSampleFormat { format } => {
                write!(
                    formatter,
                    "pixel decoding is not implemented for {format:?}"
                )
            }
            Self::InvalidStoredSampleWidth => {
                formatter.write_str("stored FITS sample has an invalid byte width")
            }
        }
    }
}

impl Error for ImageReadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Header(error) => Some(error),
            Self::Descriptor(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::OutputLengthMismatch { .. }
            | Self::SampleRangeOutOfBounds { .. }
            | Self::UnsupportedAxisCount { .. }
            | Self::PlaneOutOfBounds { .. }
            | Self::RegionOutOfBounds { .. }
            | Self::RegionOutputLengthMismatch { .. }
            | Self::OffsetOverflow
            | Self::UnsupportedSampleFormat { .. }
            | Self::InvalidStoredSampleWidth => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use crate::{BLOCK_SIZE, CARD_SIZE};

    use super::*;

    fn fixed_card(keyword: &str, value: &str) -> String {
        format!("{keyword:<8}= {value:>20}")
    }

    fn fits_image(bitpix: i64, width: usize, data: &[u8], extra_cards: &[String]) -> Vec<u8> {
        fits_image_with_axes(bitpix, &[width], data, extra_cards)
    }

    fn fits_image_with_axes(
        bitpix: i64,
        axes: &[usize],
        data: &[u8],
        extra_cards: &[String],
    ) -> Vec<u8> {
        let mut cards = vec![
            fixed_card("SIMPLE", "T"),
            fixed_card("BITPIX", &bitpix.to_string()),
            fixed_card("NAXIS", &axes.len().to_string()),
        ];
        cards.extend(axes.iter().enumerate().map(|(index, length)| {
            fixed_card(&format!("NAXIS{}", index + 1), &length.to_string())
        }));
        cards.extend_from_slice(extra_cards);
        cards.push("END".to_owned());

        let mut bytes = vec![b' '; BLOCK_SIZE];
        for (index, card) in cards.iter().enumerate() {
            let start = index * CARD_SIZE;
            let end = start + CARD_SIZE;
            if let Some(destination) = bytes.get_mut(start..end) {
                let source = card.as_bytes();
                let length = source.len().min(CARD_SIZE);
                destination[..length].copy_from_slice(&source[..length]);
            }
        }
        bytes.extend_from_slice(data);
        bytes
    }

    #[test]
    fn decodes_big_endian_i16_scaling_and_blank() {
        let stored = [-32_768_i16, -1, 0, 32_767];
        let data: Vec<u8> = stored
            .iter()
            .flat_map(|value| value.to_be_bytes())
            .collect();
        let input = fits_image(
            16,
            stored.len(),
            &data,
            &[
                fixed_card("BSCALE", "2.0"),
                fixed_card("BZERO", "32768.0"),
                fixed_card("BLANK", "-32768"),
            ],
        );
        let result = PrimaryImageReader::open(Cursor::new(input), HeaderReadOptions::default());
        assert!(result.is_ok());
        let Some(mut reader) = result.ok() else {
            return;
        };
        let mut values = [0.0; 4];
        let mut statuses = [SampleStatus::Valid; 4];

        assert!(
            reader
                .read_physical_samples(0, &mut values, &mut statuses)
                .is_ok()
        );
        assert!(values[0].is_nan());
        assert_eq!(values[1].to_bits(), 32_766.0_f64.to_bits());
        assert_eq!(values[2].to_bits(), 32_768.0_f64.to_bits());
        assert_eq!(values[3].to_bits(), 98_302.0_f64.to_bits());
        assert_eq!(
            statuses,
            [
                SampleStatus::Undefined,
                SampleStatus::Valid,
                SampleStatus::Valid,
                SampleStatus::Valid
            ]
        );
    }

    #[test]
    fn reads_a_contiguous_subset_by_sample_offset() {
        let stored = [10_i16, 20, 30, 40];
        let data: Vec<u8> = stored
            .iter()
            .flat_map(|value| value.to_be_bytes())
            .collect();
        let input = fits_image(16, stored.len(), &data, &[]);
        let result = PrimaryImageReader::open(Cursor::new(input), HeaderReadOptions::default());
        let Some(mut reader) = result.ok() else {
            return;
        };
        let mut values = [0.0; 2];
        let mut statuses = [SampleStatus::Undefined; 2];

        assert!(
            reader
                .read_physical_samples(1, &mut values, &mut statuses)
                .is_ok()
        );
        assert_eq!(
            values.map(f64::to_bits),
            [20.0_f64.to_bits(), 30.0_f64.to_bits()]
        );
        assert_eq!(statuses, [SampleStatus::Valid; 2]);
    }

    #[test]
    fn preserves_and_marks_non_finite_float_samples() {
        let stored = [1.5_f32, f32::NAN, f32::INFINITY];
        let data: Vec<u8> = stored
            .iter()
            .flat_map(|value| value.to_be_bytes())
            .collect();
        let input = fits_image(-32, stored.len(), &data, &[]);
        let result = PrimaryImageReader::open(Cursor::new(input), HeaderReadOptions::default());
        let Some(mut reader) = result.ok() else {
            return;
        };
        let mut values = [0.0; 3];
        let mut statuses = [SampleStatus::Valid; 3];

        assert!(
            reader
                .read_physical_samples(0, &mut values, &mut statuses)
                .is_ok()
        );
        assert_eq!(values[0].to_bits(), 1.5_f64.to_bits());
        assert!(values[1].is_nan());
        assert!(values[2].is_infinite() && values[2].is_sign_positive());
        assert_eq!(
            statuses,
            [
                SampleStatus::Valid,
                SampleStatus::NonFinite,
                SampleStatus::NonFinite
            ]
        );
    }

    #[test]
    fn rejects_mismatched_output_lengths_before_io() {
        let input = fits_image(16, 2, &[0, 1, 0, 2], &[]);
        let result = PrimaryImageReader::open(Cursor::new(input), HeaderReadOptions::default());
        let Some(mut reader) = result.ok() else {
            return;
        };
        let mut values = [0.0; 2];
        let mut statuses = [SampleStatus::Valid; 1];

        assert!(matches!(
            reader.read_physical_samples(0, &mut values, &mut statuses),
            Err(ImageReadError::OutputLengthMismatch {
                values: 2,
                statuses: 1
            })
        ));
    }

    #[test]
    fn rejects_sample_range_past_image_end() {
        let input = fits_image(16, 2, &[0, 1, 0, 2], &[]);
        let result = PrimaryImageReader::open(Cursor::new(input), HeaderReadOptions::default());
        let Some(mut reader) = result.ok() else {
            return;
        };
        let mut values = [0.0; 2];
        let mut statuses = [SampleStatus::Valid; 2];

        assert!(matches!(
            reader.read_physical_samples(1, &mut values, &mut statuses),
            Err(ImageReadError::SampleRangeOutOfBounds {
                start: 1,
                count: 2,
                available: 2
            })
        ));
    }

    #[test]
    fn reports_truncated_pixel_data() {
        let input = fits_image(16, 2, &[0, 1], &[]);
        let result = PrimaryImageReader::open(Cursor::new(input), HeaderReadOptions::default());
        let Some(mut reader) = result.ok() else {
            return;
        };
        let mut values = [0.0; 2];
        let mut statuses = [SampleStatus::Valid; 2];

        assert!(matches!(
            reader.read_physical_samples(0, &mut values, &mut statuses),
            Err(ImageReadError::Io(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof
        ));
    }

    #[test]
    fn leaves_other_standard_formats_explicitly_unsupported() {
        let input = fits_image(8, 2, &[1, 2], &[]);
        let result = PrimaryImageReader::open(Cursor::new(input), HeaderReadOptions::default());
        let Some(mut reader) = result.ok() else {
            return;
        };
        let mut values = [0.0; 2];
        let mut statuses = [SampleStatus::Valid; 2];

        assert!(matches!(
            reader.read_physical_samples(0, &mut values, &mut statuses),
            Err(ImageReadError::UnsupportedSampleFormat {
                format: StoredSampleFormat::Unsigned8
            })
        ));
    }

    #[test]
    fn reads_a_rectangular_region_from_a_selected_plane() {
        let stored: Vec<i16> = (0..24).collect();
        let data: Vec<u8> = stored
            .iter()
            .flat_map(|value| value.to_be_bytes())
            .collect();
        let input = fits_image_with_axes(16, &[4, 3, 2], &data, &[]);
        let result = PrimaryImageReader::open(Cursor::new(input), HeaderReadOptions::default());
        let Some(mut reader) = result.ok() else {
            return;
        };
        let mut values = [0.0; 4];
        let mut statuses = [SampleStatus::Undefined; 4];

        assert!(
            reader
                .read_physical_region(ImageRegion::new(1, 1, 1, 2, 2), &mut values, &mut statuses,)
                .is_ok()
        );
        assert_eq!(
            values.map(f64::to_bits),
            [17.0, 18.0, 21.0, 22.0].map(f64::to_bits)
        );
        assert_eq!(statuses, [SampleStatus::Valid; 4]);
    }

    #[test]
    fn rejects_a_region_crossing_an_image_edge() {
        let data = vec![0_u8; 4 * 3 * 2];
        let input = fits_image_with_axes(16, &[4, 3], &data, &[]);
        let result = PrimaryImageReader::open(Cursor::new(input), HeaderReadOptions::default());
        let Some(mut reader) = result.ok() else {
            return;
        };
        let mut values = [0.0; 4];
        let mut statuses = [SampleStatus::Valid; 4];

        assert!(matches!(
            reader.read_physical_region(
                ImageRegion::new(0, 3, 1, 2, 2),
                &mut values,
                &mut statuses
            ),
            Err(ImageReadError::RegionOutOfBounds {
                x: 3,
                y: 1,
                width: 2,
                height: 2,
                image_width: 4,
                image_height: 3
            })
        ));
    }

    #[test]
    fn rejects_a_missing_plane() {
        let data = vec![0_u8; 4 * 3 * 2];
        let input = fits_image_with_axes(16, &[4, 3], &data, &[]);
        let result = PrimaryImageReader::open(Cursor::new(input), HeaderReadOptions::default());
        let Some(mut reader) = result.ok() else {
            return;
        };
        let mut values = [0.0; 1];
        let mut statuses = [SampleStatus::Valid; 1];

        assert!(matches!(
            reader.read_physical_region(
                ImageRegion::new(1, 0, 0, 1, 1),
                &mut values,
                &mut statuses
            ),
            Err(ImageReadError::PlaneOutOfBounds {
                plane: 1,
                plane_count: 1
            })
        ));
    }

    #[test]
    fn rejects_region_output_with_wrong_area() {
        let data = vec![0_u8; 4 * 3 * 2];
        let input = fits_image_with_axes(16, &[4, 3], &data, &[]);
        let result = PrimaryImageReader::open(Cursor::new(input), HeaderReadOptions::default());
        let Some(mut reader) = result.ok() else {
            return;
        };
        let mut values = [0.0; 3];
        let mut statuses = [SampleStatus::Valid; 4];

        assert!(matches!(
            reader.read_physical_region(
                ImageRegion::new(0, 0, 0, 2, 2),
                &mut values,
                &mut statuses
            ),
            Err(ImageReadError::RegionOutputLengthMismatch {
                expected: 4,
                values: 3,
                statuses: 4
            })
        ));
    }

    #[test]
    fn rejects_rectangular_access_to_a_one_dimensional_array() {
        let input = fits_image(16, 2, &[0, 1, 0, 2], &[]);
        let result = PrimaryImageReader::open(Cursor::new(input), HeaderReadOptions::default());
        let Some(mut reader) = result.ok() else {
            return;
        };
        let mut values = [0.0; 1];
        let mut statuses = [SampleStatus::Valid; 1];

        assert!(matches!(
            reader.read_physical_region(
                ImageRegion::new(0, 0, 0, 1, 1),
                &mut values,
                &mut statuses
            ),
            Err(ImageReadError::UnsupportedAxisCount { axes: 1 })
        ));
    }
}
