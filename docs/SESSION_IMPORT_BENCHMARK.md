# FITS session import benchmark

## Purpose

`aether-import-bench` measures the production directory-to-manifest importer in
an optimized build. It keeps all integrity checks enabled and separates the
work into initial FITS-header parsing, complete-source SHA-256 fingerprinting,
post-hash header verification, source-record finalization, manifest assembly,
and filesystem/orchestration overhead.

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
portable performance promise. The filesystem-and-overhead value is the
saturating remainder after independently timed stages; it includes directory
enumeration, file opening, metadata reads, sorting, and timer boundaries.

## Usage

Run an optimized build because debug-mode parsing and hashing are not
representative:

```shell
cargo run --release -p aether-inspect --bin aether-import-bench -- \
  --passes 3 /path/to/private/fits-corpus
```

`--passes` accepts values from 1 through 20 and defaults to 3. Later passes may
benefit from the operating-system file cache, so every pass remains visible.
Input files are read but never modified.

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
