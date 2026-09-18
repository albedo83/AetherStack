use crate::{CoreError, DimensionAxis};

/// Dimensions of a multi-plane image.
///
/// The canonical memory order is planar. Within each plane, pixels are stored
/// row by row, from left to right and then from top to bottom.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Dimensions {
    width: usize,
    height: usize,
    planes: usize,
    pixel_count: usize,
}

impl Dimensions {
    /// Builds valid dimensions and checks the product before any allocation.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::ZeroDimension`] when an axis is zero and
    /// [`CoreError::PixelCountOverflow`] when the total sample count does not
    /// fit in a `usize`.
    pub fn new(width: usize, height: usize, planes: usize) -> Result<Self, CoreError> {
        if width == 0 {
            return Err(CoreError::ZeroDimension {
                axis: DimensionAxis::Width,
            });
        }
        if height == 0 {
            return Err(CoreError::ZeroDimension {
                axis: DimensionAxis::Height,
            });
        }
        if planes == 0 {
            return Err(CoreError::ZeroDimension {
                axis: DimensionAxis::Planes,
            });
        }

        let pixel_count = width
            .checked_mul(height)
            .and_then(|area| area.checked_mul(planes))
            .ok_or(CoreError::PixelCountOverflow {
                width,
                height,
                planes,
            })?;

        Ok(Self {
            width,
            height,
            planes,
            pixel_count,
        })
    }

    /// Width in pixels.
    #[must_use]
    pub const fn width(self) -> usize {
        self.width
    }

    /// Height in pixels.
    #[must_use]
    pub const fn height(self) -> usize {
        self.height
    }

    /// Number of planes or channels.
    #[must_use]
    pub const fn planes(self) -> usize {
        self.planes
    }

    /// Total sample count across all planes.
    #[must_use]
    pub const fn pixel_count(self) -> usize {
        self.pixel_count
    }

    /// Returns whether a coordinate belongs to the image.
    #[must_use]
    pub const fn contains(self, x: usize, y: usize, plane: usize) -> bool {
        x < self.width && y < self.height && plane < self.planes
    }

    /// Converts a planar coordinate into its canonical memory index.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::CoordinateOutOfBounds`] when the coordinate is
    /// outside the image. Arithmetic remains checked even though the constructor
    /// already guarantees that the total pixel count is valid.
    pub fn linear_index(self, x: usize, y: usize, plane: usize) -> Result<usize, CoreError> {
        if !self.contains(x, y, plane) {
            return Err(self.coordinate_error(x, y, plane));
        }

        let overflow = || CoreError::PixelCountOverflow {
            width: self.width,
            height: self.height,
            planes: self.planes,
        };
        let plane_stride = self.width.checked_mul(self.height).ok_or_else(overflow)?;
        let plane_offset = plane.checked_mul(plane_stride).ok_or_else(overflow)?;
        let row_offset = y.checked_mul(self.width).ok_or_else(overflow)?;

        plane_offset
            .checked_add(row_offset)
            .and_then(|offset| offset.checked_add(x))
            .ok_or_else(overflow)
    }

    pub(crate) const fn coordinate_error(self, x: usize, y: usize, plane: usize) -> CoreError {
        CoreError::CoordinateOutOfBounds {
            x,
            y,
            plane,
            width: self.width,
            height: self.height,
            planes: self.planes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_each_zero_dimension() {
        assert_eq!(
            Dimensions::new(0, 2, 1),
            Err(CoreError::ZeroDimension {
                axis: DimensionAxis::Width
            })
        );
        assert_eq!(
            Dimensions::new(2, 0, 1),
            Err(CoreError::ZeroDimension {
                axis: DimensionAxis::Height
            })
        );
        assert_eq!(
            Dimensions::new(2, 1, 0),
            Err(CoreError::ZeroDimension {
                axis: DimensionAxis::Planes
            })
        );
    }

    #[test]
    fn rejects_pixel_count_overflow() {
        assert_eq!(
            Dimensions::new(usize::MAX, 2, 1),
            Err(CoreError::PixelCountOverflow {
                width: usize::MAX,
                height: 2,
                planes: 1
            })
        );
    }

    #[test]
    fn uses_planar_row_major_indices() {
        let result = Dimensions::new(3, 2, 2);
        assert!(result.is_ok());
        let Some(dimensions) = result.ok() else {
            return;
        };

        assert_eq!(dimensions.linear_index(0, 0, 0), Ok(0));
        assert_eq!(dimensions.linear_index(2, 1, 0), Ok(5));
        assert_eq!(dimensions.linear_index(0, 0, 1), Ok(6));
        assert_eq!(dimensions.linear_index(2, 1, 1), Ok(11));
    }

    #[test]
    fn reports_out_of_bounds_coordinate_with_context() {
        let result = Dimensions::new(3, 2, 1);
        assert!(result.is_ok());
        let Some(dimensions) = result.ok() else {
            return;
        };

        assert_eq!(
            dimensions.linear_index(3, 1, 0),
            Err(CoreError::CoordinateOutOfBounds {
                x: 3,
                y: 1,
                plane: 0,
                width: 3,
                height: 2,
                planes: 1
            })
        );
    }
}
