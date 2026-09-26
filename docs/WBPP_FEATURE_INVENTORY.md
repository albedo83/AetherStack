# Preprocessing reference feature inventory

## Purpose

This document records the controls visible in the reviewed Weighted Batch
Preprocessing 3.0.1 interface. It is a product and test inventory, not a claim
that AetherStack already implements every item and not a plan to reproduce the
reference interface visually.

The non-regression rule is strict: AetherStack must not present a production
workflow as equivalent while it has lower scientific quality, weaker failure
reporting, or missing essential control. A simpler default experience is
acceptable only when the automatic decision is inspectable, reproducible, and
overridable in Advanced mode.

## Status vocabulary

| Status | Meaning |
| --- | --- |
| Implemented core | A tested engine contract exists; a desktop control may not exist yet. |
| Planned | Required for the corresponding production workflow. |
| Research-gated | Must not ship until its method, bounds, and validation data are defensible. |
| Not applicable | Intentionally replaced by a documented design with at least equal capability. |

No disabled placeholder control should appear in a release build. Unsupported
operations remain explicit in the processing plan and prevent a misleading run.

## Frame roles and grouping

The primary workspace keeps four independent roles visible: **Bias**, **Darks**,
**Flats**, and **Lights**. A short dark is still a dark. Its exposure and other
acquisition metadata can make it the best calibrator for a flat, but the file is
not silently rewritten as a fifth frame type.

Real bias frames remain supported for users and cameras that require them. A
calibration plan chooses a matched short dark or a bias according to an explicit
versioned policy. It never subtracts both blindly.

| Reference capability | AetherStack requirement | Status |
| --- | --- | --- |
| Separate Bias, Dark, Flat, and Light views | Preserve roles during import and expose independent counts, groups, masters, and diagnostics. | Native directory import, four role tabs, and separate Bias/Dark/Flat master cards implemented; deeper group editing planned |
| Add a directory, arbitrary files, or a role-specific selection | Support drag-and-drop, folder import, and explicit role assignment without losing header evidence. | Native structured-directory import implemented; arbitrary files, drag-and-drop, and explicit reassignment planned |
| Hierarchical groups | Group by checked dimensions, binning, exposure, filter, gain, offset, temperature policy, camera, and configurable keywords. | Grouping core implemented; UI planned |
| Master detection | Detect a master from metadata and product provenance, never from a filename alone. | Planned |
| Selection, inverse selection, remove, and clear | Provide keyboard-accessible bulk operations with undo before a run. | Planned |
| Astrometry columns for lights | Show observation time and future solved-coordinate status without making coordinates public provenance. | Planned |
| Custom frame roles | Admit extensions only through a typed plugin or pipeline contract. | Research-gated |
| Overscan per calibration role | Preserve overscan geometry and apply a tested section policy. | Research-gated |

Conflicting evidence is never hidden. For example, a file located in a flat
directory but declaring itself as a light remains a conflict until the user or
an explicit import profile chooses a source of truth. The resulting override is
stored in provenance.

## Global session controls

| Reference capability | AetherStack requirement | Surface | Status |
| --- | --- | --- | --- |
| Speed or quality presets | Presets may change execution strategy; any scientifically approximate path is labeled and compared with the strict oracle. | Essentials | Planned |
| Additional grouping keywords | Typed keyword rules with preview, validation, ordering, and provenance. | Advanced | Planned |
| Keyword prefix and suffix rules | Use a constrained transformation model rather than arbitrary hidden text rewriting. | Advanced | Planned |
| FITS orientation | Show source orientation and the exact output transform. | Advanced | Planned |
| Detect masters from path | Replace path-only detection with evidence scoring and explicit conflicts. | Advanced | Planned |
| Rejection maps | Generate low/high rejection, support, and reason maps with versioned semantics. | Advanced | Planned |
| Preserve white balance | Preserve raw channel scaling only when selected; never bake an undocumented display balance into science data. | Advanced | Planned |
| Save groups | Persist the complete validated session manifest automatically. | Essentials | Manifest core implemented |
| Smart naming | Preview collision-safe, portable names derived from non-private group identifiers. | Advanced | Planned |
| Cache purge | Inspect size, provenance, verification state, and affected stages before removal. | Diagnostics | Cache core implemented; UI planned |
| Reference image: automatic or manual | Show the ranking evidence and allow a pinned reference per group. | Essentials / Advanced | Planned |
| Output directory | Validate capacity, permissions, collision policy, and private staging location before a run. | Essentials | Atomic create-new core implemented |
| Diagnostics | Provide a dedicated view with stable codes, evidence, suggested actions, and exportable redacted reports. | Diagnostics | Structured core diagnostics partially implemented |

## Shared master integration controls

Bias, dark, and flat masters need the same integration foundation, with
role-specific policies layered on top.

| Reference control | AetherStack requirement | Status |
| --- | --- | --- |
| Combination | Average, median, and any future estimator are separately versioned algorithms. | Role-tagged `strict-mean-v1` master construction implemented; others planned |
| Automatic rejection | Automatic selection must publish the chosen algorithm and parameters before execution. | Planned |
| Percentile low/high | Expose units, valid ranges, minimum frame support, and exact small-sample behavior. | Planned |
| Sigma low/high | Implement deterministic robust location/scale estimation and tested iteration limits. | Planned |
| Linear-fit low/high | Define regression, normalization, clipping, degeneracy handling, and precision. | Research-gated |
| Generalized ESD outliers/significance | Validate assumptions and small-sample behavior before exposure. | Research-gated |
| RCR limit | Define the referenced robust Chauvenet method and compare against controlled outliers. | Research-gated |
| Minimum weight | Reject or report frames below a versioned weight floor. | Planned |
| Reset to defaults | Restore versioned scientific defaults and show which values changed. | Planned UI |

Every numerical control must declare its unit, inclusive or exclusive bounds,
default rationale, algorithm version, effect on provenance, and tests at both
valid boundaries. A value copied from the reference interface is observational
evidence, not automatically an AetherStack default.

## Bias and dark controls

| Reference capability | AetherStack requirement | Status |
| --- | --- | --- |
| Group darks by exposure | Exact exposure is part of the master key; tolerance is explicit and unit-bearing. | Grouping core partially implemented |
| Exposure tolerance | Use a documented matching interval and report equally good or missing candidates. | Inclusive unit-bearing planner control implemented |
| Dark optimization threshold | Scaling is off unless the selected model is validated for the camera and data. | Research-gated |
| Optimize master dark | Expose the fitted scale, residuals, safeguards, and fallback behavior. | Research-gated |
| Bias master | Build and match a true bias independently of darks. | Versioned planning, matching, strict `f64` construction, and transactional publication implemented |
| Short-dark use for flat calibration | Match exposure, gain, offset, temperature policy, binning, dimensions, and CFA state; show every candidate decision. | Versioned planning, candidate diagnostics, strict `f64` pedestal correction, and transactional flat construction implemented |
| Long-dark use for light calibration | Prefer exact exposure and acquisition conditions; disclose any tolerated mismatch. | Versioned association and transactional native execution implemented with exact exposure, explicit temperature tolerance, unique-best selection, complete mismatch evidence, selected-master provenance verification, and pre-publication source revalidation |

## Flat controls

| Reference capability | AetherStack requirement | Status |
| --- | --- | --- |
| Group by filter and exposure | Include camera, dimensions, binning, gain, offset, CFA state, filter, and exposure policy. | Grouping core partially implemented |
| Bias or dark association status | Show the chosen calibrator, rejected candidates, evidence, and blocking mismatches. | Native association model and Calibration candidate disclosure implemented |
| CFA flat state | Preserve CFA pattern and phase through calibration. | Planned |
| Large-scale rejection high/low | Define structures eligible for rejection without suppressing real illumination gradients. | Research-gated |
| Large-scale layers and growth | Bind scale parameters to image dimensions and test synthetic gradients and dust shadows. | Research-gated |
| Flat normalization | Use robust finite unmasked statistics and fail on unsafe normalization support. | Exact positive-sample median, bounded plan execution, provenance, native preview, output selection, typed progress, and cancellation implemented |

## Light calibration controls

| Reference capability | AetherStack requirement | Status |
| --- | --- | --- |
| Calibration association matrix | Display status for bias, dark, flat, optimization, CFA, and output pedestal for every light group. | Native Dark/Flat association matrix implemented with Ready/Blocked states, complete candidate evidence, and gated transactional Light execution; optimization and output-pedestal columns remain research-gated |
| Automatic bias, dark, and flat selection | Produce an inspectable plan with deterministic tie-breaking and no silent fallback. | Deterministic master and Light plans, explicit policies/tolerances, exclusive selection, candidate evidence, desktop preview, and transactional native execution implemented |
| Output pedestal: automatic or explicit | Define storage purpose, units, clipping interaction, and reversibility. | Research-gated |
| Cosmetic correction: automatic | Generate a defect model with evidence and a before/after diagnostic map. | Planned |
| Cosmetic high-sigma threshold | Define the estimator and preserve rejected-pixel reasons in masks. | Planned |
| Cosmetic correction template | Use a versioned reusable profile whose applicability is validated against camera metadata. | Planned |
| CFA images and mosaic pattern | Auto-detection must show raw keyword evidence and allow an explicit override. | Metadata core partially implemented |
| Debayer method | Provide versioned methods with CFA-phase, edge, color, and artifact tests. | Strict deterministic `f64` Malvar-He-Cutler oracle implemented for all standard Bayer phases with exact-sample, coefficient, border, overshoot, and mask tests; bounded tiled FITS execution, RGB Blink presentation, representative-camera differential validation, and additional selectable methods remain |
| Calibration diagram | Show the actual dependency graph, selected masters, parameters, warnings, and cache reuse. | Native dependency cards, selected pedestal, warnings, plan digest, and candidate disclosure implemented; graphical edges and cache reuse planned |

## Light post-calibration pipeline

| Reference capability | AetherStack requirement | Status |
| --- | --- | --- |
| Linear defect correction | Detect and correct row or column defects conservatively with reason masks. | Planned |
| Subframe weighting | Calculate documented quality metrics and preserve every metric and expression. | Strict global background and stellar-shape primitives implemented; weighting expression planned |
| Frame selection | Support automatic thresholds and an interactive review without changing metrics. | Strict metrics, phase-neutral CFA diagnostic measurement, review decisions, undo, stable Rust sorting, Blink state, bounded FITS previews, and accessible desktop Review surface implemented; ToupTek validation, persistent desktop decisions, automatic rule evidence, and CFA/RGB preview remain |
| Image registration | Provide robust matching, transforms, interpolation, and residual diagnostics. | Planned |
| Local normalization | Fit guarded background and scale models with inspectable samples and residuals. | Planned |
| Image integration | Provide deterministic weighted robust estimators, support maps, and rejection maps. | Planned |
| Autocrop | Derive the maximal valid common footprint and preview its coordinate transform. | Planned |
| Automatic integration mode | Publish the exact estimator, rejection method, and chosen parameters. | Planned |
| Astrometric solution | Keep solving optional and handle offline/catalog failure explicitly. | Research-gated |

## Detailed advanced controls

### Frame selection

The reviewed reference surface offers thresholds for FWHM, eccentricity, PSF
signal weight, median, detected-star count, and a custom expression. AetherStack
will expose a typed expression language only after each metric has a unit,
uncertainty, valid domain, and tested missing-value behavior. Comparators and
thresholds are stored in the manifest; the UI shows which rule rejected each
frame.

### Image registration

The reviewed controls include pixel interpolation, clamping threshold, maximum
stars, distortion correction, maximum spline points, rigid transformations,
detection scales, minimum structure size, hot-pixel removal, noise reduction,
sensitivity, peak response, bright threshold, maximum distortion, clustered
sources, and triangle-similarity matching.

AetherStack groups these into **detection**, **matching**, **transform model**,
and **resampling**. A control appears only if it changes an implemented method.
Registration release gates include synthetic ground truth, sub-pixel residuals,
flux behavior, mask propagation, sparse fields, crowded fields, rotation,
mirroring, partial overlap, and deterministic failure diagnostics.

### Local normalization

The reviewed controls include optional normalized-image generation, reference
generation from an integration of best frames, maximum integrated frames,
evaluation criterion, grid size, scale-evaluation method, PSF type, growth
factor, maximum stars, minimum detection SNR, clustered-source handling, and low
and high clipping levels.

AetherStack additionally requires a preview of sample support, fitted surfaces,
residuals, protected regions, and near-zero scale safeguards. Generating extra
images is an output choice, not a prerequisite for the internal model.

### Image integration

The reviewed controls include combination method, minimum weight, automatic or
explicit rejection, percentile thresholds, sigma thresholds, linear-fit
thresholds, ESD parameters, RCR limit, and independent high/low large-scale
rejection layers and growth.

AetherStack treats each combination/rejection pair as a versioned algorithm.
Automatic mode resolves to an explicit plan before processing. Rejection maps,
accepted support, per-frame weights, and failure reasons are first-class output
diagnostics.

## Validation against the reference workflow

The comparison unit is a processing product, not a directory name. A reference
export may contain calibrated, debayered, registered, master, log, and project
products without using an `integration` directory. Discovery must therefore use
validated metadata and file content rather than a hard-coded folder layout.

For a representative camera session, release comparisons will record:

- accepted and rejected frames with reasons;
- selected calibration masters and all matching tolerances;
- calibration residual and flat-field statistics;
- CFA phase and debayer edge/color artifact tests;
- registration residuals and common valid footprint;
- background, noise, FWHM, eccentricity, star count, and photometric stability;
- integrated support and low/high rejection maps;
- finite-value counts, robust summary statistics, flux error, and deterministic
  output fingerprints;
- wall time, peak resident memory, temporary-disk use, cache reuse, and restart
  behavior.

A performance win never excuses a scientific regression. An optimized CPU or
GPU path remains disabled for a stage until differential tests satisfy that
stage's published error budget against the strict CPU oracle.
