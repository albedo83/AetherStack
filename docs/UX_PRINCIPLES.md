# Desktop UX principles

## Product direction

AetherStack uses a modern dark desktop interface designed around a visible
scientific plan. The interface should feel efficient for a first session without
hiding the evidence and controls required by experienced users.

The reference workflow's strongest idea is retained: Bias, Darks, Flats, and
Lights are visibly separate roles. AetherStack replaces dense permanent control
panels with progressive disclosure, an inspectable calibration graph, and
diagnostics that explain every automatic decision.

## Information architecture

The primary workflow has five workspaces:

1. **Frames** — import, classify, group, and resolve evidence conflicts;
2. **Calibration** — build or select masters and inspect their associations;
3. **Registration** — inspect frame-pair geometry, confidence evidence, exact
   affine coefficients, and the analytical common crop before moving pixels;
4. **Run** — review the immutable plan, resource estimate, progress, and
   recoverable checkpoints;
5. **Results** — inspect products, support/rejection maps, metrics, provenance,
   and comparison reports.

The Frames workspace always exposes the four roles as distinct tabs or columns.
Counts, unresolved conflicts, blocking errors, and master readiness remain
visible without opening a settings dialog. Short darks stay under Darks; the
calibration graph shows when a short-exposure dark group calibrates flats.

The Calibration workspace derives its visible products from the native imported
manifest, never from browser-reconstructed groups. Its essential view separates
Bias, Dark, and Flat master cards and shows the exclusive flat pedestal choice.
Advanced disclosure lists every compatible or rejected pedestal candidate and
the stable mismatch reasons. Policy and tolerance edits request a fresh native
plan and visibly replace its digest. Building masters executes that exact plan
in a single native worker: the output location, live product/stage progress,
cancellation state, peak reserved memory, and completed products remain visible.
Changing the imported session or planning tolerances invalidates the prior run
state instead of presenting stale evidence.

The Registration workspace accepts only imported Light identities with native
paths. Reference and source remain explicit, pair changes invalidate stale
evidence, and asynchronous results commit only if both selected identities are
still current. An accepted solution shows source-pixel residuals, inlier count,
coverage, rotation, scale, reflection evidence, exact affine coefficients, and
an accessible text plus graphical common-crop preview. After all pairs pass,
the presenter sends only stable identities: Rust resolves the immutable
manifest, reruns every diagnostic and confidence gate, builds the canonical
all-frame geometry, and returns a visible sealed digest and common crop. The
desktop never treats frontend matrices as scientific evidence. Registered
pixels are enabled only when every sealed identity has a calibrated artifact.
The execution panel shows destination, frame-and-band progress, cancellation,
peak memory, and the final complete product count. A shared native execution
slot prevents calibration and registration memory budgets from multiplying.
Rejecting a Light in Review removes it from the Registration plan immediately;
accepting or clearing it adds it back. Any membership change visibly clears the
old transforms and digest, because rejected frames are excluded by the native
Review book rather than by a cosmetic table filter.
Once the complete registered set is published, the same workspace reveals a
dedicated result Blink viewer. It uses one locked stretch for fair comparison,
shows the reviewed source label and sequence position, supports keyboard-readable
previous, play/pause, and next controls, and never presents a partial output set.

Lights also provide a Review view with a synchronized metric table and image
viewer. Blink keeps zoom, pan, orientation, channel mapping, and display stretch
fixed while switching frames, so a visual comparison cannot be improved or
degraded accidentally by per-frame auto-stretch. Accept, reject, and undecided
states are explicit, keyboard accessible, undoable, and carry a visible reason.
The detailed behavior is defined in
[`FRAME_REVIEW_CONTRACT.md`](FRAME_REVIEW_CONTRACT.md).

High-frequency review uses visible buttons and discoverable keyboard shortcuts;
neither path bypasses the native transaction engine. Modal dialogs and text
entry suspend global shortcuts so an expert workflow never comes at the cost of
predictable keyboard behavior.

The shared Rust interaction model owns review transactions, undo, sealing, table
order, locked display state, and Blink identity transitions without depending on
a desktop toolkit. The initial Tauri surface already delegates deterministic
table sorting, manual decision transactions, and bounded undo to that model and
uses identity-bound native previews. Decision history currently lives for the
lifetime of the imported desktop session; durable restart recovery remains a
separate provenance feature. These invariants must not be approximated in
frontend state.

## Three disclosure levels

### Essentials

Essentials contains the actions needed for a defensible automatic run: import,
frame-role review, output location, quality profile, reference selection, plan
review, and run. Automatic choices are summaries linked to their evidence.

### Advanced

Advanced contains scientifically meaningful controls for grouping, master
construction, calibration, quality metrics, registration, local normalization,
integration, drizzle, and output products. Controls are grouped by stage, not by
implementation crate. Search can locate a setting by name or concept.

### Diagnostics

Diagnostics shows raw and normalized metadata, evidence conflicts, candidate
masters, chosen and rejected matches, numerical metrics, algorithm versions,
cache identity, resource estimates, progress codes, rejection/support maps, and
redacted exportable reports. It never requires reading an uncontrolled log to
understand a blocked run.

The first connected Diagnostics surface is an accessible modal instrument for
session import integrity. It separates mandatory FITS accounting from optional
quality-cache recovery, shows every restored/missing/rejected count, explains
that a cache miss is normal, and gives rejected evidence a textual warning as
well as a distinct status lamp. A bounded issue-evidence list identifies
classification, FITS, grouping, and cache failures with session-relative sources
and stable codes. It renders at most 100 rows, reports the omitted count, and
never exposes an absolute acquisition or cache path. Redacted report export
remains planned.

## Dark visual system

Dark is the default theme. Surfaces use a restrained neutral hierarchy with one
accent color for the current action. Success, warning, error, selection, and
frame role are never communicated by color alone. Text and essential icons meet
WCAG 2.2 AA contrast; scientific plots provide distinguishable line styles and
accessible data tables.

The visual identity may use restrained skeuomorphic cues from precision optical
instruments: recessed image wells, tactile transport controls, illuminated
status lamps, machined panel edges, and visibly pressed selections. These cues
must clarify containment or interaction state. They never replace a label,
reduce contrast, shrink a target, or imitate a control that has no implemented
action.

Typography and spacing scale with the operating-system accessibility settings.
Dense frame tables may use a compact mode, but primary actions and form controls
keep a minimum 44 by 44 logical-pixel target. Long paths and identifiers elide
in the middle and remain available through an accessible detail view.

## Interaction rules

- Keyboard traversal follows visual and processing order.
- Every icon-only action has an accessible name and tooltip.
- Focus is visible on every interactive element.
- Sorting never changes deterministic processing order unless the user confirms
  an explicit reorder operation.
- Destructive changes support undo before a run; cache removal previews scope.
- Validation appears beside the affected field and in the stage summary.
- Disabled controls explain the missing prerequisite.
- Units are always visible and locale-aware input is normalized explicitly.
- A reset restores versioned defaults for one section and previews the changes.
- Progress uses completed/total work and stage names, not a fabricated smooth
  percentage.
- Cancellation states which artifacts are complete, reusable, or unpublished.

## Scientific controls

Every setting shown in production has:

- a stable typed backend parameter;
- a unit and valid domain;
- an explained default or automatic selection rule;
- a visible effect on the pre-run plan;
- serialized provenance;
- boundary, invalid-input, and algorithm tests;
- a documented relationship to the strict reference implementation.

Automatic mode must resolve to concrete parameters before the run starts. The
user can inspect those parameters without switching the project permanently to
Advanced mode. A control that is not implemented does not appear to work; the
plan reports the unsupported stage and blocks execution.

The Review viewer includes an on-demand exact FITS statistics dialog for real
sources. It distinguishes complete-array pixel moments from astronomical
quality metrics, reports excluded samples explicitly, and performs no browser
side numerical reconstruction.

## Performance without quality loss

The default profile targets maximum validated quality. Execution optimizations
such as larger tiles, parallel scheduling, SIMD, cache reuse, and GPU kernels may
improve speed without changing the scientific plan. Any path with a non-zero
numerical tolerance is identified by backend and error budget, compared against
the strict `f64` CPU oracle, and selectable only where its validation gate has
passed.

Before execution, the Run workspace estimates peak memory, temporary disk,
output disk, and major work units. If the requested plan does not fit, the UI
offers scientifically equivalent execution changes first. It never silently
drops frames, disables rejection, changes interpolation, or lowers precision.

## UX verification

The desktop release gate includes:

- keyboard-only completion of import, conflict resolution, plan review, run,
  cancellation, and result inspection;
- screen-reader labels, roles, live progress, and error announcements;
- contrast and non-color-state checks in dark and light high-contrast variants;
- layout checks at supported window sizes and enlarged text scales;
- deterministic view-model tests for every automatic decision and conflict;
- component tests for ranges, units, reset behavior, and disabled prerequisites;
- end-to-end tests proving that the reviewed plan equals serialized provenance.

The initial native importer already keeps acquisition-software contradictions
visible. When a structured directory import explicitly prefers the nearest role
directory, the affected frame carries a labeled warning marker and the session
status reports the override count. A color change alone never communicates that
decision.

Visual polish is necessary, but the final authority is the plan: what data is
used, what operation will run, why each automatic choice was made, and how the
result can be reproduced.

Registration treats a successful pair solve as evidence, not as completion.
The plan review lists the fixed reference and every other Light with explicit
pending, accepted, or rejected text alongside its visual state. A counter names
the accepted and required transform totals. Selecting another source preserves
unrelated evidence, while replacing the reference invalidates all transforms
because their coordinate system has changed. Pixel execution remains absent
until the complete reviewed plan can be bound to calibrated linear products.
