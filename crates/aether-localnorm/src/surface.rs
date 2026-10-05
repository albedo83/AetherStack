use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_core::CompensatedSum;

use crate::LocalCellFit;

/// Stable identity of guarded inverse-distance coefficient interpolation.
pub const LOCAL_SURFACE_ALGORITHM_ID: &str = "local-coefficient-idw2-f64-v1";

/// Explicit support and locality requirements for coefficient interpolation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceParameters {
    minimum_control_points: usize,
    minimum_neighbors: usize,
    maximum_neighbors: usize,
    maximum_distance: f64,
}

impl SurfaceParameters {
    /// Builds strict surface controls.
    pub fn new(
        minimum_control_points: usize,
        minimum_neighbors: usize,
        maximum_neighbors: usize,
        maximum_distance: f64,
    ) -> Result<Self, SurfaceError> {
        let maximum_distance_squared = maximum_distance * maximum_distance;
        if minimum_control_points == 0
            || minimum_neighbors == 0
            || maximum_neighbors < minimum_neighbors
            || !maximum_distance.is_finite()
            || maximum_distance <= 0.0
            || !maximum_distance_squared.is_finite()
        {
            return Err(SurfaceError::InvalidParameters);
        }
        Ok(Self {
            minimum_control_points,
            minimum_neighbors,
            maximum_neighbors,
            maximum_distance,
        })
    }

    /// Minimum number of valid cells required to construct a surface.
    #[must_use]
    pub const fn minimum_control_points(self) -> usize {
        self.minimum_control_points
    }

    /// Minimum nearby controls required away from an exact control point.
    #[must_use]
    pub const fn minimum_neighbors(self) -> usize {
        self.minimum_neighbors
    }

    /// Maximum nearby controls contributing to one evaluation.
    #[must_use]
    pub const fn maximum_neighbors(self) -> usize {
        self.maximum_neighbors
    }

    /// Inclusive support radius in image pixels.
    #[must_use]
    pub const fn maximum_distance(self) -> f64 {
        self.maximum_distance
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ControlPoint {
    x: f64,
    y: f64,
    scale: f64,
    offset: f64,
}

/// Immutable coefficient surface built only from valid cell fits.
#[derive(Clone, Debug, PartialEq)]
pub struct LocalCoefficientSurface {
    controls: Vec<ControlPoint>,
    rejected_cell_count: usize,
    cell_count: usize,
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
    parameters: SurfaceParameters,
}

impl LocalCoefficientSurface {
    /// Number of valid cell models retained as controls.
    #[must_use]
    pub fn control_point_count(&self) -> usize {
        self.controls.len()
    }

    /// Number of cell fits excluded from the surface.
    #[must_use]
    pub const fn rejected_cell_count(&self) -> usize {
        self.rejected_cell_count
    }

    /// Total number of cell outcomes inspected during construction.
    #[must_use]
    pub const fn cell_count(&self) -> usize {
        self.cell_count
    }

    /// Evaluates scale and offset with explicit local-support evidence.
    pub fn evaluate(&self, x: f64, y: f64) -> Result<SurfaceEvaluation, SurfaceError> {
        let mut neighbors = Vec::new();
        neighbors
            .try_reserve_exact(self.parameters.maximum_neighbors)
            .map_err(|_| SurfaceError::AllocationFailed)?;
        self.evaluate_with_scratch(x, y, &mut neighbors)
    }

    pub(crate) const fn scratch_capacity(&self) -> usize {
        self.parameters.maximum_neighbors
    }

    pub(crate) fn evaluate_with_scratch(
        &self,
        x: f64,
        y: f64,
        neighbors: &mut Vec<(f64, usize)>,
    ) -> Result<SurfaceEvaluation, SurfaceError> {
        if !x.is_finite() || !y.is_finite() {
            return Err(SurfaceError::NonFiniteCoordinate);
        }
        if x < self.left || x >= self.right || y < self.top || y >= self.bottom {
            return Err(SurfaceError::OutsideDomain);
        }

        neighbors.clear();
        let maximum_distance_squared =
            self.parameters.maximum_distance * self.parameters.maximum_distance;
        for (index, control) in self.controls.iter().enumerate() {
            let dx = x - control.x;
            let dy = y - control.y;
            let distance_squared = dx.mul_add(dx, dy * dy);
            if distance_squared == 0.0 {
                return Ok(SurfaceEvaluation {
                    scale: control.scale,
                    offset: control.offset,
                    neighbor_count: 1,
                    furthest_distance: 0.0,
                    exact_control: true,
                });
            }
            if distance_squared <= maximum_distance_squared {
                retain_nearest(
                    neighbors,
                    (distance_squared, index),
                    self.parameters.maximum_neighbors,
                );
            }
        }
        if neighbors.len() < self.parameters.minimum_neighbors {
            return Err(SurfaceError::InsufficientLocalSupport);
        }

        let nearest_distance_squared = neighbors[0].0;
        let scale =
            stable_weighted_mean(neighbors, &self.controls, nearest_distance_squared, |p| {
                p.scale
            })?;
        let offset =
            stable_weighted_mean(neighbors, &self.controls, nearest_distance_squared, |p| {
                p.offset
            })?;
        let furthest_distance = neighbors.last().map_or(0.0, |neighbor| neighbor.0.sqrt());
        Ok(SurfaceEvaluation {
            scale,
            offset,
            neighbor_count: neighbors.len(),
            furthest_distance,
            exact_control: false,
        })
    }
}

/// Interpolated coefficients and the support used to derive them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceEvaluation {
    scale: f64,
    offset: f64,
    neighbor_count: usize,
    furthest_distance: f64,
    exact_control: bool,
}

impl SurfaceEvaluation {
    /// Interpolated source-to-reference multiplier.
    #[must_use]
    pub const fn scale(self) -> f64 {
        self.scale
    }

    /// Interpolated source-to-reference additive term.
    #[must_use]
    pub const fn offset(self) -> f64 {
        self.offset
    }

    /// Number of control points used by the evaluation.
    #[must_use]
    pub const fn neighbor_count(self) -> usize {
        self.neighbor_count
    }

    /// Distance to the furthest contributing control point.
    #[must_use]
    pub const fn furthest_distance(self) -> f64 {
        self.furthest_distance
    }

    /// Whether the coordinate exactly matched one control point.
    #[must_use]
    pub const fn exact_control(self) -> bool {
        self.exact_control
    }

    /// Applies the local affine relation without clipping.
    pub fn apply(self, source: f64) -> Result<f64, SurfaceError> {
        if !source.is_finite() {
            return Err(SurfaceError::NonFiniteValue);
        }
        let value = self.scale.mul_add(source, self.offset);
        if !value.is_finite() {
            return Err(SurfaceError::NonFiniteModel);
        }
        Ok(canonical_zero(value))
    }
}

/// Failure to build or evaluate a guarded local coefficient surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SurfaceError {
    /// Support controls or radius are invalid.
    InvalidParameters,
    /// Too few valid cells exist to construct the surface.
    InsufficientControlPoints,
    /// An evaluation coordinate is NaN or infinite.
    NonFiniteCoordinate,
    /// An evaluation coordinate lies outside the sampled image domain.
    OutsideDomain,
    /// Too few controls fall inside the configured local radius.
    InsufficientLocalSupport,
    /// A source value supplied for transformation is NaN or infinite.
    NonFiniteValue,
    /// Scratch or surface allocation failed.
    AllocationFailed,
    /// Interpolated coefficients or a transformed value are not finite.
    NonFiniteModel,
}

impl Display for SurfaceError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidParameters => "local surface parameters are inconsistent",
            Self::InsufficientControlPoints => "local surface has too few valid control points",
            Self::NonFiniteCoordinate => "local surface coordinate must be finite",
            Self::OutsideDomain => "local surface coordinate is outside its image domain",
            Self::InsufficientLocalSupport => "local surface has insufficient nearby support",
            Self::NonFiniteValue => "local surface input value must be finite",
            Self::AllocationFailed => "local surface allocation failed",
            Self::NonFiniteModel => "local surface result is outside the finite binary64 domain",
        })
    }
}

impl Error for SurfaceError {}

/// Builds an immutable surface while retaining rejected-cell totals.
pub fn build_local_surface(
    cells: &[LocalCellFit],
    parameters: SurfaceParameters,
) -> Result<LocalCoefficientSurface, SurfaceError> {
    if cells.is_empty() {
        return Err(SurfaceError::InsufficientControlPoints);
    }
    let mut controls = Vec::new();
    controls
        .try_reserve_exact(cells.len())
        .map_err(|_| SurfaceError::AllocationFailed)?;
    let mut rejected_cell_count = 0_usize;
    let mut left = f64::INFINITY;
    let mut top = f64::INFINITY;
    let mut right = 0.0_f64;
    let mut bottom = 0.0_f64;
    for cell in cells {
        let bounds = cell.bounds();
        left = left.min(bounds.x() as f64);
        top = top.min(bounds.y() as f64);
        right = right.max((bounds.x() + bounds.width()) as f64);
        bottom = bottom.max((bounds.y() + bounds.height()) as f64);
        match cell.result() {
            Ok(model) => controls.push(ControlPoint {
                x: bounds.x() as f64 + (bounds.width() - 1) as f64 * 0.5,
                y: bounds.y() as f64 + (bounds.height() - 1) as f64 * 0.5,
                scale: model.scale(),
                offset: model.offset(),
            }),
            Err(_) => rejected_cell_count += 1,
        }
    }
    if controls.len() < parameters.minimum_control_points {
        return Err(SurfaceError::InsufficientControlPoints);
    }
    Ok(LocalCoefficientSurface {
        controls,
        rejected_cell_count,
        cell_count: cells.len(),
        left,
        top,
        right,
        bottom,
        parameters,
    })
}

fn retain_nearest(neighbors: &mut Vec<(f64, usize)>, candidate: (f64, usize), limit: usize) {
    if neighbors.len() < limit {
        neighbors.push(candidate);
    } else if neighbors.last().is_some_and(|last| {
        candidate
            .0
            .total_cmp(&last.0)
            .then(candidate.1.cmp(&last.1))
            .is_lt()
    }) {
        if let Some(last) = neighbors.last_mut() {
            *last = candidate;
        }
    } else {
        return;
    }
    neighbors.sort_unstable_by(|left, right| left.0.total_cmp(&right.0).then(left.1.cmp(&right.1)));
}

fn stable_weighted_mean(
    neighbors: &[(f64, usize)],
    controls: &[ControlPoint],
    nearest_distance_squared: f64,
    coefficient: impl Fn(&ControlPoint) -> f64,
) -> Result<f64, SurfaceError> {
    let coefficient_scale = neighbors
        .iter()
        .map(|&(_, index)| coefficient(&controls[index]).abs())
        .fold(0.0_f64, f64::max);
    if coefficient_scale == 0.0 {
        return Ok(0.0);
    }
    let mut numerator = CompensatedSum::new();
    let mut denominator = CompensatedSum::new();
    for &(distance_squared, index) in neighbors {
        let weight = nearest_distance_squared / distance_squared;
        numerator.add(weight * (coefficient(&controls[index]) / coefficient_scale));
        denominator.add(weight);
    }
    let result = coefficient_scale * (numerator.total() / denominator.total());
    if !result.is_finite() {
        return Err(SurfaceError::NonFiniteModel);
    }
    Ok(canonical_zero(result))
}

fn canonical_zero(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LocalFitParameters, SamplingGridParameters, fit_local_grid, sample_local_grid};
    use aether_core::{Dimensions, PixelFlags, ScientificImage};

    type TestResult = Result<(), Box<dyn Error>>;

    fn fitted_cells() -> Result<Vec<LocalCellFit>, Box<dyn Error>> {
        let dimensions = Dimensions::new(6, 3, 1)?;
        let source_values = (1..=18).map(f64::from).collect::<Vec<_>>();
        let reference_values = source_values
            .iter()
            .enumerate()
            .map(|(index, &value)| {
                if index % 6 < 3 {
                    2.0 * value + 1.0
                } else {
                    4.0 * value + 3.0
                }
            })
            .collect();
        let source = ScientificImage::from_pixels(dimensions, source_values)?;
        let reference = ScientificImage::from_pixels(dimensions, reference_values)?;
        let samples = sample_local_grid(
            &source,
            &reference,
            None,
            0,
            SamplingGridParameters::new(3, 3, 9, 2)?,
        )?;
        Ok(fit_local_grid(
            &samples,
            LocalFitParameters::new(9, 9, 36, 1.0e-12)?,
        )?)
    }

    #[test]
    fn exact_controls_and_midpoint_interpolation_retain_support() -> TestResult {
        let cells = fitted_cells()?;
        let surface = build_local_surface(&cells, SurfaceParameters::new(2, 2, 2, 10.0)?)?;
        assert_eq!(surface.control_point_count(), 2);
        assert_eq!(surface.rejected_cell_count(), 0);
        assert_eq!(surface.cell_count(), 2);

        let exact = surface.evaluate(1.0, 1.0)?;
        assert!(exact.exact_control());
        assert_eq!(exact.scale().to_bits(), 2.0_f64.to_bits());
        assert_eq!(exact.offset().to_bits(), 1.0_f64.to_bits());
        assert_eq!(exact.neighbor_count(), 1);

        let midpoint = surface.evaluate(2.5, 1.0)?;
        assert!(!midpoint.exact_control());
        assert_eq!(midpoint.scale().to_bits(), 3.0_f64.to_bits());
        assert_eq!(midpoint.offset().to_bits(), 2.0_f64.to_bits());
        assert_eq!(midpoint.neighbor_count(), 2);
        assert_eq!(midpoint.furthest_distance().to_bits(), 1.5_f64.to_bits());
        assert_eq!(midpoint.apply(5.0)?.to_bits(), 17.0_f64.to_bits());
        Ok(())
    }

    #[test]
    fn holes_and_invalid_queries_fail_closed() -> TestResult {
        let dimensions = Dimensions::new(6, 3, 1)?;
        let mut source =
            ScientificImage::from_pixels(dimensions, (1..=18).map(f64::from).collect::<Vec<_>>())?;
        let reference = ScientificImage::from_pixels(
            dimensions,
            (1..=18).map(|value| 2.0 * f64::from(value) + 1.0).collect(),
        )?;
        for y in 0..3 {
            for x in 3..6 {
                source.mark(x, y, 0, PixelFlags::MISSING)?;
            }
        }
        let samples = sample_local_grid(
            &source,
            &reference,
            None,
            0,
            SamplingGridParameters::new(3, 3, 9, 2)?,
        )?;
        let fits = fit_local_grid(&samples, LocalFitParameters::new(3, 9, 36, 1.0e-12)?)?;
        let surface = build_local_surface(&fits, SurfaceParameters::new(1, 2, 2, 10.0)?)?;
        assert_eq!(surface.control_point_count(), 1);
        assert_eq!(surface.rejected_cell_count(), 1);
        assert_eq!(
            surface.evaluate(4.0, 1.0),
            Err(SurfaceError::InsufficientLocalSupport)
        );
        assert_eq!(
            surface.evaluate(f64::NAN, 1.0),
            Err(SurfaceError::NonFiniteCoordinate)
        );
        assert_eq!(surface.evaluate(6.0, 1.0), Err(SurfaceError::OutsideDomain));
        assert_eq!(
            SurfaceParameters::new(1, 2, 1, 10.0),
            Err(SurfaceError::InvalidParameters)
        );
        Ok(())
    }
}
