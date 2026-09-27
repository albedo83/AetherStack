# Registration geometry and reference contract

Registration operates on immutable linear scientific images. Display stretches,
preview reductions, viewport transforms, and browser coordinates never enter
feature matching, transform estimation, residual measurement, or resampling.

The implementation fixes coordinate, reference-selection, feature matching,
similarity-consensus, confidence, and strict Lanczos-3 resampling rules. Runtime
publication and broader transform models remain separate release gates.

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

The winner is refitted in binary64 least squares over a deterministic bijective
subset of its inlier pairs, then rescored. Dense or partly symmetric fields can
place several individually plausible counterparts inside the residual radius.
Candidates are ranked by independent triangle support, residual, source rank,
and reference rank; a stable greedy assignment then admits each source and
reference feature at most once. Ambiguity is resolved explicitly rather than
aborting a valid real-field consensus or silently fitting duplicate stars.
Minimum support is enforced again after refinement. The result
contains the affine representation, uniform scale, rotation, mirror state,
winning seed, sorted inlier triangle indices, sorted distinct star pairs, and a
strict compensated residual summary. It also records candidate, evaluated,
truncated, inlier, outlier, and point-evaluation counts plus upstream descriptor
truncation. Current hard bounds are 100,000 seed models and 100 million point
residual evaluations. Exceeding either configured budget is never treated as a
valid partial consensus.

Every evaluated seed is retained temporarily within the model bound so the
winner can be compared with genuinely distinct alternatives. Two models are
distinct when their maximum displacement over the four source-image corners and
center reaches the configured separation threshold. The result preserves the
best such competitor, its support, RMS, mirror state, seed, and separation. This
prevents deterministic tie breaking from being mistaken for scientific
certainty on symmetric fields.

## Confidence gate

`similarity-confidence-gate-v1` evaluates consensus evidence without changing
the transform. Its report accepts only when all configured criteria pass and
otherwise retains every rejection reason in a fixed order. Criteria cover:

- minimum refined triangle support and support ratio among hypotheses with the
  selected mirror state;
- minimum distinct one-to-one star correspondences;
- minimum support lead over the best geometrically distinct competitor;
- RMS and maximum reference-pixel residual ceilings;
- minimum horizontal and vertical star-span fractions in both source and
  reference images;
- explicit permission for reflection;
- explicit permission for truncated source descriptors, reference descriptors,
  or seed-model search.

The report stores all four span fractions, inlier ratio, optional support margin,
exact parameters, frame identities, and rejection list. A precise local cluster,
an equal-support symmetric alternative, or an incomplete search therefore cannot
silently become an automatic registration. Synthetic tests include noisy
subpixel rotation, scale, and translation rather than exact descriptor equality.

## Raw-CFA diagnostic profile

`aether-register` connects the strict FITS decoder to phase-neutral 2 × 2 CFA
cell means, canonical quality measurement, feature filtering, local triangles,
descriptor matching, similarity consensus, and the confidence gate. Its
`raw-cfa-registration-precision-v1` JSON contains content identities and
aggregate evidence but never input paths, source header cards, acquisition
metadata, target names, or pixel values. The schema is marked diagnostic-only
because resampling is not yet part of this operation.

The local ASI294MC Pro validation session uses one fixed Light as reference and
the other nine as sources. All nine comparisons passed the untruncated
confidence gate. Each comparison retained 908–1,022 distinct bijective star
pairs, covered at least 98.0% of the detection-plane height and 98.6% of its
width, and produced RMS residuals from 0.213 to 0.233 detection pixels. Since
one detection pixel spans two sensor pixels, that is 0.426–0.465 source pixels.
These measurements validate this camera/session profile only; no private file
name, digest, target, coordinate, or image data is retained in the repository.

## Strict Lanczos-3 resampling oracle

`lanczos3-normalized-f64-v1` resamples every planar channel in the reference
coordinate system. The supplied transform always remains source-to-reference;
the resampler computes its inverse once and evaluates that inverse at each
integer-centered output pixel. For source distance `d`, the one-dimensional
kernel is `sinc(pi*d) * sinc(pi*d/3)` for `abs(d) < 3` and zero otherwise. Exact
integer offsets are returned as analytical zeros, making identity and integer
translation paths bit-exact for clear samples instead of introducing tiny
floating-point neighbor weights.

The two-dimensional kernel is separable. Products and normalization weights are
accumulated in deterministic order with compensated binary64 sums, then divided
by the measured weight sum. Values are not clipped, so scientifically meaningful
negative lobes and overshoot remain available to later processing. Tests prove
constant-field preservation and unit integrated flux for an isolated source
under a fractional translation.

Every mathematically non-zero tap must lie inside the source and contain a clear,
finite sample. A footprint crossing the source boundary produces `NaN` with the
`MISSING` flag. Unusable support produces `NaN` with the union of all source
flags, plus `INVALID` for an otherwise unflagged non-finite input. Zero-weight
taps never spread unrelated defects. Statistics account for every output sample
as interpolated, outside the footprint, or withheld by masked support.

The current whole-image scalar implementation is the numerical oracle, not the
final high-volume executor. The bounded executor walks top to bottom, retains at
most the configured number of output rows, evaluates every sample in global
reference coordinates, and has bitwise differential coverage against this
oracle across multiple planes, partial final bands, boundary loss, masks, and
non-finite source values. File-backed source windows and transactional FITS
publication remain runtime responsibilities.

Before decoding each band, the source-window planner evaluates the exact same
inverse transform and analytical-zero kernels. It returns the smallest rectangle
containing every non-zero tap of every complete output kernel. Entirely disjoint
bands require no source read. Integer-aligned transforms consequently request no
invented halo, while fractional transforms include all six taps per affected
axis. This plan is geometric and does not inspect pixel values or masks.

The immutable band plan is also the authority that consumes the decoded window.
It rejects absent, unnecessary, or dimensionally different storage before
sampling, and converts global tap coordinates to window-local indices with
checked arithmetic. A disjoint plan emits a fully missing band without source
storage. Multi-plane, masked, non-finite, fractional-transform tests assemble
all window-backed bands and require bitwise equality with the whole-image
oracle, including masks and support counters.

The strict runtime traverses FITS planes in storage order and bands from top to
bottom. It decodes only the planned rectangle, reserves the larger of decode and
kernel working sets against the shared memory budget, streams the output into a
private checksum-generating FITS, validates its dimensions and checksums, and
revalidates the source fingerprint before atomic create-new publication.
Cancellation, memory exhaustion, source mutation, readback failure, and any
scientific error leave the destination absent. Band height is an execution
choice only: tested alternatives produce identical FITS bytes.

## Common geometric footprint and autocrop

`common-lanczos3-footprint-v1` evaluates only image extents and accepted
source-to-reference transforms. At each reference pixel it applies every inverse
transform and asks whether all mathematically non-zero Lanczos-3 taps lie inside
that source. Exact integer mappings retain their analytical one-tap support;
fractional mappings require the complete six-tap support on both axes. This is
the same discrete boundary rule as the resampling oracle.

Pixel masks and values are deliberately excluded. A hot, cold, saturated, or
missing sample may invalidate nearby resampled values and later reduce their
integration support, but it cannot redefine the geometric overlap or collapse
the autocrop around one local defect.

The scanner keeps a height histogram and monotonic stack proportional to output
width, not image area. It reports the exact number of common-support reference
pixels and the largest axis-aligned rectangle entirely contained in that support.
Equal-area rectangles prefer smaller top coordinate, smaller left coordinate,
larger width, then larger height. No overlap is an explicit absent rectangle,
not an invented zero-sized crop. Frame count and pixel/frame evaluations have
public hard bounds.

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
- confidence thresholds validated on broader sparse and crowded real data;
- justified distortion models with bounded control-point counts;
- explicit user override of the computed common footprint with provenance;
- a separately identified cubic option if real comparisons justify it;
- synthetic sub-pixel ground truth for shifts, scale, rotation, mirroring,
  distortion, crowding, partial overlap, hot pixels, and outliers;
- inspected ASI294MC Pro comparison against an independent implementation and
  equivalent ToupTek 585C validation.

The desktop Registration laboratory exposes the native pair diagnostic,
confidence evidence, accepted source-pixel transform, and exact autocrop. Once
all non-reference Lights pass, it submits only the selected stable identities
to Rust. The native plan-preview command resolves the complete Light set from
the immutable imported manifest, reruns every pair diagnostic and confidence
gate, constructs `registration-plan-v1`, and returns its digest and all-frame
crop. It never accepts transforms, dimensions, coverage, or crop values from
the web presenter. Execution then requires one calibrated artifact for every
sealed identity and the exact reviewed digest. Rust reconstructs the plan again,
fingerprints each artifact, verifies its embedded review identity, and runs the
complete rollback-safe publication transaction under one bounded memory budget.
The UI streams frame-and-band progress and offers cooperative cancellation; it
never presents a partially published set.

The geometry core also provides `registration-plan-v1`, the immutable boundary
for that future orchestration. It requires at least two unique reviewed frame
identities, canonicalizes discovery order by identity, requires the declared
reference to match the output canvas with the exact identity transform, and
derives one all-frame Lanczos-3 footprint. Duplicate identities, a missing or
geometrically inconsistent reference, bounded-work failures, allocation
failure, and an empty common rectangle are explicit plan errors. Its canonical
SHA-256 binds the algorithm identifiers, reference identity and canvas,
identity-sorted source dimensions, exact binary64 transform bits, coverage, and
crop without retaining machine paths.

The strict runtime's plan-bound constructor accepts a local source only with
its portable session-relative path. It re-derives the reviewed `FrameId` from
that path plus the recorded byte length and content SHA-256, looks up geometry
inside the immutable plan, and requires FITS provenance to carry the exact plan
digest. It also rechecks decoded source dimensions before creating output. The
older direct constructor remains the numerical oracle boundary; production
orchestration must use the plan-bound path.

For a color-camera production path, registration must consume calibrated linear
RGB rather than resample the raw CFA mosaic as ordinary scalar pixels. The
artifact-bound constructor therefore looks up geometry by the reviewed
`FrameId` and requires the input FITS header's `AETHFID` to match it before an
output writer exists. The artifact's own fingerprint remains `AETHINP` in the
registered output, preserving both immediate and logical identity. Raw CFA
inputs remain useful to the phase-neutral diagnostic solver only.

`run_registration_plan` is the all-frame publication boundary. The supplied
portable or embedded reviewed identities must match every canonical plan entry exactly once. Each
registered image is constructed and checksum-read back inside a private sibling
directory; all pixel sources are fingerprinted again before the first public link
appears. Final filenames contain only stable frame IDs. Create-new publication
never overwrites user data, rolls back links made by a failed or cancelled run,
and synchronizes the destination directory after the complete set is visible.
