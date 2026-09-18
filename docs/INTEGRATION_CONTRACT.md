# Strict mean integration

`integrate_mean` is the first reference image integrator. It accepts one or more
equal-sized scientific images and calculates an unweighted arithmetic mean for
each planar sample position. Weighting, statistical rejection, normalization,
and registration are intentionally absent until their independent contracts and
tests exist.

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
allocation failure are typed errors. No partial result is returned.
