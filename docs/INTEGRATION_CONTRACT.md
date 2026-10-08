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
across band heights. The desktop exposes “Exact median” as an explicit advanced
estimator, disables rejection-only controls, sends the stable `median` wire
identity, and seals `registered-median-f64-v1` in FITS and integration reports.
Existing report schemas remain readable because the estimator enum gains a new
value without changing prior payloads.

## Iterative sigma-clipped mean

The strict CPU oracle `sigma-clipped-mean-f64-v1` sorts clear finite samples
once per pixel, then iterates asymmetric low/high thresholds around a normalized
compensated mean and population standard deviation. Threshold equality is
retained. A proposed pass is applied in full only when it preserves the explicit
minimum support; convergence and a positive maximum-iteration bound are both
deterministic stop conditions. The final mean is overflow-resistant and the
support record distinguishes masks, non-finite exclusions, low rejects, and
high rejects exactly.

The registered runtime identity is `registered-sigma-mean-f64-v1`. It executes
the exact common crop in bounded bands and can publish low/high
`sigma-rejection-map-v1` companions in the same rollback-safe transaction. A
domain-separated SHA-256 over the binary64 sigma limits and integer iteration
and support bounds is required in `AETHPAR` for both the science image and its
maps. The same enforcement now covers percentile controls. Tests require exact
pixels, verified checksums, and byte-identical products across band heights.
The desktop exposes the estimator under the stable `sigma_clipped` wire value,
shows only the controls relevant to the selected rejecting estimator, and seals
the values into the integration report. Reports written before these controls
remain readable with the documented 4.0/3.0/eight-pass defaults.

## Winsorized sigma-clipped mean

`winsorized-sigma-clipped-mean-v1` uses the same validated asymmetric limits,
iteration bound, support floor, and exact rejection accounting as ordinary
sigma clipping. After the first pass, each rejected tail is replaced by its
nearest retained boundary only while estimating the next population mean and
standard deviation. This bounds outlier influence without feeding substituted
values into the final science mean and avoids the distribution collapse that
can make ordinary iterative clipping reject valid near-tail samples.

The bounded registered runtime publishes this estimator as
`registered-win-sigma-mean-f64-v1` with optional
`win-sigma-rejection-map-v1` companions. Its parameter digest uses a distinct
domain tag even when the numeric controls match ordinary sigma clipping. Tests
prove the less-aggressive retained support, separate parameter identity,
verified checksums, and byte-identical science and rejection products across
band heights. The desktop exposes `winsorized_sigma_clipped` beside ordinary
sigma clipping, reuses the validated asymmetric controls, and seals the
distinct estimator identity into new and archived integration reports.

## Ordered-sample linear-fit-clipped mean

`linear-fit-clipped-mean-f64-v1` is a one-pass, deterministic residual
estimator. For each pixel it sorts clear finite source samples by IEEE total
ordering, normalizes them by the largest absolute sample, and fits a line
against uniformly spaced symmetric rank coordinates in `(-1, 1)`. Intercept,
slope, and population residual variance use compensated binary64 accumulation.
An exact affine ramp is preserved: residual deviations no greater than
`32 * f64::EPSILON` after normalization are treated as zero.

Samples strictly below `-low_sigma` or above `high_sigma` times the fitted
population residual deviation are rejected. Equality remains accepted. Both
limits must be finite and positive, and the retained-sample floor must be at
least three. A proposed decision that would cross that floor is discarded in
full. Fewer than three usable samples, an all-zero population, or a degenerate
rank fit also retains every usable sample. The oracle never substitutes fitted
or normalized values into science output: it applies the strict compensated
mean to the original retained samples.

The registered runtime identity is `registered-lin-fit-mean-f64-v1`. It binds
both exact binary64 sigma limits and the retained-sample floor into `AETHPAR`,
executes the sealed affine or projective common crop in bounded bands, and can
atomically publish `linear-fit-rejection-map-v1` low/high count maps. Tests lock
permutation invariance, perfect-ramp preservation, asymmetric outlier evidence,
mask and non-finite accounting, the all-or-nothing support floor, exact FITS
checksums, and byte-identical products across band heights.

The desktop exposes the stable `linear_fit_clipped` wire value with explicit
5.0/3.5 defaults and a minimum support of three. Only its relevant sigma,
support, and rejection-map controls remain visible. Unlike iterative sigma
clipping, it has no pass count: iterative refitting would be a different
scientific algorithm and requires a new identifier.

## Generalized extreme Studentized deviate mean

`generalized-esd-mean-f64-v1` implements the two-sided generalized ESD
procedure documented by the NIST/SEMATECH handbook. It assumes the usable
per-pixel population is approximately normal. The maximum-outlier fraction is
an upper bound on the number of suspected anomalies, not a forced rejection
rate; its floor at the current usable sample count determines the maximum
number of sequential tests.

For candidate step `i`, the oracle calculates the largest absolute deviation
from the sample mean in units of sample standard deviation, removes that
candidate temporarily, and repeats up to the configured bound. Each statistic
is compared with its matching two-sided Student-t critical value at the exact
configured family-wise significance. The rejected count is the largest `i`
whose statistic strictly exceeds its critical value, so all earlier candidates
through that point are rejected even when an earlier individual comparison did
not pass. Critical-value equality remains accepted.

Every population is magnitude-normalized before compensated mean and variance
accumulation, preventing finite extreme values from overflowing intermediate
moments. Sorted IEEE total order and deterministic low-side tie resolution make
the result independent of source order. The final science value is the strict
compensated mean of original retained samples. Masks and non-finite values
remain separate evidence and never enter the statistical population.

The published approximation is admitted only for at least 15 usable samples.
Smaller populations, zero-spread populations, distribution failures, a zero
fractional candidate count, or a retained-support conflict reject nothing.
Controls require a finite maximum-outlier fraction in `(0, 0.5]`, significance
in `(0, 1)`, and at least three retained samples.

The registered identity is `registered-esd-mean-f64-v1`; optional low/high maps
use `esd-rejection-map-v1`. The exact fraction, significance, and support floor
are bound into `AETHPAR`. The bounded runtime reserves reusable candidate,
decision, and statistical scratch, and publishes science plus both maps as one
checksum-verified rollback-safe set. Tests reproduce the NIST 54-value example
with three detected outliers, then lock permutation invariance, conservative
small-sample behavior, masks, non-finite evidence, support-floor rollback,
parameter identity, and byte equality across band heights.

The desktop exposes `generalized_esd` with explicit 0.30 maximum-outlier and
0.05 significance defaults. Only ESD, support, and map controls remain visible,
and the 15-sample applicability gate is stated beside the controls.

### Per-source rejection attribution

Generalized ESD additionally retains the disposition of every source sample at
every output position. `RejectionAttribution` stores one of five states:
accepted, masked, non-finite, rejected low, or rejected high. The storage is a
checked three-bit cube indexed by source identity and planar output offset; it
therefore preserves the source ownership that aggregate low/high count maps
necessarily discard. Invalid packed codes, dimensions, indices, and allocation
sizes fail explicitly.

The registered runtime accounts for the packed cube in its band memory plan and
reduces each completed band into exact per-source totals. Source order is the
sealed execution order, counters use checked `u64` accumulation, and tests
require identical totals across different valid band heights. The native
desktop response carries these totals through a typed browser contract. The
active result view ranks at most 50 sources by rejected samples and reports
low, high, masked, and non-finite evidence separately.

`source-large-scale-rejection-v1` is the first source-specific spatial oracle
built on this attribution. Low and high tails are configured independently.
For `L` detection layers, a rejected source sample is a structure seed when its
square neighbourhood of radius `2^(L-1)` contains at least one full diameter of
same-tail rejects. This summed-area classifier is linear in image samples and
does not merge planes or sources. Seeds are grown by an exact square Chebyshev
radius.

Low and high growth is calculated before attribution changes. A previously
accepted sample reached by one tail is promoted to that tail; a sample reached
by both remains accepted, so processing order cannot choose its sign. Masked,
non-finite, and already rejected evidence is immutable. Promotions at one
output position are applied as a complete set only when the configured
retained-sample floor survives.

After expansion, science and clipped support are rebuilt from the original
source values. No interpolated, fitted, normalized, or synthetic value enters
the mean. Every recorded disposition is revalidated against the supplied image
mask and finiteness class; foreign source counts, dimensions, or samples fail
closed. A domain-separated SHA-256 binds tail enablement, layers, growth, and
support. The memory plan accounts for all retained tail masks, temporary seed
and growth storage, and one reusable summed-area table.

The registered runtime evaluates an expanded row window and publishes only its
non-overlapping core. The halo is the maximum enabled detection radius plus
growth radius, clipped only at global image boundaries. Science, support,
source totals, and low/high FITS maps are cropped together, so changing band
height cannot change bytes or attribution. The memory reservation includes the
expanded source images, attribution, spatial masks, summed-area table, rebuilt
science, and the simultaneous cropped core.

The runtime compares the cropped attribution immediately before and after
large-scale processing. `RejectionPromotionCounts` records only transitions
from accepted to rejected-low or rejected-high. Unchanged states are valid;
every other transition, source-count mismatch, dimension mismatch, and checked
counter overflow fails the run. Counts are accumulated across non-overlapping
band cores, so halo samples are never counted twice. The runtime retains the
same checked low/high/total record for every source identity as well as the
aggregate record. A spatial run returns explicit zero-valued records when no
promotion occurs, while non-spatial estimators return no records. Tests require
exact source and aggregate totals and equality across valid band heights.

`registered-spatial-esd-f64-v1` and `spatial-esd-rejection-map-v1` distinguish
spatial output from pixel-local ESD. Their shared parameter digest covers both
the generalized-ESD controls and the spatial policy. Native report schema 3
persists all six low/high controls. Schema 4 additionally seals the exact
aggregate low/high/total promotion counts and rejects a record whose total is
not their checked sum or whose presence disagrees with the spatial settings.
Schema 5 seals the corresponding optional record on every source. Validation
uses checked addition for each record and requires the source totals to
reconcile exactly with the aggregate; missing, unexpected, or overflowing
evidence fails closed. Schemas 1 and 2 remain readable only with spatial
expansion disabled; schema 3 remains readable without promotion evidence, and
schema 4 remains readable with aggregate evidence only. The desktop exposes
the feature solely under Generalized ESD in an advanced panel with bounded
inputs and shows the additional spatial rejections separately from the complete
per-source totals in both the active result and verified archived-report views.
The compact per-pixel cube remains execution-local, and gradient-aware
alternatives remain future versioned algorithms rather than undocumented
changes to this contract.

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
