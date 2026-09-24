use std::error::Error;
use std::fmt::{Display, Formatter};
use std::io::{Read, Seek};
use std::num::NonZeroUsize;

use aether_core::{ImageStatistics, StatisticsError, StatisticsFirstPass};

use crate::{ImageReadError, PrimaryImageReader, SampleStatus};

/// Default number of decoded `f64` samples retained by FITS statistics passes.
pub const DEFAULT_STATISTICS_CHUNK_SAMPLES: usize = 256 * 1_024;
/// Stable identifier for deterministic three-pass primary-image moments.
pub const FITS_STATISTICS_ALGORITHM_ID: &str = "fits-three-pass-moments-v1";

/// Strict moments and invalid-sample accounting for one FITS primary image.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FitsImageStatistics {
    moments: ImageStatistics,
    undefined_samples: usize,
    non_finite_samples: usize,
}

impl FitsImageStatistics {
    /// Deterministic finite-sample moments shared with the scientific core.
    #[must_use]
    pub const fn moments(self) -> ImageStatistics {
        self.moments
    }

    /// Integer pixels equal to the FITS `BLANK` sentinel.
    #[must_use]
    pub const fn undefined_samples(self) -> usize {
        self.undefined_samples
    }

    /// Stored or scaled values that decode to NaN or infinity.
    #[must_use]
    pub const fn non_finite_samples(self) -> usize {
        self.non_finite_samples
    }
}

/// Failure to compute strict bounded-memory statistics for a FITS image.
#[derive(Debug)]
pub enum FitsStatisticsError {
    /// A decoded value or status buffer could not be reserved.
    AllocationFailed {
        /// Number of samples requested in each bounded buffer.
        samples: usize,
    },
    /// Pixel decoding or source I/O failed.
    ImageRead(ImageReadError),
    /// The strict numerical summary could not be completed.
    Statistics(StatisticsError),
    /// Invalid-sample accounting overflowed the platform size domain.
    SampleCountOverflow,
}

impl Display for FitsStatisticsError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AllocationFailed { samples } => {
                write!(
                    formatter,
                    "cannot reserve FITS statistics buffers for {samples} samples"
                )
            }
            Self::ImageRead(error) => {
                write!(formatter, "cannot read FITS statistics data: {error}")
            }
            Self::Statistics(error) => {
                write!(formatter, "cannot calculate FITS statistics: {error}")
            }
            Self::SampleCountOverflow => {
                formatter.write_str("FITS invalid-sample accounting overflows")
            }
        }
    }
}

impl Error for FitsStatisticsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ImageRead(error) => Some(error),
            Self::Statistics(error) => Some(error),
            Self::AllocationFailed { .. } | Self::SampleCountOverflow => None,
        }
    }
}

impl From<ImageReadError> for FitsStatisticsError {
    fn from(value: ImageReadError) -> Self {
        Self::ImageRead(value)
    }
}

impl From<StatisticsError> for FitsStatisticsError {
    fn from(value: StatisticsError) -> Self {
        Self::Statistics(value)
    }
}

/// Calculates strict three-pass statistics without materializing the full image.
///
/// Each pass decodes consecutive physical samples in canonical FITS order. Mean
/// and variance use the same scaled compensated algorithm as in-memory
/// [`ImageStatistics`]. Chunk size changes memory use and I/O call granularity,
/// never sample order or numerical results. Integer `BLANK` pixels and decoded
/// non-finite values remain separately counted.
///
/// The input stream must remain immutable for all three passes. Higher-level
/// session execution additionally verifies the source fingerprint before and
/// after processing.
///
/// # Errors
///
/// Returns a typed allocation, decoding, accounting, or numerical failure.
pub fn primary_image_statistics<R: Read + Seek>(
    reader: &mut PrimaryImageReader<R>,
    chunk_samples: NonZeroUsize,
) -> Result<FitsImageStatistics, FitsStatisticsError> {
    let pixel_count = reader.descriptor().pixel_count();
    let chunk_u64 =
        u64::try_from(chunk_samples.get()).map_err(|_| FitsStatisticsError::SampleCountOverflow)?;
    let buffer_samples = usize::try_from(pixel_count.min(chunk_u64))
        .map_err(|_| FitsStatisticsError::SampleCountOverflow)?;
    let mut values = try_filled_vec(buffer_samples, 0.0)?;
    let mut statuses = try_filled_vec(buffer_samples, SampleStatus::Valid)?;

    let mut first = StatisticsFirstPass::new();
    let mut undefined_samples = 0_usize;
    let mut non_finite_samples = 0_usize;
    visit_chunks(reader, &mut values, &mut statuses, |values, statuses| {
        first.observe_values(values)?;
        for status in statuses {
            match status {
                SampleStatus::Valid => {}
                SampleStatus::Undefined => {
                    undefined_samples = undefined_samples
                        .checked_add(1)
                        .ok_or(FitsStatisticsError::SampleCountOverflow)?;
                }
                SampleStatus::NonFinite => {
                    non_finite_samples = non_finite_samples
                        .checked_add(1)
                        .ok_or(FitsStatisticsError::SampleCountOverflow)?;
                }
            }
        }
        Ok(())
    })?;

    let mut mean = first.finish()?;
    visit_chunks(reader, &mut values, &mut statuses, |values, _| {
        mean.observe_values(values).map_err(Into::into)
    })?;

    let mut variance = mean.finish()?;
    visit_chunks(reader, &mut values, &mut statuses, |values, _| {
        variance.observe_values(values).map_err(Into::into)
    })?;
    let moments = variance.finish()?;

    let invalid_samples = undefined_samples
        .checked_add(non_finite_samples)
        .ok_or(FitsStatisticsError::SampleCountOverflow)?;
    debug_assert_eq!(moments.non_finite_samples(), invalid_samples);
    Ok(FitsImageStatistics {
        moments,
        undefined_samples,
        non_finite_samples,
    })
}

fn try_filled_vec<T: Clone>(samples: usize, value: T) -> Result<Vec<T>, FitsStatisticsError> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(samples)
        .map_err(|_| FitsStatisticsError::AllocationFailed { samples })?;
    output.resize(samples, value);
    Ok(output)
}

fn visit_chunks<R, F>(
    reader: &mut PrimaryImageReader<R>,
    values: &mut [f64],
    statuses: &mut [SampleStatus],
    mut visit: F,
) -> Result<(), FitsStatisticsError>
where
    R: Read + Seek,
    F: FnMut(&[f64], &[SampleStatus]) -> Result<(), FitsStatisticsError>,
{
    let mut offset = 0_u64;
    let total = reader.descriptor().pixel_count();
    let capacity =
        u64::try_from(values.len()).map_err(|_| FitsStatisticsError::SampleCountOverflow)?;
    while offset < total {
        let remaining = total - offset;
        let count = usize::try_from(remaining.min(capacity))
            .map_err(|_| FitsStatisticsError::SampleCountOverflow)?;
        let value_chunk = values
            .get_mut(..count)
            .ok_or(FitsStatisticsError::SampleCountOverflow)?;
        let status_chunk = statuses
            .get_mut(..count)
            .ok_or(FitsStatisticsError::SampleCountOverflow)?;
        reader.read_physical_samples(offset, value_chunk, status_chunk)?;
        visit(value_chunk, status_chunk)?;
        offset = offset
            .checked_add(
                u64::try_from(count).map_err(|_| FitsStatisticsError::SampleCountOverflow)?,
            )
            .ok_or(FitsStatisticsError::SampleCountOverflow)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::io::Cursor;

    use aether_core::{Dimensions, ScientificImage, StatisticsError};

    use crate::{BLOCK_SIZE, CARD_SIZE, HeaderReadOptions, PrimaryImageReader, write_f64_primary};

    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn StdError>>;

    fn reader(values: Vec<f64>) -> TestResult<PrimaryImageReader<Cursor<Vec<u8>>>> {
        let dimensions = Dimensions::new(values.len(), 1, 1)?;
        let image = ScientificImage::from_pixels(dimensions, values)?;
        let mut encoded = Vec::new();
        write_f64_primary(&mut encoded, &image)?;
        Ok(PrimaryImageReader::open(
            Cursor::new(encoded),
            HeaderReadOptions::default(),
        )?)
    }

    fn signed16_reader_with_blank() -> TestResult<PrimaryImageReader<Cursor<Vec<u8>>>> {
        let mut encoded = vec![b' '; BLOCK_SIZE];
        for (index, (keyword, value)) in [
            ("SIMPLE", "T"),
            ("BITPIX", "16"),
            ("NAXIS", "1"),
            ("NAXIS1", "4"),
            ("BLANK", "-32768"),
        ]
        .into_iter()
        .enumerate()
        {
            let card = format!("{keyword:<8}= {value:>20}");
            let start = index * CARD_SIZE;
            let destination = encoded
                .get_mut(start..start + card.len())
                .ok_or("test card lies outside header")?;
            destination.copy_from_slice(card.as_bytes());
        }
        encoded[5 * CARD_SIZE..5 * CARD_SIZE + 3].copy_from_slice(b"END");
        for value in [i16::MIN, 1_i16, 2_i16, i16::MIN] {
            encoded.extend_from_slice(&value.to_be_bytes());
        }
        encoded.resize(2 * BLOCK_SIZE, 0);
        Ok(PrimaryImageReader::open(
            Cursor::new(encoded),
            HeaderReadOptions::default(),
        )?)
    }

    #[test]
    fn chunk_size_does_not_change_strict_statistics() -> TestResult {
        let values = vec![1.0e16, 1.0, -1.0e16, 4.0, 6.0, 9.0];
        let mut one_sample_reader = reader(values.clone())?;
        let mut whole_reader = reader(values)?;

        let one_sample = primary_image_statistics(
            &mut one_sample_reader,
            NonZeroUsize::new(1).ok_or("non-zero test chunk")?,
        )?;
        let whole = primary_image_statistics(
            &mut whole_reader,
            NonZeroUsize::new(64).ok_or("non-zero test chunk")?,
        )?;

        assert_eq!(one_sample, whole);
        assert!((whole.moments().mean() - (20.0 / 6.0)).abs() <= 2.0 * f64::EPSILON);
        assert_eq!(whole.undefined_samples(), 0);
        assert_eq!(whole.non_finite_samples(), 0);
        Ok(())
    }

    #[test]
    fn retains_decoded_non_finite_accounting() -> TestResult {
        let mut input = reader(vec![1.0, f64::NAN, f64::INFINITY, 5.0])?;
        let statistics = primary_image_statistics(
            &mut input,
            NonZeroUsize::new(2).ok_or("non-zero test chunk")?,
        )?;

        assert_eq!(statistics.moments().total_samples(), 4);
        assert_eq!(statistics.moments().usable_samples(), 2);
        assert_eq!(statistics.moments().non_finite_samples(), 2);
        assert_eq!(statistics.undefined_samples(), 0);
        assert_eq!(statistics.non_finite_samples(), 2);
        assert_eq!(statistics.moments().minimum().to_bits(), 1.0_f64.to_bits());
        assert_eq!(statistics.moments().maximum().to_bits(), 5.0_f64.to_bits());
        assert_eq!(statistics.moments().mean().to_bits(), 3.0_f64.to_bits());
        Ok(())
    }

    #[test]
    fn distinguishes_integer_blank_from_non_finite_data() -> TestResult {
        let mut input = signed16_reader_with_blank()?;
        let statistics = primary_image_statistics(
            &mut input,
            NonZeroUsize::new(3).ok_or("non-zero test chunk")?,
        )?;

        assert_eq!(statistics.moments().total_samples(), 4);
        assert_eq!(statistics.moments().usable_samples(), 2);
        assert_eq!(statistics.moments().non_finite_samples(), 2);
        assert_eq!(statistics.undefined_samples(), 2);
        assert_eq!(statistics.non_finite_samples(), 0);
        assert_eq!(statistics.moments().mean().to_bits(), 1.5_f64.to_bits());
        Ok(())
    }

    #[test]
    fn reports_an_image_without_finite_samples() -> TestResult {
        let mut input = reader(vec![f64::NAN, f64::NEG_INFINITY])?;
        let result = primary_image_statistics(
            &mut input,
            NonZeroUsize::new(1).ok_or("non-zero test chunk")?,
        );

        assert!(matches!(
            result,
            Err(FitsStatisticsError::Statistics(
                StatisticsError::NoUsableSamples {
                    total: 2,
                    masked: 0,
                    non_finite: 2,
                }
            ))
        ));
        Ok(())
    }
}
