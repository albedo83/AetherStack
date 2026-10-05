# Local-normalization memory baseline

This document records deterministic reservation evidence, not wall-clock or
resident-set measurements. The public runtime estimator uses the same checked
calculation that execution reserves before decoding pixels.

## Quality-first profile

The baseline uses 128 × 128 cells, at most 4,096 retained pairs per cell,
24,576 stellar candidates, 1,000,000 pairwise slopes, a 128-row application
band, and the complete diagnostic grid. FITS pixels are decoded to binary64 and
carry explicit quality flags.

| Camera geometry | Active cells | Required reservation | Ceiling gate |
| --- | ---: | ---: | ---: |
| ZWO ASI294MC Pro, 4,144 × 2,822 × 1 | 759 | 440,485,472 bytes (420.1 MiB) | below 512 MiB |
| ToupTek ATR585C, 3,840 × 2,160 × 1 | 510 | 310,198,272 bytes (295.8 MiB) | below 512 MiB |

The figures include two active input planes, one 128-row output band, FITS
decode statuses, retained cell samples, accepted and rejected diagnostics,
quality candidates, pairwise-slope scratch, and the atomic writer buffer. They
are intentionally conservative because additive components from different
execution phases are not overlapped away.

Planar RGB reuses the same source/reference/application storage one plane at a
time. Only the small accumulated diagnostic component grows with plane count.

## Regression contract

Runtime tests require both priority-camera geometries to remain below 512 MiB
under this profile, require a three-plane 294 geometry to remain below twice
the one-plane reservation, and prove that every published breakdown component
sums exactly to the reserved total. Any future algorithm that needs more memory
must update the public estimator, desktop preflight, this baseline, and the
scientific rationale in the same change.
