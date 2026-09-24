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
3. **Pipeline** — configure quality measurement, selection, registration,
   normalization, integration, drizzle, and outputs;
4. **Run** — review the immutable plan, resource estimate, progress, and
   recoverable checkpoints;
5. **Results** — inspect products, support/rejection maps, metrics, provenance,
   and comparison reports.

The Frames workspace always exposes the four roles as distinct tabs or columns.
Counts, unresolved conflicts, blocking errors, and master readiness remain
visible without opening a settings dialog. Short darks stay under Darks; the
calibration graph shows when a short-exposure dark group calibrates flats.

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

## Dark visual system

Dark is the default theme. Surfaces use a restrained neutral hierarchy with one
accent color for the current action. Success, warning, error, selection, and
frame role are never communicated by color alone. Text and essential icons meet
WCAG 2.2 AA contrast; scientific plots provide distinguishable line styles and
accessible data tables.

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

Visual polish is necessary, but the final authority is the plan: what data is
used, what operation will run, why each automatic choice was made, and how the
result can be reproduced.
