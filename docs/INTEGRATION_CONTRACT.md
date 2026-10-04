# Strict mean integration

`integrate_mean` is the first reference image integrator. It accepts one or more
equal-sized scientific images and calculates an unweighted arithmetic mean for
each planar sample position. Statistical rejection and frame weighting are
separate versioned estimators rather than hidden behavior changes.

`integrate_mean_region` applies the same estimator to one non-empty rectangular
region contained by every input plane. Its output uses the region width and
height with the original plane count. The implementation maps output samples
directly into full-frame sources in planar order; it does not allocate cropped
copies or process pixels outside the requested extent. This is the strict CPU
primitive used by future registered stacking with the sealed common crop.

## Eligibility and support

A source sample participates only when its quality mask is clear and its value
is finite. Every output pixel has a `PixelSupport` record containing separate
counts for accepted, masked, and unmasked non-finite inputs. Their sum always
equals the number of input images.

When at least one input is accepted, the output is the mean of accepted values
and its mask is clear. Exclusions remain visible in the support record without
invalidating a usable result.

When no input is accepted, the output value is NaN and the mask contains
`MISSING`. All mask bits from rejected inputs are retained. `INVALID` is also set
when at least one unmasked NaN or infinity was observed.

## Precision and determinism

Each output pixel uses two passes over inputs in caller-supplied order. The first
classifies samples and finds the largest absolute accepted value. The second
divides every accepted value by that scale and by the accepted count, then uses
Neumaier compensated summation. The normalized result is constrained to the
observed normalized range before rescaling.

This ordering prevents a set of large equal values from overflowing an
otherwise finite mean and preserves small residuals during cancellation. Input
order is part of the strict execution profile and later orchestration must use
canonical manifest order. Spatial tiling does not alter the reduction order for
any pixel, so tile dimensions are not scientific parameters.

The per-pixel support representation accepts at most `u32::MAX` inputs. Empty
input, count overflow, dimension mismatch, internal invariant failure, and
allocation failure are typed errors. Empty, overflowing, or out-of-bounds
regions are also explicit failures. No partial result is returned. Tests prove
that a full-frame region is bit-identical to `integrate_mean`, that multi-plane
crop coordinates preserve planar order, and that excluded-sample accounting is
unchanged inside a crop.

## Exact median

`integrate_median` is the strict `median-f64-v1` CPU oracle. At each planar
sample position it collects only clear finite values, selects the middle rank
with IEEE total ordering, and retains the same accepted, masked, and non-finite
support accounting as the strict mean. Odd populations return an observed
sample exactly. Even populations use a finite overflow-safe midpoint: a bounded
difference for same-sign neighbors and half-before-addition for opposite signs.
The result canonicalizes signed zero.

One reusable scratch vector is reserved for at most one value per input frame;
allocation failure is explicit. A pixel with no eligible value follows the same
NaN, `MISSING`, retained-mask, and `INVALID` policy as mean integration. Tests
cover odd and even populations, equal `f64::MAX` values, opposite finite
extremes, masked and non-finite evidence, empty input, and dimension mismatch.
The registered-stack runtime exposes this estimator as
`registered-median-f64-v1`. It processes the exact sealed affine or projective
common crop in bounded bands, reserves the reusable per-pixel rank scratch, and
publishes one checksum-verified binary64 FITS product with the distinct
estimator identity. Tests require exact median pixels and byte-identical output
across band heights. Desktop selection and report-schema exposure remain the
next provenance boundary.

## Frame-weighted mean

`integrate_weighted_mean` accepts one finite, strictly positive `FrameWeight`
per image. A masked or non-finite sample leaves both numerator and denominator,
so partial support cannot darken a pixel. Values and weights are independently
scaled before compensated accumulation, which avoids overflow at finite
binary64 extremes and makes a common positive rescaling of all weights
numerically invariant. Input order remains the stable manifest order, and the
same exact accepted, masked, and non-finite support accounting is retained.

`balanced-psf-weight-v1` is the first transparent expression:

`w = (SNR/SNR_ref)^2 × (FWHM_ref/FWHM)^2 × (1-e^2)/(1-e_ref^2)`.

It is evaluated in the logarithmic domain and bounded only at the positive
finite binary64 limits. Signal-to-noise and FWHM must be positive, eccentricity
must be in `[0, 1)`, and all three metrics must come from the same versioned
measurement profile and image scale. The formula is not an undocumented clone
of another application: its terms, reference, identifier, and exact resulting
weights are inspectable provenance.

## Percentile-clipped mean

`integrate_percentile_clipped_mean` is a separate deterministic estimator, not
a hidden mode of the strict mean. At each pixel, clear finite samples are
sorted with IEEE total ordering. The algorithm rejects
`floor(n × low_fraction)` and `floor(n × high_fraction)` samples from the two
tails only when the configured minimum number of retained samples remains.
Masked and non-finite samples never enter rank calculation.

The retained samples use the same scaled Neumaier mean as the strict oracle.
Every pixel reports accepted, masked, non-finite, low-rejected, and
high-rejected counts whose sum equals the input count. Support absence is never
mislabeled as statistical rejection.

When requested, `materialize_percentile_rejection_map` converts the rejection
evidence into two separate binary64 images with the same width, height, plane
count, and planar order as the integrated band. One image contains exact
low-tail counts and the other exact high-tail counts. The source counts are
`u32`, so their conversion to `f64` is exact. Map masks are clear because
masked and non-finite exclusions remain different support categories and are
not statistical rejections.

## Registered stack execution

`run_registered_stack` is the bounded FITS-to-FITS orchestration for a sealed
registration plan. The source list must contain each planned frame exactly once
and is canonicalized by reviewed identity. Each registered artifact must carry
that identity in `AETHFID`, the exact plan digest in `AETHPLN`, fully verified
FITS checksums, reference-canvas dimensions, and the same plane count.

The executor reads one common-crop band from every source in canonical order,
integrates it with the strict estimator, and streams the cropped result into a
private binary64 FITS. Its memory reservation covers all decoded source bands,
the integrated image and support map, decode status, vector storage, and writer
buffer. Band height affects I/O and peak memory only, never numerical order.
After private checksum readback, every source is fingerprinted again before the
create-new publication. Cancellation, insufficient memory, source mutation,
stale plan evidence, checksum failure, or an existing destination leaves no
new public product.

The runtime exposes the percentile estimator under
`registered-percentile-mean-v1`. Its provenance identity is distinct from
`registered-crop-mean-v1`, and its larger support record plus reusable sorting
scratch are included in the logical memory reservation. Optional low/high maps
use the separate `percentile-rejection-map-v1` provenance identity. Their two
band images and three FITS writer buffers are also included in the reservation.

The runtime exposes the weighted estimator under
`registered-weighted-mean-v1`. `RegisteredWeightSet` first binds every weight
to a reviewed frame identity, sorts those identities canonically, and hashes a
domain marker, the weight-expression identifier, every frame ID, and every
exact binary64 weight. When the balanced-PSF constructor derives those weights,
the digest additionally covers the exact reference and per-frame metrics. The
request accepts the set only if it matches the sealed plan exactly and output
provenance carries that digest in `AETHPAR`. Weights are then reordered together
with sources and used independently in every bounded band. A caller cannot
select this estimator through the generic constructor and accidentally omit
its weight set.

Science and both companion files are completely written and checksum-verified
while private. Their destinations must be distinct, their source counts and
plan digests must agree, and publication never overwrites an existing file.
The executor then publishes the three files as a rollback-safe set. If a later
destination collides or publication fails, every earlier file created by that
run is removed; pre-existing files are never modified. A rollback failure is a
separate typed error so an operator can identify the incomplete set.

The desktop keeps strict mean as the visible default. Its collapsed advanced
section can select the percentile estimator, configure both tail fractions and
the retained-sample floor, and request both rejection maps. JavaScript only
transports these values. Rust validates them again, derives adjacent create-new
map destinations from the chosen science path, binds all provenance, and owns
the complete transaction.

The desktop adapter reconstructs the registration plan from the current
reviewed Light membership before every stack run. It accepts only the complete
published registered artifact set, opens a native FITS destination chooser,
forwards bounded progress and cancellation, and reports the exact crop
dimensions and peak reserved memory after atomic publication. JavaScript never
reads or integrates scientific pixels.

After publication, the desktop may request a bounded display preview of the
integrated product. Rust estimates the display transform and renders RGB or a
selected scalar plane; the browser retains only a revocable PNG object URL.
When rejection maps exist, the same viewer exposes explicit Science, Low
reject, and High reject tabs. Each selection requests its own scalar preview
from Rust, revokes the previous object URL, and rejects stale asynchronous
responses by plan, science output, and selected product identity.
This display path cannot mutate, replace, or validate the scientific FITS, and
a preview failure does not invalidate a successfully published stack.
