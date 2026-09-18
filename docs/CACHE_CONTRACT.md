# Verified artifact cache

The `aether-cache` crate stores immutable processing artifacts for later
checkpoint and restart support. Cache lookup is never accepted solely because a
file exists. Every artifact is structurally validated and its complete payload
is hashed before a reader is returned.

## Operation keys

`CacheKey` is a lowercase SHA-256 value derived from:

- the fixed `aetherstack-cache-key-v1` prefix;
- the byte length and bytes of a canonical domain identifier;
- the byte length and exact canonical operation descriptor.

Explicit lengths make concatenation unambiguous, and domains separate unrelated
schemas. A domain starts with a lowercase ASCII letter and may contain lowercase
letters, digits, `.`, `_`, and `-`, up to 64 bytes. The caller owns the operation
descriptor schema and must include every scientific parameter, input digest,
execution profile, and algorithm version that can affect the result.

Keys are sharded by their first two hexadecimal digits. An artifact is stored as
`<root>/<first-two>/<full-key>.artifact`. Keys contain no source path or target
name, so the portable cache layout does not disclose private acquisition paths.

## Artifact format

Version 1 uses one fixed 52-byte big-endian header followed immediately by the
payload:

| Bytes | Field |
| ---: | --- |
| 8 | ASCII magic `AETHCACH` |
| 4 | unsigned format version |
| 8 | unsigned payload byte length |
| 32 | binary SHA-256 of the payload |

The complete file length must equal `52 + payload_length`. Truncation and
trailing bytes are both errors. The payload format belongs to the operation
domain rather than to the cache container.

## Publication

`ArtifactStore::publish` creates a unique temporary file in the final shard,
writes a placeholder header, and streams the source through a fixed 64 KiB
buffer while calculating its exact length and SHA-256. It then fills the header,
flushes buffered bytes, synchronizes the file, and publishes with a hard link.

Hard-link creation provides create-new semantics. An existing key is never
replaced:

- an existing verified artifact with the same length and digest is reused;
- different verified bytes produce a key-collision error;
- a corrupt existing artifact produces an explicit validation error.

Temporary files are removed by an RAII guard after every pre-publication error.
On Unix, shard metadata is synchronized after a new link is published. On other
supported systems, the complete file is still atomically visible and its bytes
are synchronized, while portable directory synchronization is unavailable in
the Rust standard library.

## Lookup and failure boundaries

`open_verified` rejects symbolic links, directories, malformed magic, unknown
versions, impossible or mismatched lengths, read failures, and digest mismatch.
It hashes the complete payload before returning a reader bounded to the declared
payload length.

The store does not delete or repair corrupt entries automatically. Recovery
policy belongs to a higher layer because deletion changes shared cache state.

The strict runtime uses this store for versioned integrated-tile checkpoints.
It publishes them only after final source-fingerprint verification, can reuse
them after cancellation, and treats invalid entries as hard errors. Streaming
final FITS output remains separate Phase 3 work.
