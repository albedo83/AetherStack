# AetherStack

AetherStack is a Rust engine for calibrating, registering, and stacking
astrophotography data. Its design prioritizes numerical accuracy, reproducible
results, explainable decisions, and bounded-memory processing of large FITS and
SER sessions.

The project is under active development. The current milestone establishes a
strict CPU reference implementation and the scientific contracts that future
optimized CPU and GPU paths must match.

## Current capabilities

- checked multi-plane image dimensions and planar memory layout;
- per-pixel quality masks;
- deterministic tile traversal with clipped halos;
- compensated double-precision summation;
- bounded FITS primary-header reading with raw 80-byte card retention;
- strict and tolerant FITS conformance reports;
- checked image-HDU layouts and bounded random-access decoding into `f64`
  scientific tiles for 16-bit integer, 32-bit floating-point, and 64-bit
  floating-point data, including scaling and invalid-pixel mask propagation;
- conformant big-endian binary64 primary-FITS stream output with deterministic
  NaN substitution, exact block padding, atomic create-new publication, and
  validated path-free processing provenance;
- traceable camera and acquisition metadata normalization;
- explainable frame classification with explicit conflict policies;
- exact, hashable session-grouping keys with explicit missing-field reports;
- versioned, deterministic JSON session manifests with strict validation,
  retained FITS diagnostics, and streaming SHA-256 source fingerprints;
- bounded directory-to-manifest ingestion with explicit per-source failures,
  unassigned-source reporting, and no symbolic-link traversal;
- cooperative cancellation, validated machine-readable progress events, and
  atomic RAII memory reservations for future pipeline stages;
- strict `f64` dark-and-flat calibration with conservative quality-mask
  propagation and explicit flat-divisor thresholds;
- deterministic masked image statistics with compensated, overflow-resistant
  mean and variance calculations;
- strict unweighted mean integration with compensated normalized accumulation
  and exact per-pixel support accounting;
- streaming corpus inspection without loading pixel arrays.

The priority camera profiles currently cover:

- ZWO ASI294MC Pro color data;
- ToupTek ATR585C color data.

ToupTek 571M monochrome support is planned when representative source files are
available. DSLR-specific behavior is outside the current product scope.

See the [implementation plan](docs/IMPLEMENTATION_PLAN.md) and the anonymized
[corpus inventory](docs/CORPUS_INVENTORY.md) for the architectural rationale and
the evidence driving format support. The [session manifest contract](docs/SESSION_MANIFEST.md)
documents the current portable interchange schema and its validation rules. The
[runtime contracts](docs/RUNTIME_CONTRACTS.md) define cancellation, progress,
and memory-accounting behavior for later pipeline stages. The initial
[calibration contract](docs/CALIBRATION_CONTRACT.md) fixes the equation,
precision, and mask behavior used by the CPU vertical slice. The
[statistics contract](docs/STATISTICS_CONTRACT.md) defines usable samples and
the strict reference moment calculations. The initial
[integration contract](docs/INTEGRATION_CONTRACT.md) defines reduction order,
sample eligibility, output masks, and support maps. The
[FITS output contract](docs/FITS_OUTPUT_CONTRACT.md) defines the strict binary64
encoding and unavailable-sample representation.

## Build and test

The Rust toolchain is pinned in `rust-toolchain.toml`.

```shell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo doc --workspace --all-features --no-deps
```

Inspect a FITS file or directory without loading image pixels:

```shell
cargo run -p aether-inspect -- /path/to/fits-corpus
cargo run -p aether-inspect -- --strict --examples 10 /path/to/fits-corpus
```

Full acquisition data must never be committed. Test cases must use small,
redistributable synthetic fixtures with no private paths, coordinates, object
names, or observer metadata.

## Project principles

- Scientific correctness is tested before optimization.
- The deterministic `f64` CPU path is the numerical oracle.
- Optimized implementations require differential tests and documented tolerances.
- Missing or contradictory metadata is reported, never silently invented.
- Pixel processing is tiled and memory-bounded from the first implementation.
- Public interfaces and non-obvious invariants are documented in English.

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) before submitting a change. Bug reports
and small, test-backed pull requests are welcome while the core architecture is
being established.

## License

AetherStack is licensed under the [MIT License](LICENSE).
