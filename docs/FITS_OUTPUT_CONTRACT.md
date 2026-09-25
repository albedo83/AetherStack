# Strict binary64 FITS output

`F64PrimaryStreamWriter` incrementally encodes one primary FITS HDU with
`BITPIX=-64`. A one-plane image uses axes `(width, height)`; a multi-plane image
uses `(width, height, planes)`. The core planar row-major representation
therefore maps directly to FITS storage order without transposition.

Callers provide consecutive chunks in exact FITS order. Chunk boundaries are
not represented in the file and may correspond to rows, full-width tile bands,
or a complete image. The writer counts every accepted sample, rejects a chunk
that would exceed the dimensions declared in the header, and refuses to finish
an incomplete stream. `write_f64_primary` remains the complete-image convenience
API and uses the same incremental implementation.

Mandatory logical and integer values use fixed FITS columns. Every card occupies
exactly 80 bytes, the header ends with `END`, unused header bytes are spaces, and
the data unit is padded with zero bytes. Both units end on 2,880-byte boundaries.

## Sample representation

A clear finite input value is written bit-for-bit as IEEE 754 binary64 in
big-endian byte order. No `BSCALE` or `BZERO` conversion is applied.

A sample with any quality flag, or an unflagged NaN or infinity, is replaced by
the canonical quiet-NaN payload `0x7ff8000000000000`. The write summary reports
the exact number of substitutions. This rule provides one stable byte-level
representation for unavailable output pixels while the in-memory mask retains
the detailed reason before serialization.

The reader supports `BITPIX=-64` directly, so synthetic output tests reopen the
stream, validate the header and image descriptor, and compare every finite value
by its binary64 bits.

## Failure boundary

The stream writer reports size overflow, internal image-invariant failure,
sample-count mismatch, generated-card overflow, and destination I/O errors. An
arbitrary stream may contain a valid prefix after an I/O error. Flushing,
durable synchronization, and atomic filesystem publication belong to the file
publisher and are not implied by a successful stream write. FITS checksums
remain future work.

## Processing provenance

`FitsOutputProvenance` validates identifiers before output begins. The manifest
digest is exactly 64 lowercase hexadecimal digits. The group identifier follows
the session manifest's portable form: one to 64 ASCII letters, digits, `.`, `_`,
or `-`. The versioned algorithm identifier is limited to 32 ASCII bytes and uses
only lowercase letters, digits, `.`, `_`, and `-`. Its first byte must be a
lowercase letter or digit. A product must represent at least one source image.

`write_f64_primary_with_provenance` emits version 3 of these cards before `END`:

| Card | Meaning |
| --- | --- |
| `CREATOR` | AetherStack package name and version |
| `AETHVER` | Version of this provenance-card contract |
| `AETHMAN` | SHA-256 of the canonical session-manifest bytes |
| `AETHPLN` | Optional SHA-256 of the canonical product-driving plan bytes |
| `AETHGRP` | Exact session group identifier |
| `AETHALG` | Versioned integration algorithm identifier |
| `AETHSRC` | Number of source images represented by the product |
| `AETHINP` | Optional exact source SHA-256 for a single-frame product |

`AETHPLN` is mandatory for plan-driven calibration products and absent for
products that do not depend on a plan. A master product stores its canonical
master-plan digest. A calibrated integrated Light product stores its canonical
Light-plan digest; that plan already binds the exact master-plan digest. The
card therefore binds pixels transitively to every matching policy, tolerance,
selected master, selected pedestal, and recorded candidate diagnostic.

`AETHINP` is accepted only when `AETHSRC = 1`. It makes a calibrated individual
frame self-identifying without exposing its local path or acquisition filename.
The runtime requires it to equal the signal fingerprint before single-Light
calibration begins.

The corresponding atomic API places the same cards in the synchronized
temporary stream before publication. Provenance deliberately contains stable
identifiers rather than source paths, target names, observer details, or other
private acquisition metadata.

## Atomic create-new publication

`AtomicF64PrimaryStreamWriter` creates a unique private file in the output
directory and accepts incremental chunks through a bounded buffer. `finish`
completes padding and flushes all bytes while leaving the destination absent.
The resulting `CompletedAtomicFits` can provide a read-only clone for bounded
validation and statistics before publication.

`publish` synchronizes the completed file, closes its write handle, and creates
the destination as a hard link to the complete private file. Hard-link creation
fails when the destination exists, so two concurrent publishers cannot
overwrite each other and a reader never observes a partial destination. The
temporary link is removed afterward. `write_f64_primary_atomic_new` is the
complete-image convenience API over the same two-phase mechanism.

## `DATASUM` and `CHECKSUM`

Atomic output implements the registered FITS checksum convention. Every data
byte is accumulated while it is streamed as big-endian 32-bit words using
one's-complement end-around carry. Zero data padding is included. At finish:

1. `DATASUM` receives the unsigned decimal checksum of the padded data records
   as a quoted string;
2. the header is checksummed with `CHECKSUM` initialized to 16 ASCII zeroes;
3. the complement of the combined header and data checksum is encoded using the
   recommended 16-character alphanumeric algorithm;
4. the completed header is patched inside the still-private staging file;
5. the implementation verifies that the complete HDU now sums to
   one's-complement negative zero before returning it for readback.

The generic write-only stream API cannot safely backpatch a header and therefore
does not claim embedded checksums. The atomic seekable writer generates both
cards by default. The write summary returns the numerical data checksum and the
encoded complete-HDU checksum when they were generated.

`PrimaryImageReader::verify_checksums` independently rereads the exact padded
header and data ranges using fixed-size storage. For each keyword it distinguishes
absence, an explicitly undefined value, malformed syntax, a mismatch, and a
valid result. Truncated padding is an I/O failure rather than a checksum
mismatch. Verification restores the caller's stream position on success.

Unit tests cover arbitrary streaming chunk boundaries, end-around carry, the
registered encoding example, corruption, malformed and undefined values, and
truncation. A generated output was also accepted independently by CFITSIO's
`fitsverify` and Astropy's checksum and data-checksum verification. The
convention detects likely accidental corruption; it is not a cryptographic
authenticity mechanism.

This API deliberately does not replace an existing artifact. Replacement needs
a separate policy because portable standard-library rename behavior differs
between supported operating systems. On Unix, directory metadata is synchronized
after publication. Other supported systems retain atomic visibility and synced
file contents, but the Rust standard library does not expose an equivalent
portable directory handle.

Errors before publication leave no destination and the temporary guard attempts
cleanup. Errors during temporary-link cleanup or directory synchronization
report that the destination is already published, preventing a caller from
mistakenly retrying under another name.

The implementation follows the
[registered FITS checksum convention](https://fits.gsfc.nasa.gov/registry/checksum.html).
