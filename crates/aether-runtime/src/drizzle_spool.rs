use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use aether_core::PixelFlags;
use aether_drizzle::DrizzleTileResult;

const BUFFER_BYTES: usize = 64 * 1_024;
const MAX_CREATE_ATTEMPTS: usize = 128;
static SPOOL_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Private planar storage used to transpose completed horizontal bands.
pub(crate) struct DrizzlePlanarSpool {
    width: usize,
    height: usize,
    plane_samples: usize,
    next_y: usize,
    science: BufWriter<File>,
    flags: BufWriter<File>,
    weights: BufWriter<File>,
    support: BufWriter<File>,
    guard: SpoolPathGuard,
}

/// Sequential reader over one complete private spool.
pub(crate) struct CompletedDrizzlePlanarSpool {
    science: BufReader<File>,
    flags: BufReader<File>,
    weights: BufReader<File>,
    support: BufReader<File>,
    remaining_samples: usize,
    _guard: SpoolPathGuard,
}

/// Failure while creating, filling, or reading private planar Drizzle storage.
#[derive(Debug)]
pub enum DrizzleSpoolError {
    /// Complete output dimensions are empty or overflow their sample domain.
    InvalidDimensions,
    /// A band is out of order, not full-width RGB, or outside the output.
    InvalidBand,
    /// Finalization was requested before every output row was written.
    Incomplete {
        /// Required complete output height.
        expected_rows: usize,
        /// Number of top-to-bottom rows written so far.
        actual_rows: usize,
    },
    /// Caller-provided streaming buffers do not share one length.
    OutputLengthMismatch,
    /// Private spool files could not be created safely.
    Create(io::Error),
    /// Seeking, writing, flushing, or reading private spool bytes failed.
    Io(io::Error),
}

impl Display for DrizzleSpoolError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidDimensions => formatter.write_str("Drizzle spool dimensions are invalid"),
            Self::InvalidBand => formatter.write_str("Drizzle spool received a noncanonical band"),
            Self::Incomplete {
                expected_rows,
                actual_rows,
            } => write!(
                formatter,
                "Drizzle spool has {actual_rows} rows but expected {expected_rows}"
            ),
            Self::OutputLengthMismatch => {
                formatter.write_str("Drizzle spool output slices have different lengths")
            }
            Self::Create(error) => {
                write!(formatter, "cannot create private Drizzle spool: {error}")
            }
            Self::Io(error) => write!(formatter, "cannot access private Drizzle spool: {error}"),
        }
    }
}

impl Error for DrizzleSpoolError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Create(error) | Self::Io(error) => Some(error),
            Self::InvalidDimensions
            | Self::InvalidBand
            | Self::Incomplete { .. }
            | Self::OutputLengthMismatch => None,
        }
    }
}

struct SpoolPathGuard {
    paths: Vec<PathBuf>,
}

impl Drop for SpoolPathGuard {
    fn drop(&mut self) {
        for path in &self.paths {
            let _ignored = fs::remove_file(path);
        }
    }
}

impl DrizzlePlanarSpool {
    pub(crate) fn create(width: usize, height: usize) -> Result<Self, DrizzleSpoolError> {
        let plane_samples = width
            .checked_mul(height)
            .filter(|value| *value != 0)
            .ok_or(DrizzleSpoolError::InvalidDimensions)?;
        let root = std::env::temp_dir();
        for _ in 0..MAX_CREATE_ATTEMPTS {
            let sequence = SPOOL_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let stem = format!(
                "aetherstack-drizzle-spool-{}-{sequence}",
                std::process::id()
            );
            let paths = ["science", "flags", "weights", "support"]
                .map(|kind| root.join(format!("{stem}-{kind}.tmp")));
            match create_files(&paths) {
                Ok([science, flags, weights, support]) => {
                    return Ok(Self {
                        width,
                        height,
                        plane_samples,
                        next_y: 0,
                        science: BufWriter::with_capacity(BUFFER_BYTES, science),
                        flags: BufWriter::with_capacity(BUFFER_BYTES, flags),
                        weights: BufWriter::with_capacity(BUFFER_BYTES, weights),
                        support: BufWriter::with_capacity(BUFFER_BYTES, support),
                        guard: SpoolPathGuard {
                            paths: paths.into_iter().collect(),
                        },
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(DrizzleSpoolError::Create(error)),
            }
        }
        Err(DrizzleSpoolError::Create(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "temporary name attempts exhausted",
        )))
    }

    pub(crate) fn write_band(
        &mut self,
        result: &DrizzleTileResult,
    ) -> Result<(), DrizzleSpoolError> {
        let bounds = result.bounds();
        let dimensions = bounds.dimensions();
        if bounds.origin_x() != 0
            || bounds.origin_y() as usize != self.next_y
            || dimensions.width() != self.width
            || dimensions.planes() != 3
            || dimensions.height() == 0
            || self
                .next_y
                .checked_add(dimensions.height())
                .is_none_or(|bottom| bottom > self.height)
        {
            return Err(DrizzleSpoolError::InvalidBand);
        }
        let band_samples = self
            .width
            .checked_mul(dimensions.height())
            .ok_or(DrizzleSpoolError::InvalidBand)?;
        for plane in 0..3_usize {
            let local_start = plane
                .checked_mul(band_samples)
                .ok_or(DrizzleSpoolError::InvalidBand)?;
            let local_end = local_start
                .checked_add(band_samples)
                .ok_or(DrizzleSpoolError::InvalidBand)?;
            let global_start = plane
                .checked_mul(self.plane_samples)
                .and_then(|value| value.checked_add(self.next_y * self.width))
                .ok_or(DrizzleSpoolError::InvalidBand)?;
            seek_sample(&mut self.science, global_start, size_of_f64())?;
            seek_sample(&mut self.flags, global_start, 1)?;
            seek_sample(&mut self.weights, global_start, size_of_f64())?;
            seek_sample(&mut self.support, global_start, size_of_u64())?;
            for index in local_start..local_end {
                self.science
                    .write_all(&result.values()[index].to_le_bytes())
                    .map_err(DrizzleSpoolError::Io)?;
                self.flags
                    .write_all(&[result.flags()[index].bits()])
                    .map_err(DrizzleSpoolError::Io)?;
                self.weights
                    .write_all(&result.weights()[index].to_le_bytes())
                    .map_err(DrizzleSpoolError::Io)?;
                self.support
                    .write_all(&result.contribution_counts()[index].to_le_bytes())
                    .map_err(DrizzleSpoolError::Io)?;
            }
        }
        self.next_y += dimensions.height();
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<CompletedDrizzlePlanarSpool, DrizzleSpoolError> {
        if self.next_y != self.height {
            return Err(DrizzleSpoolError::Incomplete {
                expected_rows: self.height,
                actual_rows: self.next_y,
            });
        }
        flush_and_rewind(&mut self.science)?;
        flush_and_rewind(&mut self.flags)?;
        flush_and_rewind(&mut self.weights)?;
        flush_and_rewind(&mut self.support)?;
        let science = into_file(self.science)?;
        let flags = into_file(self.flags)?;
        let weights = into_file(self.weights)?;
        let support = into_file(self.support)?;
        let remaining_samples = self
            .plane_samples
            .checked_mul(3)
            .ok_or(DrizzleSpoolError::InvalidDimensions)?;
        Ok(CompletedDrizzlePlanarSpool {
            science: BufReader::with_capacity(BUFFER_BYTES, science),
            flags: BufReader::with_capacity(BUFFER_BYTES, flags),
            weights: BufReader::with_capacity(BUFFER_BYTES, weights),
            support: BufReader::with_capacity(BUFFER_BYTES, support),
            remaining_samples,
            _guard: self.guard,
        })
    }
}

impl CompletedDrizzlePlanarSpool {
    pub(crate) fn remaining_samples(&self) -> usize {
        self.remaining_samples
    }

    pub(crate) fn read_chunk(
        &mut self,
        science: &mut [f64],
        flags: &mut [PixelFlags],
        weights: &mut [f64],
        support: &mut [u64],
    ) -> Result<usize, DrizzleSpoolError> {
        if science.len() != flags.len()
            || science.len() != weights.len()
            || science.len() != support.len()
        {
            return Err(DrizzleSpoolError::OutputLengthMismatch);
        }
        let count = science.len().min(self.remaining_samples);
        for index in 0..count {
            science[index] = read_f64(&mut self.science)?;
            let mut byte = [0_u8; 1];
            self.flags
                .read_exact(&mut byte)
                .map_err(DrizzleSpoolError::Io)?;
            flags[index] = PixelFlags::from_bits_retain(byte[0]);
            weights[index] = read_f64(&mut self.weights)?;
            support[index] = read_u64(&mut self.support)?;
        }
        self.remaining_samples -= count;
        Ok(count)
    }
}

fn create_files(paths: &[PathBuf; 4]) -> io::Result<[File; 4]> {
    let mut created = Vec::new();
    for path in paths {
        match OpenOptions::new()
            .write(true)
            .read(true)
            .create_new(true)
            .open(path)
        {
            Ok(file) => created.push((path.clone(), file)),
            Err(error) => {
                for (path, _) in created {
                    let _ignored = fs::remove_file(path);
                }
                return Err(error);
            }
        }
    }
    let mut files = created.into_iter().map(|(_, file)| file);
    Ok([
        files.next().ok_or_else(missing_spool_file)?,
        files.next().ok_or_else(missing_spool_file)?,
        files.next().ok_or_else(missing_spool_file)?,
        files.next().ok_or_else(missing_spool_file)?,
    ])
}

fn missing_spool_file() -> io::Error {
    io::Error::other("private Drizzle spool file is missing")
}

fn seek_sample(
    writer: &mut BufWriter<File>,
    sample: usize,
    bytes: usize,
) -> Result<(), DrizzleSpoolError> {
    let offset = sample
        .checked_mul(bytes)
        .and_then(|value| u64::try_from(value).ok())
        .ok_or(DrizzleSpoolError::InvalidBand)?;
    writer
        .seek(SeekFrom::Start(offset))
        .map(|_| ())
        .map_err(DrizzleSpoolError::Io)
}

fn flush_and_rewind(writer: &mut BufWriter<File>) -> Result<(), DrizzleSpoolError> {
    writer.flush().map_err(DrizzleSpoolError::Io)?;
    writer
        .seek(SeekFrom::Start(0))
        .map(|_| ())
        .map_err(DrizzleSpoolError::Io)
}

fn into_file(writer: BufWriter<File>) -> Result<File, DrizzleSpoolError> {
    writer
        .into_inner()
        .map_err(|error| DrizzleSpoolError::Io(error.into_error()))
}

const fn size_of_f64() -> usize {
    std::mem::size_of::<f64>()
}

const fn size_of_u64() -> usize {
    std::mem::size_of::<u64>()
}

fn read_f64(reader: &mut BufReader<File>) -> Result<f64, DrizzleSpoolError> {
    let mut bytes = [0_u8; 8];
    reader
        .read_exact(&mut bytes)
        .map_err(DrizzleSpoolError::Io)?;
    Ok(f64::from_le_bytes(bytes))
}

fn read_u64(reader: &mut BufReader<File>) -> Result<u64, DrizzleSpoolError> {
    let mut bytes = [0_u8; 8];
    reader
        .read_exact(&mut bytes)
        .map_err(DrizzleSpoolError::Io)?;
    Ok(u64::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use aether_drizzle::{
        DrizzleOutputBounds, DrizzleParameters, DrizzleTileAccumulator, DrizzleTileBounds,
        deposit_detector_footprint, project_detector_footprint,
    };
    use aether_registration::ProjectiveTransform;

    use super::*;

    type TestResult = Result<(), Box<dyn Error>>;

    fn band(origin_y: u32) -> Result<DrizzleTileResult, Box<dyn Error>> {
        let bounds = DrizzleTileBounds::new(0, origin_y, 2, 1, 3)?;
        let mut accumulator = DrizzleTileAccumulator::new(bounds)?;
        for x in 0..2_u32 {
            let footprint = project_detector_footprint(
                x,
                origin_y,
                ProjectiveTransform::IDENTITY,
                DrizzleParameters::new(1, 1.0)?,
            )?;
            accumulator.accumulate(
                &deposit_detector_footprint(
                    footprint,
                    f64::from(origin_y * 10 + x),
                    1.0,
                    2,
                    2,
                    DrizzleOutputBounds::new(2, 2, 4)?.maximum_contributions(),
                )?,
                usize::try_from((origin_y + x) % 3)?,
            )?;
        }
        Ok(accumulator.finish()?)
    }

    #[test]
    fn transposes_bands_into_complete_planar_order_without_bit_changes() -> TestResult {
        let first = band(0)?;
        let second = band(1)?;
        let mut spool = DrizzlePlanarSpool::create(2, 2)?;
        spool.write_band(&first)?;
        spool.write_band(&second)?;
        let mut spool = spool.finish()?;
        let mut science = [0.0; 12];
        let mut flags = [PixelFlags::CLEAR; 12];
        let mut weights = [0.0; 12];
        let mut support = [0_u64; 12];
        assert_eq!(
            spool.read_chunk(&mut science, &mut flags, &mut weights, &mut support)?,
            12
        );
        assert_eq!(spool.remaining_samples(), 0);

        for plane in 0..3_usize {
            let first_start = plane * 2;
            let output_start = plane * 4;
            assert_eq!(
                science[output_start..output_start + 2]
                    .iter()
                    .map(|value| value.to_bits())
                    .collect::<Vec<_>>(),
                first.values()[first_start..first_start + 2]
                    .iter()
                    .map(|value| value.to_bits())
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                science[output_start + 2..output_start + 4]
                    .iter()
                    .map(|value| value.to_bits())
                    .collect::<Vec<_>>(),
                second.values()[first_start..first_start + 2]
                    .iter()
                    .map(|value| value.to_bits())
                    .collect::<Vec<_>>()
            );
        }
        Ok(())
    }

    #[test]
    fn rejects_out_of_order_and_incomplete_bands() -> TestResult {
        let mut spool = DrizzlePlanarSpool::create(2, 2)?;
        assert!(matches!(
            spool.write_band(&band(1)?),
            Err(DrizzleSpoolError::InvalidBand)
        ));
        spool.write_band(&band(0)?)?;
        assert!(matches!(
            spool.finish(),
            Err(DrizzleSpoolError::Incomplete {
                expected_rows: 2,
                actual_rows: 1
            })
        ));
        Ok(())
    }
}
