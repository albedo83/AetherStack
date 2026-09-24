# Anonymized astronomy corpus inventory

Inventory date: September 18, 2026.

## Scope and privacy

This inventory covers the main astronomy corpus and a separate dark-frame
library. It records aggregate technical characteristics only. Absolute paths,
user names, sky coordinates, target names, observer details, and source files
are deliberately excluded from the repository.

The production scanner reads files as a stream, limits open descriptors, does
not follow symbolic links, and reads only the primary FITS header during this
phase. Pixel arrays are not loaded.

## Focused processing-validation session

A separate local-only session is the current end-to-end comparison corpus. It
contains 135 raw ZWO ASI294MC Pro FITS files: 10 light frames, 50 long-exposure
darks, 25 flats, and 50 short-exposure darks intended to calibrate the flats.
All 135 primary headers pass strict parsing and describe 4,144 × 2,822,
16-bit, `RGGB` data acquired by N.I.N.A.

The 25 flat files declare a light frame type in their headers while their exact
directory role is flat. Strict classification therefore reports 25 evidence
conflicts. The comparison manifest must record an explicit directory-preference
override rather than hiding this acquisition-software inconsistency.

Short-exposure calibration frames remain classified as darks. Exposure and the
other acquisition fields separate them from the long-exposure dark group and
allow a future calibration planner to associate them with flats. This model
also leaves Bias as a distinct supported role for sessions containing genuine
bias frames.

The local reference processing export contains calibrated, debayered,
registered, master, log, and project-metadata products. Its final integrated
product is in the master category; there is no required `integration` category.
Comparison tooling must discover products from validated metadata and content,
not from a hard-coded directory layout. No source or reference product is
committed to the repository.

## Corpus size

| Extension | Count |
|---|---:|
| `.fits` | 21,170 |
| `.fit` | 3,207 |
| `.xisf` | 266 |
| `.cr3` | 316 |
| `.tif` | 70 |
| `.seq` | 11 |
| `.lst` | 24 |
| other | 58 |
| **Total** | **25,122** |

The main corpus contains 24,377 FITS files across roughly 48 target directories.
Some individual targets contain several thousand files. The separate dark-frame
library adds 631 FITS files: 250 from the ZWO ASI294MC Pro and 381 from the
ToupTek ATR585C.

## Complete primary-header scan

The in-tree Rust scanner processed the entire main FITS corpus:

- 24,377 files discovered and 24,377 primary headers read;
- 24,371 headers strictly conformant;
- 6 interpretable but non-conformant headers;
- 0 structural read failures and 0 traversal failures;
- 24,377 valid checked image layouts and 0 invalid layouts;
- 0 truncated primary arrays and 0 unavailable file sizes;
- 10 files without an instrument identifier.

### Pixel representations and dimensions

| Count | `BITPIX` |
|---:|---:|
| 24,275 | `16` |
| 102 | `-32` |

| Count | Dimensions |
|---:|---|
| 7,522 | 3,840 × 2,160, 2D |
| 2 | 3,840 × 2,160 × 3 |
| 1 | 4,029 × 2,648 × 3 |
| 16,723 | 4,144 × 2,822, 2D |
| 60 | 4,144 × 2,822 × 3 |
| 69 | 6,024 × 4,020, 2D, outside the priority scope |

### Instruments and acquisition software

| Count | Raw instrument identifier |
|---:|---|
| 7,468 | `ATR585C` |
| 50 | `ATR585C(USB2.0)` |
| 16,780 | `ZWO ASI294MC Pro` |
| 69 | DSLR identifier, outside the priority scope |

Canonical priority counts are 7,518 ToupTek ATR585C files and 16,780 ZWO
ASI294MC Pro files. The scan also found 24,302 `RGGB` declarations and 75 files
without a CFA declaration. Acquisition software was declared as N.I.N.A. in
21,162 files and ZWO ASIAIR Plus in 3,136 files; 79 files did not declare it.

## Priority camera profiles

### ZWO ASI294MC Pro color

- 4,144 × 2,822 CFA images;
- `BITPIX=16`, commonly with `BSCALE=1` and `BZERO=32768`;
- `RGGB` CFA pattern;
- gain 120 and offset 30 in the inspected raw sample;
- exposure, sensor temperature, binning, and frame type present;
- newer N.I.N.A. acquisitions also declare filter, set temperature, and Bayer
  offsets;
- acquisition software is absent in some older files.

### ToupTek ATR585C color

- 3,840 × 2,160 CFA images;
- `BITPIX=16`, commonly with `BZERO=32768`;
- `RGGB` CFA pattern;
- raw identifiers `ATR585C` and `ATR585C(USB2.0)` map to one canonical model;
- 2.9 µm pixels;
- gain 120 and offset 512 in the inspected raw sample;
- exposure, acquisition date, measured temperature, and set temperature present
  in the inspected N.I.N.A. sample.

The external library includes darks at 1, 2, 5, 10, 20, 30, 60, and 90 seconds,
plus a 3,840 × 2,160 `float32` master dark.

All 631 dark-library files also have valid image layouts and complete primary
arrays. The library has no structural read failure or traversal failure.

### ToupTek 571M monochrome

This camera belongs to the future functional scope, but representative files are
not present in the current corpus. No keyword mapping, dimensions, gain, offset,
or sample representation will be invented before a real sample is supplied.

## Frame-type evidence

Raw header values include 221 bias, 1,184 dark, 401 flat, and 22,496 light
declarations, plus 75 missing values. Header strings use mixed capitalization.

Classification combines recognized header values, exact parent-directory names,
and explicit file-name prefixes. Strict agreement resolves:

| Resolved type | Count |
|---|---:|
| Bias | 221 |
| Dark | 1,025 |
| Flat | 401 |
| Light | 21,151 |

The scan found 1,506 evidence conflicts and 73 unresolved files:

| Count | Evidence conflict |
|---:|---|
| 158 | header dark, bias directory, dark file-name prefix |
| 1 | header dark, bias file-name prefix |
| 278 | header light, bias directory |
| 463 | header light, dark directory |
| 606 | header light, flat directory |

These results prohibit a global implicit precedence rule. The default policy
requires agreement. Import profiles may explicitly prefer a header, directory,
or file-name source, and the resulting audit record must state that a conflict
was overridden.

## Processed products and FITS conformance

Observed processed products include 2D `float32` masters and three-plane
`float32` color cubes. Dimensions and CFA metadata may disappear or change after
processing; instrument metadata is not consistently retained.

Six products are readable but violate fixed-format alignment requirements for
mandatory FITS cards. Across those products, the scanner reported 32
`non_standard_fixed_value` diagnostics. Independent `fitsverify` checks agreed
with the Rust diagnostics on representative files:

- individual 294MC and 585C raw darks pass strict validation;
- a 585C master dark has five misaligned mandatory cards;
- one three-plane color stack has six analogous errors.

The reader therefore provides both modes:

- strict mode rejects error-level standard violations;
- tolerant mode reads unambiguous values and retains structured diagnostics;
- neither mode rewrites or silently repairs the source;
- every original 80-byte header card remains available for auditing.

The first bounded pixel-reader validation sampled both ends of three real arrays
without retaining their paths or contents: one 4,144 × 2,822 294MC raw dark, one
3,840 × 2,160 585C raw dark, and the 3,840 × 2,160 585C `float32` master dark.
All 24,576 sampled physical values decoded successfully and were finite. This is
a compatibility smoke test; synthetic and independent differential tests remain
the release criterion.

A separate differential check compared eight strategically distributed sample
positions in each of those three arrays against Astropy 7.2.0. All 24 converted
`f64` values matched bit for bit. Astropy independently warned about the known
fixed-card formatting problem in the 585C master. No source path, pixel value, or
private header content from that check is retained in the repository.

The same independent reader then covered all 28,283,168 pixels across the three
arrays. Rust and Astropy reported identical finite/invalid counts, minima,
maxima, and 17-digit reference sums. The Rust totals used deterministic Neumaier
compensation; the independent totals used Python's accurately rounded `fsum`.

Rectangular access was additionally checked on a three-plane `float32` processed
image. A 2 × 2 region from each plane matched Astropy bit for bit, validating the
FITS axis order, plane stride, row stride, and region packing.

## Implementation consequences

1. Apply `BSCALE` and `BZERO` before scientific computation while retaining the
   stored type and stored values for provenance.
2. Never infer frame type from `IMAGETYP` alone.
3. Store raw metadata, canonical values, source keywords, and confidence
   separately.
4. Carry CFA pattern and origin through crops and transforms because either can
   change Bayer phase.
5. Accept 2D monochrome, 2D CFA, and three-plane images in the first pixel
   reader.
6. Represent missing information explicitly; never synthesize a value.
7. Emit a structured diagnostic for each tolerated standard deviation.
8. Exercise 4,144 × 2,822 and 3,840 × 2,160 dimensions without committing the
   private source files.

## Required public fixtures

Public fixtures must be synthetic, minimal, and free of private metadata. They
must cover:

- signed 16-bit storage representing unsigned values through `BZERO=32768`;
- `RGGB` CFA data with and without explicit Bayer offsets;
- a color 585C profile with digital offset 512;
- a monochrome profile without CFA only after a 571M sample is available;
- missing optional keywords and contradictory frame-type evidence;
- 2D `float32` masters and three-plane `float32` images;
- conformant and interpretable non-conformant mandatory cards;
- truncated input, a missing `END` card, and dimensions causing overflow.
