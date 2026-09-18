use crate::{CoreError, Dimensions};

/// Half-open spatial rectangle: `[x, right) × [y, bottom)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rect {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

impl Rect {
    const fn new(x: usize, y: usize, width: usize, height: usize) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// First included column.
    #[must_use]
    pub const fn x(self) -> usize {
        self.x
    }

    /// First included row.
    #[must_use]
    pub const fn y(self) -> usize {
        self.y
    }

    /// Rectangle width.
    #[must_use]
    pub const fn width(self) -> usize {
        self.width
    }

    /// Rectangle height.
    #[must_use]
    pub const fn height(self) -> usize {
        self.height
    }

    /// First excluded column.
    #[must_use]
    pub const fn right(self) -> usize {
        self.x + self.width
    }

    /// First excluded row.
    #[must_use]
    pub const fn bottom(self) -> usize {
        self.y + self.height
    }
}

/// Number of context pixels required around a tile core.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Halo {
    left: usize,
    right: usize,
    top: usize,
    bottom: usize,
}

impl Halo {
    /// Builds a potentially asymmetric halo.
    #[must_use]
    pub const fn new(left: usize, right: usize, top: usize, bottom: usize) -> Self {
        Self {
            left,
            right,
            top,
            bottom,
        }
    }

    /// Builds an identical halo on all four sides.
    #[must_use]
    pub const fn uniform(radius: usize) -> Self {
        Self::new(radius, radius, radius, radius)
    }

    /// Requested extension on the left.
    #[must_use]
    pub const fn left(self) -> usize {
        self.left
    }

    /// Requested extension on the right.
    #[must_use]
    pub const fn right(self) -> usize {
        self.right
    }

    /// Requested extension at the top.
    #[must_use]
    pub const fn top(self) -> usize {
        self.top
    }

    /// Requested extension at the bottom.
    #[must_use]
    pub const fn bottom(self) -> usize {
        self.bottom
    }
}

/// Spatial tile for one plane, with a write core and a read region.
///
/// Operators write only to [`Self::core`] and may read from [`Self::read`]. The
/// read region is clipped at image edges; choosing a boundary condition remains
/// the operator's responsibility.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Tile {
    plane: usize,
    core: Rect,
    read: Rect,
}

impl Tile {
    /// Plane containing the tile.
    #[must_use]
    pub const fn plane(self) -> usize {
        self.plane
    }

    /// Region produced by the operator.
    #[must_use]
    pub const fn core(self) -> Rect {
        self.core
    }

    /// Readable region, including a halo clipped at image edges.
    #[must_use]
    pub const fn read(self) -> Rect {
        self.read
    }
}

/// Reusable description of an image tile layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TileGrid {
    dimensions: Dimensions,
    tile_width: usize,
    tile_height: usize,
    halo: Halo,
}

impl TileGrid {
    /// Builds a tile grid traversed in deterministic order.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::ZeroTileExtent`] when a tile width or height is zero.
    pub const fn new(
        dimensions: Dimensions,
        tile_width: usize,
        tile_height: usize,
        halo: Halo,
    ) -> Result<Self, CoreError> {
        if tile_width == 0 || tile_height == 0 {
            return Err(CoreError::ZeroTileExtent {
                width: tile_width,
                height: tile_height,
            });
        }

        Ok(Self {
            dimensions,
            tile_width,
            tile_height,
            halo,
        })
    }

    /// Dimensions of the tiled image.
    #[must_use]
    pub const fn dimensions(self) -> Dimensions {
        self.dimensions
    }

    /// Maximum width of a tile core.
    #[must_use]
    pub const fn tile_width(self) -> usize {
        self.tile_width
    }

    /// Maximum height of a tile core.
    #[must_use]
    pub const fn tile_height(self) -> usize {
        self.tile_height
    }

    /// Requested halo around each core.
    #[must_use]
    pub const fn halo(self) -> Halo {
        self.halo
    }

    /// Iterates by plane, then tile row, and finally from left to right.
    #[must_use]
    pub const fn iter(self) -> TileIter {
        TileIter {
            grid: self,
            x: 0,
            y: 0,
            plane: 0,
        }
    }
}

/// Deterministic, allocation-free iterator over a [`TileGrid`].
#[derive(Clone, Debug)]
pub struct TileIter {
    grid: TileGrid,
    x: usize,
    y: usize,
    plane: usize,
}

impl Iterator for TileIter {
    type Item = Tile;

    fn next(&mut self) -> Option<Self::Item> {
        let dimensions = self.grid.dimensions;
        if self.plane >= dimensions.planes() {
            return None;
        }
        let plane = self.plane;

        // Subtractions are safe because of the iterator's progression
        // invariants. Using the remaining distance avoids `x + tile_width`,
        // which could overflow for dimensions close to `usize::MAX`.
        let core_width = (dimensions.width() - self.x).min(self.grid.tile_width);
        let core_height = (dimensions.height() - self.y).min(self.grid.tile_height);
        let core = Rect::new(self.x, self.y, core_width, core_height);

        let read_x = core.x().saturating_sub(self.grid.halo.left());
        let read_y = core.y().saturating_sub(self.grid.halo.top());
        let read_right = core
            .right()
            .saturating_add(self.grid.halo.right())
            .min(dimensions.width());
        let read_bottom = core
            .bottom()
            .saturating_add(self.grid.halo.bottom())
            .min(dimensions.height());
        let read = Rect::new(read_x, read_y, read_right - read_x, read_bottom - read_y);

        self.x = core.right();
        if self.x == dimensions.width() {
            self.x = 0;
            self.y = core.bottom();
            if self.y == dimensions.height() {
                self.y = 0;
                self.plane += 1;
            }
        }

        Some(Tile { plane, core, read })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dimensions(width: usize, height: usize, planes: usize) -> Option<Dimensions> {
        Dimensions::new(width, height, planes).ok()
    }

    #[test]
    fn rejects_zero_tile_extent() {
        let Some(dimensions) = dimensions(4, 4, 1) else {
            return;
        };
        assert!(matches!(
            TileGrid::new(dimensions, 0, 2, Halo::default()),
            Err(CoreError::ZeroTileExtent { .. })
        ));
    }

    #[test]
    fn covers_every_pixel_once_with_tile_cores() {
        let Some(dimensions) = dimensions(5, 4, 2) else {
            return;
        };
        let grid_result = TileGrid::new(dimensions, 3, 3, Halo::uniform(1));
        assert!(grid_result.is_ok());
        let Some(grid) = grid_result.ok() else {
            return;
        };
        let mut visits = vec![0_u8; dimensions.pixel_count()];
        let tiles: Vec<_> = grid.iter().collect();

        assert_eq!(tiles.len(), 8);
        for tile in tiles {
            let core = tile.core();
            for y in core.y()..core.bottom() {
                for x in core.x()..core.right() {
                    let index_result = dimensions.linear_index(x, y, tile.plane());
                    assert!(index_result.is_ok());
                    let Some(index) = index_result.ok() else {
                        continue;
                    };
                    let Some(visit_count) = visits.get_mut(index) else {
                        continue;
                    };
                    *visit_count += 1;
                }
            }
        }

        assert!(visits.iter().all(|count| *count == 1));
    }

    #[test]
    fn clips_halo_at_image_edges() {
        let Some(dimensions) = dimensions(10, 10, 1) else {
            return;
        };
        let grid_result = TileGrid::new(dimensions, 4, 4, Halo::uniform(2));
        assert!(grid_result.is_ok());
        let Some(grid) = grid_result.ok() else {
            return;
        };
        let first = grid.iter().next();

        assert_eq!(
            first,
            Some(Tile {
                plane: 0,
                core: Rect::new(0, 0, 4, 4),
                read: Rect::new(0, 0, 6, 6)
            })
        );
    }

    #[test]
    fn traverses_planes_in_order() {
        let Some(dimensions) = dimensions(2, 1, 3) else {
            return;
        };
        let grid_result = TileGrid::new(dimensions, 2, 1, Halo::default());
        assert!(grid_result.is_ok());
        let Some(grid) = grid_result.ok() else {
            return;
        };
        let planes: Vec<_> = grid.iter().map(Tile::plane).collect();

        assert_eq!(planes, vec![0, 1, 2]);
    }
}
