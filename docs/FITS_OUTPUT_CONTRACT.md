# Strict binary64 FITS output

`write_f64_primary` encodes a scientific image as one primary FITS HDU with
`BITPIX=-64`. A one-plane image uses axes `(width, height)`; a multi-plane image
uses `(width, height, planes)`. The core planar row-major representation therefore
maps directly to FITS storage order without transposition.

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
generated-card overflow, and destination I/O errors. An arbitrary stream may
contain a valid prefix after an I/O error. Flushing, durable synchronization,
atomic filesystem publication, provenance cards, and checksums belong to the
higher-level file publisher and are not implied by a successful stream write.

## Atomic create-new publication

`write_f64_primary_atomic_new` creates a unique temporary file in the output
directory, writes through a bounded buffer, flushes it, and synchronizes its
contents. It then creates the destination as a hard link to the complete
temporary file. Hard-link creation fails when the destination exists, so two
concurrent publishers cannot overwrite each other and a reader never observes a
partial destination. The temporary link is removed after publication.

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

Provenance cards and FITS checksums remain higher-level output work and are not
yet claimed by this contract.
