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
