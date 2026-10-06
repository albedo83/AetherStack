use aether_registration::{ImagePoint, ProjectiveTransform};

use crate::{DrizzleError, DrizzleOutputBounds, DrizzleParameters, DrizzleTileBounds, finite_i64};

/// Stable identity of conservative inverse-homography source-window planning.
pub const DRIZZLE_SOURCE_WINDOW_ALGORITHM_ID: &str = "drizzle-source-window-projective-v1";

/// Inclusive-origin, exclusive-extent detector window needed by one output tile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DrizzleSourceWindow {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl DrizzleSourceWindow {
    /// First included detector column.
    #[must_use]
    pub const fn x(self) -> u32 {
        self.x
    }

    /// First included detector row.
    #[must_use]
    pub const fn y(self) -> u32 {
        self.y
    }

    /// Number of included detector columns.
    #[must_use]
    pub const fn width(self) -> u32 {
        self.width
    }

    /// Number of included detector rows.
    #[must_use]
    pub const fn height(self) -> u32 {
        self.height
    }

    /// Whether one detector coordinate belongs to the planned window.
    #[must_use]
    pub const fn contains(self, x: u32, y: u32) -> bool {
        x >= self.x
            && y >= self.y
            && (x as u64) < self.x as u64 + self.width as u64
            && (y as u64) < self.y as u64 + self.height as u64
    }
}

/// Plans a conservative detector window for one bounded output tile.
///
/// The tile's continuous output-pixel rectangle is converted back to reference
/// coordinates and mapped through the inverse source-to-reference homography.
/// The inverse denominator must retain one nonzero sign at all four corners;
/// because it is linear, this rejects every projective pole inside the tile.
/// Expanding the resulting source quadrilateral bounds by half a detector drop
/// guarantees that every photosite whose projected drop can overlap the tile is
/// included. The returned rectangle is clipped to the detector, or `None` when
/// the source and tile are disjoint.
pub fn plan_drizzle_source_window(
    source_width: u32,
    source_height: u32,
    source_to_reference: ProjectiveTransform,
    parameters: DrizzleParameters,
    tile: DrizzleTileBounds,
    output: DrizzleOutputBounds,
) -> Result<Option<DrizzleSourceWindow>, DrizzleError> {
    if source_width == 0 || source_height == 0 || tile.dimensions().planes() != 3 {
        return Err(DrizzleError::InvalidGeometry);
    }
    let tile_right = u64::from(tile.origin_x()) + tile.dimensions().width() as u64;
    let tile_bottom = u64::from(tile.origin_y()) + tile.dimensions().height() as u64;
    if tile_right > u64::from(output.width()) || tile_bottom > u64::from(output.height()) {
        return Err(DrizzleError::InvalidGeometry);
    }

    let scale = f64::from(parameters.scale());
    let left = f64::from(tile.origin_x()) / scale - 0.5;
    let top = f64::from(tile.origin_y()) / scale - 0.5;
    let right = tile_right as f64 / scale - 0.5;
    let bottom = tile_bottom as f64 / scale - 0.5;
    let reference_corners = [(left, top), (right, top), (right, bottom), (left, bottom)];
    let inverse = source_to_reference
        .inverse()
        .map_err(|_| DrizzleError::TransformFailed)?;
    validate_denominator(inverse, reference_corners)?;

    let mut minimum_x = f64::INFINITY;
    let mut maximum_x = f64::NEG_INFINITY;
    let mut minimum_y = f64::INFINITY;
    let mut maximum_y = f64::NEG_INFINITY;
    for (x, y) in reference_corners {
        let source = inverse
            .apply(ImagePoint::new(x, y).map_err(|_| DrizzleError::InvalidGeometry)?)
            .map_err(|_| DrizzleError::TransformFailed)?;
        minimum_x = minimum_x.min(source.x());
        maximum_x = maximum_x.max(source.x());
        minimum_y = minimum_y.min(source.y());
        maximum_y = maximum_y.max(source.y());
    }

    let half_drop = parameters.drop_shrink() * 0.5;
    let first_x = finite_i64((minimum_x - half_drop).floor())?.max(0);
    let last_x = finite_i64((maximum_x + half_drop).floor())?.min(i64::from(source_width) - 1);
    let first_y = finite_i64((minimum_y - half_drop).floor())?.max(0);
    let last_y = finite_i64((maximum_y + half_drop).floor())?.min(i64::from(source_height) - 1);
    if first_x > last_x || first_y > last_y {
        return Ok(None);
    }
    let x = u32::try_from(first_x).map_err(|_| DrizzleError::InvalidGeometry)?;
    let y = u32::try_from(first_y).map_err(|_| DrizzleError::InvalidGeometry)?;
    let width = u32::try_from(last_x - first_x + 1).map_err(|_| DrizzleError::InvalidGeometry)?;
    let height = u32::try_from(last_y - first_y + 1).map_err(|_| DrizzleError::InvalidGeometry)?;
    Ok(Some(DrizzleSourceWindow {
        x,
        y,
        width,
        height,
    }))
}

fn validate_denominator(
    transform: ProjectiveTransform,
    corners: [(f64, f64); 4],
) -> Result<(), DrizzleError> {
    let matrix = transform.coefficients();
    let mut expected_sign = None;
    for (x, y) in corners {
        let denominator = matrix[2][0].mul_add(x, matrix[2][1].mul_add(y, matrix[2][2]));
        if !denominator.is_finite() || denominator == 0.0 {
            return Err(DrizzleError::TransformFailed);
        }
        let sign = denominator.is_sign_positive();
        if expected_sign.is_some_and(|expected| expected != sign) {
            return Err(DrizzleError::TransformFailed);
        }
        expected_sign = Some(sign);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use aether_registration::ProjectiveTransform;

    use super::*;
    use crate::{deposit_detector_footprint, project_detector_footprint};

    type TestResult = Result<(), Box<dyn Error>>;

    #[test]
    fn identity_scale_two_returns_a_conservative_detector_window() -> TestResult {
        let tile = DrizzleTileBounds::new(4, 2, 2, 4, 3)?;
        let window = plan_drizzle_source_window(
            10,
            10,
            ProjectiveTransform::IDENTITY,
            DrizzleParameters::new(2, 1.0)?,
            tile,
            DrizzleOutputBounds::new(20, 20, 16)?,
        )?
        .ok_or("identity tile unexpectedly missed the detector")?;

        assert_eq!(window.x(), 1);
        assert_eq!(window.y(), 0);
        assert_eq!(window.width(), 3);
        assert_eq!(window.height(), 4);
        assert!(window.contains(2, 2));
        assert!(!window.contains(4, 2));
        Ok(())
    }

    #[test]
    fn translated_disjoint_tile_requires_no_detector_read() -> TestResult {
        let transform =
            ProjectiveTransform::new([[1.0, 0.0, 100.0], [0.0, 1.0, 100.0], [0.0, 0.0, 1.0]])?;
        assert_eq!(
            plan_drizzle_source_window(
                8,
                8,
                transform,
                DrizzleParameters::new(1, 1.0)?,
                DrizzleTileBounds::new(0, 0, 4, 4, 3)?,
                DrizzleOutputBounds::new(16, 16, 16)?,
            )?,
            None
        );
        Ok(())
    }

    #[test]
    fn planned_window_contains_every_brute_force_contributor() -> TestResult {
        let transform = ProjectiveTransform::new([
            [0.98, -0.08, 2.2],
            [0.06, 1.01, 1.4],
            [0.001, -0.0007, 1.0],
        ])?;
        let parameters = DrizzleParameters::new(2, 0.73)?;
        let tile = DrizzleTileBounds::new(5, 4, 8, 7, 3)?;
        let output = DrizzleOutputBounds::new(24, 24, 64)?;
        let window = plan_drizzle_source_window(10, 9, transform, parameters, tile, output)?
            .ok_or("projective oracle tile unexpectedly missed the detector")?;
        let mut contributors = 0_u32;
        for y in 0..9 {
            for x in 0..10 {
                let deposition = deposit_detector_footprint(
                    project_detector_footprint(x, y, transform, parameters)?,
                    1.0,
                    1.0,
                    output.width(),
                    output.height(),
                    output.maximum_contributions(),
                )?;
                let contributes = deposition.contributions().iter().any(|contribution| {
                    u64::from(contribution.x()) >= u64::from(tile.origin_x())
                        && u64::from(contribution.x())
                            < u64::from(tile.origin_x()) + tile.dimensions().width() as u64
                        && u64::from(contribution.y()) >= u64::from(tile.origin_y())
                        && u64::from(contribution.y())
                            < u64::from(tile.origin_y()) + tile.dimensions().height() as u64
                });
                if contributes {
                    contributors += 1;
                    assert!(
                        window.contains(x, y),
                        "planned window omitted contributing detector pixel ({x}, {y})"
                    );
                }
            }
        }
        assert!(contributors > 0);
        Ok(())
    }

    #[test]
    fn rejects_inverse_projective_horizon_and_out_of_output_tile() -> TestResult {
        let horizon =
            ProjectiveTransform::new([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.5, 0.0, 1.0]])?;
        assert_eq!(
            plan_drizzle_source_window(
                10,
                10,
                horizon,
                DrizzleParameters::new(1, 1.0)?,
                DrizzleTileBounds::new(1, 0, 3, 3, 3)?,
                DrizzleOutputBounds::new(10, 10, 16)?,
            ),
            Err(DrizzleError::TransformFailed)
        );
        assert_eq!(
            plan_drizzle_source_window(
                10,
                10,
                ProjectiveTransform::IDENTITY,
                DrizzleParameters::new(1, 1.0)?,
                DrizzleTileBounds::new(9, 0, 2, 2, 3)?,
                DrizzleOutputBounds::new(10, 10, 16)?,
            ),
            Err(DrizzleError::InvalidGeometry)
        );
        Ok(())
    }
}
