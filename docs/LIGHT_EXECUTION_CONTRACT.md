# Light execution contract

The Light executor consumes one immutable session manifest, its canonical
master-construction plan, and the canonical Light association plan derived from
both. It produces one calibrated and integrated FITS image per Light group. It
does not select masters, scale darks, debayer, register, normalize backgrounds,
or silently repair an incomplete plan.

## Accepted graph

Construction regenerates both plans from their serialized options and rejects
any digest or content mismatch. Every Light product must select exactly one Dark
and one normalized Flat product present in the master plan. An empty plan is not
an executable operation.

Only selected generated masters are opened. Their regular-file status and FITS
provenance must match all of the following before pixel processing begins:

- the canonical session-manifest SHA-256;
- the canonical master-plan SHA-256;
- the selected source-group identifier;
- the role-specific versioned master algorithm;
- the exact number of raw frames represented by the master.

This prevents a correctly named but stale, foreign, or manually replaced master
from entering calibration.

## Scientific operation

For every clear finite sample, the strict reference operation is

```text
(light - dark) / normalized_flat
```

Calibration and stable-order mean integration use binary64 arithmetic and the
mask rules in the calibration and integration contracts. A caller supplies the
flat-divisor floor and spatial tile dimensions explicitly. Tile dimensions are
execution parameters and do not alter strict output bytes.

The initial executor publishes an integrated calibrated product for each Light
group. Individual calibrated-frame export is a separate future product contract
and must not be inferred from this operation.

## Bounded execution

Raw frames are read by tiles through the shared memory budget. Cancellation is
checked before master loading, between products, inside each strict pipeline,
before source revalidation, and during publication. Progress identifies the
canonical product index, product count, group identifier, and typed pipeline
stage.

The executor fingerprints every raw Light and selected generated master again
after all computations and before publication. Any missing, unreadable, or
changed input fails the complete transaction.

## Publication boundary

Every output is first completed and validated in a private sibling staging
directory. All public destination names are checked before processing, and
existing files are never replaced. Only after every product and final input
revalidation succeeds are private files exposed through create-new hard links.
If cancellation or publication fails partway through, links created by that
transaction are removed in reverse order. Staging is removed by its owner on
every return path.

The product filename is `integrated-light-<group-id>.fits`. Its `AETHMAN` card
stores the manifest digest and `AETHPLN` stores the canonical Light-plan digest.
The Light plan itself stores the canonical master-plan digest, providing a
complete transitive provenance chain without embedding local paths.

## Covered failure cases

Unit tests cover numerical Dark/Flat calibration and mean integration, exact
provenance cards, stale-master rejection, existing-destination preservation,
early cancellation cleanup, private-staging cleanup, and rejection of a
structurally valid but non-canonical Light plan.
