# Local normalization contract

Local normalization adapts a registered source image to an explicitly selected
reference without allowing display transforms, rejected pixels, or protected
astronomical structures to bias the scientific model. The implementation is
split into independently testable sampling, local fitting, spatial
regularization, application, and publication stages.

## Local affine model

The first strict oracle fits

`reference = scale × source + offset`

inside one spatial cell. Callers provide only clear, finite source/reference
pairs from identical registered coordinates. Mask construction and spatial
sampling stay outside the estimator and must retain their own support evidence.

`local-theil-sen-affine-f64-v1` uses a bounded Theil-Sen slope followed by a
median intercept. Both axes are normalized before fallback slope construction
so opposite-sign binary64 extremes cannot overflow pair differences. Exact
finite direct differences are preferred to preserve simple affine relations.
Duplicate source values do not create slopes, but remain part of intercept and
residual accounting.

The result records the input count, usable pairwise-slope count, and median
absolute residual. A configured lower bound on `abs(scale)` rejects locally
flat or ill-conditioned solutions before they can amplify noise. Applying a
model rejects non-finite input and output instead of clipping either value.

## Masked spatial sampling

`local-cell-priority-sampling-f64-v1` partitions a selected registered plane
into a row-major grid with edge-clipped cells. Source, reference, and optional
protected-region masks must have identical dimensions. Every pixel is assigned
to exactly one category using a fixed precedence: protected region, source
quality mask, reference quality mask, non-finite pair, or eligible pair. This
accounting makes sparse or contaminated cells visible instead of silently
changing their estimator support.

Each cell retains at most the configured number of eligible pairs. Selection
uses a stable coordinate-derived priority and the retained coordinates are
returned in canonical row-major order. Repeated runs and different scan timing
therefore produce identical samples without allocating storage proportional to
the full cell. The cell count and retained samples per cell both have explicit
pre-allocation limits.

Grid fitting preserves this canonical cell order and emits one outcome per
cell. A well-supported cell retains its affine coefficients and residual;
sparse, degenerate, or unsafe cells retain the exact typed fit failure together
with their sampling evidence. One rejected cell therefore cannot erase valid
neighboring evidence or turn a partially supported surface into an apparently
complete one.

## Guarded coefficient surface

`local-coefficient-idw2-f64-v1` constructs immutable control points only from
successful cell fits. Scale and offset are interpolated independently from a
bounded number of nearest controls using inverse squared distance. Distances
are normalized by the nearest neighbor and both weighted sums use compensated
binary64 accumulation, avoiding unnecessary overflow and loss of small terms.

An exact control-point coordinate returns that measured model without dilution.
Every other coordinate requires a configured minimum number of controls inside
an inclusive maximum radius. Coordinates outside the sampled domain, holes with
insufficient local support, non-finite inputs, allocation failures, and
non-finite results are distinct failures. Each evaluation reports its support
count, furthest contributing distance, and whether it was an exact control.

## Full-image application

`local-surface-apply-f64-v1` requires exactly one coefficient surface per image
plane and reuses one bounded nearest-neighbor scratch buffer for the complete
image. It performs no per-pixel allocation. Existing quality flags take
precedence and are copied unchanged. Clear non-finite inputs gain `INVALID`;
coordinates without surface support retain their source value and gain
`MISSING`; finite affine overflow retains its source value and gains `INVALID`.
Only successfully transformed values remain clear.

The application report accounts for every planar sample exactly once as
transformed, inherited-masked, non-finite input, unsupported surface, or
non-finite result. This makes partial model coverage visible to the runtime and
prevents unchanged source values from being consumed as normalized science.

## Protected astronomical structures

`stellar-fwhm-protection-mask-v1` converts the existing deterministic stellar
quality catalog into conservative circular exclusion regions. The major-axis
FWHM controls the radius, with explicit growth, minimum and maximum radius, and
an additional multiplier for saturated sources whose wings are less reliably
described by measured moments. Source coordinates and widths are never inferred
or repaired by the rasterizer.

Catalog size and total bounding-box pixel visits have hard limits. The complete
work estimate is checked before mask allocation or rasterization. The report
retains source count, edge-clipped footprints, candidate visits, unique
protected pixels, and overlap hits. Only the selected plane receives the
`REJECTED` protection flag; other planes remain clear. The resulting mask plugs
directly into the sampling precedence defined above.

## Canonical plan identity

`aether-local-normalization-parameters-v1` hashes all five versioned algorithm
identities and every protection, sampling, fitting, and surface control using
length-prefixed strings, big-endian `u64` integers, and exact big-endian
binary64 bit patterns. Known-answer tests lock the canonical SHA-256 encoding.

`aether-local-normalization-plan-v1` then binds that parameter digest to the
exact lowercase SHA-256 identities of the source and reference FITS files. Paths
and platform-sized binary layouts never enter either digest. Noncanonical source
identities and controls outside the canonical `u64` domain fail before a plan is
created.

The runtime request constructor independently re-encodes the supplied controls
and cross-checks both file fingerprints against the plan. Output provenance must
name the local-application algorithm, represent exactly two inputs, and carry
the same plan and parameter digests. Any mismatch fails before FITS input or
destination I/O begins.

## Resource and failure boundaries

The caller sets minimum and maximum sample counts and a hard maximum number of
pairwise slopes. Work that exceeds any bound fails before allocating scratch
storage. Empty variation, allocation failure, non-finite coefficients, and an
unsafe near-zero scale are distinct typed failures.

The current contract covers stellar protection, sampling, per-cell fitting,
guarded coefficient interpolation, and full-image application. Additional
non-stellar protected regions, optional surface regularization, provenance, and
bounded FITS publication remain subsequent milestones. Runtime provenance must
carry the exact parameter and plan digests defined here.
