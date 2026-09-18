use std::error::Error;
use std::fmt::{Display, Formatter};

/// Axis involved in a dimension error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DimensionAxis {
    /// Horizontal axis.
    Width,
    /// Vertical axis.
    Height,
    /// Plane or channel axis.
    Planes,
}

impl Display for DimensionAxis {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Width => "width",
            Self::Height => "height",
            Self::Planes => "planes",
        };
        formatter.write_str(name)
    }
}

/// Errors raised when a fundamental invariant is violated.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CoreError {
    /// An image dimension is zero.
    ZeroDimension {
        /// Axis whose extent is zero.
        axis: DimensionAxis,
    },
    /// The total pixel count cannot be represented on this platform.
    PixelCountOverflow {
        /// Requested width.
        width: usize,
        /// Requested height.
        height: usize,
        /// Requested plane count.
        planes: usize,
    },
    /// The supplied sample count does not match the dimensions.
    PixelCountMismatch {
        /// Required sample count.
        expected: usize,
        /// Received sample count.
        actual: usize,
    },
    /// Memory for a requested sample or mask buffer could not be reserved.
    AllocationFailed {
        /// Number of elements requested by the allocation.
        elements: usize,
    },
    /// A coordinate lies outside the image.
    CoordinateOutOfBounds {
        /// Requested horizontal coordinate.
        x: usize,
        /// Requested vertical coordinate.
        y: usize,
        /// Requested plane.
        plane: usize,
        /// Available width.
        width: usize,
        /// Available height.
        height: usize,
        /// Available plane count.
        planes: usize,
    },
    /// A tile has a zero width or height.
    ZeroTileExtent {
        /// Requested tile width.
        width: usize,
        /// Requested tile height.
        height: usize,
    },
}

impl Display for CoreError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ZeroDimension { axis } => {
                write!(formatter, "image dimension `{axis}` must be non-zero")
            }
            Self::PixelCountOverflow {
                width,
                height,
                planes,
            } => write!(
                formatter,
                "pixel count overflows usize for dimensions {width}x{height}x{planes}"
            ),
            Self::PixelCountMismatch { expected, actual } => write!(
                formatter,
                "pixel count mismatch: expected {expected}, received {actual}"
            ),
            Self::AllocationFailed { elements } => write!(
                formatter,
                "cannot reserve memory for {elements} image elements"
            ),
            Self::CoordinateOutOfBounds {
                x,
                y,
                plane,
                width,
                height,
                planes,
            } => write!(
                formatter,
                "coordinate ({x}, {y}, {plane}) is outside image {width}x{height}x{planes}"
            ),
            Self::ZeroTileExtent { width, height } => write!(
                formatter,
                "tile dimensions must be non-zero, received {width}x{height}"
            ),
        }
    }
}

impl Error for CoreError {}
