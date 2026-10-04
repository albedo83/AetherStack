# FITS session import benchmark

## Purpose

`aether-import-bench` measures the production directory-to-manifest importer in
an optimized build. It keeps all integrity checks enabled and separates the
work into source-analysis wall time, aggregate worker time for initial FITS
headers, complete-source SHA-256 fingerprinting, post-hash header verification
and source-record finalization, plus manifest assembly and
filesystem/orchestration overhead.

The post-hash header pass is intentional. AetherStack compares the parsed header
and image layout before and after fingerprinting so a source that changes during
import cannot silently enter a manifest.

## Privacy and interpretation

The benchmark prints only aggregate file counts, byte counts, failure counts,
unassigned counts, and monotonic durations. It never prints the corpus path,
file names, digests, header values, normalized metadata, or pixels. It verifies
that the complete canonical manifest identity and aggregate evidence remain
stable between passes without disclosing that identity.

Timings are operational measurements, not scientific provenance and not a
portable performance promise. Under parallel analysis, aggregate worker times
can exceed wall time and must not be added to it. The filesystem-and-overhead
value is the saturating remainder after source-analysis wall time and manifest
assembly; it includes directory enumeration, file opening, metadata reads,
sorting, and timer boundaries.

## Usage

Run an optimized build because debug-mode parsing and hashing are not
representative:

```shell
cargo run --release -p aether-inspect --bin aether-import-bench -- \
  --passes 3 --jobs 1 /path/to/private/fits-corpus
```

`--passes` accepts values from 1 through 20 and defaults to 3. `--jobs` accepts
values from 1 through 32 and defaults to conservative sequential analysis.
Later passes may benefit from the operating-system file cache, so every pass
remains visible. Input files are read but never modified.

## Optimization rule

An optimization is accepted only when the relevant stage improves on repeated
measurements while manifest identity, failure accounting, classification,
source fingerprints, and mutation detection remain unchanged. Stage telemetry
exists to prevent speculative changes from weakening those guarantees.

## Initial ASI294MC measurement

The initial priority-camera run on 4 October 2026 covered 135 regular FITS
sources and 3,158,611,200 bytes per pass. All canonical manifest evidence
remained stable across three passes.

| Pass | Total seconds | SHA-256 seconds | SHA-256 share |
| ---: | ---: | ---: | ---: |
| 1 | 1.929766 | 1.876635 | 97.25% |
| 2 | 1.765804 | 1.720647 | 97.44% |
| 3 | 1.679204 | 1.635455 | 97.40% |

The median pass therefore identifies complete-source hashing—not traversal,
header parsing, classification, or manifest assembly—as the only meaningful
optimization target for this cached local-storage measurement. Parallel source
analysis must remain bounded and opt-in until it proves beneficial on both warm
and cold storage without changing canonical output.

## Bounded parallel analysis

The production importer can analyze independent sources concurrently while
keeping directory traversal, byte-budget admission, failure retention, and
final manifest construction deterministic. Open files are limited to one batch
of at most `--jobs` entries, and all results re-enter canonical discovery order.
The default library profile remains sequential.

An exploratory sweep showed useful scaling through eight workers. A follow-up
alternating comparison then ran two three-pass one-worker series and two
three-pass eight-worker series on the same warm corpus. Every pass reproduced
identical canonical manifest evidence.

| Jobs | Combined six-pass median | Speed-up over 1 job |
| ---: | ---: | ---: |
| 1 | 1.807751 s | 1.00× |
| 8 | 0.732811 s | 2.47× |

The desktop interactive importer therefore selects the host's available
parallelism capped at eight workers. Diagnostics expose this worker limit beside
the exact byte count and measured native scan time. The explicit library and
benchmark defaults remain one worker for callers that prefer sequential storage
access or need to characterize another device first.
