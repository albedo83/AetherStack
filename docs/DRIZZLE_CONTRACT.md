# Drizzle footprint contract

This contract fixes the strict CPU geometry before tiled accumulation, CFA
routing, or desktop controls are introduced. Drizzle consumes original detector
pixels and an accepted source-to-reference projective transform; it never uses a
previously resampled registered image as scientific input.

## Coordinates and drops

Detector and reference pixel centers use integer coordinates. A detector pixel
centered at `(x, y)` starts with a square drop whose side is the configured
`drop_shrink` in `(0, 1]`. Its corners are mapped through the complete accepted
homography. A reference coordinate `r` then maps to the finer output grid as:

```text
output = (r + 0.5) * scale - 0.5
```

This convention maps reference pixel edges exactly onto output pixel edges. The
strict profile accepts integer scales from 1 through 8. A projective denominator
must remain finite, nonzero, and of one sign at all four corners; because the
denominator is linear over the source square, this rejects every pole crossing
the drop.

## Flux and weight

The projected quadrilateral is clipped against output pixel squares. For an
overlap area `a` and complete projected area `A`, the deposited fraction is
`a / A`. A source value `v` with frame weight `w` contributes:

```text
weighted_flux = v * w * a / A
weight        =     w * a / A
```

Consequently, a footprint wholly inside the output conserves its complete
weighted flux and weight for every supported scale, shrink, rotation, or
reflection. A boundary-crossing footprint reports the retained fraction instead
of silently renormalizing missing support back into the image.

## Determinism and bounds

Candidate output pixels are visited in row-major order. The integer bounding box
is checked against an explicit contribution ceiling before clipping or
allocation. Convex clipping uses fixed eight-vertex scratch storage, so no
per-output-pixel heap allocation occurs. Non-finite values, invalid weights,
unrepresentable bounds, degenerate footprints, projective poles, and exhausted
work ceilings fail with typed errors.

The initial unit oracles cover exact 2× edge mapping, drop shrink, rotated
footprints, boundary loss, projective poles, work limits, stable ordering, and
weighted-flux conservation.

## CFA routing

CFA Drizzle selects red, green, or blue exclusively from the original detector
coordinate and the declared RGGB, BGGR, GRBG, or GBRG phase. Routing happens
before geometric projection, so a rotation, reflection, translation, or scale
cannot change the physical filter that measured a sample. The two green phases
share the canonical green output plane. Unknown mosaics fail explicitly, and no
missing chromatic sample is interpolated during deposition.

## Sample eligibility

Quality is decided before CFA routing, transformation, clipping, or allocation.
A sample carrying any known or future mask bit deposits neither flux nor weight
and retains every original bit in exclusion evidence. An unflagged NaN or
infinity is excluded separately and gains the `INVALID` bit. Only clear finite
samples reach the footprint projector. This precedence makes hot, cold,
saturated, missing, rejected, invalid, and forward-compatible defects auditable
without letting them consume geometry work.

## Tiled accumulation

Every accumulation tile owns a half-open global rectangle: its left and top
edges are included, while its right and bottom edges are excluded. Adjacent
tiles therefore own each geometric contribution exactly once. The strict
profile accepts either one monochrome plane or three planar CFA planes.

Weighted flux and weight use independent binary64 Neumaier accumulators in the
stable source-deposition order. A per-pixel unsigned support counter records how
many detector footprints contributed. Final science values are `weighted flux /
weight`; a zero-weight output is never replaced with a numeric background. It is
published as NaN with `MISSING`, zero weight, and zero support instead. Tile
evidence accounts separately for depositions, contributions seen, contributions
owned, contributions outside, and unsupported output samples. Tests require
bit-identical science, weight, support, and flag maps when a complete tile is
replaced by adjacent tiles and reassembled.

Atomic FITS publication remains a subsequent milestone.
