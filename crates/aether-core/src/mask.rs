use std::ops::{BitOr, BitOrAssign};

use crate::{CoreError, Dimensions};

/// Quality flags attached to a pixel.
///
/// Multiple flags may be combined. [`Self::CLEAR`] represents a usable pixel
/// that has not been marked.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(transparent)]
pub struct PixelFlags(u8);

impl PixelFlags {
    /// No known defect.
    pub const CLEAR: Self = Self(0);
    /// Missing sample or a sample that cannot be computed.
    pub const MISSING: Self = Self(1 << 0);
    /// Saturated sensor or digital conversion.
    pub const SATURATED: Self = Self(1 << 1);
    /// Detected hot pixel.
    pub const HOT: Self = Self(1 << 2);
    /// Detected cold pixel.
    pub const COLD: Self = Self(1 << 3);
    /// Value rejected by a statistical operation.
    pub const REJECTED: Self = Self(1 << 4);
    /// Non-finite or otherwise invalid value.
    pub const INVALID: Self = Self(1 << 5);

    /// Builds flags from their binary representation.
    ///
    /// Unknown bits are retained to allow the format to evolve without losing
    /// information during a round trip.
    #[must_use]
    pub const fn from_bits_retain(bits: u8) -> Self {
        Self(bits)
    }

    /// Returns the stable binary representation of these flags.
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Returns whether no defect is marked.
    #[must_use]
    pub const fn is_clear(self) -> bool {
        self.0 == 0
    }

    /// Returns whether all requested flags are present.
    #[must_use]
    pub const fn contains(self, flags: Self) -> bool {
        (self.0 & flags.0) == flags.0
    }
}

impl BitOr for PixelFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for PixelFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// Quality mask with exactly the same dimensions as an image.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PixelMask {
    dimensions: Dimensions,
    flags: Vec<PixelFlags>,
}

impl PixelMask {
    /// Creates a mask whose pixels are initially valid.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::AllocationFailed`] when the backing storage cannot
    /// be reserved.
    pub fn clear(dimensions: Dimensions) -> Result<Self, CoreError> {
        let mut flags = Vec::new();
        flags
            .try_reserve_exact(dimensions.pixel_count())
            .map_err(|_| CoreError::AllocationFailed {
                elements: dimensions.pixel_count(),
            })?;
        flags.resize(dimensions.pixel_count(), PixelFlags::CLEAR);
        Ok(Self { dimensions, flags })
    }

    /// Mask dimensions.
    #[must_use]
    pub const fn dimensions(&self) -> Dimensions {
        self.dimensions
    }

    /// Reads the flags of one pixel.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::CoordinateOutOfBounds`] when the coordinate is
    /// outside the mask.
    pub fn get(&self, x: usize, y: usize, plane: usize) -> Result<PixelFlags, CoreError> {
        let index = self.dimensions.linear_index(x, y, plane)?;
        self.flags
            .get(index)
            .copied()
            .ok_or_else(|| self.dimensions.coordinate_error(x, y, plane))
    }

    /// Adds flags without clearing existing ones.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::CoordinateOutOfBounds`] when the coordinate is
    /// outside the mask.
    pub fn insert(
        &mut self,
        x: usize,
        y: usize,
        plane: usize,
        flags: PixelFlags,
    ) -> Result<(), CoreError> {
        let index = self.dimensions.linear_index(x, y, plane)?;
        let coordinate_error = || self.dimensions.coordinate_error(x, y, plane);
        let target = self.flags.get_mut(index).ok_or_else(coordinate_error)?;
        *target |= flags;
        Ok(())
    }

    /// Replaces all flags for one pixel.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::CoordinateOutOfBounds`] when the coordinate is
    /// outside the mask.
    pub fn set(
        &mut self,
        x: usize,
        y: usize,
        plane: usize,
        flags: PixelFlags,
    ) -> Result<(), CoreError> {
        let index = self.dimensions.linear_index(x, y, plane)?;
        let coordinate_error = || self.dimensions.coordinate_error(x, y, plane);
        let target = self.flags.get_mut(index).ok_or_else(coordinate_error)?;
        *target = flags;
        Ok(())
    }

    /// Linear read-only view of all flags.
    #[must_use]
    pub fn as_slice(&self) -> &[PixelFlags] {
        &self.flags
    }

    /// Linear mutable view of all flags.
    ///
    /// The slice cannot be resized, preserving the one-to-one correspondence
    /// between image samples and quality flags.
    #[must_use]
    pub fn as_mut_slice(&mut self) -> &mut [PixelFlags] {
        &mut self.flags
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dimensions() -> Option<Dimensions> {
        Dimensions::new(2, 2, 1).ok()
    }

    #[test]
    fn new_mask_is_clear() {
        let Some(dimensions) = test_dimensions() else {
            return;
        };
        let result = PixelMask::clear(dimensions);
        assert!(result.is_ok());
        let Some(mask) = result.ok() else {
            return;
        };

        assert!(mask.as_slice().iter().all(|flags| flags.is_clear()));
    }

    #[test]
    fn combines_independent_quality_flags() {
        let Some(dimensions) = test_dimensions() else {
            return;
        };
        let result = PixelMask::clear(dimensions);
        let Some(mut mask) = result.ok() else {
            return;
        };

        assert_eq!(mask.insert(1, 0, 0, PixelFlags::HOT), Ok(()));
        assert_eq!(mask.insert(1, 0, 0, PixelFlags::SATURATED), Ok(()));

        let flags = mask.get(1, 0, 0);
        assert!(flags.is_ok());
        let flags = flags.unwrap_or_default();
        assert!(flags.contains(PixelFlags::HOT));
        assert!(flags.contains(PixelFlags::SATURATED));
        assert!(!flags.contains(PixelFlags::REJECTED));
    }

    #[test]
    fn retains_unknown_bits() {
        let flags = PixelFlags::from_bits_retain(0b1100_0000);
        assert_eq!(flags.bits(), 0b1100_0000);
    }

    #[test]
    fn rejects_out_of_bounds_update() {
        let Some(dimensions) = test_dimensions() else {
            return;
        };
        let result = PixelMask::clear(dimensions);
        let Some(mut mask) = result.ok() else {
            return;
        };

        assert!(matches!(
            mask.insert(2, 0, 0, PixelFlags::HOT),
            Err(CoreError::CoordinateOutOfBounds { .. })
        ));
    }

    #[test]
    fn exposes_a_fixed_length_mutable_view() {
        let Some(dimensions) = test_dimensions() else {
            return;
        };
        let result = PixelMask::clear(dimensions);
        let Some(mut mask) = result.ok() else {
            return;
        };

        let flags = mask.as_mut_slice();
        assert_eq!(flags.len(), 4);
        flags[2] = PixelFlags::INVALID;

        assert_eq!(mask.get(0, 1, 0), Ok(PixelFlags::INVALID));
    }

    #[test]
    fn reports_mask_allocation_failure() {
        let result = Dimensions::new(usize::MAX, 1, 1);
        let Some(dimensions) = result.ok() else {
            return;
        };

        assert!(matches!(
            PixelMask::clear(dimensions),
            Err(CoreError::AllocationFailed {
                elements: usize::MAX
            })
        ));
    }
}
