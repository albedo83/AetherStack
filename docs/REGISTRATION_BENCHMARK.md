# Registration resampling benchmark

## Purpose

`aether-registration-bench` measures the strict scalar Lanczos-3 reference
implementation for both affine and projective geometry. It is an opt-in
engineering tool for tracking throughput while numerical behavior remains the
primary contract. It performs no FITS decoding or filesystem I/O, so results
isolate image allocation, inverse mapping, support evaluation, interpolation,
and mask production.

This is not an end-to-end registration benchmark. FITS reads, fingerprinting,
transaction staging, checksum readback, and publication must be measured
separately before drawing conclusions about complete workflow latency.

## Reproducibility and bounds

The source image is generated deterministically in memory. Every pass seals all
output pixel bits and mask bits and fails if either changes. The command accepts
at most 8,192 pixels per axis, three planes, 100 million total samples, and 20
passes. These limits prevent accidental unbounded allocations while still
covering representative astronomy sensors.

Each reported duration contains only the resampling call. Source generation and
output sealing occur outside the timed interval. Throughput is reported as
output megapixels per second, where a pixel in each plane counts as one sample.
Each geometry performs one untimed, sealed warm-up first, then reports every
timed pass and their median. The warm-up output is also the comparison baseline,
so a difference between warm and timed execution fails the run.

## Usage

Always use an optimized build:

```shell
cargo run --release -p aether-inspect --bin aether-registration-bench -- \
  --width 4144 --height 2822 --planes 3 --passes 5
```

Defaults are 1,024 × 768, one plane, and three passes. The affine and projective
runs use nearly identical transforms; the projective run adds mild finite
perspective terms. Results are machine-specific evidence, not portable release
thresholds. Record the CPU, compiler, power mode, dimensions, plane count, and
per-pass values when comparing revisions.

## Development measurement

The following local release-build measurement records the effect of sharing the
plane-invariant kernel normalization across RGB planes. It is evidence for this
specific development machine, not a portable performance promise. Both runs used
512 × 384 pixels, three planes, three passes, the same deterministic input, and
the same compiler and power state. These historical measurements predate the
explicit untimed warm-up now performed by the tool.

| Geometry | Before, median MP/s | After, median MP/s | Change |
| --- | ---: | ---: | ---: |
| Affine | 7.690 | 8.986 | +16.85% |
| Projective | 7.579 | 8.882 | +17.19% |

Every timed pass produced the same sealed pixel and mask outputs. The existing
multi-plane differential tests additionally require bit-identical pixels, masks,
statistics, and band-height behavior against the scalar oracle.

## Optimization rule

An optimization is admissible only after differential tests prove identical
pixels, masks, accounting, and band-height behavior against the strict scalar
oracle. A faster result with altered output is a different algorithm and must
receive a new versioned identity and published error budget.
