# Frame review and Blink contract

The frame reviewer combines objective quality measurements with fast visual
comparison. It assists a decision; it never hides why a frame was accepted or
rejected and never changes the deterministic processing order merely because a
table was sorted.

## Review states and evidence

Each light has one explicit state: undecided, accepted, or rejected. A manual
rejection stores a stable reason code and optional note. An automatic rule stores
the metric version, comparator, threshold, measured value, and missing-value
behavior. Changing a threshold recomputes the proposed state but does not erase
manual decisions. Bulk changes are previewed and undoable until the run plan is
sealed.

Pixel minimum, maximum, mean, dispersion, and invalid counts are useful
diagnostics but are not quality scores. Ranking requires versioned astronomical
metrics with declared units and tested validity domains:

- robust background location and noise;
- detected and usable star counts;
- median stellar FWHM and eccentricity with dispersion/support;
- saturation and clipped-area fractions;
- a signal-to-noise proxy and documented signal weight;
- registration residuals and common-footprint coverage when available.

No composite score exists until its expression, normalization, missing-value
policy, and direction are visible. The default table keeps the individual
metrics available beside any weight.

## Viewer fidelity

The viewer keeps scientific pixels immutable. Debayering, channel combination,
black and white points, transfer function, color balance, zoom, rotation, and
mirroring belong to a display transform and never modify source data or quality
measurements. The UI identifies raw-CFA, debayered-preview, monochrome, and
integrated-product views.

Automatic stretch resolves to concrete black point, white point, midtone, and
algorithm version. Clipped shadows and highlights can be overlaid independently.
NaN, infinity, `BLANK`, saturation, and quality-mask states use distinguishable
overlays and remain inspectable at pixel level.

Large images use bounded tiles and cached preview levels. A preview cache key
includes the source fingerprint, plane/channel interpretation, debayer method,
orientation, reduction filter, and display-transform version. Cached previews
are display artifacts and never become scientific pipeline inputs.

## Blink mode

Blink switches among selected frames while preserving the same viewport and
display transform. This lock is mandatory: independent auto-stretches can hide
background changes, transparency loss, gradients, or clipping. Users may choose
one shared auto-stretch derived from the selected set, or freeze the current
manual transform. The chosen mode remains visible.

Frames are prefetched within a memory budget, but the current frame is never
replaced by an unresolved or partially decoded preview. Slow I/O reduces the
cadence instead of dropping silently to a lower-fidelity transform. Playback can
move forward, backward, bounce, or follow a manually selected comparison set.

Keyboard actions cover previous/next frame, play/pause, accept, reject, undo,
zoom reset, fit, and overlay toggles. Rejection never advances without first
committing the visible frame identity. Screen readers announce file position,
review state, important metrics, and playback state; status is never encoded by
color alone.

## Sorting and reproducibility

Table sorting is a view operation. Ties use a stable source identifier and do
not alter integration order. Filters distinguish missing metrics from numerical
zero. Selecting a reference, rejecting a frame, or explicitly changing reduction
order is a separate recorded action.

The sealed session plan records accepted sources, rejection reasons, metric and
expression versions, thresholds, and the selected reference. The output
provenance binds to that plan rather than to ephemeral viewer state.

## Release gates

The reviewer is not complete until synthetic PSFs validate star position, flux,
FWHM, and eccentricity across noise, saturation, CFA phase, image edges, and
crowded fields. Blink tests must prove transform locking, exact frame identity,
undo behavior, bounded cache use, keyboard-only operation, enlarged-text layout,
and non-color status communication. Real-camera validation covers ASI294MC Pro
and ToupTek 585C before their quality presets are enabled.
