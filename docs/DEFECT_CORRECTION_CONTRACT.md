# Detector defect correction contract

Detector defects are scientific evidence, not display blemishes. AetherStack
therefore keeps the immutable defect map separate from the corrected image and
accounts for every requested replacement.

## Local robust detection

The strict CPU oracle compares each finite, unmasked detector sample with a
local median. Local spread is the median absolute deviation multiplied by
`1.482602218505602`; positive and negative limits are independent. A separate
absolute-residual floor prevents zero-MAD neighbourhoods from classifying
rounding noise.

The neighbourhood is explicit and bounded:

- radius is between one and eight lattice steps;
- stride one compares adjacent monochrome or plane-local samples;
- stride two compares only the same Bayer phase;
- the minimum usable-neighbour count must fit the selected radius.

Masked and non-finite centres are reported as unavailable and are never silently
reclassified. Masked and non-finite neighbours do not influence the estimator.
Large-scale dark-current or flat-field structure is not removed before the local
comparison; locality is the protection against treating that structure as a
detector defect.

## Conservative replacement

Correction reads every neighbour from the immutable input. Earlier corrections
can therefore never alter a later result. Mapped samples are replaced by the
exact median of finite, unmasked, non-defective samples on the selected lattice.
For CFA data, stride two is mandatory at the orchestration boundary so measured
colour phases cannot contaminate one another.

A source sample carrying saturation, missing-data, rejection, invalidity, or an
unknown future mask bit is not repaired. If clean support is insufficient, the
output becomes canonical NaN and retains the HOT or COLD reason with MISSING.
After a successful replacement the corrected image is usable, while the returned
defect map permanently retains the original HOT or COLD evidence.

## Evidence

Detection reports examined, unsupported, unavailable, hot, and cold totals.
Correction reports requested, corrected, unsupported, and source-mask-blocked
totals. These counters must partition their respective decisions without
overflow before native FITS publication is enabled.

Independent master-derived maps combine by set union. Contradictory HOT and COLD
evidence is retained rather than resolved by precedence, and merge evidence
reports the conflict count. The map API rejects empty flags and every non-defect
mask reason.

The implementation deliberately does not guess camera-specific thresholds,
repair complete rows or columns, or publish a default profile. Explicit
parameters, source fingerprints, and the map-producing policy are bound into
output provenance. A profile may be called automatic only after representative
ASI294MC and ToupTek 585C validation fixes defensible defaults.

## Native FITS transaction

The runtime now binds role-tagged dark and normalized-flat controls into one
path-free SHA-256 parameter seal. Reference order cannot change this identity;
duplicate roles are rejected. Each reference is fingerprinted before analysis,
requires strict FITS acceptance and valid `DATASUM` plus `CHECKSUM`, and is
fingerprinted again after processing.

Memory is reserved before pixel decoding. The published full-frame model covers
the FITS status vector, immutable image samples and masks, accumulated,
temporary, and cloned defect maps, robust-statistics scratch, corrected image,
transport image, and both output buffers. The deterministic peaks are
234,020,736 bytes for a 4,144 × 2,822 ASI294MC plane and 166,021,376 bytes for a
3,840 × 2,160 ToupTek 585C plane.

Map transport uses exact binary64 integer values `0`, `4`, `8`, and `12`.
Decoding rejects masked, non-finite, fractional, negative, out-of-range, and
foreign-bit samples. Corrected science and its map are privately completed,
checksum-verified, source-revalidated, and exposed by one create-new product-set
publication. A companion collision leaves the corrected destination absent and
does not alter the existing file.

## Desktop inspection and queues

The desktop retains per-reference detection evidence and exact unique merged
HOT, COLD, and conflict counts. The companion preview is categorical: reduction
ORs source bits, clear samples are black, HOT is orange, COLD is cyan, and
combined evidence is violet. It never averages mask values or applies a display
stretch. Before and after science previews continue to share one transform
derived from the calibrated input.

One queue may process every calibrated Light whose reviewed group has both Dark
and Flat associations, or explicitly limit execution to the reviewed Light.
Rust resolves the complete scope from the current artifact registry, places the
reviewed Light first, then resumes native calibration order. It verifies every
master and lattice, assigns both output names, rejects aliases and duplicates,
and reports all existing-destination collisions before correction begins.

The preflight returns a domain-separated SHA-256 plan identity covering the
current manifest and Light plan, strict parameter seal, memory limit, ordered
source identities and indices, and both destination paths. Browser code must
reconcile exact membership and identity metadata and uses the returned order and
paths verbatim; it cannot synthesize a queue or filename. An incomplete,
foreign, blocked, duplicated, or malformed plan fails closed.

Atomicity is deliberately scoped to each Light/map pair so memory remains
bounded and successfully published pairs survive a later failure or user
cancellation. No individual pair can be partially published. The desktop shows
whole-queue progress while retaining the current pair's five native stages. Its
terminal report preserves both seals, completion count, replacement accounting,
HOT/COLD/conflict evidence, and the worst observed reserved-memory peak.

The reviewed plan also lives in native state. Each execution request must match
the plan seal and exact next index as well as its identity, group, destinations,
controls, memory limit, manifest, and Light-plan identities. Immediately before
launch, Rust rechecks that both destinations are absent in the same real,
non-symbolic directory. The native cursor advances only after atomic pair
publication, so an item cannot be skipped or replayed.

Every successful response attests the plan seal, item index, completed count,
total count, and completion flag. Desktop aggregation rejects a discontinuity.
After cancellation or failure, the current process may explicitly resume at the
first unpublished item; changing controls or replacing the session discards
that capability. Completed products are never recomputed during a resume.

Once the native cursor reaches the end, the user can export a bounded JSON
report. Rust aggregates correction and categorical-map evidence and the worst
reserved-memory peak, computes SHA-256 over canonical report bytes, and wraps
the report and digest in a versioned envelope. Publication is create-new and
never overwrites an existing file.
