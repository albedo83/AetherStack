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

The interaction model implements manual accept, reject, and clear transactions
against content-derived frame identities. A transaction is first returned as an
immutable generation-bound preview, then applied atomically. One undo operation
reverts the complete transaction. Sealing advances the generation, discards undo
history, and permanently blocks mutation.

The first automatic-rule evaluator is a separate non-mutating layer. It supports
strict thresholds for background, noise, stellar SNR, detected and usable star
counts, major-axis FWHM, and eccentricity. Scalar and count values remain
distinct, missing evidence resolves through an explicit retain-or-reject policy,
duplicate metric rules are rejected, and every rule is evaluated even after one
failure. The result is only a retain/reject proposal with complete evidence; it
cannot edit, clear, or override a manual decision. A bounded selection plan now
seals those rules and results in immutable processing order. Its versioned,
path-free canonical SHA-256 includes frame identities, exact typed measurements,
missing states, and proposals while excluding display labels and machine paths.
Changing a rule, metric, or processing position therefore changes the plan
identity. Successful quality measurements are now retained natively under the
stable frame identity and exact artifact path. The selection adapter resolves
only that stored evidence, validates typed rules, rejects missing, duplicate, or
foreign identities, restores processing order from its native review book, and
returns the plan without mutating manual state. The browser bridge transports
artifact identities and rules but contains neither measurements nor a threshold
evaluator. Custom expressions remain planned.

The desktop editor exposes one to seven simultaneous quality gates in an
expandable instrument panel. Gates can be added and removed without allowing an
empty set or duplicate metric. Each gate makes the metric, strict direction,
typed threshold, and missing-value policy visible. Metric-aware defaults keep
new controls valid while remaining explicit and editable. A native preview reports
retained and rejected totals, the canonical digest, and an `AUTO KEEP` or
`AUTO REJECT` badge beside each frame. A rejected badge names the sole failing
metric or the number of failed gates, while its accessible description lists
every failed or missing-rejected metric. These badges are recommendations only;
the manual state marker and undo history remain unchanged. Editing a rule,
changing pixel stage, switching role, importing a session, or refreshing any
quality result invalidates the previous preview and cancels stale responses.
The selected-frame evidence instrument expands the same ordered native result
into metric, measured value or explicit missing state, strict comparator,
threshold, and pass/fail outcome. It never repeats the threshold calculation.

Raw-source diagnostic quality evidence is restartable. A versioned JSON payload
records the stable frame identity, explicit source byte length and SHA-256,
quality profile, detection-plane/background/star algorithm identities, complete
accounting, and validated metrics. Its immutable cache container independently
seals payload length and SHA-256. The cache key binds the source identity and
current algorithm versions but excludes machine paths, allowing a whole session
directory to move. Import derives the frame identity again, verifies the cache
container and bounded payload, rejects unknown fields or versions, revalidates
every metric and algorithm pairing, and indexes restored evidence under the new
absolute artifact path. Missing, stale, oversized, malformed, or corrupt cache
entries are treated as absent evidence and can never enter a selection plan.
Calibrated runtime products are intentionally excluded until their own product
provenance has an equivalent durable identity.
The selected-frame quality badge distinguishes a fresh in-process measurement
from restored evidence. Restored evidence is labelled `QUALITY · RESTORED`, and
its explanatory text starts with `Verified cache`; a fresh result remains
`QUALITY · DIAGNOSTIC`. This is provenance only: both states expose the same
strictly validated native result and neither changes a review decision.
The session status separately reports restored, missing, and rejected cache
counts for eligible Bayer Lights. Missing is a normal first-run state. Rejected
means an entry was present but failed identity, container, payload, schema, or
scientific validation; it produces a warning and no evidence is installed.
Applying a preview requires a dedicated confirmation dialog. Rust rebuilds the
plan from its retained evidence under the same synchronization boundary and
requires the canonical digest to match before mutation. It converts proposals
only for undecided frames, records automatic exclusions as `quality_rules`, and
preserves every existing manual decision. The complete effective batch is one
native transaction, so one Undo restores it atomically.

The desktop adapter initializes one native review book after a successful
session import. Accept, reject, clear, and undo commands mutate that book under a
single synchronization boundary and return only identity-scoped confirmed
patches. Repeating an already-active decision is idempotent and does not consume
an undo slot. The browser disables decision actions while a transaction is in
flight and never constructs its own undo history. This history is currently
process-local; durable restart recovery must be tied to the future session plan
and provenance format rather than browser storage.

Pixel minimum, maximum, mean, dispersion, and invalid counts are useful
diagnostics but are not quality scores. Ranking requires versioned astronomical
metrics with declared units and tested validity domains:

- robust background location and noise;
- detected and usable star counts;
- median stellar FWHM and eccentricity with dispersion/support;
- saturation and clipped-area fractions;
- median background-referenced stellar SNR and a documented signal weight;
- registration residuals and common-footprint coverage when available.

No composite score exists until its expression, normalization, missing-value
policy, and direction are visible. The default table keeps the individual
metrics available beside any weight.

The strict quality estimator now reports median background-referenced SNR over
the same unsaturated stellar population used for aggregate FWHM and
eccentricity. Missing stellar support remains `None`; it is never displayed as
zero or admitted to the positive-only weight expression.

The strict backend implements global median/MAD background clipping and
local-maximum stellar moments for a prepared monochrome detection plane. The
desktop may explicitly measure declared standard Bayer lights through the
versioned complete-cell transform in
[`QUALITY_CONTRACT.md`](QUALITY_CONTRACT.md). Results remain visibly diagnostic
and cannot trigger automatic rejection until the evaluator is connected through
a previewed native transaction. Automatic camera presets remain disabled
until both ASI294MC Pro and ToupTek 585C validation satisfy every release gate.

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

The initial backend now produces deterministic power-of-two scalar reductions
directly from bounded FITS regions and maps them through a locked grayscale
display transform. It records valid and excluded support for every output pixel.
Debayered/RGB composition, shared automatic-stretch resolution, persistent
preview caching, and desktop tile transport remain release gates described in
[`PREVIEW_CONTRACT.md`](PREVIEW_CONTRACT.md).

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

The desktop adapter prefetches a symmetric neighbourhood in deterministic order,
favoring the next playback frame. Equal work is coalesced, foreground selection
can join an in-flight request, and a session or role change invalidates its whole
generation. Every late or uncached browser resource is revoked; accepted entries
remain inside the five-entry and 32 MiB encoded-artifact cache bounds.

Diagnostic quality measurement can run for one declared CFA light, one
calibrated planar RGB light, or every eligible light in the active view. CFA
frames use complete-cell phase-neutral detection; RGB frames use linked linear
Rec. 709 luminance. Batch measurement is deliberately serial:
only one immutable FITS frame and its exact statistical scratch are active at a
time. Switching frame type cancels the batch generation without discarding
already completed, identity-keyed measurements. The action reports processed
and total frame counts; each row independently retains ready or failed status.

Keyboard actions cover previous/next frame, play/pause, accept, reject, undo,
zoom reset, fit, and overlay toggles. Rejection never advances without first
committing the visible frame identity. Screen readers announce file position,
review state, important metrics, and playback state; status is never encoded by
color alone.

The initial desktop review surface exposes `A` for accept, `R` to open the
mandatory rejection-reason dialog, `C` to clear a decision, and platform undo
(`Command+Z` or `Control+Z`). The same commands remain visible buttons with
`aria-keyshortcuts`; shortcuts are ignored during text entry, key repeat,
imports, pending transactions, and modal dialogs.

The initial Blink state machine separates the visible frame from a pending
preview request. A decoded preview becomes current only when its stable identity
exactly matches the pending identity. Stale renderer responses leave both states
unchanged. Pausing cancels an unresolved timer request. Playback mode or the
shared display lock cannot change under a pending request, so one preview can
never be displayed under another frame's label or transform.

## Sorting and reproducibility

Table sorting is a view operation. Ties use a stable source identifier and do
not alter integration order. Filters distinguish missing metrics from numerical
zero. Selecting a reference, rejecting a frame, or explicitly changing reduction
order is a separate recorded action.

The sealed session plan records accepted sources, rejection reasons, metric and
expression versions, thresholds, and the selected reference. The output
provenance binds to that plan rather than to ephemeral viewer state.

The backend review model implements processing-order, label, review-state,
background, noise, star-count, FWHM, and eccentricity sorts. Missing placement is
explicit and independent of ascending or descending direction. Every sort
returns a new list of identities and cannot mutate processing order.

## Release gates

The reviewer is not complete until synthetic PSFs validate star position, flux,
FWHM, and eccentricity across noise, saturation, CFA phase, image edges, and
crowded fields. Blink tests must prove transform locking, exact frame identity,
undo behavior, bounded cache use, keyboard-only operation, enlarged-text layout,
and non-color status communication. Real-camera validation covers ASI294MC Pro
and ToupTek 585C before their quality presets are enabled.
