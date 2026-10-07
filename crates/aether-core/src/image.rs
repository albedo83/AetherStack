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
    /// exactly match the declared dimensions, or [`CoreError::AllocationFailed`]
    /// when the matching quality mask cannot be reserved.
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
            mask: PixelMask::clear(dimensions)?,
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

    /// Returns mutable views of samples and mask together.
    ///
    /// The fixed-length views preserve the image invariant while allowing a
    /// processing kernel to update values and flags in one deterministic pass.
    #[must_use]
    pub fn pixels_and_mask_mut(&mut self) -> (&mut [T], &mut PixelMask) {
        (&mut self.pixels, &mut self.mask)
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
    /// Copies a row window from every plane while preserving sample flags.
    pub fn crop_rows(&self, start: usize, height: usize) -> Result<Self, CoreError> {
        let end = start
            .checked_add(height)
            .ok_or(CoreError::InvalidRowRange {
                start,
                height,
                total_height: self.dimensions.height(),
            })?;
        if height == 0 || end > self.dimensions.height() {
            return Err(CoreError::InvalidRowRange {
                start,
                height,
                total_height: self.dimensions.height(),
            });
        }
        let dimensions =
            Dimensions::new(self.dimensions.width(), height, self.dimensions.planes())?;
        let row_width = self.dimensions.width();
        let source_plane_area = row_width * self.dimensions.height();
        let output_plane_area = row_width * height;
        let mut output = Self::filled(dimensions, self.pixels[0].clone())?;
        for plane in 0..self.dimensions.planes() {
            let source_begin = plane * source_plane_area + start * row_width;
            let source_end = source_begin + output_plane_area;
            let output_begin = plane * output_plane_area;
            let output_end = output_begin + output_plane_area;
            output.pixels[output_begin..output_end]
                .clone_from_slice(&self.pixels[source_begin..source_end]);
            output.mask.as_mut_slice()[output_begin..output_end]
                .copy_from_slice(&self.mask.as_slice()[source_begin..source_end]);
        }
        Ok(output)
    }

    /// Creates an image whose samples share the same initial value.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::AllocationFailed`] when the pixel or mask storage
    /// cannot be reserved.
    pub fn filled(dimensions: Dimensions, value: T) -> Result<Self, CoreError> {
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(dimensions.pixel_count())
            .map_err(|_| CoreError::AllocationFailed {
                elements: dimensions.pixel_count(),
            })?;
        pixels.resize(dimensions.pixel_count(), value);
        Self::from_pixels(dimensions, pixels)
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
        let result = Image::filled(dimensions, 7.0_f64);
        let Some(mut image) = result.ok() else {
            return;
        };

        assert_eq!(image.mark(0, 0, 0, PixelFlags::SATURATED), Ok(()));
        assert_eq!(image.get(0, 0, 0), Ok(&7.0));
        assert_eq!(image.mask().get(0, 0, 0), Ok(PixelFlags::SATURATED));
    }

    #[test]
    fn exposes_pixels_and_mask_for_one_pass_kernels() {
        let Some(dimensions) = test_dimensions() else {
            return;
        };
        let result = Image::filled(dimensions, 0_u16);
        let Some(mut image) = result.ok() else {
            return;
        };

        let (pixels, mask) = image.pixels_and_mask_mut();
        assert_eq!(pixels.len(), mask.as_slice().len());
        pixels[1] = 42;
        mask.as_mut_slice()[1] = PixelFlags::HOT;

        assert_eq!(image.get(1, 0, 0), Ok(&42));
        assert_eq!(image.mask().get(1, 0, 0), Ok(PixelFlags::HOT));
    }

    #[test]
    fn reports_pixel_allocation_failure() {
        let result = Dimensions::new(usize::MAX, 1, 1);
        let Some(dimensions) = result.ok() else {
            return;
        };

        assert!(matches!(
            Image::filled(dimensions, 0_u8),
            Err(CoreError::AllocationFailed {
                elements: usize::MAX
            })
        ));
    }

    #[test]
    fn crops_rows_from_all_planes_with_masks() -> Result<(), CoreError> {
        let dimensions = Dimensions::new(2, 3, 2)?;
        let mut image = Image::from_pixels(dimensions, (0_u16..12).collect())?;
        image.mask_mut().as_mut_slice()[2] = PixelFlags::HOT;
        image.mask_mut().as_mut_slice()[8] = PixelFlags::SATURATED;

        let cropped = image.crop_rows(1, 1)?;
        assert_eq!(cropped.dimensions(), Dimensions::new(2, 1, 2)?);
        assert_eq!(cropped.pixels(), &[2, 3, 8, 9]);
        assert_eq!(cropped.mask().as_slice()[0], PixelFlags::HOT);
        assert_eq!(cropped.mask().as_slice()[2], PixelFlags::SATURATED);
        Ok(())
    }

    #[test]
    fn rejects_invalid_image_row_crops() -> Result<(), CoreError> {
        let image = Image::filled(Dimensions::new(2, 3, 1)?, 0_u8)?;
        assert!(matches!(
            image.crop_rows(0, 0),
            Err(CoreError::InvalidRowRange { .. })
        ));
        assert!(matches!(
            image.crop_rows(3, 1),
            Err(CoreError::InvalidRowRange { .. })
        ));
        Ok(())
    }
}
