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

- deterministic multi-scale star detection and invariant feature descriptors;
- robust correspondence search with explicit ambiguity and sparse-field errors;
- translation, affine, and projective model fitting with inspectable inliers;
- justified distortion models with bounded control-point counts;
- flux-tested cubic and Lanczos resampling with conservative mask propagation;
- common-footprint and coverage diagnostics;
- synthetic sub-pixel ground truth for shifts, scale, rotation, mirroring,
  distortion, crowding, partial overlap, hot pixels, and outliers;
- inspected ASI294MC Pro and ToupTek 585C comparisons against an independent
  implementation.

Until those gates pass, reference selection and affine residuals are foundation
APIs and diagnostics, not evidence of a complete registration pipeline.
