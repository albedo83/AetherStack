# Strict frame-quality contract

Frame quality is measured from immutable linear scientific pixels, never from a
screen stretch or compressed preview. Every metric has a versioned algorithm,
unit, input contract, support count, missing-value behavior, and deterministic
reduction order.

## Global background reference

`global-mad-clip-v1` is the initial strict reference estimator. It collects only
clear finite pixels from the selected plane, then iterates:

1. exact median background location;
2. exact median absolute deviation (MAD);
3. Gaussian-consistent noise `MAD * 1.482602218505602`;
4. symmetric clipping at the configured number of noise sigmas.

The output records original usable, masked, and non-finite counts; retained and
clipped support; iteration count; and convergence. A uniform image legitimately
has zero noise. Operations requiring a detection threshold reject that state
instead of inventing noise. Exact order statistics require memory proportional
to usable pixels and are the correctness oracle for future bounded spatial
estimators.

## Stellar measurement reference

`local-max-moments-v1` operates on one explicitly selected, prepared monochrome
detection plane. It finds deterministic 3x3 local maxima above a background-plus-
sigma threshold. Equal-valued plateaus retain the lowest planar pixel index.
Candidates are ordered by descending peak and lower peaks inside the configured
minimum separation are suppressed.

For each retained candidate, a circular aperture includes clear finite pixels
above a lower measurement floor. Background-subtracted flux weights produce a
sub-pixel centroid and covariance matrix. The covariance eigenvalues are
corrected analytically for the explicit Gaussian isophote truncation, then
reported as major/minor FWHM in pixels and eccentricity
`sqrt(1 - minor_variance / major_variance)`. Background SNR is explicitly a
proxy: aperture signal divided by background sigma and square-root support. It
is not an electron-domain shot-noise model.

Saturated stars remain in the detailed result and carry a saturation flag, but
do not contribute to median FWHM or eccentricity. Results also report raw and
suppressed candidate counts, rejected measurements, measurement support, and the
exact detection threshold. Candidate count and every major allocation have
explicit failure bounds.

## CFA-neutral diagnostic plane

Raw Bayer mosaics are never passed directly to the stellar detector: channel
response differences would create phase-dependent structure and biased moments.
`cfa-cell-mean-v1` instead collapses each complete origin-aligned 2 by 2 Bayer
cell into one compensated arithmetic mean. Every standard Bayer phase contains
the same one red, two green, and one blue samples in that cell, so their storage
order cannot alternate the detection signal. Output coordinates have a scale of
two source pixels, and reported FWHM is converted back to source-pixel units.

The transform accepts only a one-plane image with even axes and a separately
verified standard Bayer declaration. It requires all four inputs: any mask or
non-finite value invalidates the complete output cell and retains the union of
quality flags. This keeps the effective channel weights constant rather than
silently averaging partial cells. It is a detection plane, not a debayered color
product.

The initial desktop profile exposes this transform only as an explicit expert
diagnostic. It does not authorize automatic rejection. Strong spatial gradients
also remain a release blocker: the global estimator is a strict baseline, while
production selection requires a tested spatial background/noise model.
The native adapter rejects sources above 64 Mi samples before allocation. Its
first diagnostic profile does not infer a saturation level from container type;
saturation count and threshold therefore remain explicitly unavailable.

The initial estimator does not deblend overlapping sources or fit a full PSF
family. Minimum separation prevents duplicate peaks, but crowded or nebulous
fields require later deblending and spatial diagnostics. Missing or degenerate
measurements remain explicit; they never become numerical zero.

## Validation gates

Synthetic tests cover outlier clipping, masks, non-finite values, uniform data,
multi-plane isolation, full-range median arithmetic, elliptical Gaussian
centroids and shapes, multiple sources, saturation, parameter validation, and
candidate bounds. Before a camera preset can reject real frames automatically,
validation must additionally cover:

- sub-pixel positions, rotations, flux ranges, and PSF families;
- CFA phases and the supported ASI294MC Pro and ToupTek 585C profiles;
- gradients, vignetting, nebulosity, crowded fields, and image edges;
- hot pixels, cosmic rays, clipping, saturation blooms, and masks;
- comparison with an independent reference implementation and inspected data;
- deterministic results across thread counts and optimized backends.

Until those gates pass, the measurements are diagnostics for expert review and
Blink, not silent automatic rejection authority.

The current synthetic suite covers all four standard Bayer phase permutations,
complete-cell mask and non-finite propagation, compensated cell arithmetic,
partial-cell rejection, and the low-isophote truncation correction. One declared
RGGB ASI294MC Pro light also passes the complete native diagnostic path. The
ToupTek 585C comparison and independent-reference tolerances remain open gates.
