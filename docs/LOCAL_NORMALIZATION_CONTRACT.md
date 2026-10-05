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

`aether-local-normalization-parameters-v2` hashes all seven versioned algorithm
identities and every robust-background, stellar-measurement, protection,
sampling, fitting, and surface control using
length-prefixed strings, big-endian `u64` integers, and exact big-endian
binary64 bit patterns. Known-answer tests lock the canonical SHA-256 encoding.

`aether-local-normalization-plan-v2` then binds that parameter digest to the
exact lowercase SHA-256 identities of the source and reference FITS files. Paths
and platform-sized binary layouts never enter either digest. Noncanonical source
identities and controls outside the canonical `u64` domain fail before a plan is
created.

The runtime request constructor independently re-encodes the supplied controls
and cross-checks both file fingerprints against the plan. Output provenance must
name the local-application algorithm, represent exactly two inputs, and carry
the same plan and parameter digests. Any mismatch fails before FITS input or
destination I/O begins.

The runtime executor reserves a conservative peak before decoding either pixel
array. It reads source and reference one plane at a time, measures and protects
stellar structure independently, fits and applies one guarded surface, and
streams that normalized plane into a private binary64 FITS product. Checked
evidence accumulation preserves complete-image accounting without retaining
complete planar RGB arrays. Structural and checksum readback plus fresh source
and reference fingerprints must all pass before create-new atomic publication.
Cancellation, memory failure, dimension mismatch, scientific failure, or
readback failure leaves no public output.

The same overflow-checked peak estimator is a public runtime preflight contract.
Desktop preflight reads only the source and reference headers, requires identical
dimensions, validates the complete parameter set, and reports the required bytes
plus configured headroom. Execution calls that exact estimator again, so the
preview cannot drift from the reservation enforced by the memory budget.

Successful execution retains every accepted cell model in deterministic plane
and grid order. Diagnostic evidence contains the cell-center coordinate, robust
scale and offset, and median absolute residual. This evidence is derived from
the exact surface controls used for publication; consumers must not reconstruct
or refit controls from display pixels.

The complete plane-major cell grid is retained alongside accepted controls.
Each diagnostic binds its edge-clipped bounds to mutually exclusive sampling
counts, eligible and retained support, and either the accepted affine model or
the exact typed fit rejection. Adapters expose stable rejection codes; they must
not infer rejection causes from counts or display imagery.

Execution can emit the shared machine-readable progress protocol under the
stable `local-normalization` stage. Work becomes determinate after input headers
agree: one unit covers private-stream initialization, one covers each decoded,
fitted, applied, and written plane, one covers private output validation, and
one covers atomic publication. Sequence numbers
are monotonic; successful, failed, and cancelled terminal events retain canonical
state and failure codes.

## Resource and failure boundaries

The caller sets minimum and maximum sample counts and a hard maximum number of
pairwise slopes. Work that exceeds any bound fails before allocating scratch
storage. Empty variation, allocation failure, non-finite coefficients, and an
unsafe near-zero scale are distinct typed failures.

The current contract covers stellar protection, sampling, per-cell fitting,
guarded coefficient interpolation, inspectable spatial controls, plane-streamed
application, provenance, and bounded atomic FITS publication. Additional
non-stellar protected regions, optional surface regularization, row-banded
single-plane execution, and a dense residual raster remain subsequent milestones.
