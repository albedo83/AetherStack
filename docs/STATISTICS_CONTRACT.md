# Strict image statistics

`image_statistics` is the reference summary operation for a scientific image or
tile. It traverses samples in deterministic planar row-major order and uses only
unmasked finite values.

The public first-pass, mean-pass, and variance-pass accumulators apply the same
algorithm to bounded chunks. The first pass may receive tile-row bands because
counts, extrema, and scale are order-independent. The mean and variance passes
must receive values in canonical planar order; their result is invariant to
chunk boundaries but not to sample reordering. Each repeated pass verifies that
its usable-sample count matches the first pass.

A sample with any quality bit is counted as masked, even if its stored value is
also NaN or infinite. An unmasked NaN or infinity is counted separately as
non-finite. If no usable sample remains, the operation returns a typed error with
all exclusion counts rather than manufacturing zero-valued statistics.

## Precision strategy

The implementation uses three passes:

1. count usable, masked, and non-finite samples while finding the finite range
   and largest absolute value;
2. divide each usable value by that scale and by the sample count, then calculate
   the mean with Neumaier compensated summation;
3. calculate squared deviations in normalized coordinates with another
   compensated sum, divide by the population or sample denominator, and rescale.

Dividing before summation prevents the mean of several large equal samples from
overflowing. Normalized deviations protect the intermediate square. Rescaling
multiplies by the normalized variance before the second scale factor, preserving
finite results when a small normalized spread offsets a large data scale.

The final normalized mean is constrained to the observed normalized range. This
only removes a possible last-bit excursion introduced by floating-point
rounding; a mathematical arithmetic mean cannot lie outside that range.

## Results and failures

The summary reports total, usable, masked, and unmasked non-finite counts;
minimum; maximum; mean; population variance; and sample variance when at least
two samples are usable. Standard deviations are derived from the corresponding
variances. Signed zero results are canonicalized to positive zero.

When the true variance is outside the finite binary64 result domain, the
operation returns `VarianceOverflow`. It never silently stores infinity as an
ordinary scientific statistic.

A changed or truncated repeated stream returns `PassSampleMismatch`. Aggregate
count overflow is also a typed failure. These checks let a staged FITS output be
used as bounded backing storage without weakening the in-memory reference
contract.

## FITS streaming statistics

`primary_image_statistics` applies the same three-pass algorithm directly to a
seekable primary FITS image. It retains only a caller-sized value buffer and
status buffer, decodes samples in canonical order on every pass, and produces
bit-identical results for different chunk boundaries. The source must remain
immutable across the passes; session execution additionally verifies its
fingerprint before and after processing.

Integer `BLANK` samples and stored or scaled NaN/infinity values are reported
separately. Both are excluded from the finite moments. Allocation, decoding,
sample-count, no-support, pass-mismatch, and variance-overflow failures remain
typed rather than being converted to partial statistics.

The `aether-stats` command applies this operation to files or deterministic
directory traversals without following symbolic links. Its JSON Lines output is
versioned per record and is suitable for batch inspection and the future frame
review view. These pixel moments are descriptive diagnostics; they are not a
substitute for astronomical quality metrics such as background, noise, FWHM,
eccentricity, star count, or signal weight.
