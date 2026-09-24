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

`write_f64_primary_with_provenance` emits these cards before `END`:

| Card | Meaning |
| --- | --- |
| `CREATOR` | AetherStack package name and version |
| `AETHVER` | Version of this provenance-card contract |
| `AETHMAN` | SHA-256 of the canonical session-manifest bytes |
| `AETHGRP` | Exact session group identifier |
| `AETHALG` | Versioned integration algorithm identifier |
| `AETHSRC` | Number of source images represented by the product |

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

FITS `CHECKSUM` and `DATASUM` cards remain future output work and are not yet
claimed by this contract.
