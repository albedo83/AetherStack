# Master calibration planning contract

The master planner converts exact session groups into a deterministic,
versioned description of the bias, dark, and flat masters to build. It plans
scientific dependencies without touching pixels. The runtime now consumes that
exact plan for strict-mean bias and dark integration and pedestal-corrected,
exact-median-normalized flat integration. Statistical rejection, uncertainty
propagation, and defect-map generation remain later Phase 4 work.

## Roles remain distinct

Bias, dark, flat, and light are acquisition roles, not folder conventions. A
short exposure made with the sensor covered remains a dark. It can be selected
as the pedestal calibrator for a flat without being reclassified as a
"dark-flat". A genuine bias remains a separate bias group and produces its own
master.

Every bias, dark, and flat group produces one planned master product. Light and
unknown groups do not. The plan is bound to the canonical session manifest by
its SHA-256 digest, sorted by stable group identifier, encoded as bounded JSON,
and tagged with `schema_version = 1`.

`canonical_sha256` hashes the exact deterministic pretty JSON bytes, including
their final newline. Every emitted master must store this digest in its FITS
`AETHPLN` card. This makes changes to matching policy, tolerances, selections,
candidate evidence, or group identifiers visible in product provenance and
runtime cache identities.

## Exclusive flat pedestal correction

A flat must have exactly one of these states:

- one matched short-exposure dark;
- one true bias;
- an unresolved blocking reason.

The representation cannot select a dark and a bias together. This prevents the
common error of subtracting a bias separately from a dark that already contains
the detector pedestal.

The caller must select one explicit policy:

- require a matched dark;
- require a bias;
- prefer a matched dark, then use a bias only when no dark is compatible.

An ambiguous dark result never falls back to a bias. Ambiguity requires a user
decision because fallback would hide two equally ranked valid darks.

## Conservative matching

Camera model, FITS-order image axes, gain, digital offset, binning, and CFA
pattern must all be present and exactly equal. Filter is intentionally ignored
for bias and dark matching. A dark exposure must lie inside the caller-supplied
inclusive tolerance in seconds.

Measured sensor temperature is used when the flat declares it. The candidate
must then also declare a measured temperature; a set point is not silently
substituted. When the flat has no measured temperature, both groups must have a
set point. The absolute delta must lie inside the caller-supplied inclusive
tolerance in degrees Celsius.

Compatible darks are ranked first by absolute exposure delta and then by
absolute temperature delta. Compatible biases are ranked by absolute
temperature delta. Lexical group order makes output deterministic but never
breaks a scientific tie: all candidates sharing the best numerical score are
reported as ambiguous.

## Diagnostics and bounds

For every flat, the plan retains each bias and dark candidate as compatible or
rejected. Rejections report every failed field as missing, different, or outside
tolerance. This evidence is intended to drive both an advanced association table
and a calibration dependency graph.

JSON decoding is limited to 16 MiB. Construction retains at most 100,000
flat-to-pedestal evaluations and fails before allocation when that bound would be
exceeded. Major plan vectors use fallible exact reservations. A plan is ready
only when every flat has one unambiguous pedestal source.

## Transactional execution

Execution revalidates the manifest digest, product roles, selected dependencies,
source fingerprints, physical session root, and destination directory. Bias and
dark dependencies run before flats even though the serialized plan remains in
canonical group order. Every product is tiled under one explicit memory budget,
written and read back in a private sibling directory, and tagged with both the
manifest and plan digests.

No public product is created while calculations are running. Once the complete
set has succeeded, create-new hard links publish each already complete FITS file
without overwriting an existing path. A publication or cancellation failure
rolls back only links created by that execution, and the output directory is
durably synchronized where the platform supports directory synchronization.
Multi-file publication is not claimed to be one indivisible filesystem
operation: a concurrent observer can see a short prefix while links are being
created, but a failed call does not intentionally leave a partial product set.

## Validation coverage

Unit tests cover dark preference, bias fallback, dark-only and bias-only policy,
inclusive tolerance boundaries, missing metadata, cross-camera and geometry
mismatches, measured-versus-set-point temperature behavior, dark and bias ties,
manifest binding, deterministic JSON round trips, schema and size rejection,
candidate-matrix bounds, and rejection of a serialized selection that
contradicts its policy. Runtime tests additionally cover dependency-first
execution, provenance binding, normalized pixel recovery, destination
preflight, cancellation, and cleanup after a late flat failure.
