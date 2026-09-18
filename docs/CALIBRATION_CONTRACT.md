# Initial calibration contract

The first strict CPU calibration primitive applies one deliberately narrow
equation to equal-sized signal, dark, and normalized-flat tiles:

```text
calibrated = (signal - dark) / flat
```

All operands and the result are IEEE 754 binary64 values. Samples are visited
once in planar row-major order. There is no reduction and therefore no
order-dependent accumulation in this operation.

This primitive does not guess whether a dark contains bias, scale a dark by
exposure, normalize a flat, construct masters, or propagate uncertainty. Those
are separate scientific policies planned for the master-calibration phase.

## Flat threshold

The caller supplies a finite, non-negative minimum absolute flat value. A flat
whose absolute value is less than or equal to that threshold is unusable. The
threshold has no library default because changing it can change results and must
remain visible in processing provenance. Positive and negative zero are
canonicalized to the same threshold.

## Masks and non-finite values

Input mask bits are combined without discarding unknown bits. Any flagged input
produces a canonical NaN output and retains the combined flags. This conservative
rule prevents a later stage from accidentally consuming a value whose source was
already known to be unsuitable.

An unflagged NaN or infinity in any input produces NaN and adds `INVALID`. An
unsafe flat divisor does the same. When finite inputs produce a non-finite result
through IEEE 754 overflow, the exact result is retained and marked `INVALID`.

The function never mutates any source image. Dimension mismatches and allocation
failures are typed errors and no partially calibrated image is returned.
