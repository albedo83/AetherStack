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

## Desktop transport

The initial native bridge returns a bounded PNG through Tauri's raw binary IPC
response. Preview bytes are therefore not expanded into a JSON number array or
base64 string. Rust owns FITS parsing, level selection, scalar reduction,
display mapping, and PNG encoding; the TypeScript layer owns only the temporary
browser object URL used for presentation.

Every completed object URL is paired with the stable frame identity that
requested it. The presenter displays it only when that identity still matches
the selected frame. A delayed response from an earlier Blink request cannot be
shown under the current filename. Releasing a browser URL is idempotent and does
not alter scientific cache state.

The native command enforces a two-megapixel desktop limit and a fixed 256
Ki-sample decode chunk even when a caller requests larger dimensions. It accepts
only an explicit validated display transform. A separate Rust command estimates
the versioned `aether-preview-auto-stretch-v1` transform from the selected
reference frame. The estimator ignores unsupported pixels, uses the median and
Gaussian-consistent MAD for the black point, limits the white point to the
99.95th percentile, and maps the measured background to 25% gray. Its median,
scaled MAD, quantile, and exact support count remain inspectable. The presenter
locks that explicit transform for subsequent frames in the active Blink role.

## Current scope and release gates

The implemented path reads one selected plane from a 2D or 3D primary FITS image
and produces scalar, grayscale, or native PNG output. The desktop Review surface
accepts only identity-bound preview URLs. Native directory selection now runs
the bounded session scanner, derives stable frame identities, preserves role
conflicts, loads a real reference-stretched preview, offers fitted and actual
preview-pixel presentation, and delegates metric-table sorting to the Rust
review model. The renderer does not yet demosaic a Bayer image, compose RGB
planes, estimate a multi-frame aggregate stretch, cache pyramid levels, or
stream viewport tiles. Those capabilities require versioned cache keys and
camera-aware tests before they are presented as complete.

Synthetic tests cover edge support, non-finite exclusion, no-support blocks,
chunk-size invariance, safety limits, automatic level selection, exact linear
mapping, missing-pixel presentation, finite nonlinear transfer endpoints, PNG
dimensions, robust automatic-stretch behavior, invalid native transform
rejection, cancelled directory selection, and stale frontend identity rejection.
A representative 4144 by 2822 ASI294MC Pro light also reduced to 1036 by 706 at
the automatically selected level with all 11,694,368 source samples accounted
for. The complete 135-frame local ASI294MC Pro comparison session also imports
under its explicit directory-preference policy, retains the 25 known flat/header
conflicts, and produces a bounded PNG from a real light. ToupTek 585C validation
and color preview presets remain required.
