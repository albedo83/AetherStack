//! Fundamental types and invariants for the AetherStack engine.
//!
//! This crate knows no file format or compute backend. It defines the shared,
//! unambiguous in-memory representation used by readers and algorithms.

mod dimensions;
mod error;
mod image;
mod mask;
mod numerics;
mod tile;

pub use dimensions::Dimensions;
pub use error::{CoreError, DimensionAxis};
pub use image::Image;
pub use mask::{PixelFlags, PixelMask};
pub use numerics::{CompensatedSum, compensated_sum};
pub use tile::{Halo, Rect, Tile, TileGrid, TileIter};

/// Reference scientific image used by the strict computation profile.
pub type ScientificImage = Image<f64>;
