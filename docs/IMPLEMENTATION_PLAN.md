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
├── aether-review/        # review decisions, stable sorting, Blink state
├── aether-preview/       # bounded FITS reduction and display mapping
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

### Phase 1 — Repository and scientific foundations (`complete`)

Deliverables:

- pinned Rust toolchain and warning-clean workspace;
- cross-platform CI for formatting, linting, tests, and documentation;
- checked image dimensions and planar layout;
- quality masks and deterministic tile traversal;
- compensated `f64` reductions;
- public contribution, security, and licensing documents.

Exit criterion: all invariants have unit tests and the repository passes the full
quality gate on supported platforms.

### Phase 2 — FITS ingestion and session generation (`complete`)

Deliverables:

- bounded primary-header parser with raw card retention;
- strict and tolerant diagnostics;
- traceable metadata normalization and priority camera profiles;
- explainable frame classification;
- streaming inventory CLI;
- safe image-HDU descriptor and tiled/row pixel reader;
- ~~scaling, blanking, and mask propagation;~~
- ~~versioned session manifest and grouping rules;~~
- synthetic public FITS fixtures and differential tests.

Exit criterion: priority-camera raw files and processed `float32` products can be
read reproducibly, classified without hidden assumptions, and compared against an
independent FITS implementation.

### Phase 3 — Runtime, cache, and vertical CPU slice (`complete`)

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
2. ~~Complete explicit classification policies and exact session-grouping keys.~~
3. ~~Introduce a checked FITS image-HDU descriptor without allocating pixels.~~
4. ~~Implement bounded sample-range and rectangular tile reading for 16-bit
   integer and 32-bit floating data.~~
5. ~~Apply `BSCALE`, `BZERO`, `BLANK`, and non-finite status into `f64`, then
   propagate unusable samples into the shared quality mask.~~
6. ~~Generate tiny synthetic 294MC/585C-shaped semantic fixtures, not full-size
   camera files.~~
7. ~~Compare distributed samples and full-array summary statistics with an
   independent FITS reader.~~
8. ~~Add manifest serialization only after grouping types are stable.~~
9. ~~Implement streaming SHA-256 source fingerprints and a bounded manifest
   generator without retaining file payloads.~~
10. ~~Add bounded directory-to-manifest orchestration with explicit handling for
    per-file failures and unassigned sources.~~
11. ~~Define runtime cancellation, progress, and memory-budget contracts.~~
12. ~~Build the first strict CPU vertical slice from tile ingestion through simple
    calibration, statistics, mean integration, and FITS output.~~
    The dark-and-normalized-flat `f64` calibration kernel, mask contract, strict
    image statistics, and unweighted mean integration are complete;
    binary64 FITS encoding, readback, and atomic create-new publication are
    complete. Validated output provenance and tiled end-to-end orchestration are
    complete, including cancellation, logical working-set enforcement, and
    before/after verification of every immutable source fingerprint.
13. ~~Add verified content-addressed checkpoints, restart tests, and streaming
    output so large integrations no longer retain the complete final image.
    The immutable artifact store, operation-key derivation, streaming payload
    digest, collision detection, verified lookup, integrated-tile checkpointing,
    and interruption/restart equivalence tests are complete. Streaming output
    now writes bounded scan-line bands to a private FITS stream. Exact statistics
    are calculated by bounded readback before atomic publication.~~
14. ~~Implement FITS `DATASUM` and `CHECKSUM` generation and verification. Add
    corruption, malformed-keyword, chunk-boundary, registered-vector, and
    interoperability tests before enabling the cards in atomic output.~~
15. ~~Begin Phase 4 with versioned bias, dark, and flat master plans, including
    short-exposure dark matching. The planner supports real bias frames and uses
    an exclusive short-dark-or-bias association so both sources cannot be
    subtracted blindly. Matching tolerances, rejected-candidate evidence,
    ambiguity, schema validation, deterministic serialization, and memory bounds
    are explicit.~~
16. ~~Implement deterministic strict-mean master construction from these plans,
    followed by guarded flat normalization and synthetic equation recovery tests.
    Role-tagged strict-mean construction, exclusive pedestal subtraction, and
    exact-median flat normalization primitives are complete, including mask
    propagation, per-pixel and global support diagnostics, allocation failures,
    numerical edge cases, and a synthetic subtract-integrate-normalize equation
    test. Plan-bound, tiled FITS execution is complete for direct bias and dark
    masters, including memory reservations, cancellation, exact source
    fingerprint revalidation, readback statistics, and atomic create-new
    publication. The calibrated flat executor now enforces exclusive pedestal
    subtraction before integration, reserves the complete exact-median peak,
    applies one global normalization scalar, revalidates raw-flat and pedestal
    fingerprints, and publishes the normalized FITS atomically. The complete
    dependency graph now executes as one bounded transaction: products remain
    private until every calculation succeeds, final paths are create-new links,
    publication failures roll back only this run, and every FITS product embeds
    the exact manifest and plan digests.~~
17. Begin the frame-review foundation with strict bounded-memory FITS pixel
    statistics and deterministic batch output. The three-pass library operation,
    separate invalid-sample accounting, JSON Lines tool output, synthetic tests,
    and representative ASI294MC Pro validation are complete. Robust astronomical
    quality metrics, preview pyramids, and the internal Blink view remain next.
18. Begin strict astronomical quality measurement. Exact iterative median/MAD
    background clipping and deterministic local-maximum stellar measurements now
    report centroids, threshold-corrected major/minor FWHM, eccentricity,
    background SNR, saturation, and support with explicit work bounds. A
    versioned complete-cell CFA detection plane now covers every standard Bayer
    phase, propagates invalid support strictly, and exposes manual diagnostic
    measurements in Review. A real ASI294MC Pro light passes the complete path.
    Spatial background modeling, ToupTek 585C comparison, independent-reference
    tolerances, and automatic-selection authorization remain release gates.
19. Establish the toolkit-independent Review/Blink interaction model. Stable
    frame identities, validated quality-table summaries, view-only deterministic
    sorting, previewed decision batches, bounded transactional undo, sealing,
    shared display transforms, exact pending-preview identity, and forward,
    backward, and bounce traversal are implemented and unit tested. Automatic
    rule evidence and preview tile caching remain next. The first accessible
    Tauri presenter is now implemented without moving these invariants into
    frontend state.
20. Implement the first bounded preview path. FITS planes now reduce through
    deterministic power-of-two compensated means with exact valid/excluded
    support, explicit resource limits, I/O chunk invariance, automatic level
    selection, versioned grayscale display mapping, a robust inspectable
    reference-stretch estimator, and representative ASI294MC Pro scalar and PNG
    validation. CFA demosaicing, RGB composition, multi-frame aggregate stretch,
    cache publication, broader real-camera validation, and viewport tile
    streaming remain next.
21. Begin the native desktop surface. A Tauri 2 shell now hosts an accessible
    dark Review/Blink workspace with separate Bias, Darks, Flats, and Lights,
    deterministic typed actions, responsive layouts, and tactile instrument-like
    surfaces. A bounded Rust command produces PNG previews over raw binary IPC;
    the presenter rejects stale frame identities and releases browser resources
    idempotently. Native directory selection now imports a strict session in a
    worker, explicitly prefers structured directory roles when acquisition
    headers conflict, marks every override, resolves one reference stretch, and
    keeps it locked across Blink frames. Fit and preview-pixel views are real,
    deterministic table sorting delegates to the Rust review model, and
    an on-demand inspector exposes exact fixed-buffer three-pass FITS moments.
    Unfinished commands are exposed as unavailable instead of inert controls.
    Declared standard CFA lights can now request strict diagnostic background,
    noise, stellar count, FWHM, and eccentricity without using display pixels;
    the table and instrument badges expose completion and limitations.
    Single-frame or serial whole-session measurement remains explicitly
    diagnostic, reports bounded batch progress, and keeps the viewer responsive
    without multiplying exact-median scratch allocations. Manual accept, reject,
    clear, and undo operations now execute against the native generation-bound
    transaction engine and return identity-scoped patches to the presenter. The
    Blink presenter owns a five-entry/32 MiB LRU artifact
    cache and generation-cancelled adjacent-frame prefetch; foreground selection
    joins matching speculative work without weakening exact frame identity.
    The second desktop workspace now keeps the exact imported manifest in native
    memory and derives a plan-digest-bound Calibration view from explicit
    pedestal policy and tolerances. Its separate Bias/Dark/Flat product cards
    expose metadata, exclusive selected dependencies, blocking reasons, and all
    compatible or rejected candidates through progressive disclosure; the web
    presenter never reconstructs scientific groups. Calibration can now choose
    an output directory and execute that exact native plan in one memory-bounded
    worker with typed live progress, cooperative cancellation, transactional
    whole-plan publication, and explicit peak-memory reporting. The immutable
    imported manifest is shared into execution without copying its file set.
    Arbitrary-file import, durable decision recovery, multi-frame stretch
    estimation, spatial quality modeling, advanced construction controls, and
    ToupTek validation remain next.
22. Begin Light calibration association planning. A versioned plan now binds
    every Light group to the exact session manifest and master plan, evaluates
    every Dark and normalized Flat candidate, requires exact Dark exposure,
    applies one explicit inclusive Dark-temperature tolerance, and requires an
    exact Flat filter. Equal best Darks and multiple applicable Flats remain
    blocking ambiguities. Bounded canonical JSON, digest binding, missing-field
    evidence, tamper rejection, and role-specific tests are complete. Native
    preview, calibrated-Light publication, dark scaling, defect correction, and
    uncertainty propagation remain next.
23. Expose Light associations in the desktop calibration laboratory. The native
    preview now constructs the exact digest-bound Light plan only after every
    master dependency is ready, and projects selected Dark/Flat identities,
    temperature evidence, missing fields, ambiguities, and every rejected
    candidate through typed IPC. The dark UI renders a responsive execution-gate
    matrix with explicit Ready/Blocked states and expandable evidence. Native
    calibrated-Light publication remains next.
24. Execute canonical Light calibration plans transactionally. The native
    runtime now rejects non-canonical or empty plans, verifies each selected
    Dark and Flat FITS product against the exact manifest and master-plan
    digests, and runs strict tiled `f64` calibration plus stable-order mean
    integration for every Light group. Raw Lights and generated masters are
    fingerprinted again before whole-set publication. Products remain in a
    private staging directory until the complete plan succeeds; cancellation,
    an existing destination, stale provenance, source mutation, or any later
    product failure publishes nothing. The calibrated product stores the Light
    plan digest, which transitively binds its master plan. Desktop execution
    controls and progress presentation remain next.
25. Expose Light execution through the desktop calibration laboratory. The
    command adapter rebuilds the manifest-bound master and Light plans, compares
    all three reviewed digests, and accepts only explicit master/output
    directories, flat-divisor floor, tile shape, and memory budget. Master and
    Light tasks share one native cancellation slot so their memory budgets
    cannot overlap. The dark responsive UI unlocks calibrated integration only
    after the reviewed masters were successfully published, then reports typed
    progress, peak reserved memory, final product count, safe cancellation, and
    failure-without-partial-publication. Native and presenter tests cover the
    bridge, state gating, progress, and cancellation controls.
26. Begin lossless post-calibration frame products. The runtime now has a
    dedicated single-Light `f64` executor with distinct provenance, exact
    statistics, operation-separated verified checkpoints, bounded memory,
    source revalidation, and atomic publication. Provenance version 3 adds the
    optional `AETHINP` exact source digest, which this path requires. Its pixels
    are invariant to tile shape and it deliberately performs no integration.
    Whole-plan export now stages every canonical Light source privately, reports
    group and source progress, revalidates all Lights and masters, and publishes
    the complete set with rollback; cancellation after an already calibrated
    frame still leaves the destination empty. Integrated and per-frame modes are
    separate so calibration is never duplicated.
27. Expose lossless Light outputs in the desktop. The native command and typed
    presenter bridge now select either canonical per-source calibrated frames
    or integrated groups, default to inspectable calibrated frames, preserve
    source-aware progress, and return every published path and exact source
    identity. Both choices remain behind the one bounded native execution slot.
    The dark instrument UI names the active transaction, adapts its primary
    action and result copy to the selected mode, disables the selector while a
    run is active, and keeps the control readable at the minimum supported
    width. Native and DOM tests cover both modes and the selection boundary.
    Next, feed calibrated products directly into Blink, then add debayering,
    quality weighting, and registration.
28. Feed calibrated Light products directly into Blink. Every native result now
    carries the stable review identity and label of its exact manifest source.
    After successful publication, the desktop atomically opens the complete
    calibrated set in the Frames workspace while retaining the raw acquisition
    set behind an explicit Raw/Calibrated stage switch. Manual decisions remain
    attached to the shared source identity; display previews, exact statistics,
    diagnostic quality, and locked stretches are isolated by pixel-source path.
    The presenter rejects the complete stage transition on a missing,
    duplicated, or non-Light identity. Native, binding, DOM, accessibility, and
    cache-separation tests cover the handoff. Debayering is the next processing
    stage, followed by quality weighting and registration.
29. Establish the strict Bayer demosaicing oracle. The dedicated
    `aether-demosaic` crate now reconstructs planar linear RGB with the versioned
    Malvar-He-Cutler 5x5 gradient-corrected filters in deterministic `f64`.
    RGGB, BGGR, GRBG, and GBRG phases are explicit; measured CFA samples remain
    exact; interpolation is unclipped; whole-sample symmetric borders and
    conservative mask propagation are normative. Unit tests lock the published
    coefficients, every phase, boundary behavior, defect evidence, non-finite
    handling, and typed rejection. Next, execute this oracle through a bounded
    halo-tiled FITS transaction, publish provenance-bearing RGB products, and
    expose their color previews in Blink before quality weighting and
    registration.
30. Execute strict demosaicing as a bounded FITS transaction. The runtime now
    reads calibrated CFA bands with the exact two-row halo, preserves global
    phase and reflected-edge coordinates, reconstructs one RGB plane at a time,
    and streams canonical planar binary64 output. Its memory reservation depends
    on width and band height rather than full image height. The source is hashed
    before execution and immediately before publication; the complete private
    output must pass dimensional and FITS-checksum readback before atomic
    create-new publication. Cancellation and every pre-publication failure leave
    no destination. Tests prove byte identity across band heights and exact
    agreement with the full-frame oracle. Next, orchestrate per-frame RGB output
    from the reviewed Light plan and expose color Blink previews.
31. Publish reviewed-Light-plan RGB outputs as one transaction. The runtime now
    requires the exact calibrated result bound to the current manifest, master
    plan, and Light plan; validates its complete canonical frame set; verifies
    each calibrated FITS provenance and checksum; takes CFA phase only from the
    reviewed group; and stages every bounded demosaic before set publication.
    A final calibrated-input fingerprint pass detects mutation after an earlier
    frame completed, while cancellation and publication failures roll back the
    complete RGB set. Tests cover exact RGB values and provenance, progress,
    cancellation, late mutation, and staging cleanup. Next, decode the planar
    RGB products into bounded color Blink previews and connect them to the
    existing review controls.
32. Define deterministic linked-color preview mapping. The preview core now
    combines three congruent bounded scalar reductions, estimates one robust
    stretch from common-support linear Rec. 709 luminance, and applies that
    transform unchanged to all channels. Missing support in any plane produces
    the explicit missing-data style for the complete pixel, preventing false
    color. Tests lock channel order, exact RGBA endpoints, common-support
    behavior, and luminance estimation. Next, expose this path through native
    PNG transport and make reviewed RGB Light products the calibrated Blink
    source.
33. Carry reviewed RGB Lights into native color Blink. Preview IPC now uses an
    explicit tagged scalar-plane or planar-RGB interpretation; RGB decoding
    reduces the three canonical planes under identical bounds, estimates the
    linked luminance stretch, and produces one native PNG. Calibrated execution
    automatically follows an all-standard-Bayer frame transaction with the
    whole-plan demosaic transaction, returns both artifact paths, and selects
    RGB in Blink while retaining the raw source identity and decisions. Mono or
    mixed plans stay scalar by policy. Rust tests lock channel order, linked
    estimation, final RGB publication, and FITS axes; frontend tests lock the
    wire shape, cache separation, and visible RGB state. Next, extend
    quality/selection to calibrated RGB without weakening CFA diagnostics, then
    implement registration.
34. Measure calibrated RGB Lights without display contamination. The quality
    core now assembles versioned linear Rec. 709 luminance from exact R/G/B
    planes in enforced order, combines masks conservatively, rejects incomplete
    or mismatched channels, and accepts one decoded channel at a time to avoid
    retaining the full cube. Native quality IPC validates `[width, height, 3]`,
    Review caches RGB metrics by their distinct product path, and serial batch
    measurement works in calibrated Blink. Tests cover exact luminance, channel
    order, masks, native stellar measurement, and the RGB wire request. These
    remain expert diagnostics rather than automatic rejection authority. Next,
    define deterministic registration coordinates and reference selection.
35. Fix registration geometry and automatic reference selection before matching.
    The new registration core defines integer-centered continuous pixel
    coordinates, finite nonsingular source-to-reference affine maps, explicit
    inversion and execution-order composition, and compensated mean/RMS/maximum
    residual evidence. Its `equal-ordinal-ranks-v1` reference policy validates
    FWHM, eccentricity, star count, and noise; preserves all per-metric ranks;
    rejects duplicate identities and bounded-resource failures; and cannot vary
    with input order. The normative contract states clearly that feature
    matching, robust model fitting, resampling, common-footprint calculation,
    and ASI294MC Pro/ToupTek 585C comparison remain release gates. Next,
    implement deterministic star-feature extraction for registration without
    duplicating the quality detector's scientific primitives.
36. Derive matching features from the canonical quality measurements. The
    registration core now validates measured dimensions and exact catalog
    controls, excludes saturated, low-SNR, elongated, and border sources in a
    fixed evidence order, ranks eligible stars deterministically, retains their
    centroid/photometric/shape support, accounts for output truncation, and
    bounds both input and output populations. The catalog stores both algorithm
    identities and its complete parameters, while fallible preallocation keeps
    sorting failures explicit. Synthetic tests exercise ranking, truncation,
    mutually exclusive exclusions, dimensions, margins, and invalid controls.
    Next, build scale- and rotation-invariant local descriptors from these
    catalogs, with ambiguity bounds before correspondence search.
37. Build bounded local triangle descriptors without global combinatorial work.
    High-ranked anchors use a bounded nearest-neighbor set; unique triangles
    retain canonical feature mappings, two side ratios, normalized area,
    absolute scale evidence, and mirror-sensitive orientation. Controls limit
    anchors, neighbors, geometric degeneracy, attempted work, and output count.
    Statistics distinguish short, degenerate, duplicate, and explicitly
    truncated work. Tests prove translation/rotation/scale invariance, mirror
    reversal, canonical 3-4-5 geometry, deduplication, output-limit termination,
    insufficient support, and invalid controls. Next, implement a tolerant
    descriptor index and ambiguity-preserving correspondence hypotheses before
    any transform is accepted.
38. Match invariant descriptors without hiding ambiguity or unbounded work. A
    sorted three-dimensional grid examines only neighboring tolerance cells,
    then verifies exact invariant errors, scale range, and an explicit mirror
    policy. Per-descriptor and global best sets remain bounded and deterministic;
    the result retains canonical feature pairs, normalized errors, scale, mirror
    state, comparison counts, ambiguity counts, both discard counts, and input
    truncation evidence. Tests cover rotation/translation/scale, reflection
    policy, symmetric ambiguity, both retention limits, exhausted comparison
    budgets, empty geometry, and invalid controls. Next, form robust transform
    hypotheses and require consensus across independent feature correspondences.
39. Require deterministic multi-triangle consensus before accepting a
    similarity transform. Every bounded seed is fitted in compensated binary64
    arithmetic and scored only against hypotheses with the same mirror state;
    all three stars must pass the reference-pixel residual threshold. The winner
    is refitted over distinct one-to-one star pairs and rescored, with minimum
    support enforced after refinement. Results expose the transform, scale,
    rotation, reflection, seed, triangle inliers, star correspondences, strict
    residuals, outliers, input truncation, and complete work accounting. Tests
    cover exact rotation/scale/translation, reflection, competing symmetric
    transforms, model truncation, sparse support, exhausted work, validation,
    and frame identity. Next, define confidence and sparse-field policy, then
    compare this strict similarity path on synthetic sub-pixel and real camera
    frames before introducing broader affine or distortion models.
40. Separate deterministic selection from scientific confidence. Consensus now
    retains the best geometrically distinct competing transform using maximum
    displacement across source corners and center, so a symmetric tie remains
    visible. A versioned fail-closed gate reports every failed requirement for
    triangle support, inlier ratio, distinct stars, winner margin, RMS/worst
    residual, source/reference two-axis coverage, reflection, and truncated
    evidence. Tests accept broad precise geometry; reject equal-support symmetry,
    localized stars, and model truncation; and recover known noisy subpixel
    rotation, scale, and translation. Next, exercise these diagnostics on the
    ASI294MC Pro corpus and define camera-backed default thresholds before any
    automatic registration is exposed in the desktop pipeline.
41. Exercise the complete raw-CFA registration chain on the local ASI294MC Pro
    session. A new path-free Rust diagnostic connects strict FITS decoding,
    phase-neutral CFA preparation, quality measurement, features, descriptors,
    matching, consensus, and confidence under one versioned bounded profile.
    Real dense-field evidence exposed colliding but plausible triangle votes;
    consensus now ranks them by support and residual and extracts a deterministic
    bijective star assignment. All nine source-to-reference comparisons pass
    without truncation with 908–1,022 star pairs and 0.213–0.233 detection-pixel
    RMS. Next, compare transforms and registered pixels against an independent
    implementation, validate ToupTek 585C and more diverse fields, then implement
    flux- and mask-tested resampling before desktop automation.
42. Fix the strict registration-resampling oracle before optimizing execution.
    The new normalized binary64 Lanczos-3 path inverse-maps reference pixel
    centers through the explicit source-to-reference transform, preserves exact
    integer samples, retains negative lobes, uses compensated numerator and
    weight sums, and refuses incomplete or unusable non-zero support with exact
    mask evidence. Tests lock planar identity, integer and fractional shifts,
    constant preservation, point-source flux, unclipped output, invalid support,
    footprint accounting, and output validation. Next, create a bounded band
    executor with bitwise oracle comparison, derive the common valid footprint,
    and connect accepted registration plans to atomic FITS publication.
43. Derive autocrop from exact interpolation support rather than image content.
    The bounded common-footprint scanner inverse-maps every discrete reference
    center through every accepted transform, applies the same analytical-zero
    Lanczos boundary rule as resampling, and finds the largest all-covered
    rectangle with width-proportional histogram storage. It excludes pixel masks
    by design so local defects cannot shrink the geometric field, retains exact
    coverage counts, uses documented tie breakers, and reports no-overlap
    explicitly. Next, expose this crop in the registration diagnostic and build
    the bounded band executor and atomic registered-FITS transaction.
44. Bound registered output materialization without changing the numerical
    contract. The Lanczos-3 band executor validates its canvas once, traverses
    top to bottom in global reference coordinates, holds no more than the chosen
    number of output rows, aggregates exact support counts, and returns stable
    completion. Differential tests reconstruct a multi-plane result containing
    boundary loss, sensor flags, and a non-finite input and require bitwise pixel
    equality plus identical masks and counters against the full-image oracle.
    Next, connect bands to windowed FITS input and atomic registered-FITS output.
45. Plan exact file-backed reads for each registered band. The source-window
    planner scans discrete global output centers with the production inverse
    transform and exact non-zero Lanczos taps, returning the smallest source
    rectangle actually required by complete kernels. Integer alignment adds no
    artificial halo, fractional coordinates retain every necessary tap, and a
    disjoint band becomes an explicit no-read result. Next, make the band kernel
    consume these windows and bind it to transactional FITS publication.
46. Execute registration directly from the planned source rectangle. An
    immutable band plan binds complete source and reference dimensions, global
    band coordinates, transform, and exact window. Execution rejects missing,
    unnecessary, or misshaped decoded storage, translates taps with checked
    global-to-local arithmetic, and emits disjoint bands without I/O. A
    multi-plane differential test reconstructs the entire registered result and
    requires bitwise oracle parity for pixels, masks, and support accounting.
    Next, connect this contract to bounded FITS reads and atomic publication.
47. Publish registered frames as strict bounded transactions. The runtime reads
    only each plan's exact rectangle and plane, reserves decode/kernel peaks,
    streams canonical binary64 samples into a private checksummed FITS, validates
    the complete staged product, re-fingerprints the source, and finally uses
    create-new atomic publication. Cancellation, insufficient memory, mutation,
    or failure leaves no destination. Tests require byte-identical output across
    band heights and bitwise equality with the full-image multi-plane oracle.
    Next, expose accepted transforms and common-crop preview in the desktop plan.
48. Expose accepted registration geometry without overstating execution. The
    versioned diagnostic now lifts the CFA detection-plane similarity into exact
    source-pixel coordinates and derives the common Lanczos-3 footprint only
    after the confidence gate passes. A typed Tauri command and desktop
    Registration laboratory select explicit Light identities, reject stale
    asynchronous results, show residual/support evidence and affine
    coefficients, and render the autocrop both graphically and as accessible
    dimensions. The surface remains explicitly diagnostic-only. Next, build the
    immutable multi-frame registration plan that chooses or pins one reference,
    aggregates every accepted transform, and executes the existing atomic FITS
    transaction per reviewed Light.
49. Fix the immutable multi-Light geometry boundary. `registration-plan-v1`
    binds unique reviewed frame identities, one exact identity reference,
    accepted source-to-reference transforms, canonical identity order, and the
    single exact all-frame Lanczos crop. It refuses undersized sets, duplicates,
    a missing or inconsistent reference, bounded footprint failures, allocation
    failure, and empty common support before any output work begins. A
    domain-separated canonical SHA-256 binds every identity, exact transform
    bit, algorithm, coverage decision, and crop without paths. Next, bind
    confidence-gated pair diagnostics to this digest and assign its atomic
    runtime outputs without re-deriving geometry.
50. Bind one registered output to reviewed plan evidence. The runtime now
    re-derives a source's stable review identity from portable relative path,
    byte length, and content SHA-256; obtains transform and reference canvas
    only from the canonical plan; requires the output provenance to carry that
    exact plan digest; and rechecks decoded source dimensions before staging.
    Tests reject missing digest, a substituted portable identity, and dimension
    drift without publishing output. Next, wrap these plan-bound requests in an
    all-or-nothing multi-frame transaction with shared cancellation and progress.
51. Publish an immutable registration plan as one transaction. The runtime now
    requires every reviewed `FrameId` exactly once, orders work and names output
    canonically, stages and checksum-validates every registered FITS privately,
    revalidates the complete source set, then publishes with create-new links
    and rollback. Existing destinations fail before numerical work; cancellation
    after an already staged frame exposes nothing. Shared memory accounting and
    frame-indexed progress cover the complete run. Next, bind the desktop's
    accepted diagnostics into this multi-frame plan and expose an explicit
    review-before-run surface without weakening the confidence gate.
52. Make multi-frame registration progress explicit in the desktop. The dark
    Registration laboratory now retains each accepted source-to-reference
    diagnostic by stable frame identity, shows the reference and every pending,
    accepted, or rejected Light in one accessible plan list, and declares
    readiness only when all non-reference transforms are accepted. Changing the
    reference invalidates the collected evidence; changing only the source does
    not discard unrelated accepted transforms. Next, construct the immutable
    native plan from this complete evidence while preserving the required CFA
    diagnostic to calibrated-linear-RGB execution boundary.
53. Preserve reviewed identity across derived pixel artifacts. FITS provenance
    version 4 adds `AETHFID` for a stable single-frame review identity distinct
    from the immediate-input `AETHINP`. Calibrated CFA and demosaiced RGB outputs
    derive and propagate it from the canonical manifest. Registration can now
    bind plan geometry to a calibrated linear artifact only when its embedded
    identity matches, and propagates the identity to registered outputs. Tests
    reject substitution before publication and execute a complete artifact-
    backed plan. Next, assemble and submit the native plan from the desktop's
    complete accepted evidence and exact RGB artifact set.
54. Seal the desktop registration plan in Rust. Once every pair diagnostic is
    accepted, the web presenter submits only the selected stable Light
    identities. The native command resolves those identities against the
    immutable imported manifest, requires the complete Light set exactly once,
    reruns every raw-CFA diagnostic and confidence gate, and constructs the
    canonical all-frame plan without accepting frontend matrices or dimensions.
    The Registration laboratory shows the returned SHA-256 and exact common
    crop as native sealed evidence. Synthetic shifted-star tests exercise the
    complete FITS-to-plan path; malformed, incomplete, duplicate, and
    confidence-rejected requests fail closed. Next, bind that reviewed digest
    to the calibrated RGB artifact set and execute the existing all-or-nothing
    runtime transaction from the desktop.
55. Execute the sealed registration plan from the desktop. The command accepts
    the reviewed digest, stable identities, and absolute calibrated artifact
    paths, then reconstructs the plan from the immutable manifest before any
    output work. Every artifact is fingerprinted and bound to its embedded
    reviewed identity; RGB color-camera products and calibrated mono products
    share the same strict path. One native execution slot, bounded memory,
    frame-and-band progress, cooperative cancellation, create-new publication,
    and rollback protect the complete set. The dark Registration laboratory
    exposes artifact readiness, destination, progress, cancellation, peak
    memory, and completion without presenting partial products. Tests execute
    a shifted synthetic FITS plan end to end and prove a stale digest publishes
    nothing. Next, bind manual Review acceptance to eligible plan membership and
    add a registered-frame Blink result view.
56. Bind registration membership to the native Review book. Explicitly rejected
    Lights are excluded; accepted and undecided Lights remain eligible so review
    is opt-out rather than silently destructive. Preview and execution snapshot
    this Rust-owned set, require at least two eligible identities, and compare
    the browser's identity list against it exactly. A decision that changes
    membership invalidates pending diagnostics, sealed geometry, execution
    evidence, and artifact readiness in the presenter. Native tests prove that
    rejecting one of a two-Light set makes registration fail closed. Next, add
    a registered-frame Blink result view and crop-aware stack integration.
57. Add a registered-frame Blink result viewer backed by the existing bounded
    native FITS renderer. The presenter binds the complete published set back to
    reviewed identities and rejects partial, duplicate, or foreign results. One
    native stretch is estimated from the first registered artifact and locked
    across manual or timed playback; preview identities include the sealed plan
    digest, the browser cache is bounded by entries and encoded bytes, adjacent
    frames are prefetched, and every object URL is revoked on invalidation.
    Presenter and binding tests cover exact pixel interpretation, stale preview
    suppression, accessible controls, and fail-closed set reconciliation. Next,
    add crop-aware registered-frame integration.
58. Add the strict crop-aware mean primitive. `IntegrationRegion` rejects empty
    or overflowing extents, and `integrate_mean_region` validates containment
    against every equal-sized input before reading source samples directly into
    a cropped planar output. It allocates no full-frame crop copies and retains
    the exact two-pass compensated estimator, mask rules, stable source order,
    and per-pixel support accounting. Tests cover multi-plane coordinate
    mapping, masked and non-finite samples inside the crop, bounds failures, and
    bit identity between the full-frame region and the established entry point.
    Next, stream registered FITS bands through this primitive and atomically
    publish a plan-bound integrated product.
59. Add the plan-bound registered stack runtime. It requires the exact canonical
    frame identity set and output provenance bound to the sealed registration
    digest. Every registered input must carry matching `AETHFID` and `AETHPLN`
    cards, pass full checksum verification, use the reference canvas, and agree
    on mono or planar-RGB shape. The executor reads only common-crop FITS bands,
    applies the stable compensated mean, accounts for its complete logical
    working set, re-fingerprints every source after calculation, validates the
    private output and publishes one create-new FITS atomically. Tests cover the
    exact cropped RGB mean, progress, memory refusal, cancellation between
    bands, late source mutation, stale plan binding, and no-output rollback.
    Next, expose this transaction through the desktop execution panel.
60. Expose registered common-crop integration in the desktop Registration
    laboratory. The adapter reconstructs the current reviewed plan, accepts the
    complete published registered identity set, fingerprints every artifact,
    and delegates all pixel work to the bounded Rust executor. A native
    create-new FITS chooser, typed progress, cooperative cancellation, exact
    output dimensions, and peak-memory evidence are presented in an accessible
    dark instrument panel beneath Registered Blink. Tests cover the IPC shape,
    UI action, and native registration-to-stack transaction. Next, add a
    result preview for the final integrated product and rejection-map products.
61. Render the final integrated FITS inside the Registration laboratory after
    publication. The frontend asks Rust to estimate one display-only transform
    and render a bounded RGB or scalar PNG; it retains only a revocable object
    URL, rejects stale asynchronous responses by plan and output identity, and
    keeps preview failure separate from scientific product validity. The final
    product remains an untouched binary64 FITS. Next, design and persist
    high/low rejection-map products without weakening the strict estimator.
62. Add the first independently versioned robust estimator: deterministic
    percentile-tail rejection followed by the scaled compensated mean. Exact
    support records separate accepted, masked, non-finite, low-rejected, and
    high-rejected samples. Runtime execution supports the estimator in bounded
    bands under `registered-percentile-mean-v1`, including support storage and
    sorting scratch in the memory budget, while strict mean remains the desktop
    default. Next, atomically publish the evidence records as companion maps
    before exposing these advanced controls.
63. Publish percentile rejection evidence as two independent binary64 FITS
    products. Low-tail and high-tail counts preserve source dimensions, planar
    order, and exact integer values under `percentile-rejection-map-v1`.
    Science and both maps are fully staged and checksum-verified before a
    rollback-safe create-new publication set; a late companion collision
    removes every product created by the run while preserving pre-existing
    data. Path, provenance, estimator, memory, checksum, and rollback tests fail
    closed. Next, expose estimator parameters and optional rejection maps in a
    clearly separated desktop advanced mode.
64. Expose robust integration through a collapsed desktop advanced section
    while retaining strict compensated mean as the default. The presenter
    offers the estimator, low/high fractions, retained-sample floor, and an
    explicit rejection-map switch; changing any setting invalidates a stale
    result preview. Typed IPC carries choices without processing pixels, and
    Rust revalidates every value, derives adjacent map paths, binds provenance,
    and executes the rollback-safe transaction. Native, bridge, presenter,
    accessibility, and production-build tests cover the complete path. Next,
    render low/high maps inside the result viewer with linked science/map
    navigation and a false-color rejection overlay.
65. Add linked result-product navigation for Science, Low reject, and High
    reject in the integrated viewer. Companion tabs remain disabled unless the
    native transaction returned their paths. Every selection uses the bounded
    Rust FITS renderer, treats maps as scalar planes, revokes the previous PNG,
    and rejects stale responses against the plan, science output, and selected
    product. Presenter and production-build tests cover availability, selection,
    identity, and accessible tab semantics. Next, add a perceptually uniform
    false-color rejection palette and science-overlay opacity control.
66. Establish a reusable dark instrument-control language without replacing
    native accessibility semantics. Selects retain their platform popup and
    keyboard model behind a consistent beveled face, while number inputs gain
    explicit decrement and increment controls, direct entry, focus treatment,
    disabled-state synchronization, and readable units. The narrow calibration
    console uses full-width control rows instead of compressing decimal values.
    Presenter interaction, accessibility, formatting, production-build, and
    visual-browser checks cover the result. Next, apply the same control system
    to the rejection-map palette and science-overlay opacity control.
67. Make rejection evidence visually actionable without modifying scientific
    products. Rust maps scalar low-tail counts through a Viridis-family palette
    and high-tail counts through an Inferno-family palette; exact zero counts
    are transparent, missing support remains explicit, and RGB requests reject
    diagnostic palettes. The desktop layers the bounded diagnostic PNG over a
    separately rendered science preview with an accessible live opacity control,
    stale-response guards, and deterministic resource revocation. Native,
    bridge, presenter, accessibility, and production-build tests cover the full
    path. Next, add quantitative rejection histograms and per-pixel inspection
    before expanding the estimator family.
68. Add an exact, bounded rejection-count histogram for every published map.
    The native inspector streams fixed-size FITS chunks, accepts only finite
    non-negative integer counts, bounds distinct bins, checks all counters, and
    reports zero, affected, maximum, and per-count sample totals under a stable
    algorithm identifier. The desktop loads this evidence independently from
    the PNG so a diagnostic failure cannot invalidate the scientific product or
    its preview, and renders a bounded accessible distribution below the overlay.
    Native malformed-count, bridge, presenter, accessibility, lint, and build
    tests cover the path. Next, add exact coordinate inspection for science and
    both rejection maps.
69. Add exact coordinate inspection across the integrated science product and
    both rejection maps. A click is mapped through the contained preview into
    full-resolution integer coordinates, while accessible X/Y inputs provide
    the same operation without a pointer. Rust reopens each FITS product, checks
    matching dimensions and plane counts, reads one sample per plane with
    checked offsets, preserves missing science values, and accepts only exact
    non-negative rejection counts. Stale asynchronous results cannot replace a
    newer selection. Native boundary, plane-order, bridge, presenter,
    accessibility, lint, and build tests cover the path. Next, define the
    weighted-integration contract and its versioned quality-weight expression.
70. Define the strict weighted-integration oracle and its first transparent
    quality expression. Frame weights are finite and strictly positive typed
    values; masked and non-finite samples leave both numerator and denominator;
    stable manifest order, independent value/weight normalization, compensated
    `f64` accumulation, exact support accounting, and representable-boundary
    tests make the numerical contract explicit. `balanced-psf-weight-v1` uses
    squared relative stellar SNR, squared inverse relative FWHM, and the
    relative squared minor/major axis ratio, evaluated in the logarithmic
    domain and bound to finite positive binary64. Algorithm identifiers and the
    reference metrics are provenance, and any formula change requires a new
    identifier. Next, bind reviewed frame metrics and weights into the bounded
    registered-stack runtime and write them into product provenance.
71. Execute identity-bound weighted means through the bounded registered-stack
    runtime. A canonical weight set hashes its expression identifier, sorted
    reviewed frame identities, and exact binary64 weights; construction rejects
    missing, duplicated, or foreign identities. The balanced-PSF constructor
    also hashes the exact reference and per-frame quality metrics before
    deriving weights. FITS provenance version 5 adds the optional `AETHPAR`
    digest, and weighted execution requires it to match before any output
    begins. Canonical source/weight ordering, per-band memory accounting,
    streamed checksum publication, and exact weighted-pixel tests cover the
    path. Next, carry measured per-frame SNR into the review model and expose
    the complete weight evidence in the UI.
72. Carry the weighting signal statistic through quality measurement and frame
    review. Rust now reports the exact median background-referenced SNR over
    unsaturated measured stars, validates it as positive optional review data,
    and preserves it across native sort requests. The desktop model and selected
    frame metric strip expose “Stellar SNR” beside FWHM and eccentricity, with
    explicit missing state. Native estimator, review validation, bridge,
    presenter, frontend, lint, and production-build tests cover the path. Next,
    add the advanced weighted-estimator choice with a preflight weight table and
    reference selection before enabling execution.
73. Expose balanced PSF weighting as a fail-closed advanced integration mode.
    The desktop uses one explicit planned-frame reference as the unit weight,
    previews SNR, FWHM, eccentricity, and relative weight for every
    planned identity, and disables execution unless all metrics come from the
    calibrated-Light review set. The browser preview is explanatory only: the
    native adapter validates the complete identity set, reconstructs every
    typed metric, derives the canonical weight set again, and binds its digest
    to `AETHPAR` before opening an output transaction. Incomplete, duplicated,
    foreign, or invalid evidence fails before publication. Native end-to-end,
    bridge, formula, presenter, accessibility, lint, formatting, and production
    build tests cover the path. Next, add automatic reference recommendation
    and explicit expert override without weakening the sealed evidence chain.
74. Add deterministic weight-reference recommendation and expert override.
    Automatic mode selects the planned calibrated Light with the greatest
    balanced signal-resolution-roundness score, breaking exact ties by stable
    frame identity, so the displayed reference has unit weight and all other
    preview weights are at most one. Experts may select any planned frame with
    complete metrics. The selected identity travels separately from the metric
    array; Rust validates that it belongs to the exact sealed set, derives the
    canonical weights from that reference, and rejects reference data on every
    non-weighted request. Formula, presenter, bridge, native adapter, lint, and
    build tests cover the path. Next, expose a native pre-execution digest and
    effective-weight summary so the confirmation surface can be archived before
    a long integration run.
75. Recompute and expose canonical weight evidence before starting a long run.
    A dedicated native preflight validates the lower-case plan digest, unique
    planned identities, complete quality evidence, and the selected reference,
    then returns sorted effective weights plus the exact `AETHPAR` digest. The
    desktop rejects stale or incoherent responses, displays the sealed digest
    before opening the destination chooser, and invalidates it if settings or
    the registration plan change while the chooser is open. Execution uses the
    same Rust validator, and an end-to-end test proves that the preview digest
    is the digest written into the output FITS header. Next, persist the native
    preflight result in the view model and add an exportable integration report.
76. Persist the native weight seal as structured review state. The weighted
    integration surface now shows the Rust algorithm identifier and complete
    SHA-256 evidence digest beside the effective-weight table, and labels the
    exact number of natively sealed frame identities. The seal is hidden unless
    its plan digest and selected reference still match the current complete
    preflight, and state transitions discard it when settings, evidence, or
    execution fail. Presenter coverage proves both visibility and stale-state
    invalidation. Next, export the sealed plan, settings, weights, execution
    counters, and output provenance as a deterministic integration report.
77. Publish a deterministic integration report beside every successful stack.
    The native adapter records the sealed plan and manifest digests, exact
    integration settings, bounded-memory counters, registered source identities
    and fingerprints, optional canonical weights, output dimensions, FITS byte
    accounting, `DATASUM`, and `CHECKSUM` evidence. The report excludes clocks
    and absolute paths, carries a SHA-256 over its canonical payload, uses
    create-new synchronized publication, never overwrites an existing report,
    and rolls back newly produced FITS files if report publication fails. The
    result UI exposes both report path and digest. Native strict, weighted,
    rejection-map, no-overwrite, presenter, lint, and build tests cover the
    path. Next, add a report inspector with schema validation and human-readable
    provenance summaries.
78. Add strict native integration-report inspection. The bounded reader accepts
    only absolute paths and a four-megabyte maximum document, rejects unknown
    fields, unsupported schema or algorithm identifiers, malformed digests,
    duplicate source/product identities, incoherent weighted evidence, missing
    science products, and invalid FITS checksum summaries. It recomputes the
    canonical payload SHA-256 before returning a compact provenance summary.
    Native tamper-detection and bridge-contract tests cover the entry point.
    Next, connect this validator to a reopenable report inspector in the result
    workspace with human-readable source, estimator, memory, and checksum views.
79. Connect native report verification to the integration result surface. A
    dedicated action validates the sidecar in Rust, rejects stale responses if
    another stack replaces the active result, and cross-checks both report and
    plan digests before trusting the returned summary. The dark instrument card
    exposes idle, loading, verified, and failed states with a concise source,
    product, estimator, and schema summary; full paths and digests remain
    selectable evidence. Presenter, accessibility-contract, frontend, lint,
    and production-build tests cover the interaction. Next, expand the report
    inspector into a reopenable result workspace for reports from prior runs.
80. Reopen deterministic reports from prior runs. The integration surface now
    accepts an explicit JSON report chosen through the native file dialog,
    keeps its path separate from the active stack result, and displays no digest
    or provenance summary until the bounded Rust validator accepts the file.
    Loading a prior report neither mutates the current FITS products nor claims
    they belong to the open session. Dialog, presenter, stale-response, lint,
    and build coverage protect the workflow. Next, add a dedicated result
    workspace that can pair a verified report with its referenced FITS products
    and flag missing or checksum-inconsistent artifacts.
81. Verify every FITS product referenced by a reopened integration report. The
    native inspector confines product names to one sibling path component,
    refuses symbolic links and non-regular files, compares exact stream length,
    dimensions, sample count, manifest/plan/algorithm/source provenance, stored
    checksum evidence, and independently recalculated `DATASUM` and `CHECKSUM`.
    Missing, malformed, mismatched, and verified products remain distinct UI
    states so a valid JSON seal cannot imply that its image artifacts are still
    intact. Native tests cover missing products, modified pixels, restored
    checksums, and path traversal; the result card lists each artifact and its
    exact verification state. Next, let the result workspace preview verified
    products directly from a reopened report without attaching them to the
    active processing session.
82. Preview verified products from reopened reports. Native inspection now
    returns the sealed output dimensions required to select RGB or scalar
    rendering without guessing. The desktop constructs a report-scoped preview
    identity, exposes only individually verified science and rejection-map
    products, retains false-color rejection histograms, and clears prior
    resources before switching away from an active result. Archived products
    remain display-only and never replace the processing session's registered
    artifacts. Presenter coverage proves that missing products stay disabled
    and a verified external science product renders under its report digest.
    Next, separate archived result inspection into a focused workspace with a
    source-evidence browser and explicit return to the active session.

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
- Malvar, He, and Cutler, “High-quality linear interpolation for demosaicing of
  Bayer-patterned color images”:
  <https://www.microsoft.com/en-us/research/publication/high-quality-linear-interpolation-for-demosaicing-of-bayer-patterned-color-images/>
- Apache Arrow IPC: <https://arrow.apache.org/docs/format/Columnar.html#serialization-and-interprocess-communication-ipc>
