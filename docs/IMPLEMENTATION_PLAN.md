# AetherStack implementation plan

## 1. Goal and scope

AetherStack will provide a scientifically defensible Rust engine for
calibrating, registering, normalizing, integrating, and drizzling astronomical
images. It must support conventional deep-sky FITS sessions and large lucky
imaging SER sequences without requiring all pixels to fit in memory.

Current camera priorities are:

1. ZWO ASI294MC Pro color;
2. ToupTek ATR585C color;
3. ToupTek 571M monochrome after representative files become available.

DSLR-specific behavior is not a product priority. Generic standards-compliant
files may remain readable, but they do not drive architecture or release gates.

The central quality requirements are maximum practical precision, deterministic
reference results, robust malformed-input handling, high test density, and
clear English documentation suitable for a public open-source repository.

## 2. Architectural decisions

### 2.1 Library-first pipeline

The first production path is an in-process Rust library graph. Individual tools
remain thin adapters around stable libraries. This gives algorithms typed access
to buffers, reduces serialization, and makes unit testing straightforward.

Separate worker processes and Arrow IPC can be added at deliberate isolation or
distributed-execution boundaries. They are not the default transport for every
pixel array.

### 2.2 Deterministic CPU oracle

The strict execution profile uses `f64`, stable traversal order, checked
arithmetic, and documented compensated or pairwise reductions. It is the oracle
for optimized CPU and GPU implementations.

WGPU does not provide uniformly portable double precision. GPU kernels may use
`f32` or mixed precision only after an error budget and differential tests show
that a specific operation stays within its scientific tolerance. A GPU path must
never silently replace the strict profile.

### 2.3 Tiled processing from the start

Every pixel operator declares:

- the core tile it writes;
- the halo it reads;
- its boundary condition;
- scratch-memory requirements;
- whether it can run in place;
- its deterministic merge rule.

Tile dimensions are execution parameters, not scientific parameters. Changing a
tile size in strict mode must not change the result beyond the operation's
documented exactness or tolerance contract.

### 2.4 Data integrity and provenance

Source files are immutable. Each artifact records source fingerprints, normalized
parameters, execution profile, software and schema versions, diagnostics,
explicit policy overrides, and output checksums.

Raw FITS cards remain available beside canonical metadata. Normalization never
erases the original value or its source keyword.

## 3. Rust workspace

The intended workspace evolves toward:

```text
crates/
├── aether-core/          # image model, masks, tiles, numerical primitives
├── aether-fits/          # FITS headers, HDUs, pixels, checksums, writer
├── aether-metadata/      # traceable normalization and camera profiles
├── aether-session/       # classification, grouping, manifest generation
├── aether-inspect/       # bounded corpus inventory CLI
├── aether-ser/           # SER indexing and frame access
├── aether-cache/         # content-addressed artifacts and atomic commits
├── aether-runtime/       # pipeline graph, scheduling, cancellation, progress
├── aether-calibration/   # masters, calibration, cosmetic correction
├── aether-quality/       # background, noise, stars, FWHM, eccentricity
├── aether-registration/  # matching, transforms, resampling
├── aether-localnorm/     # local background and scale models
├── aether-integration/   # weighting, rejection, accumulation
├── aether-drizzle/       # footprint projection and contribution maps
├── aether-gpu/           # WGPU kernels and capability negotiation
└── aether-cli/           # stable command-line interface
```

Crates are introduced only when their contracts are clear. Empty architectural
scaffolding is avoided.

## 4. Fundamental contracts

### 4.1 Image model

An image carries:

- checked width, height, and plane count;
- planar row-major sample storage;
- an explicit sample type;
- a quality mask with missing, saturated, hot, cold, rejected, and invalid bits;
- CFA pattern and phase where applicable;
- acquisition metadata and processing provenance;
- a coordinate transform describing crops and geometric operations.

No image constructor accepts a buffer whose sample count differs from checked
dimensions. Data-dependent indexing and allocation use checked arithmetic.

### 4.2 Numerical contract

For every algorithm, documentation states:

- input and accumulation precision;
- reduction order and determinism guarantees;
- treatment of NaN, infinity, saturation, and masked pixels;
- absolute, relative, and ULP tolerances where relevant;
- minimum sample support;
- overflow and underflow behavior;
- CPU/GPU equivalence criteria.

Scientific defaults are versioned. Changing a default that can affect results is
a schema or algorithm-version change, not an invisible implementation detail.

### 4.3 Errors and observability

Library code returns typed errors and structured diagnostics. Input-dependent
panics are forbidden. Long-running stages support cancellation and emit
machine-readable progress without writing uncontrolled per-pixel logs.

Warnings distinguish recoverable conformance deviations, missing metadata,
evidence conflicts, degraded numerical paths, and unsupported features.

## 5. Ingestion

### 5.1 FITS

The FITS layer must support:

- 2,880-byte logical blocks and exact 80-byte card retention;
- primary HDUs and image extensions;
- `BITPIX` values 8, 16, 32, 64, -32, and -64;
- big-endian decoding;
- `BSCALE`, `BZERO`, `BLANK`, NaN, and infinity;
- 2D monochrome, 2D CFA, and three-plane images;
- bounded header/HDU sizes and checked dimension products;
- `CHECKSUM` and `DATASUM` verification;
- seek-based and tile/row-based pixel access;
- FITS output with atomic replacement and provenance.

Strict mode rejects standard errors. Tolerant mode accepts only unambiguous
deviations, preserves diagnostics, and never repairs the source in place.

Before adopting an external FITS dependency, compare a small safe Rust layer over
CFITSIO with the current native parser. The decision must consider conformance,
threading, static distribution, error reporting, fuzzability, maintenance, and
performance. Independent `fitsverify` comparison remains part of validation.

### 5.2 Metadata and camera profiles

Each normalized value retains:

1. the raw value and source keyword;
2. the canonical value;
3. confidence (`Exact` or `Normalized` initially);
4. any conflicts with equivalent keywords or path evidence.

Camera profiles describe known identifiers, sensor kind, expected dimensions,
pixel size when declared, CFA behavior, and useful vendor keywords. Profiles are
validation aids, not excuses to synthesize missing values.

Frame classification combines header, exact directory, and explicit file-name
evidence. The default requires agreement. Preference policies are caller-selected
and auditable; they do not fall back to an unrelated source.

### 5.3 SER

SER support will validate the file header, color identifier, endianness, pixel
depth, frame count, dimensions, timestamps, and trailer length. Frame access is
indexed and lazy. Truncated or inconsistent sequences report the exact safe
prefix rather than reading beyond available bytes.

### 5.4 XISF

XISF input follows after the FITS vertical slice. Parsing must bound XML and
attachment sizes, validate compression metadata and checksums, and preserve
unsupported properties. FITS 32-bit or 64-bit output is acceptable before a
robust XISF writer exists.

## 6. Sessions, cache, and recovery

The session manifest is versioned and contains file fingerprints, canonical
metadata, grouping decisions, unresolved conflicts, master dependencies,
parameters, and requested outputs.

Cache keys hash the normalized operation, algorithm version, parameters, input
artifact checksums, and execution profile. Artifacts are written to a temporary
file, flushed, verified, and atomically renamed. An incomplete artifact is never
considered valid.

Memory, disk, and VRAM budgets are explicit scheduler inputs. File descriptors
and concurrent readers are bounded. Cancellation leaves source files untouched
and cache state recoverable.

## 7. Delivery roadmap

### Phase 0 — Characterize real data (`substantially complete`)

Deliverables:

- anonymized extension and FITS-header inventory;
- representative 294MC and 585C header analysis;
- strict/tolerant conformance comparison with `fitsverify`;
- documented classification conflicts;
- fixture requirements derived without publishing private files.

Exit criterion: every current ingestion decision is tied to measured corpus
evidence. The 571M remains explicitly pending; its files are not searched for or
its format guessed.

### Phase 1 — Repository and scientific foundations (`in progress`)

Deliverables:

- pinned Rust toolchain and warning-clean workspace;
- cross-platform CI for formatting, linting, tests, and documentation;
- checked image dimensions and planar layout;
- quality masks and deterministic tile traversal;
- compensated `f64` reductions;
- public contribution, security, and licensing documents.

Exit criterion: all invariants have unit tests and the repository passes the full
quality gate on supported platforms.

### Phase 2 — FITS ingestion and session generation (`in progress`)

Deliverables:

- bounded primary-header parser with raw card retention;
- strict and tolerant diagnostics;
- traceable metadata normalization and priority camera profiles;
- explainable frame classification;
- streaming inventory CLI;
- safe image-HDU descriptor and tiled/row pixel reader;
- scaling, blanking, and mask propagation;
- versioned session manifest and grouping rules;
- synthetic public FITS fixtures and differential tests.

Exit criterion: priority-camera raw files and processed `float32` products can be
read reproducibly, classified without hidden assumptions, and compared against an
independent FITS implementation.

### Phase 3 — Runtime, cache, and vertical CPU slice

Build a minimal end-to-end CPU pipeline: ingest one group, read tiles, apply a
simple calibration, calculate statistics, integrate by mean, and write FITS.
Add cancellation, progress events, memory budgets, atomic cache artifacts, and
provenance.

Exit criterion: interruption and restart produce the same verified output as an
uninterrupted strict run.

### Phase 4 — Masters and calibration

Implement bias, dark, flat, and flat-dark masters; exposure-scaled dark handling;
flat normalization; saturation and invalid-value masks; variance/uncertainty
tracking; defect maps; and conservative cosmetic correction.

Exit criterion: synthetic equations are recovered within tolerance and selected
real-data statistics agree with an independent reference pipeline.

### Phase 5 — Quality measurement

Implement robust background/noise estimation, star detection, sub-pixel
centroids, FWHM, eccentricity, SNR proxy, and versioned weighting expressions.

Exit criterion: estimates remain accurate on synthetic PSFs across background,
noise, saturation, and edge cases.

### Phase 6 — Registration

Implement invariant feature matching, robust model estimation, translation,
affine and projective transforms, distortion models where justified, Lanczos and
cubic resampling, transform composition, and mask propagation.

Exit criterion: sub-pixel residuals on synthetic ground truth and robust behavior
on sparse, crowded, rotated, mirrored, and partially overlapping fields.

### Phase 7 — Robust integration

Implement deterministic weighted accumulation, streaming mean/variance, median
where memory allows, sigma clipping, Winsorized clipping, optional generalized
ESD, and per-pixel rejection diagnostics.

Exit criterion: controlled outlier experiments match analytical expectations and
remain stable at small sample counts.

### Phase 8 — Local normalization

Implement masked sampling, robust local background/scale estimates, surface
regularization, support diagnostics, near-zero scale protection, and provenance
for the selected reference.

Exit criterion: injected gradients are recovered without suppressing protected
astronomical structures.

### Phase 9 — Drizzle

Implement geometric detector-pixel footprints, scale and drop-shrink controls,
CFA drizzle, contribution/weight maps, masks, tiled accumulation, and flux tests.

Exit criterion: conserve flux and recover expected sampling on synthetic dither
patterns while remaining bounded in memory.

### Phase 10 — GPU acceleration

Add capability negotiation, reusable buffers, batched transfers, shader tests,
device-loss recovery, and differential validation per kernel. Prioritize
high-arithmetic-intensity stages; keep parsing and metadata on CPU.

Exit criterion: every enabled backend satisfies the operation-specific error
budget or cleanly falls back to CPU.

### Phase 11 — SER and lucky imaging

Add lazy SER frame access, batch quality estimation, top-percent selection,
high-volume registration, memory-budgeted integration, and resumable checkpoints.

Exit criterion: process a tens-of-thousands-frame sequence without unbounded
memory growth or descriptor leaks.

### Phase 12 — Stable CLI and desktop integration

Stabilize command schemas, machine-readable progress, dry-run plans, diagnostics,
cache inspection, and the Tauri client. Arrow IPC is introduced only at justified
external boundaries.

Exit criterion: all UI actions are expressible by a documented CLI/API and no
scientific behavior exists only in the interface layer.

## 8. Test strategy

### 8.1 Test layers

- **Unit tests:** constructors, indexing, masks, parsers, metadata precedence,
  statistics, kernels, and every known failure branch.
- **Property tests:** arbitrary dimensions, card sequences, tilings, transforms,
  masks, and numerical ranges.
- **Metamorphic tests:** tile-size invariance, constant-offset behavior, flux
  scaling, permutation rules, crop/CFA phase, and transform composition.
- **Differential tests:** FITS behavior against CFITSIO or another independent
  implementation; CPU optimized and GPU paths against the strict oracle.
- **Golden tests:** small synthetic fixtures with reviewed checksums and summary
  values.
- **Fuzzing:** FITS cards and HDUs, future XISF XML, SER headers, session JSON,
  dimensions, and cache manifests.
- **End-to-end tests:** restart, cancellation, corrupt cache, low disk space,
  missing inputs, and unsupported GPU capability.

### 8.2 Tolerances

Exact equality is required for parsing, integer transformations, masks, grouping,
and deterministic metadata. Floating-point algorithms define absolute and
relative error, ULP bounds where useful, flux error, geometric residual, and
accepted non-finite behavior. A single global epsilon is prohibited.

### 8.3 Fixture policy

Private source data remains outside Git. Public fixtures are generated from
documented synthetic recipes and contain no private paths, coordinates, target
names, observer details, or device serial numbers. Each regression fixture is
small enough for routine CI.

## 9. Quality and publication

Every pull request must pass:

```shell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps
```

Release preparation later adds dependency license review, vulnerability auditing,
minimum-supported-Rust-version checks, fuzz smoke tests, benchmark regression
thresholds, and reproducible release artifacts.

Comments and API documentation are written in English. They explain domain
meaning, invariants, precision decisions, format constraints, and non-obvious
safety reasoning. They do not narrate obvious syntax.

## 10. Immediate implementation sequence

1. ~~Finish repository publication and baseline CI.~~
2. Complete session-grouping keys; explicit classification policies are done.
3. ~~Introduce a checked FITS image-HDU descriptor without allocating pixels.~~
4. ~~Implement bounded sample-range reading for 16-bit integer and 32-bit
   floating data.~~
5. ~~Apply `BSCALE`, `BZERO`, `BLANK`, and non-finite status into `f64`.~~
6. Generate tiny synthetic 294MC/585C-shaped semantic fixtures, not full-size
   camera files.
7. Compare samples and summary statistics with an independent FITS reader.
8. Add manifest serialization only after grouping types are stable.

## 11. Stable-release definition

AetherStack is ready for a stable release only when supported inputs are bounded
and fuzz-tested, the strict CPU path is deterministic, optimized paths have
published error budgets, provenance is complete, cache recovery is tested,
camera profiles are backed by real evidence, public APIs are documented, and the
repository contains no private acquisition data or machine-specific information.

## 12. Normative references

- FITS Standard 4.0: <https://fits.gsfc.nasa.gov/fits_standard.html>
- FITS checksum convention: <https://fits.gsfc.nasa.gov/registry/checksum.html>
- CFITSIO: <https://heasarc.gsfc.nasa.gov/docs/software/fitsio/fitsio.html>
- SER format overview: <https://github.com/olegkutkov/ser-file-format>
- WGPU: <https://wgpu.rs/>
- Apache Arrow IPC: <https://arrow.apache.org/docs/format/Columnar.html#serialization-and-interprocess-communication-ipc>
