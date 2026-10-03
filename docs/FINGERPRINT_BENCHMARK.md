# FITS fingerprint throughput benchmark

## Purpose

AetherStack revalidates immutable source fingerprints before publication and
when a user explicitly reconnects an archived integration report to local FITS
files. These reads must remain bounded, cancellable in the desktop workflow,
and fast enough that integrity evidence does not become a reason to weaken the
scientific contract.

`aether-fingerprint-bench` measures the same public 64 KiB streaming SHA-256
primitive used by the desktop source verifier. It is an opt-in engineering tool,
not a product command and not a substitute for an end-to-end pipeline profile.

## Privacy and safety

The benchmark:

- never follows symbolic links;
- accepts only regular `.fits`, `.fit`, and `.fts` files;
- stores only in-memory byte lengths and SHA-256 values between passes;
- prints aggregate counts, byte totals, elapsed time, and throughput only;
- never prints the corpus path, source file names, digests, header metadata, or
  pixel content;
- fails if any byte length or fingerprint changes between passes.

Acquisition files remain excluded by `.gitignore`. Benchmark output should be
reviewed before publication even though the default report is path-private.

## Usage

Run an optimized build because debug-mode hashing is not representative:

```shell
cargo run --release -p aether-inspect --bin aether-fingerprint-bench -- \
  --passes 3 /path/to/private/fits-corpus
```

`--passes` accepts values from 1 through 20 and defaults to 3. `--limit N`
selects the first `N` files after deterministic path ordering, which is useful
for a fast smoke measurement. A full pass reads every selected byte exactly
once. Later passes commonly benefit from the operating-system file cache, so
each pass remains visible and the median is not described as cold-storage
performance.

## Initial ASI294MC measurement

The initial local priority-camera run on 3 October 2026 covered 135 regular FITS
files and 3,158,611,200 bytes per pass. All evidence remained stable across
three passes.

| Pass | Seconds | Throughput |
| ---: | ---: | ---: |
| 1 | 2.201036 | 1,368.58 MiB/s |
| 2 | 1.710299 | 1,761.26 MiB/s |
| 3 | 1.679621 | 1,793.43 MiB/s |

The measured median was **1,761.26 MiB/s**. This is a development baseline for
the current machine and cache state, not a portable requirement. It does not yet
justify platform-specific sequential-read hints: the next useful measurement is
end-to-end archived-source verification, including cancellation checkpoints and
desktop progress delivery, on cold and warm storage conditions.
