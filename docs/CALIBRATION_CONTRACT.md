# Calibration contract

## Light calibration

The first strict CPU calibration primitive applies one deliberately narrow
equation to equal-sized signal, dark, and normalized-flat tiles:

```text
calibrated = (signal - dark) / flat
```

All operands and the result are IEEE 754 binary64 values. Samples are visited
once in planar row-major order. There is no reduction and therefore no
order-dependent accumulation in this operation.

This primitive does not guess whether a dark contains bias, scale a dark by
exposure, construct masters, or propagate uncertainty. Those are separate
scientific policies in the master-calibration phase.

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

## Exclusive pedestal subtraction

`subtract_pedestal` applies `corrected = signal - pedestal` in binary64. The
versioned master plan selects either one true bias or one matched short dark; the
pixel primitive is intentionally unable to subtract both in one call. Inputs
must have identical dimensions.

Existing mask bits are combined, including unknown future bits. A flagged input
produces canonical NaN with the combined flags. An unflagged non-finite input
adds `INVALID` and produces NaN. Finite overflow is retained and marked invalid,
while exact zero is canonicalized to positive zero. Neither source is mutated.

## Robust flat normalization

`normalize_flat` derives one exact median from clear, finite, strictly positive
samples of a pedestal-corrected flat. Masked, non-finite, and non-positive
samples are counted separately. The caller supplies both a minimum accepted
sample count and an exclusive non-negative lower bound for the median; there are
no library defaults.

Median selection uses a fallibly allocated vector containing exactly the
accepted samples and expected-linear-time selection rather than a full sort.
For an even sample count, the two central positive values use
`lower + (upper - lower) / 2`, avoiding overflow for values near `f64::MAX`.

The output divides each clear, finite, positive pixel by the median. Existing
flagged pixels remain flagged and become NaN. Non-finite and non-positive pixels
become NaN and add `INVALID`; finite division overflow is retained and marked
invalid. The result records the median and complete sample accounting for
provenance and diagnostics.

## Strict mean master construction

`construct_strict_mean_master` tags an integration as bias, dark, or flat and
records the algorithm identifier `strict-mean-v1`. It uses the strict integration
oracle: input order follows canonical manifest order, each output pixel has
accepted/masked/non-finite support counts, and normalized compensated
accumulation avoids overflowing equal large values.

Bias and dark frames are integrated directly at this stage. Before flat
integration, every source flat must have the single pedestal selected by the
master plan subtracted. The flat master is then normalized with the guarded
exact-median primitive. This ordering is covered by a synthetic equation test.
Rejection and weighting are not silently implied by `strict-mean-v1`; they will
be separate versioned algorithms.
