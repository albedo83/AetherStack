# AetherStack product and architecture brief

## Vision

AetherStack is a high-performance calibration, registration, and stacking engine
for deep-sky astrophotography and lucky imaging. It aims to combine advanced
processing such as local normalization, robust statistical rejection, and
drizzle with a predictable workflow and bounded I/O.

The engine must handle both hundreds of long-exposure FITS images and SER
sequences containing tens of thousands of frames. Scientific correctness and
traceability take priority over throughput; acceleration is accepted only after
comparison with a deterministic CPU reference.

## Technology choices

- **Core engine:** Rust for memory safety, explicit concurrency, and predictable
  performance.
- **Reference numerics:** deterministic `f64` CPU implementations.
- **GPU acceleration:** WGPU where an operation can meet documented numerical
  tolerances across Vulkan, Metal, and DirectX 12 backends.
- **External data interchange:** Apache Arrow IPC for structured metadata and
  selected inter-process boundaries, not for moving every intermediate pixel
  buffer.
- **Desktop interface:** a decoupled Tauri application with a web frontend after
  the processing contracts stabilize.

## Architectural principles

The initial implementation is a library-first, in-process pipeline. Splitting
every stage into a separate executable would create serialization overhead,
failure complexity, and unnecessary memory copies. Stable process boundaries can
be added later for scripting, isolation, and distributed execution.

Pixel buffers use typed Rust storage and deterministic tile traversal. Stages
exchange references or owned buffers in memory. A versioned session manifest and
content-addressed cache provide restartability without relying on a permanently
resident shared-memory graph.

Every processing result records:

- source inputs and their fingerprints;
- normalized parameters and execution profile;
- software and schema versions;
- warnings, tolerated format deviations, and policy overrides;
- numerical summary and output checksum.

## Processing stages

### Ingest and session planning

The ingest layer scans directories, reads FITS/XISF headers and SER container
metadata, and builds a versioned dependency graph. It groups frames by explicit
camera, acquisition, exposure, temperature, gain, offset, binning, filter, and
CFA rules. Contradictory evidence is preserved and must be resolved by an
explicit import policy.

SER sequences are indexed without loading all frames into memory.

### Master generation

Bias, dark, flat, and flat-dark groups produce reference frames using tested
combinations of mean, weighted mean, median, and robust rejection. Accumulation
uses numerically stable methods and deterministic reduction order in strict mode.

### Calibration and cosmetic correction

Calibration applies bias/dark subtraction and flat division with explicit masks
for invalid, saturated, and missing samples. Dark optimization is optional and
must never be silently enabled.

Cosmetic correction derives hot and cold defects from master statistics or a
declared defect map. Replacement uses a documented neighborhood rule and marks
every changed pixel. CFA data is corrected before demosaicing to avoid spreading
defects across color channels.

### Quality measurement

The scoring stage detects stars and estimates sub-pixel centroids, FWHM,
eccentricity, background, noise, and a signal-to-noise proxy. The weighting
formula is versioned and included in provenance. Lucky-imaging selection can
retain a declared percentile or quality threshold from a SER sequence.

### Registration

Registration matches invariant geometric features, estimates a robust transform,
and reports residual error and inlier count. The transform model progresses from
translation and affine fits to higher-order distortion only when supported by
the data. Resampling uses a documented kernel such as Lanczos-3; drizzle retains
original pixel footprints instead of resampling first.

### Local normalization

Local normalization estimates spatially varying background and scale surfaces
between a target and reference image. Invalid regions, stars, nebula structures,
and low-support tiles require robust masking and regularization.

The conceptual transform is:

$$
I_{norm}(x,y) = \left(I(x,y) - B_{target}(x,y)\right)
\frac{S_{ref}(x,y)}{S_{target}(x,y)} + B_{ref}(x,y)
$$

The implementation must guard near-zero scale, extrapolation beyond supported
regions, and propagation of input masks.

### Integration and rejection

Aligned, normalized samples are combined using their quality weights. Rejection
options include sigma clipping, Winsorized sigma clipping, and generalized ESD
only after their statistical assumptions and small-sample behavior are tested.
Outputs include rejection counts and reason masks, not only the integrated image.

### Drizzle

Drizzle projects the footprint of each original detector pixel onto a finer
output grid. Scale, drop shrink, weight, CFA phase, masks, and geometric
transforms are explicit inputs. Tile-based processing is mandatory because both
the output image and contribution maps can exceed available RAM or VRAM.

## Execution profiles

- **Strict:** deterministic `f64` CPU processing, checked arithmetic, stable
  iteration order, and full diagnostics. This is the correctness oracle.
- **Balanced:** parallel CPU processing with tested reductions and equivalent
  scientific behavior within documented tolerances.
- **Accelerated:** GPU kernels for eligible stages, always covered by differential
  tests against strict CPU results.

Profile changes are part of provenance. A run must never silently switch to a
less precise profile.

## User interfaces

The first interface is a stable Rust API plus small command-line tools for
inspection and pipeline execution. Machine-readable progress events will later
support a Tauri desktop client and external automation.

A conceptual end-to-end command may eventually resemble:

```shell
aether run /data/session \
  --masters auto \
  --cosmetic-correction auto \
  --score stars \
  --register auto \
  --local-normalization \
  --integrate winsorized \
  --drizzle-scale 2 \
  --output result.fits
```

The command is intentionally conceptual. Public command syntax will be stabilized
only after the session schema and vertical CPU pipeline are proven.

## Non-functional requirements

- No input-dependent panic, unchecked allocation, or unbounded header parsing.
- No modification of source acquisition files.
- Bounded memory and file-descriptor use on large sessions.
- Unit tests for each invariant and failure path.
- Property, metamorphic, differential, fuzz, and golden tests where appropriate.
- English public documentation and comments explaining scientific intent.
- No private acquisition data, machine-specific paths, or credentials in the
  public repository.
- Reproducible CI on Linux, macOS, and Windows.

## Success criteria

A stable release requires reproducible calibration and integration results on
the supported camera profiles, successful recovery after interruption, complete
provenance, a documented numerical error budget, fuzz-tested parsers, and
differential validation against independent tools or published reference data.
