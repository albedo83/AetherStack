# Bayer demosaicing contract

## Scientific boundary

Demosaicing consumes one calibrated, linear, single-plane Bayer mosaic and
produces a planar linear RGB image at the same width and height. It never changes
the raw or calibrated source. White balance, color-space conversion, saturation,
display stretching, registration, and integration are separate operations.

The first strict CPU oracle implements the 5x5 gradient-corrected linear method
published by Malvar, He, and Cutler. Its stable algorithm identifier is
`malvar-he-cutler-f64-v1`. The implementation supports RGGB, BGGR, GRBG, and GBRG
at the stored image origin. An unknown declaration is rejected; a pattern is
never guessed from pixel values.

## Precision and determinism

Input, output, coefficients, products, and accumulation use `f64`. Kernel taps
are traversed in a fixed row-major order and represented as integer numerators
over powers of two. Measured CFA samples are copied bit-for-bit into their output
channel. Interpolated results are neither rounded nor clipped: calibrated
negatives and linear-filter overshoot remain available to later processing.

The 5x5 halo uses whole-sample symmetric reflection. The image edge itself is not
duplicated. This rule is part of the algorithm version and applies equally to
all four Bayer phases.

## Mask semantics

Every non-zero interpolation tap must contain a finite, unmasked source sample.
If one support sample is marked missing, saturated, hot, cold, rejected, or
invalid, the affected output channel is NaN and retains the union of those flags
plus `MISSING`. A non-finite source also contributes `INVALID`. A measured CFA
sample retains its own value and mask without contamination from neighboring
pixels.

This conservative first contract deliberately refuses to hide defects. A future
defect-aware interpolation mode requires its own algorithm identifier, evidence,
and differential tests.

## Validation gates

Unit tests cover all standard CFA phases, exact preservation of measured
samples, published center coefficients, edge reflection, planar RGB ordering,
mask and non-finite propagation, unclipped overshoot, and typed input rejection.
Optimized implementations must be compared against this oracle on synthetic
edge/color targets and representative calibrated ASI294MC Pro and ToupTek 585C
frames before becoming selectable.

## Bounded FITS execution

The strict runtime writes one planar binary64 RGB FITS product in red, green,
blue plane order. It processes one channel at a time and one scan-line band at a
time. Each source read contains the core plus the clipped two-pixel vertical
halo required by the 5x5 filter. Global coordinates remain attached to the read
window, so neither CFA phase nor whole-image reflection changes at an internal
band boundary.

Band height is an execution parameter. Changing it must produce byte-identical
FITS output, including checksums and provenance. The memory reservation covers
the largest decoded source band, its temporary sample statuses, one reconstructed
output band, and the streaming writer buffer. It depends on image width and band
height, not full image height.

The output remains private until all three planes have been written, the staged
dimensions and both FITS checksums have been verified, cancellation has been
checked, and the input fingerprint has been recalculated. Publication uses
create-new semantics. Cancellation, insufficient memory, source mutation,
invalid readback, an existing destination, or any earlier failure publishes no
partial result.

## Reviewed Light-plan execution

The whole-plan executor accepts only the exact calibrated result associated with
the current manifest, master plan, and Light plan. Its frame list must match
every planned group and canonical source position exactly. Each calibrated FITS
must carry the expected manifest, Light-plan, group, algorithm, source-count,
and raw-input provenance cards, and both embedded checksums must verify before
the frame can enter demosaicing.

The Bayer phase comes from the reviewed manifest group, never from a filename or
an output-directory heuristic. Each RGB product is first completed inside a
private sibling staging directory. After every frame succeeds, all calibrated
inputs are fingerprinted again and the complete set is exposed with create-new
hard links. Cancellation, input mutation, a stale header, checksum failure, an
existing destination, or a publication failure leaves no public RGB subset.
