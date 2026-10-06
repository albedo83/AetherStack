use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_core::{CoreError, Dimensions, PixelFlags, PixelMask, ScientificImage};

const MAX_RADIUS: usize = 8;
const ROBUST_NORMAL_SCALE: f64 = 1.482_602_218_505_602;

/// Explicit controls for local detector-defect discovery.
///
/// Neighbours are sampled on a square lattice around each detector sample. A
/// stride of two compares only samples with the same Bayer phase; a stride of
/// one is appropriate for monochrome sensors or already separated planes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DefectDetectionParameters {
    radius: usize,
    stride: usize,
    minimum_neighbours: usize,
    hot_sigma: f64,
    cold_sigma: f64,
    minimum_absolute_deviation: f64,
}

impl DefectDetectionParameters {
    /// Validates a fully explicit local-detection policy.
    ///
    /// Sigma limits must be finite and strictly positive. The absolute floor
    /// prevents a zero-MAD neighbourhood from classifying harmless rounding
    /// noise as a defect.
    pub fn new(
        radius: usize,
        stride: usize,
        minimum_neighbours: usize,
        hot_sigma: f64,
        cold_sigma: f64,
        minimum_absolute_deviation: f64,
    ) -> Result<Self, DefectMapError> {
        validate_neighbourhood(radius, stride, minimum_neighbours)?;
        if !hot_sigma.is_finite() || hot_sigma <= 0.0 {
            return Err(DefectMapError::InvalidSigma {
                kind: DefectKind::Hot,
                value: hot_sigma,
            });
        }
        if !cold_sigma.is_finite() || cold_sigma <= 0.0 {
            return Err(DefectMapError::InvalidSigma {
                kind: DefectKind::Cold,
                value: cold_sigma,
            });
        }
        if !minimum_absolute_deviation.is_finite() || minimum_absolute_deviation < 0.0 {
            return Err(DefectMapError::InvalidDeviationFloor {
                value: minimum_absolute_deviation,
            });
        }
        Ok(Self {
            radius,
            stride,
            minimum_neighbours,
            hot_sigma,
            cold_sigma,
            minimum_absolute_deviation,
        })
    }

    /// Radius measured in lattice steps, not detector pixels.
    #[must_use]
    pub const fn radius(self) -> usize {
        self.radius
    }

    /// Detector-pixel distance between neighbouring lattice samples.
    #[must_use]
    pub const fn stride(self) -> usize {
        self.stride
    }

    /// Minimum usable local support required to make a decision.
    #[must_use]
    pub const fn minimum_neighbours(self) -> usize {
        self.minimum_neighbours
    }

    /// Positive robust-sigma limit for hot samples.
    #[must_use]
    pub const fn hot_sigma(self) -> f64 {
        self.hot_sigma
    }

    /// Positive robust-sigma limit for cold samples.
    #[must_use]
    pub const fn cold_sigma(self) -> f64 {
        self.cold_sigma
    }

    /// Smallest absolute residual eligible for classification.
    #[must_use]
    pub const fn minimum_absolute_deviation(self) -> f64 {
        self.minimum_absolute_deviation
    }

    fn maximum_neighbours(self) -> usize {
        maximum_neighbours(self.radius)
    }
}

/// Explicit controls for conservative local defect replacement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DefectCorrectionParameters {
    radius: usize,
    stride: usize,
    minimum_neighbours: usize,
}

impl DefectCorrectionParameters {
    /// Validates the bounded replacement neighbourhood.
    pub fn new(
        radius: usize,
        stride: usize,
        minimum_neighbours: usize,
    ) -> Result<Self, DefectMapError> {
        validate_neighbourhood(radius, stride, minimum_neighbours)?;
        Ok(Self {
            radius,
            stride,
            minimum_neighbours,
        })
    }

    /// Radius measured in lattice steps.
    #[must_use]
    pub const fn radius(self) -> usize {
        self.radius
    }

    /// Detector-pixel distance between lattice samples.
    #[must_use]
    pub const fn stride(self) -> usize {
        self.stride
    }

    /// Minimum clean support needed to replace a defect.
    #[must_use]
    pub const fn minimum_neighbours(self) -> usize {
        self.minimum_neighbours
    }

    fn maximum_neighbours(self) -> usize {
        maximum_neighbours(self.radius)
    }
}

/// Defect direction used in parameter diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DefectKind {
    /// Positive local outlier.
    Hot,
    /// Negative local outlier.
    Cold,
}

impl Display for DefectKind {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hot => formatter.write_str("hot"),
            Self::Cold => formatter.write_str("cold"),
        }
    }
}

/// Stable detector defect map, kept separate from corrected science pixels.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DefectMap {
    mask: PixelMask,
}

impl DefectMap {
    /// Creates an empty map for explicit construction or testing.
    pub fn clear(dimensions: Dimensions) -> Result<Self, DefectMapError> {
        Ok(Self {
            mask: PixelMask::clear(dimensions).map_err(DefectMapError::Core)?,
        })
    }

    /// Map dimensions.
    #[must_use]
    pub const fn dimensions(&self) -> Dimensions {
        self.mask.dimensions()
    }

    /// Read-only defect flags.
    #[must_use]
    pub const fn mask(&self) -> &PixelMask {
        &self.mask
    }

    /// Marks a known defect without erasing existing evidence.
    pub fn insert(
        &mut self,
        x: usize,
        y: usize,
        plane: usize,
        flags: PixelFlags,
    ) -> Result<(), DefectMapError> {
        validate_defect_flags(flags)?;
        self.mask
            .insert(x, y, plane, flags)
            .map_err(DefectMapError::Core)
    }
}

/// Complete accounting for combining independent master-derived maps.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DefectMapMergeEvidence {
    input_maps: usize,
    defective_samples: usize,
    hot_samples: usize,
    cold_samples: usize,
    conflicting_samples: usize,
}

impl DefectMapMergeEvidence {
    /// Number of maps merged in caller order.
    #[must_use]
    pub const fn input_maps(self) -> usize {
        self.input_maps
    }
    /// Unique samples carrying at least one defect reason.
    #[must_use]
    pub const fn defective_samples(self) -> usize {
        self.defective_samples
    }
    /// Unique samples carrying HOT evidence.
    #[must_use]
    pub const fn hot_samples(self) -> usize {
        self.hot_samples
    }
    /// Unique samples carrying COLD evidence.
    #[must_use]
    pub const fn cold_samples(self) -> usize {
        self.cold_samples
    }
    /// Samples carrying both HOT and COLD evidence after the merge.
    #[must_use]
    pub const fn conflicting_samples(self) -> usize {
        self.conflicting_samples
    }
}

/// Auditable totals from local defect discovery.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DefectDetectionEvidence {
    examined: usize,
    insufficient_support: usize,
    unavailable_centres: usize,
    hot: usize,
    cold: usize,
}

impl DefectDetectionEvidence {
    /// Number of image samples visited.
    #[must_use]
    pub const fn examined(self) -> usize {
        self.examined
    }
    /// Samples for which too few valid neighbours existed.
    #[must_use]
    pub const fn insufficient_support(self) -> usize {
        self.insufficient_support
    }
    /// Masked or non-finite centre samples that were not classified.
    #[must_use]
    pub const fn unavailable_centres(self) -> usize {
        self.unavailable_centres
    }
    /// Samples classified as hot.
    #[must_use]
    pub const fn hot(self) -> usize {
        self.hot
    }
    /// Samples classified as cold.
    #[must_use]
    pub const fn cold(self) -> usize {
        self.cold
    }
}

/// Auditable totals from one conservative correction pass.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DefectCorrectionEvidence {
    requested: usize,
    corrected: usize,
    insufficient_support: usize,
    blocked_by_source_mask: usize,
}

impl DefectCorrectionEvidence {
    /// Defect-map samples presented to the correction pass.
    #[must_use]
    pub const fn requested(self) -> usize {
        self.requested
    }
    /// Samples replaced by a same-lattice median.
    #[must_use]
    pub const fn corrected(self) -> usize {
        self.corrected
    }
    /// Requested samples lacking sufficient clean neighbours.
    #[must_use]
    pub const fn insufficient_support(self) -> usize {
        self.insufficient_support
    }
    /// Requested samples carrying an unrelated source-mask reason.
    #[must_use]
    pub const fn blocked_by_source_mask(self) -> usize {
        self.blocked_by_source_mask
    }
}

/// Result of one correction pass. The original reasons remain in `defect_map`.
#[derive(Clone, Debug, PartialEq)]
pub struct CorrectedDefects {
    image: ScientificImage,
    defect_map: DefectMap,
    evidence: DefectCorrectionEvidence,
}

impl CorrectedDefects {
    /// Corrected image. Successful replacements are usable in this mask.
    #[must_use]
    pub const fn image(&self) -> &ScientificImage {
        &self.image
    }
    /// Immutable reasons that caused samples to be considered.
    #[must_use]
    pub const fn defect_map(&self) -> &DefectMap {
        &self.defect_map
    }
    /// Complete correction accounting.
    #[must_use]
    pub const fn evidence(&self) -> DefectCorrectionEvidence {
        self.evidence
    }
    /// Consumes the wrapper and returns the corrected image.
    #[must_use]
    pub fn into_image(self) -> ScientificImage {
        self.image
    }
}

/// Failure raised before a defensible defect result can be returned.
#[derive(Clone, Debug, PartialEq)]
pub enum DefectMapError {
    /// Radius falls outside the bounded implementation range.
    InvalidRadius {
        /// Rejected radius.
        radius: usize,
    },
    /// Only monochrome/plane-local or Bayer-phase sampling is supported.
    InvalidStride {
        /// Rejected detector-pixel stride.
        stride: usize,
    },
    /// Requested support cannot fit in the configured neighbourhood.
    InvalidMinimumNeighbours {
        /// Requested minimum.
        minimum: usize,
        /// Largest possible support for this radius.
        maximum: usize,
    },
    /// A sigma limit is non-finite or not positive.
    InvalidSigma {
        /// Limit direction.
        kind: DefectKind,
        /// Rejected value.
        value: f64,
    },
    /// The absolute residual floor is negative or non-finite.
    InvalidDeviationFloor {
        /// Rejected value.
        value: f64,
    },
    /// Source and map dimensions differ.
    DimensionMismatch {
        /// Science image dimensions.
        source: Dimensions,
        /// Defect map dimensions.
        map: Dimensions,
    },
    /// A map insertion contained no defect or a non-defect mask reason.
    InvalidDefectFlags {
        /// Rejected stable mask bits.
        bits: u8,
    },
    /// A merge requires at least one map.
    NoDefectMaps,
    /// Allocation or coordinate failure from the shared image core.
    Core(CoreError),
}

impl Display for DefectMapError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRadius { radius } => write!(
                formatter,
                "defect radius {radius} is outside 1..={MAX_RADIUS}"
            ),
            Self::InvalidStride { stride } => {
                write!(formatter, "defect stride {stride} is not 1 or 2")
            }
            Self::InvalidMinimumNeighbours { minimum, maximum } => write!(
                formatter,
                "minimum neighbour count {minimum} is outside 1..={maximum}"
            ),
            Self::InvalidSigma { kind, value } => write!(
                formatter,
                "{kind} sigma must be finite and positive, received {value}"
            ),
            Self::InvalidDeviationFloor { value } => write!(
                formatter,
                "minimum absolute deviation must be finite and non-negative, received {value}"
            ),
            Self::DimensionMismatch { source, map } => write!(
                formatter,
                "defect-map dimensions {}x{}x{} do not match source dimensions {}x{}x{}",
                map.width(),
                map.height(),
                map.planes(),
                source.width(),
                source.height(),
                source.planes()
            ),
            Self::InvalidDefectFlags { bits } => write!(
                formatter,
                "defect map accepts only non-empty HOT/COLD bits, received {bits:#010b}"
            ),
            Self::NoDefectMaps => formatter.write_str("at least one defect map is required"),
            Self::Core(error) => Display::fmt(error, formatter),
        }
    }
}

/// Combines master-derived maps without discarding contradictory evidence.
///
/// Input order cannot change the result. A sample classified HOT by one map and
/// COLD by another retains both reasons and is reported as a conflict for later
/// diagnostics instead of applying an arbitrary precedence rule.
pub fn merge_defect_maps(
    maps: &[&DefectMap],
) -> Result<(DefectMap, DefectMapMergeEvidence), DefectMapError> {
    let Some(first) = maps.first() else {
        return Err(DefectMapError::NoDefectMaps);
    };
    let dimensions = first.dimensions();
    for map in maps.iter().skip(1) {
        if map.dimensions() != dimensions {
            return Err(DefectMapError::DimensionMismatch {
                source: dimensions,
                map: map.dimensions(),
            });
        }
    }
    let mut merged = DefectMap::clear(dimensions)?;
    for map in maps {
        for (output, input) in merged
            .mask
            .as_mut_slice()
            .iter_mut()
            .zip(map.mask.as_slice())
        {
            *output |= *input;
        }
    }
    let mut evidence = DefectMapMergeEvidence {
        input_maps: maps.len(),
        ..DefectMapMergeEvidence::default()
    };
    for flags in merged.mask.as_slice() {
        let hot = flags.contains(PixelFlags::HOT);
        let cold = flags.contains(PixelFlags::COLD);
        evidence.defective_samples += usize::from(hot || cold);
        evidence.hot_samples += usize::from(hot);
        evidence.cold_samples += usize::from(cold);
        evidence.conflicting_samples += usize::from(hot && cold);
    }
    Ok((merged, evidence))
}

impl Error for DefectMapError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Core(error) => Some(error),
            _ => None,
        }
    }
}

/// Detects local positive and negative outliers with a median/MAD estimator.
///
/// Masked and non-finite samples never contribute to the model. The comparison
/// is strictly local, which avoids turning legitimate large-scale dark-current
/// or flat-field structure into a detector defect. Equality at a threshold is
/// retained as non-defective to make the decision boundary unambiguous.
pub fn detect_local_defects(
    reference: &ScientificImage,
    parameters: DefectDetectionParameters,
) -> Result<(DefectMap, DefectDetectionEvidence), DefectMapError> {
    let dimensions = reference.dimensions();
    let mut map = DefectMap::clear(dimensions)?;
    let mut values = Vec::new();
    let mut deviations = Vec::new();
    values
        .try_reserve_exact(parameters.maximum_neighbours())
        .map_err(|_| {
            DefectMapError::Core(CoreError::AllocationFailed {
                elements: parameters.maximum_neighbours(),
            })
        })?;
    deviations
        .try_reserve_exact(parameters.maximum_neighbours())
        .map_err(|_| {
            DefectMapError::Core(CoreError::AllocationFailed {
                elements: parameters.maximum_neighbours(),
            })
        })?;
    let mut evidence = DefectDetectionEvidence::default();

    for plane in 0..dimensions.planes() {
        for y in 0..dimensions.height() {
            for x in 0..dimensions.width() {
                evidence.examined += 1;
                let index = dimensions
                    .linear_index(x, y, plane)
                    .map_err(DefectMapError::Core)?;
                let centre = reference.pixels()[index];
                if !reference.mask().as_slice()[index].is_clear() || !centre.is_finite() {
                    evidence.unavailable_centres += 1;
                    continue;
                }
                collect_neighbours(reference, x, y, plane, parameters, &mut values)?;
                if values.len() < parameters.minimum_neighbours {
                    evidence.insufficient_support += 1;
                    continue;
                }
                let local_median = median(&mut values);
                deviations.clear();
                deviations.extend(values.iter().map(|value| (value - local_median).abs()));
                let robust_sigma = median(&mut deviations) * ROBUST_NORMAL_SCALE;
                let residual = centre - local_median;
                let absolute = residual.abs();
                if absolute <= parameters.minimum_absolute_deviation {
                    continue;
                }
                if residual > robust_sigma * parameters.hot_sigma {
                    map.mask.as_mut_slice()[index] |= PixelFlags::HOT;
                    evidence.hot += 1;
                } else if residual < -(robust_sigma * parameters.cold_sigma) {
                    map.mask.as_mut_slice()[index] |= PixelFlags::COLD;
                    evidence.cold += 1;
                }
            }
        }
    }
    Ok((map, evidence))
}

/// Replaces mapped defects with the exact median of clean same-lattice samples.
///
/// All neighbours are read from the immutable input, so adjacent defects and
/// traversal order cannot feed corrected values back into the result. A sample
/// with an unrelated mask reason is never repaired. Failed corrections retain
/// their defect reason and add `MISSING`; successful corrections clear only the
/// `HOT` and `COLD` bits while the separate defect map preserves provenance.
pub fn correct_defects(
    source: &ScientificImage,
    defect_map: &DefectMap,
    parameters: DefectCorrectionParameters,
) -> Result<CorrectedDefects, DefectMapError> {
    if source.dimensions() != defect_map.dimensions() {
        return Err(DefectMapError::DimensionMismatch {
            source: source.dimensions(),
            map: defect_map.dimensions(),
        });
    }
    let dimensions = source.dimensions();
    let mut output = source.clone();
    let mut values = Vec::new();
    values
        .try_reserve_exact(parameters.maximum_neighbours())
        .map_err(|_| {
            DefectMapError::Core(CoreError::AllocationFailed {
                elements: parameters.maximum_neighbours(),
            })
        })?;
    let defect_bits = (PixelFlags::HOT | PixelFlags::COLD).bits();
    let mut evidence = DefectCorrectionEvidence::default();

    for plane in 0..dimensions.planes() {
        for y in 0..dimensions.height() {
            for x in 0..dimensions.width() {
                let index = dimensions
                    .linear_index(x, y, plane)
                    .map_err(DefectMapError::Core)?;
                let mapped = defect_map.mask.as_slice()[index];
                if !mapped.contains(PixelFlags::HOT) && !mapped.contains(PixelFlags::COLD) {
                    continue;
                }
                evidence.requested += 1;
                let source_flags = source.mask().as_slice()[index];
                let unrelated = source_flags.bits() & !defect_bits;
                if unrelated != 0 {
                    output.mask_mut().as_mut_slice()[index] |= mapped;
                    evidence.blocked_by_source_mask += 1;
                    continue;
                }
                collect_clean_correction_neighbours(
                    source,
                    defect_map,
                    x,
                    y,
                    plane,
                    parameters,
                    &mut values,
                )?;
                if values.len() < parameters.minimum_neighbours {
                    output.pixels_mut()[index] = f64::NAN;
                    output.mask_mut().as_mut_slice()[index] = mapped | PixelFlags::MISSING;
                    evidence.insufficient_support += 1;
                    continue;
                }
                output.pixels_mut()[index] = median(&mut values);
                output.mask_mut().as_mut_slice()[index] =
                    PixelFlags::from_bits_retain(source_flags.bits() & !defect_bits);
                evidence.corrected += 1;
            }
        }
    }

    Ok(CorrectedDefects {
        image: output,
        defect_map: defect_map.clone(),
        evidence,
    })
}

fn collect_neighbours(
    image: &ScientificImage,
    x: usize,
    y: usize,
    plane: usize,
    parameters: DefectDetectionParameters,
    values: &mut Vec<f64>,
) -> Result<(), DefectMapError> {
    values.clear();
    visit_neighbours(
        image.dimensions(),
        x,
        y,
        parameters.radius,
        parameters.stride,
        |nx, ny| {
            let index = image
                .dimensions()
                .linear_index(nx, ny, plane)
                .map_err(DefectMapError::Core)?;
            let value = image.pixels()[index];
            if image.mask().as_slice()[index].is_clear() && value.is_finite() {
                values.push(value);
            }
            Ok(())
        },
    )
}

fn collect_clean_correction_neighbours(
    image: &ScientificImage,
    map: &DefectMap,
    x: usize,
    y: usize,
    plane: usize,
    parameters: DefectCorrectionParameters,
    values: &mut Vec<f64>,
) -> Result<(), DefectMapError> {
    values.clear();
    visit_neighbours(
        image.dimensions(),
        x,
        y,
        parameters.radius,
        parameters.stride,
        |nx, ny| {
            let index = image
                .dimensions()
                .linear_index(nx, ny, plane)
                .map_err(DefectMapError::Core)?;
            let value = image.pixels()[index];
            if image.mask().as_slice()[index].is_clear()
                && map.mask.as_slice()[index].is_clear()
                && value.is_finite()
            {
                values.push(value);
            }
            Ok(())
        },
    )
}

fn validate_neighbourhood(
    radius: usize,
    stride: usize,
    minimum_neighbours: usize,
) -> Result<(), DefectMapError> {
    if radius == 0 || radius > MAX_RADIUS {
        return Err(DefectMapError::InvalidRadius { radius });
    }
    if stride == 0 || stride > 2 {
        return Err(DefectMapError::InvalidStride { stride });
    }
    let maximum = maximum_neighbours(radius);
    if minimum_neighbours == 0 || minimum_neighbours > maximum {
        return Err(DefectMapError::InvalidMinimumNeighbours {
            minimum: minimum_neighbours,
            maximum,
        });
    }
    Ok(())
}

fn validate_defect_flags(flags: PixelFlags) -> Result<(), DefectMapError> {
    let allowed = (PixelFlags::HOT | PixelFlags::COLD).bits();
    if flags.bits() == 0 || flags.bits() & !allowed != 0 {
        return Err(DefectMapError::InvalidDefectFlags { bits: flags.bits() });
    }
    Ok(())
}

fn maximum_neighbours(radius: usize) -> usize {
    let side = radius * 2 + 1;
    side * side - 1
}

fn visit_neighbours<F>(
    dimensions: Dimensions,
    x: usize,
    y: usize,
    radius: usize,
    stride: usize,
    mut visit: F,
) -> Result<(), DefectMapError>
where
    F: FnMut(usize, usize) -> Result<(), DefectMapError>,
{
    let radius = isize::try_from(radius).map_err(|_| DefectMapError::InvalidRadius { radius })?;
    let stride = isize::try_from(stride).map_err(|_| DefectMapError::InvalidStride { stride })?;
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            if dx == 0 && dy == 0 {
                continue;
            }
            let Some(nx) = x.checked_add_signed(dx * stride) else {
                continue;
            };
            let Some(ny) = y.checked_add_signed(dy * stride) else {
                continue;
            };
            if nx < dimensions.width() && ny < dimensions.height() {
                visit(nx, ny)?;
            }
        }
    }
    Ok(())
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_unstable_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        values[middle - 1] * 0.5 + values[middle] * 0.5
    } else {
        values[middle]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn Error>>;

    fn parameters(stride: usize) -> Result<DefectDetectionParameters, DefectMapError> {
        DefectDetectionParameters::new(2, stride, 8, 5.0, 5.0, 1.0)
    }

    fn correction_parameters(stride: usize) -> Result<DefectCorrectionParameters, DefectMapError> {
        DefectCorrectionParameters::new(2, stride, 8)
    }

    fn image(width: usize, height: usize, value: f64) -> TestResult<ScientificImage> {
        Ok(ScientificImage::filled(
            Dimensions::new(width, height, 1)?,
            value,
        )?)
    }

    #[test]
    fn rejects_invalid_policies() {
        assert!(matches!(
            DefectDetectionParameters::new(0, 1, 1, 5.0, 5.0, 0.0),
            Err(DefectMapError::InvalidRadius { .. })
        ));
        assert!(matches!(
            DefectDetectionParameters::new(1, 3, 1, 5.0, 5.0, 0.0),
            Err(DefectMapError::InvalidStride { .. })
        ));
        assert!(matches!(
            DefectDetectionParameters::new(1, 1, 9, 5.0, 5.0, 0.0),
            Err(DefectMapError::InvalidMinimumNeighbours { .. })
        ));
        assert!(matches!(
            DefectDetectionParameters::new(1, 1, 1, f64::NAN, 5.0, 0.0),
            Err(DefectMapError::InvalidSigma {
                kind: DefectKind::Hot,
                ..
            })
        ));
        assert!(matches!(
            DefectDetectionParameters::new(1, 1, 1, 5.0, 5.0, -1.0),
            Err(DefectMapError::InvalidDeviationFloor { .. })
        ));
    }

    #[test]
    fn detects_hot_and_cold_local_outliers() -> TestResult {
        let mut reference = image(9, 9, 100.0)?;
        reference.pixels_mut()[4 * 9 + 4] = 500.0;
        reference.pixels_mut()[6 * 9 + 6] = -100.0;
        let (map, evidence) = detect_local_defects(&reference, parameters(1)?)?;
        assert!(map.mask().get(4, 4, 0)?.contains(PixelFlags::HOT));
        assert!(map.mask().get(6, 6, 0)?.contains(PixelFlags::COLD));
        assert_eq!(evidence.hot(), 1);
        assert_eq!(evidence.cold(), 1);
        assert_eq!(evidence.examined(), 81);
        Ok(())
    }

    #[test]
    fn robust_spread_controls_classification_boundary() -> TestResult {
        let mut reference = image(7, 7, 100.0)?;
        for y in 1..=5 {
            for x in 1..=5 {
                reference.pixels_mut()[y * 7 + x] = if (x + y) % 2 == 0 { 99.0 } else { 101.0 };
            }
        }
        reference.pixels_mut()[3 * 7 + 3] = 104.0;
        let (map, _) = detect_local_defects(
            &reference,
            DefectDetectionParameters::new(2, 1, 8, 5.0, 5.0, 1.0)?,
        )?;
        assert!(map.mask().get(3, 3, 0)?.is_clear());
        Ok(())
    }

    #[test]
    fn absolute_floor_protects_constant_neighbourhoods() -> TestResult {
        let mut reference = image(7, 7, 42.0)?;
        reference.pixels_mut()[3 * 7 + 3] = 42.5;
        let (map, _) = detect_local_defects(&reference, parameters(1)?)?;
        assert!(map.mask().get(3, 3, 0)?.is_clear());
        Ok(())
    }

    #[test]
    fn masked_and_nonfinite_centres_are_not_reclassified() -> TestResult {
        let mut reference = image(7, 7, 100.0)?;
        reference.pixels_mut()[3 * 7 + 3] = f64::NAN;
        reference.mark(2, 2, 0, PixelFlags::SATURATED)?;
        let (map, evidence) = detect_local_defects(&reference, parameters(1)?)?;
        assert!(map.mask().get(3, 3, 0)?.is_clear());
        assert!(map.mask().get(2, 2, 0)?.is_clear());
        assert_eq!(evidence.unavailable_centres(), 2);
        Ok(())
    }

    #[test]
    fn bayer_stride_uses_only_matching_detector_phase() -> TestResult {
        let dimensions = Dimensions::new(9, 9, 1)?;
        let mut pixels = Vec::with_capacity(dimensions.pixel_count());
        for y in 0..9 {
            for x in 0..9 {
                pixels.push(if (x + y) % 2 == 0 { 10.0 } else { 1_000.0 });
            }
        }
        let mut reference = ScientificImage::from_pixels(dimensions, pixels)?;
        reference.pixels_mut()[4 * 9 + 4] = 60.0;
        let (map, _) = detect_local_defects(&reference, parameters(2)?)?;
        assert!(map.mask().get(4, 4, 0)?.contains(PixelFlags::HOT));
        assert!(map.mask().get(4, 5, 0)?.is_clear());
        Ok(())
    }

    #[test]
    fn correction_uses_original_clean_neighbours_and_preserves_map() -> TestResult {
        let mut source = image(9, 9, 10.0)?;
        source.pixels_mut()[4 * 9 + 4] = 1_000.0;
        source.pixels_mut()[4 * 9 + 5] = 2_000.0;
        let mut map = DefectMap::clear(source.dimensions())?;
        map.insert(4, 4, 0, PixelFlags::HOT)?;
        map.insert(5, 4, 0, PixelFlags::HOT)?;
        let corrected = correct_defects(&source, &map, correction_parameters(1)?)?;
        assert_eq!(
            corrected.image().pixels()[4 * 9 + 4].to_bits(),
            10.0_f64.to_bits()
        );
        assert_eq!(
            corrected.image().pixels()[4 * 9 + 5].to_bits(),
            10.0_f64.to_bits()
        );
        assert!(corrected.image().mask().get(4, 4, 0)?.is_clear());
        assert!(
            corrected
                .defect_map()
                .mask()
                .get(4, 4, 0)?
                .contains(PixelFlags::HOT)
        );
        assert_eq!(corrected.evidence().corrected(), 2);
        Ok(())
    }

    #[test]
    fn correction_fails_closed_without_support() -> TestResult {
        let source = image(3, 3, 10.0)?;
        let mut map = DefectMap::clear(source.dimensions())?;
        map.insert(1, 1, 0, PixelFlags::COLD)?;
        let corrected = correct_defects(&source, &map, correction_parameters(2)?)?;
        assert!(corrected.image().pixels()[4].is_nan());
        let flags = corrected.image().mask().get(1, 1, 0)?;
        assert!(flags.contains(PixelFlags::COLD));
        assert!(flags.contains(PixelFlags::MISSING));
        assert_eq!(corrected.evidence().insufficient_support(), 1);
        Ok(())
    }

    #[test]
    fn correction_refuses_unrelated_mask_reasons() -> TestResult {
        let mut source = image(7, 7, 10.0)?;
        source.mark(3, 3, 0, PixelFlags::SATURATED)?;
        let mut map = DefectMap::clear(source.dimensions())?;
        map.insert(3, 3, 0, PixelFlags::HOT)?;
        let corrected = correct_defects(&source, &map, correction_parameters(1)?)?;
        assert_eq!(corrected.image().pixels()[24].to_bits(), 10.0_f64.to_bits());
        let flags = corrected.image().mask().get(3, 3, 0)?;
        assert!(flags.contains(PixelFlags::SATURATED));
        assert!(flags.contains(PixelFlags::HOT));
        assert_eq!(corrected.evidence().blocked_by_source_mask(), 1);
        Ok(())
    }

    #[test]
    fn dimension_mismatch_is_explicit() -> TestResult {
        let source = image(5, 5, 1.0)?;
        let map = DefectMap::clear(Dimensions::new(4, 5, 1)?)?;
        assert!(matches!(
            correct_defects(&source, &map, correction_parameters(1)?),
            Err(DefectMapError::DimensionMismatch { .. })
        ));
        Ok(())
    }

    #[test]
    fn map_rejects_non_defect_reasons() -> TestResult {
        let dimensions = Dimensions::new(2, 2, 1)?;
        let mut map = DefectMap::clear(dimensions)?;
        assert!(matches!(
            map.insert(0, 0, 0, PixelFlags::CLEAR),
            Err(DefectMapError::InvalidDefectFlags { bits: 0 })
        ));
        assert!(matches!(
            map.insert(0, 0, 0, PixelFlags::SATURATED),
            Err(DefectMapError::InvalidDefectFlags { .. })
        ));
        Ok(())
    }

    #[test]
    fn merge_retains_union_and_conflicts_independent_of_order() -> TestResult {
        let dimensions = Dimensions::new(3, 1, 1)?;
        let mut dark = DefectMap::clear(dimensions)?;
        dark.insert(0, 0, 0, PixelFlags::HOT)?;
        dark.insert(1, 0, 0, PixelFlags::HOT)?;
        let mut flat = DefectMap::clear(dimensions)?;
        flat.insert(1, 0, 0, PixelFlags::COLD)?;
        flat.insert(2, 0, 0, PixelFlags::COLD)?;
        let (merged, evidence) = merge_defect_maps(&[&dark, &flat])?;
        let (reversed, _) = merge_defect_maps(&[&flat, &dark])?;
        assert_eq!(merged, reversed);
        assert_eq!(evidence.input_maps(), 2);
        assert_eq!(evidence.defective_samples(), 3);
        assert_eq!(evidence.hot_samples(), 2);
        assert_eq!(evidence.cold_samples(), 2);
        assert_eq!(evidence.conflicting_samples(), 1);
        let conflict = merged.mask().get(1, 0, 0)?;
        assert!(conflict.contains(PixelFlags::HOT));
        assert!(conflict.contains(PixelFlags::COLD));
        Ok(())
    }

    #[test]
    fn merge_rejects_empty_and_mismatched_sets() -> TestResult {
        assert!(matches!(
            merge_defect_maps(&[]),
            Err(DefectMapError::NoDefectMaps)
        ));
        let first = DefectMap::clear(Dimensions::new(2, 2, 1)?)?;
        let second = DefectMap::clear(Dimensions::new(3, 2, 1)?)?;
        assert!(matches!(
            merge_defect_maps(&[&first, &second]),
            Err(DefectMapError::DimensionMismatch { .. })
        ));
        Ok(())
    }
}
