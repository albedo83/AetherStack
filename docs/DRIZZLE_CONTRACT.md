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

## Complete CFA frame execution

A frame executor validates its single-plane detector shape, supported CFA
phase, positive finite frame weight, and RGB tile containment before inspecting
the first sample. It then visits original photosites exactly once in detector
row-major order. Per-frame evidence partitions every source sample into existing
mask, unflagged non-finite, or geometrically deposited categories; deposited
samples whose footprint misses the global output remain distinct. The executor
also counts geometric contributions before tile ownership filtering, while the
tile retains its independent owned/outside accounting. This separation makes
cropping, defective input, and missing output support distinguishable.

## Bounded detector reads

Before reading pixels for a tile, the executor maps the tile's continuous output
rectangle back through the output scale and inverse source-to-reference
homography. The inverse projective denominator must be finite, nonzero, and keep
one sign at all four tile corners; linearity then proves that no horizon crosses
the rectangle. The source quadrilateral bounds are expanded by half the physical
drop width and clipped to the detector. A disjoint tile requires no detector
read. The integer window is deliberately conservative at exact boundaries, and
a brute-force projective oracle requires it to contain every photosite whose
forward-projected drop contributes nonzero area to the tile.

Regional accumulation retains the planned window origin explicitly. Local
sample `(0, 0)` is therefore evaluated at the window's global detector
coordinate for both CFA phase and projective mapping. The regional image must
match the planned extent exactly. A bit-identity oracle compares science,
weight, support, and flags from a bounded regional read against accumulation of
the complete detector frame into the same tile.

The runtime FITS adapter derives the detector extent from the validated primary
array, plans against the accumulator's exact tile, and materializes only that
rectangle. Two-dimensional arrays and three-dimensional arrays with exactly one
plane are accepted. A disjoint source returns before pixel decoding, while all
other read and accumulation failures retain their typed cause.

Multiple opened sources are reduced into a tile strictly in their declared
order. Every source retains its own projective transform, CFA origin, and
positive weight. Aggregate evidence distinguishes examined from intersecting
frames and checked-sums every per-frame sample category. A failure names the
stable source index; because earlier sources may already have contributed, the
caller discards the private accumulator rather than publishing partial work.

## FITS product publication

A complete origin-aligned result publishes as one create-new transaction with
three checksummed binary64 primary images: normalized science, accumulated
weight, and detector contribution count. Each product has a distinct algorithm
identity while sharing the same manifest, plan, parameter, group, and source
bindings. Every private stream is read back and its dimensions and checksums are
verified before any destination becomes visible.

Missing science samples use the canonical FITS NaN. Their weight and support
companions remain valid numeric zeros so downstream diagnostics can distinguish
no support without interpreting a missing numeric payload. Support counts are
accepted only through `2^53`, the complete consecutive-integer domain of
binary64. All private files are synchronized before the first public link; a
collision never overwrites and rolls back earlier links from the same product
set. Cleanup and durability failures after visibility remain explicitly typed.
