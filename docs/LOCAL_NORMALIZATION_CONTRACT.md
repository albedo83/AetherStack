# Local normalization contract

Local normalization adapts a registered source image to an explicitly selected
reference without allowing display transforms, rejected pixels, or protected
astronomical structures to bias the scientific model. The implementation is
split into independently testable sampling, local fitting, spatial
regularization, application, and publication stages.

## Local affine model

The first strict oracle fits

`reference = scale × source + offset`

inside one spatial cell. Callers provide only clear, finite source/reference
pairs from identical registered coordinates. Mask construction and spatial
sampling stay outside the estimator and must retain their own support evidence.

`local-theil-sen-affine-f64-v1` uses a bounded Theil-Sen slope followed by a
median intercept. Both axes are normalized before fallback slope construction
so opposite-sign binary64 extremes cannot overflow pair differences. Exact
finite direct differences are preferred to preserve simple affine relations.
Duplicate source values do not create slopes, but remain part of intercept and
residual accounting.

The result records the input count, usable pairwise-slope count, and median
absolute residual. A configured lower bound on `abs(scale)` rejects locally
flat or ill-conditioned solutions before they can amplify noise. Applying a
model rejects non-finite input and output instead of clipping either value.

## Masked spatial sampling

`local-cell-priority-sampling-f64-v1` partitions a selected registered plane
into a row-major grid with edge-clipped cells. Source, reference, and optional
protected-region masks must have identical dimensions. Every pixel is assigned
to exactly one category using a fixed precedence: protected region, source
quality mask, reference quality mask, non-finite pair, or eligible pair. This
accounting makes sparse or contaminated cells visible instead of silently
changing their estimator support.

Each cell retains at most the configured number of eligible pairs. Selection
uses a stable coordinate-derived priority and the retained coordinates are
returned in canonical row-major order. Repeated runs and different scan timing
therefore produce identical samples without allocating storage proportional to
the full cell. The cell count and retained samples per cell both have explicit
pre-allocation limits.

## Resource and failure boundaries

The caller sets minimum and maximum sample counts and a hard maximum number of
pairwise slopes. Work that exceeds any bound fails before allocating scratch
storage. Empty variation, allocation failure, non-finite coefficients, and an
unsafe near-zero scale are distinct typed failures.

The current contract covers sampling and the per-cell oracle. Detection or
construction of protected-source masks, surface regularization, interpolation,
full-frame application, provenance, and bounded FITS publication remain
subsequent milestones and must not alter either algorithm silently.
