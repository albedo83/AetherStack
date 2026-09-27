# Registration geometry and reference contract

Registration operates on immutable linear scientific images. Display stretches,
preview reductions, viewport transforms, and browser coordinates never enter
feature matching, transform estimation, residual measurement, or resampling.

The initial implementation fixes the coordinate and automatic-reference rules.
It deliberately does not claim that star matching or resampling is implemented.
Those stages must extend this contract and pass their own synthetic and
real-corpus release gates.

## Scientific coordinate convention

Image coordinates are continuous `f64` values in source-pixel units. The center
of the first stored pixel is `(0, 0)`; `x` increases to the right and `y`
increases downward. Coordinates may be negative or lie outside an image after a
transform. NaN and infinity are rejected at construction boundaries, and signed
zero is canonicalized to positive zero.

`AffineTransform` always maps a source image into reference-image coordinates:

```text
x_reference = m00 * x_source + m01 * y_source + tx
y_reference = m10 * x_source + m11 * y_source + ty
```

The linear part must be finite and nonsingular. Application, inversion, and
composition reject results outside the finite numerical domain. Composition is
defined in execution order: `source_to_intermediate.then(intermediate_to_ref)`
returns `source_to_ref`. This direction must remain explicit in serialized plans
and user diagnostics.

## Residual evidence

A registration correspondence contains one measured source point and its
reference-frame point. Residuals are Euclidean distances after applying the
source-to-reference transform and are reported in reference pixels. The strict
summary reports support count, arithmetic mean, root mean square, and maximum.
Mean and squared-distance reductions use compensated `f64` summation in input
order. No correspondence is silently clipped or reordered; robust model fitting
must separately report its inlier rule and the evidence it excluded.

An empty correspondence set, non-finite transformed point, or overflowing
distance is an error rather than a fabricated zero statistic.

## Matching feature catalog

`quality-filtered-stars-v1` converts the canonical
`local-max-moments-v1` quality measurements into registration features. It does
not implement a second star detector. This keeps background estimation, local
maxima, sub-pixel centroids, aperture flux, FWHM, eccentricity, saturation, and
mask behavior identical between Review and registration.

The catalog accepts the measured image axes and verifies their pixel count
against the upstream quality evidence. Each measurement is assigned exactly
once, in this exclusion order:

1. saturated aperture;
2. background SNR below the inclusive configured minimum;
3. eccentricity above the inclusive configured maximum;
4. centroid inside the configured border margin;
5. otherwise eligible but beyond the maximum output count.

Eligible features are ordered by descending background SNR, aperture flux, and
peak. Lower eccentricity, vertical coordinate, and horizontal coordinate are
deterministic tie breakers. The output carries a zero-based matching rank,
centroid, photometric evidence, shape evidence, measurement support, exact
selection parameters, upstream and catalog algorithm identities, and all
mutually exclusive exclusion counts. Saturated stars can remain visible in
Review but never enter this initial matching catalog.

One catalog accepts at most one million upstream measurements and emits at most
65,536 features. Sorting storage is reserved fallibly before selection; invalid
axes, an all-excluding border margin, invalid thresholds, allocation failure,
coordinate failure, and evidence-count inconsistency remain explicit errors.
The catalog is intended for calibrated mono or linear RGB luminance products.
Raw-CFA cell coordinates must not be mixed with full-resolution coordinates.

## Local triangle descriptors

`local-triangle-ratios-v1` builds candidate geometry from the ranked feature
catalog without enumerating every global triplet. It visits at most 4,096
leading features as anchors. For each anchor it sorts the other features by
Euclidean distance and combines at most the nearest 32 neighbors in pairs. The
validated worst-case bound is 2.1 million attempted local triangles, and no more
than one million unique descriptors may be emitted.

Every triangle stores the shortest/longest and middle/longest side ratios plus
`abs(cross) / longest_side²`. These three quantities are invariant under
translation, rotation, and uniform scale. The absolute longest side remains as
evidence for later scale estimation. Vertices use a canonical order of apex,
short-side endpoint, and middle-side endpoint, with feature rank resolving an
exact isosceles tie. Orientation is evaluated in the downward-positive image
coordinate system. It is not part of the invariant ratio key: rotation and
scale preserve it, while reflection reverses it and can therefore be diagnosed
or explicitly allowed by the future matcher.

Triangles below the absolute size threshold or normalized-area threshold are
rejected before matching. Repeated discovery of the same three feature ranks
from another anchor is counted and deduplicated. Generation order is fully
deterministic. When the output bound is filled, the engine continues only until
it proves that another unique valid triangle exists, marks the catalog as
stopped at the output limit, and terminates instead of hiding unbounded work.
The output records visited anchors, attempted triangles, size and degeneracy
rejections, duplicates, exact parameters, and source feature count.

## Tolerant descriptor matching

`triangle-grid-hypotheses-v1` compares the three invariant descriptor values
with explicit absolute error budgets. Reference descriptors are sorted into a
three-dimensional integer grid whose cell widths equal those budgets. A source
descriptor searches only its at most 27 adjacent cells and then applies the
exact side-ratio, normalized-area, scale, and reflection-policy checks. This
keeps lookup deterministic without treating quantization as scientific proof.

Every accepted triangle hypothesis retains its source and reference descriptor
indices, all three canonical feature-rank pairs, maximum side-ratio error,
area error, normalized three-dimensional distance, scale estimate, and mirror
state. The matcher deliberately does not select a transform or collapse a
triangle to a unique star mapping. It retains the best configured number of
candidates per source descriptor and the best configured number globally,
ordered by normalized error and stable indices.

Exact comparisons, geometric candidates, source descriptors without a match,
ambiguous source descriptors, both levels of discarded candidates, and retained
reflections are counted. The output also propagates whether either input
descriptor catalog was truncated. Allocation, arithmetic overflow, empty
catalogs, identical frame identities, invalid controls, and an exceeded exact
comparison budget fail explicitly. Current hard bounds are 256 retained
candidates per source descriptor, one million global hypotheses, and 50 million
exact comparisons.

## Similarity consensus

`triangle-similarity-consensus-v1` evaluates leading descriptor hypotheses as
deterministic transform seeds. Each seed fits a least-squares source-to-reference
similarity from its three canonical star pairs. Orientation-preserving models use
one uniform scale and rotation; reflected models use the corresponding
orientation-reversing basis. Hypotheses with different mirror state never vote
for one another.

A triangle supports a model only when all three transformed stars are within the
inclusive residual threshold in reference pixels. Candidate ordering prefers
more supporting triangles, then lower compensated RMS residual, lower descriptor
distance, non-reflected geometry, and stable hypothesis index. The winning model
must have at least two supporting triangles and three distinct feature pairs.
Its distinct pairs must form a one-to-one mapping: contradictory source or
reference assignments fail explicitly instead of being averaged.

The winner is refitted in binary64 least squares over all distinct inlier pairs,
then rescored. Minimum support is enforced again after refinement. The result
contains the affine representation, uniform scale, rotation, mirror state,
winning seed, sorted inlier triangle indices, sorted distinct star pairs, and a
strict compensated residual summary. It also records candidate, evaluated,
truncated, inlier, outlier, and point-evaluation counts plus upstream descriptor
truncation. Current hard bounds are 100,000 seed models and 100 million point
residual evaluations. Exceeding either configured budget is never treated as a
valid partial consensus.

## Automatic reference selection

Each eligible frame supplies a stable content-derived `FrameId` and four
validated metrics:

- median stellar FWHM in source pixels, lower is better;
- median stellar eccentricity in `[0, 1)`, lower is better;
- usable detected-star count, higher is better;
- robust background noise in source units, lower is better.

The `equal-ordinal-ranks-v1` policy assigns an ordinal rank to every metric.
Exact metric ties share a rank; the next distinct value retains its sorted
position, so rank gaps are intentional. The four ranks have equal weight. The
lowest sum wins, followed deterministically by lowest worst individual rank,
FWHM rank, star-count rank, eccentricity rank, noise rank, and finally stable
frame identity.

The result retains every individual rank, total, and worst rank in canonical
frame-identity order. Input order cannot affect either evidence or the winner.
Duplicate identities, empty sets, invalid metrics, excessive candidate counts,
allocation failure, and score overflow are explicit errors. A future UI may pin
a manual reference, but it must record that override and must not relabel it as
the automatic winner.

## Remaining release gates

Production registration still requires:

- deterministic multi-scale enhancement beyond the initial local descriptors;
- translation-only, affine, and projective model selection beyond the strict
  similarity model;
- confidence and sparse-field policies validated on representative real data;
- justified distortion models with bounded control-point counts;
- flux-tested cubic and Lanczos resampling with conservative mask propagation;
- common-footprint and coverage diagnostics;
- synthetic sub-pixel ground truth for shifts, scale, rotation, mirroring,
  distortion, crowding, partial overlap, hot pixels, and outliers;
- inspected ASI294MC Pro and ToupTek 585C comparisons against an independent
  implementation.

Until those gates pass, reference selection and affine residuals are foundation
APIs and diagnostics, not evidence of a complete registration pipeline.
