# Runtime contracts

The `aether-runtime` crate defines execution behavior shared by future pipeline
stages. It intentionally contains no scheduler and no scientific algorithm. The
contracts are small enough to test exhaustively before concurrency is introduced
into calibration, registration, and integration code.

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
