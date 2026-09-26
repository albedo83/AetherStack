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
