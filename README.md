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
- conformant big-endian binary64 primary-FITS output with incremental sample
  encoding, deterministic NaN substitution, exact block padding, private
  pre-publication readback, registered `DATASUM`/`CHECKSUM` generation and
  verification, atomic create-new publication, and validated path-free
  processing provenance;
- traceable camera and acquisition metadata normalization;
- explainable frame classification with explicit conflict policies;
- exact, hashable session-grouping keys with explicit missing-field reports;
- versioned bias, dark, and flat master plans with exclusive short-dark-or-bias
  flat calibration, explicit tolerances, candidate diagnostics, and bounded
  deterministic serialization;
- versioned Light-to-Dark-and-Flat association plans bound to both the session
  manifest and master plan, with exact Dark exposure, explicit temperature
  tolerance, filter-aware Flat matching, complete candidate evidence, and no
  silent ambiguity resolution;
- versioned, deterministic JSON session manifests with strict validation,
  retained FITS diagnostics, canonical manifest digests, and streaming SHA-256
  source fingerprints;
- bounded directory-to-manifest ingestion with explicit per-source failures,
  unassigned-source reporting, and no symbolic-link traversal;
- cooperative cancellation, validated machine-readable progress events, and
  atomic RAII memory reservations for pipeline stages;
- strict `f64` dark-and-flat calibration with conservative quality-mask
  propagation and explicit flat-divisor thresholds;
- exclusive `f64` bias-or-dark pedestal subtraction and exact-median flat
  normalization with complete support accounting and explicit safety guards;
- role-tagged, versioned strict-mean bias, dark, and pedestal-corrected flat
  master construction with exact per-pixel contribution accounting;
- bounded plan-driven FITS execution for bias, dark, and pedestal-corrected
  normalized-flat masters, with deterministic tiling, explicit peak-memory
  reservations, pre/post source verification, exact readback statistics, and
  private whole-plan staging followed by create-new publication with rollback;
- bounded Light-plan execution that verifies selected master provenance,
  calibrates and strictly integrates every Light group in `f64`, revalidates
  all raw and generated inputs, and publishes the complete product set as one
  rollback-safe transaction;
- bounded single-Light `f64` calibration with a distinct non-integration
  algorithm identity, exact statistics, verified checkpoints, source
  revalidation, and whole-plan rollback-safe export for Blink and later
  post-processing;
- deterministic masked image statistics with compensated, overflow-resistant
  mean and variance calculations;
- strict three-pass FITS pixel statistics with fixed-size decoding buffers,
  separate `BLANK` and non-finite accounting, deterministic corpus traversal,
  and human-readable or JSON Lines batch output;
- exact iterative median/MAD background estimation and deterministic stellar
  local-maximum, centroid, FWHM, eccentricity, saturation, and support
  measurements on prepared linear detection planes;
- a toolkit-independent frame-review and Blink state model with stable source
  identities, previewed decision transactions, bounded undo, view-only sorting,
  locked display state, and exact asynchronous preview commits;
- bounded FITS preview reduction with chunk-size-invariant compensated means,
  complete valid/excluded support accounting, automatic pyramid-level choice,
  explicit linear, midtone, or asinh grayscale display mapping, linked-luminance
  RGB mapping that preserves channel ratios, and a bounded desktop artifact cache
  keyed by identity and transform with generation-cancelled adjacent prefetch;
- a Tauri 2 desktop shell with an accessible, responsive dark Review/Blink
  workspace and an instrument-inspired Calibration laboratory, separate
  acquisition roles and master products, inspectable flat-pedestal evidence,
  a native Light-to-Dark-and-Flat association matrix, diagnostic metrics,
  explicit manual decisions, a single bounded native calibration slot,
  transactional master execution plus selectable per-frame or integrated-Light
  output, source-aware live progress, direct identity-preserving calibrated
  Blink review with an explicit raw/calibrated switch, cooperative cancellation,
  and a
  presenter boundary that leaves scientific state in Rust;
- native structured-directory import with retained classification conflicts,
  content-derived frame identities, an inspectable robust reference stretch,
  identity-safe real FITS preview loading, and Rust-owned deterministic review
  sorting, transactional manual decisions, and bounded undo;
- an on-demand desktop FITS inspector exposing exact three-pass primary-array
  moments and invalid-sample accounting without loading the array into the web
  presenter;
- explicit diagnostic frame-quality measurement for declared standard Bayer
  lights through a phase-neutral complete-cell plane, with robust background,
  noise, stellar count, source-pixel FWHM, and eccentricity shown in Review,
  including serial bounded-memory whole-session measurement;
- a strict, versioned `f64` Malvar-He-Cutler demosaicing oracle for RGGB, BGGR,
  GRBG, and GBRG mosaics, with exact measured samples, explicit reflected-edge
  behavior, unclipped linear output, conservative defect-mask propagation, and
  a memory-bounded band executor that atomically publishes checksum-verified
  planar RGB FITS products, plus an all-or-nothing reviewed-Light-plan RGB
  exporter that revalidates every calibrated input before set publication;
- strict unweighted mean integration with compensated normalized accumulation
  and exact per-pixel support accounting;
- a tested strict CPU vertical slice that reads FITS tiles, applies dark/flat
  calibration, integrates in stable order, streams scan-line bands without a
  full final-image allocation, calculates exact three-pass output statistics,
  and atomically publishes a provenance-bearing binary64 FITS product only
  after full pre-run and pre-publication source-fingerprint verification;
- immutable, sharded cache artifacts with domain-separated operation keys,
  streaming payload digests, atomic create-new publication, collision handling,
  mandatory full verification on lookup, and restartable integrated-tile
  checkpoints wired into the strict pipeline;
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
[master plan contract](docs/MASTER_PLAN_CONTRACT.md) defines conservative
short-dark and true-bias association for flat construction. The
[light calibration plan contract](docs/LIGHT_CALIBRATION_PLAN_CONTRACT.md)
defines exact Dark and normalized-Flat association for every Light group. The
[Light execution contract](docs/LIGHT_EXECUTION_CONTRACT.md) defines the
all-or-nothing calibrated-frame and calibrated-integration transactions. The
[runtime contracts](docs/RUNTIME_CONTRACTS.md) define cancellation, progress,
and memory-accounting behavior for later pipeline stages. The initial
[calibration contract](docs/CALIBRATION_CONTRACT.md) fixes the equation,
precision, and mask behavior used by the CPU vertical slice. The
[statistics contract](docs/STATISTICS_CONTRACT.md) defines usable samples and
the strict reference moment calculations. The initial
[integration contract](docs/INTEGRATION_CONTRACT.md) defines reduction order,
sample eligibility, output masks, and support maps. The
[demosaicing contract](docs/DEMOSAIC_CONTRACT.md) fixes the first strict Bayer
reconstruction algorithm, phase handling, border rule, precision, and mask
semantics. The
[FITS output contract](docs/FITS_OUTPUT_CONTRACT.md) defines the strict binary64
encoding and unavailable-sample representation. The
[cache contract](docs/CACHE_CONTRACT.md) defines immutable operation keys,
artifact verification, and publication failure boundaries.
The [reference feature inventory](docs/WBPP_FEATURE_INVENTORY.md) records the
preprocessing controls that must be considered for scientific parity, while the
[UX principles](docs/UX_PRINCIPLES.md) define the modern dark interface,
progressive disclosure, diagnostics, and accessibility requirements. The
[frame review contract](docs/FRAME_REVIEW_CONTRACT.md) defines quality-table,
viewer, Blink, rejection, and reproducibility behavior. The
[quality contract](docs/QUALITY_CONTRACT.md) defines the strict initial
background and stellar measurement algorithms and their current release gates.
The [preview contract](docs/PREVIEW_CONTRACT.md) separates bounded display
artifacts from scientific pixels and records the remaining viewer release gates.

## Build and test

The Rust toolchain is pinned in `rust-toolchain.toml`.

```shell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo doc --workspace --all-features --no-deps
```

Build and test the desktop presenter:

```shell
cd apps/desktop
npm ci
npm run format:check
npm run build
npm test
npm run tauri:dev
```

Inspect a FITS file or directory without loading image pixels:

```shell
cargo run -p aether-inspect -- /path/to/fits-corpus
cargo run -p aether-inspect -- --strict --examples 10 /path/to/fits-corpus
```

Calculate strict pixel statistics without loading complete images:

```shell
cargo run -p aether-stats -- /path/to/fits-file-or-directory
cargo run -p aether-stats -- --strict --jsonl /path/to/fits-corpus
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
