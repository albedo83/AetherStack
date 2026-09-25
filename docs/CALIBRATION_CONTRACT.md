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

### Bounded single-frame FITS execution

`run_strict_calibration_pipeline` applies this primitive to one immutable Light
and writes one binary64 FITS image without integrating it with another frame.
The request requires `AETHSRC = 1` and the distinct algorithm identifier
`strict-calibrated-light-v1`, so a calibrated exposure cannot be mistaken for a
stack. It also requires `AETHINP` to equal the source fingerprint SHA-256, so
the product retains an exact path-free identity. It uses the same strict FITS
validation, source fingerprints, bounded
spatial tiling, exact complete-image statistics, verified checkpoints, final
source revalidation, and atomic create-new publication as the integration path.

The memory reservation reflects the single-frame peak: signal, Dark, normalized
Flat, calibrated output tile, one output band, bounded statistics buffers, and
the writer buffer. It does not reserve integration support or a vector of
calibrated frames. Tests fix exact output values and masks, reject incoherent
provenance, and require byte-identical products for different tile shapes.

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

### Bounded FITS execution

`run_strict_master_pipeline` executes direct bias and dark master construction
from a canonical source list. The request requires an `AETHPLN` binding to the
exact master-plan SHA-256 and an `AETHSRC` count equal to the complete source
list. Every source is fingerprinted before reading and again after calculation,
so a frame that changes during a run prevents publication.

The executor traverses a deterministic spatial-plane tile grid, reserves the
derived peak working set before allocating, integrates each tile with
`strict-mean-v1`, and streams full-width bands to a private binary64 FITS file.
It then performs bounded readback statistics and atomically publishes with
create-new semantics. Cancellation, malformed FITS input, dimension mismatch,
memory exhaustion, source mutation, and output collision leave no partial
destination.

This direct path rejects `Flat` requests. `run_strict_flat_master_pipeline`
provides the separate required sequence: it reads the single pedestal selected
by the plan, subtracts it once from every raw flat tile, integrates the corrected
tiles in canonical source order, derives one exact positive-sample median from
the complete integrated master, and normalizes every output pixel by that one
scalar. The pedestal is fingerprinted before use and revalidated with the raw
flats before publication.

Normalized flat outputs use the distinct provenance algorithm identifier
`strict-flat-v1`; they are never mislabeled as a direct `strict-mean-v1`
product.

Exact global median selection requires storage proportional to the integrated
image. The flat executor therefore derives and reserves its worst-case peak
before allocating: the integrated image, normalized image, exact-median scratch,
tile working set, statistics buffers, and writer buffer are all included. This
is intentionally an in-memory performance path for current astronomy-camera
dimensions. An insufficient configured budget fails before output creation;
the runtime never substitutes a histogram approximation or per-tile
normalization. Refusing those shortcuts prevents an apparently valid but
scientifically inconsistent flat master.
