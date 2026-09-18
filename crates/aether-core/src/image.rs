use crate::{CoreError, Dimensions, PixelFlags, PixelMask};

/// Multi-plane image and its associated quality mask.
///
/// The buffer follows the planar order described by [`Dimensions`]. This type
/// performs no implicit conversion: a value always retains the caller-selected
/// sample type.
#[derive(Clone, Debug, PartialEq)]
pub struct Image<T> {
    dimensions: Dimensions,
    pixels: Vec<T>,
    mask: PixelMask,
}

impl<T> Image<T> {
    /// Builds an image from a planar buffer.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::PixelCountMismatch`] when the buffer length does not
    /// exactly match the declared dimensions.
    pub fn from_pixels(dimensions: Dimensions, pixels: Vec<T>) -> Result<Self, CoreError> {
        if pixels.len() != dimensions.pixel_count() {
            return Err(CoreError::PixelCountMismatch {
                expected: dimensions.pixel_count(),
                actual: pixels.len(),
            });
        }

        Ok(Self {
            dimensions,
            pixels,
            mask: PixelMask::clear(dimensions),
        })
    }

    /// Image dimensions.
    #[must_use]
    pub const fn dimensions(&self) -> Dimensions {
        self.dimensions
    }

    /// Complete read-only planar buffer.
    #[must_use]
    pub fn pixels(&self) -> &[T] {
        &self.pixels
    }

    /// Complete mutable planar buffer.
    ///
    /// Its length cannot be changed, preserving the invariant between the
    /// dimensions, pixels, and mask.
    #[must_use]
    pub fn pixels_mut(&mut self) -> &mut [T] {
        &mut self.pixels
    }

    /// Complete read-only quality mask.
    #[must_use]
    pub const fn mask(&self) -> &PixelMask {
        &self.mask
    }

    /// Complete mutable quality mask.
    #[must_use]
    pub const fn mask_mut(&mut self) -> &mut PixelMask {
        &mut self.mask
    }

    /// Reads a pixel by coordinate.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::CoordinateOutOfBounds`] when the coordinate is
    /// outside the image.
    pub fn get(&self, x: usize, y: usize, plane: usize) -> Result<&T, CoreError> {
        let index = self.dimensions.linear_index(x, y, plane)?;
        self.pixels
            .get(index)
            .ok_or_else(|| self.dimensions.coordinate_error(x, y, plane))
    }

    /// Returns a mutable pixel by coordinate.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::CoordinateOutOfBounds`] when the coordinate is
    /// outside the image.
    pub fn get_mut(&mut self, x: usize, y: usize, plane: usize) -> Result<&mut T, CoreError> {
        let index = self.dimensions.linear_index(x, y, plane)?;
        let coordinate_error = || self.dimensions.coordinate_error(x, y, plane);
        self.pixels.get_mut(index).ok_or_else(coordinate_error)
    }

    /// Marks a pixel without changing its value.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::CoordinateOutOfBounds`] when the coordinate is
    /// outside the image.
    pub fn mark(
        &mut self,
        x: usize,
        y: usize,
        plane: usize,
        flags: PixelFlags,
    ) -> Result<(), CoreError> {
        self.mask.insert(x, y, plane, flags)
    }
}

impl<T: Clone> Image<T> {
    /// Creates an image whose samples share the same initial value.
    #[must_use]
    pub fn filled(dimensions: Dimensions, value: T) -> Self {
        Self {
            dimensions,
            pixels: vec![value; dimensions.pixel_count()],
            mask: PixelMask::clear(dimensions),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dimensions() -> Option<Dimensions> {
        Dimensions::new(2, 2, 1).ok()
    }

    #[test]
    fn rejects_incorrect_buffer_length() {
        let Some(dimensions) = test_dimensions() else {
            return;
        };
        assert_eq!(
            Image::from_pixels(dimensions, vec![1_u16, 2, 3]),
            Err(CoreError::PixelCountMismatch {
                expected: 4,
                actual: 3
            })
        );
    }

    #[test]
    fn coordinates_access_expected_pixel() {
        let Some(dimensions) = test_dimensions() else {
            return;
        };
        let result = Image::from_pixels(dimensions, vec![10_u16, 11, 12, 13]);
        assert!(result.is_ok());
        let Some(mut image) = result.ok() else {
            return;
        };

        assert_eq!(image.get(1, 1, 0), Ok(&13));
        assert_eq!(image.get_mut(0, 1, 0).map(|pixel| *pixel = 42), Ok(()));
        assert_eq!(image.get(0, 1, 0), Ok(&42));
    }

    #[test]
    fn marking_does_not_change_pixel_value() {
        let Some(dimensions) = test_dimensions() else {
            return;
        };
        let mut image = Image::filled(dimensions, 7.0_f64);

        assert_eq!(image.mark(0, 0, 0, PixelFlags::SATURATED), Ok(()));
        assert_eq!(image.get(0, 0, 0), Ok(&7.0));
        assert_eq!(image.mask().get(0, 0, 0), Ok(PixelFlags::SATURATED));
    }
}
