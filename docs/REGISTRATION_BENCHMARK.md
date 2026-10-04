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

## Optimization rule

An optimization is admissible only after differential tests prove identical
pixels, masks, accounting, and band-height behavior against the strict scalar
oracle. A faster result with altered output is a different algorithm and must
receive a new versioned identity and published error budget.
