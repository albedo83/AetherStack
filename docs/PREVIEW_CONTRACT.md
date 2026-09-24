# Bounded preview contract

Preview images are display artifacts. They never replace source pixels, feed a
quality metric, or become inputs to calibration, registration, normalization, or
integration. Every cache or UI boundary must preserve that separation.

## Deterministic reduction

The initial scalar preview uses power-of-two box reduction. Level `n` maps each
output pixel to a non-overlapping `2^n` square source block; edge blocks retain
their actual dimensions. Finite usable source values are combined in canonical
plane-row-column order with compensated `f64` accumulation. The result is
therefore bit-identical across configured FITS I/O chunk sizes.

Undefined integer samples, stored or scaled non-finite samples, and future
quality-mask exclusions never enter the mean. Every reduced pixel records:

- its deterministic mean, or NaN when it has no valid support;
- the valid source-sample count;
- the excluded source-sample count;
- the union of exclusion reason flags.

No-support pixels additionally carry the missing flag. A partially supported
pixel retains its value and remains distinguishable through the support arrays;
the renderer does not silently treat it as fully valid.

## Resource bounds

The caller supplies a maximum output-pixel count and FITS I/O chunk size, both
validated against global safety bounds. Output dimensions and every allocation
use checked arithmetic. Source rows wider than the I/O budget are read in
left-to-right chunks; otherwise consecutive complete rows are batched. This
preserves canonical accumulation order without allocating the complete FITS
image.

The level selector chooses the finest power-of-two level satisfying maximum
width, maximum height, and maximum pixel count together. It fails explicitly if
the implemented level range cannot satisfy the request.

## Display mapping

Grayscale RGBA mapping accepts only a validated, resolved display transform from
the review model. Black point, white point, midtone, transfer-function kind, and
transform schema version remain explicit. Linear, midtone, and asinh mappings
are display-only operations.

Pixels without valid support may be transparent for a separate accessible
overlay or use a high-contrast checkerboard. The scalar values and support
evidence remain unchanged. A production UI must pair any color cue with an icon,
pattern, label, or inspectable status.

## Current scope and release gates

The implemented path reads one selected plane from a 2D or 3D primary FITS image
and produces scalar or grayscale output. It does not yet demosaic a Bayer image,
compose RGB planes, resolve a shared automatic stretch, cache pyramid levels, or
stream tiles to a desktop surface. Those capabilities require versioned cache
keys and camera-aware tests before they are presented as complete.

Synthetic tests cover edge support, non-finite exclusion, no-support blocks,
chunk-size invariance, safety limits, automatic level selection, exact linear
mapping, missing-pixel presentation, and finite nonlinear transfer endpoints.
A representative 4144 by 2822 ASI294MC Pro light also reduced to 1036 by 706 at
the automatically selected level with all 11,694,368 source samples accounted
for. Real ASI294MC Pro and ToupTek 585C validation remains required for color
preview presets.
