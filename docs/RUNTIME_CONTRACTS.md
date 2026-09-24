# Runtime contracts

The `aether-runtime` crate defines execution behavior shared by pipeline stages
and owns the first narrow strict CPU orchestration path. The reusable contracts
remain small enough to test exhaustively before concurrency is introduced into
calibration, registration, and integration code.

## Cooperative cancellation

`CancellationToken` is a cloneable view of one monotonic atomic flag. The first
call to `cancel` changes the state permanently; later calls are idempotent.
Workers call `checkpoint` between bounded units such as tiles, rows, or input
frames. An algorithm must document the maximum work performed between
checkpoints. Source files remain immutable, and a cancelled stage must not
publish a partially written artifact.

Cancellation is cooperative rather than asynchronous. The runtime never tears a
thread down while it owns buffers or output state. Acquire/release atomic
ordering makes state published before a cancellation request visible to workers
that observe it.

## Progress events

`ProgressEvent` is a validated snapshot with:

- a positive, monotonically allocated sequence number;
- a canonical stage identifier;
- a lifecycle state;
- completed units and an optional positive total;
- a stable code only for failed or cancelled terminal states.

Stage identifiers and terminal codes use a restricted lowercase ASCII form so
they remain portable in JSON, command-line output, logs, and future IPC. Counts
are integers rather than percentages. This avoids rounding ambiguity and lets a
client calculate its own presentation.

The constructor and JSON decoder enforce the same invariants. Started stages
have completed zero units, progress never exceeds a known total, and a completed
stage reaches its known total exactly. Unknown totals remain valid for discovery
or streaming stages.

`ProgressSequence` assigns unique values atomically. Invalid events do not
consume sequence numbers. Transport adapters are responsible for serializing
delivery if they require observation order to match sequence order.
The scheduler that owns a stage must also prevent regressions in completed units
and must emit at most one terminal state; those stream-level rules cannot be
decided from an isolated snapshot.

## Memory accounting

`MemoryBudget` is shared across workers. `try_reserve` atomically checks and
reserves a positive byte count before an allocation is attempted. The returned
`MemoryReservation` is non-cloneable and releases its bytes through `Drop`.

Stages retain a reservation for the complete lifetime of the corresponding
allocation. Reservations cover owned pixel buffers, masks, temporary transform
storage, and other material memory. Small fixed control structures may be
accounted separately, but that policy must be explicit at the scheduler
boundary.

The budget reports current, available, and peak reserved bytes. Concurrent tests
hold reservations across synchronization barriers to verify that the configured
limit is never exceeded and that capacity is recovered after every guard is
dropped.

These reservations are accounting contracts, not allocator hooks. Allocation
failure remains a separate typed error even after a reservation succeeds.

## Strict CPU vertical slice

`run_strict_pipeline` connects the existing reference components without adding
a second implementation of their numerical rules. One request contains a stable
ordered signal list, one dark master, one already normalized flat master,
explicit calibration parameters, validated output provenance, an output path,
and the FITS acceptance policy. Every `PipelineSource` pairs its local path with
the exact byte length and SHA-256 previously recorded by session ingestion.

The run performs these bounded steps:

1. hash every complete input and require its manifest fingerprint;
2. inspect every FITS input and require exactly equal checked dimensions;
3. reserve the complete logical pixel working set;
4. traverse spatial-plane tiles in deterministic order;
5. reopen and read one signal at a time, limiting live file descriptors;
6. calculate `(signal - dark) / flat` using the strict `f64` kernel;
7. integrate calibrated tiles using the strict mean oracle in signal-list order;
8. assemble one bounded full-width tile-row band at a time and append it to a
   private FITS stream in canonical planar order;
9. calculate exact three-pass complete-image statistics using the band masks for
   first-pass accounting and bounded staged-output readback for later passes;
10. hash every complete input again to detect changes during processing;
11. publish verified checkpoints, when enabled, and then publish one create-new
    binary64 FITS product with validated provenance and embedded `DATASUM` and
    `CHECKSUM` integrity cards.

The source count and algorithm identifier in provenance must exactly describe
the executed operation. The fixed identifier is `strict-mean-v1`. Errors name an
input role and signal index but deliberately omit filesystem paths.

Progress starts before input inspection. The total becomes known after image
dimensions establish the tile count. A successful run emits running events for
each completed tile, the statistics pass, final source revalidation, and
optional checkpoint publication, followed by one completed event after output
publication. Failures and cancellation emit stable lowercase codes.

Cancellation checkpoints occur before inspection, between inspected inputs,
between signal reads, between tiles, between staged-output readback chunks,
throughout final source revalidation, between checkpoint publications, and
before final publication. A fingerprint mismatch identifies only the source
role and signal index; local paths are not copied into errors or output
metadata. The atomic writer is not interrupted after publication begins; it
exposes either no new destination or one complete synchronized FITS stream.

The final integrated image is never retained in memory. The largest output
allocation is one full-width band whose height is bounded by the configured tile
height. Signal working storage remains bounded by tile area and signal count.
Two fixed readback buffers support the mean and variance passes, and the FITS
publisher owns one fixed output buffer. The logical reservation is therefore
independent of total image height once the image is taller than one band. Header
memory is bounded separately by `HeaderReadOptions`.

## Integrated-tile checkpoints

`StrictPipelineRequest::with_cache` enables verified checkpoints for integrated
tiles. The `strict-mean-tile-v1` operation key includes the manifest digest,
group and algorithm identifiers, flat threshold, complete image dimensions,
tile coordinates, FITS acceptance mode, and the ordered fingerprints of every
signal and both masters. Local paths and the destination path are excluded, so
moving an unchanged session does not invalidate or disclose it. Tests require a
scientific parameter or source fingerprint change to produce a different key.

The tile payload has a versioned fixed header followed by each binary64 value
and its exact quality-flag byte in planar order. Cache-container length and
SHA-256 validation occurs first; the runtime then independently validates the
tile payload's magic, version, dimensions, and sample count.

Existing verified checkpoints are loaded before FITS tile reads and counted in
the result. Newly computed checkpoints are not published immediately. Their
exact payloads are appended to a private sequential spool while calculation
proceeds. The pipeline finishes the staged FITS stream, calculates its
statistics, and revalidates every complete source before replaying the spool
into the immutable cache. A changed source therefore cannot poison the expected
operation key. The spool is removed on success, cancellation, or failure.

Checkpoint publication is itself cancellable between tiles. A cancellation
after checkpoint publication but before final FITS publication leaves no output
and preserves valid restart material. The restart test requires every tile to be
reused and the resulting FITS bytes to equal an uninterrupted no-cache run.
Malformed or corrupt cache entries stop the run explicitly; they are never
treated as misses and are never deleted automatically.
