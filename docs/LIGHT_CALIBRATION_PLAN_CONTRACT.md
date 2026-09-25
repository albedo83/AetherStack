# Light calibration plan contract

## Purpose

The light calibration planner assigns exactly one Dark master and one normalized
Flat master to every complete Light group. It is an evidence document, not an
execution shortcut: compatible and rejected candidates remain serialized beside
the selected associations.

The plan is bound to both upstream scientific inputs:

- the canonical session-manifest SHA-256;
- the canonical master-construction-plan SHA-256.

Changing source metadata, grouping, flat-pedestal policy, or any master-plan
candidate evidence therefore changes the light plan identity.

## Dark matching

A Dark candidate requires exact agreement for:

- camera model;
- FITS axes;
- exposure duration;
- gain and offset;
- binning;
- CFA pattern and phase for a color or unknown sensor.

Filter is deliberately ignored for Darks. Exposure is exact because AetherStack
does not yet implement or claim a dark-scaling algorithm. A nearly equal
exposure remains an explicit mismatch instead of being silently scaled or
subtracted unchanged.

Temperature uses the measured sensor value when the Light has one. Otherwise,
the set point is used. The configured absolute tolerance is finite,
non-negative, inclusive, and serialized in the plan. When several compatible
Darks exist, the smallest temperature delta wins. An equal best score is
ambiguous and blocks execution.

## Flat matching

A normalized Flat candidate requires exact agreement for:

- camera model;
- FITS axes;
- gain and offset;
- binning;
- filter;
- CFA pattern and phase for a color or unknown sensor.

Flat exposure and temperature are not compared after pedestal correction,
integration, and global normalization. If multiple normalized Flat groups match
the same Light, the plan remains ambiguous. The planner does not invent a
ranking by filename, discovery order, exposure, or frame count.

## Missing metadata

Missing required Light metadata has a distinct blocking state for Dark and Flat
selection. Role-specific requirements do not leak across associations: a
missing filter cannot reject a Dark, and a missing exposure cannot reject a
Flat. Every rejected candidate retains stable field and reason enums for the UI
and machine-readable diagnostics.

For known color cameras, CFA evidence is mandatory. A known monochrome camera
does not require a Bayer pattern. An unprofiled camera without CFA evidence
remains conservative because its sensor type is unknown.

## Serialization and bounds

Schema version 1 uses deterministic pretty JSON with one terminating newline.
The canonical SHA-256 hashes those exact bytes. Decoding rejects unknown fields,
unsupported versions, malformed or non-canonical digests, duplicated products,
role-inconsistent evidence, and a selected association that is not the unique
best compatible candidate.

Input JSON is limited to 16 MiB. The retained candidate matrix is limited to
100,000 Light-to-master evaluations, and major plan vectors use fallible exact
reservations. Exceeding a bound is a typed failure, never silent truncation.

## Readiness

A product is ready only when both associations are resolved. The complete plan
is ready only when every Light product is ready. Runtime execution must still
revalidate the manifest, master plan, light plan, source fingerprints, generated
master identities, filesystem roots, and output destinations before processing.
