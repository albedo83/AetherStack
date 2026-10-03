# Session diagnostics report contract

Session diagnostics are derived from the native imported manifest and verified
quality-cache recovery results. The browser may request an export destination,
but it never reconstructs report evidence or supplies counts, codes, identities,
or a digest.

## Redaction

The report deliberately excludes the session name, absolute root, absolute
source paths, relative source paths, FITS labels, cache paths, and image
contents. Each issue receives only an ordered token such as `source-000001`, a
stable category, and a stable machine code. Tokens preserve the relationship
between one issue and its category inside a report without providing a mapping
back to a private acquisition name.

The four categories are `classification`, `fits`, `grouping`, and
`quality_cache`. Counts remain complete even when no issue items exist. Quality
cache misses are counted but are not issue items because absence is normal on a
first measurement pass.

## Version and integrity

The JSON envelope has `schemaVersion` 1, a lowercase `reportSha256`, and one
`report` object. The report contains:

- algorithm identity `aetherstack-session-diagnostics-v1`;
- the canonical imported manifest SHA-256;
- complete import and quality-cache counts;
- ordered redacted issue items.

`reportSha256` is SHA-256 over the compact JSON serialization of the typed
`report` object, not over the pretty-printed envelope. The exported envelope is
pretty printed, ends with one newline, and is limited to 16 MiB.

## Publication

The destination must be an absolute `.json` path and must not already exist.
Rust writes and synchronizes a private sibling, publishes it with create-new
hard-link semantics, removes the private file, and synchronizes the parent
directory. Failure after publication rolls the destination back. An existing
file is never overwritten, including under concurrent publication.

The UI reports cancellation as a normal state, success with the item count and
a shortened digest, and failure without claiming that a file was created.

## Required tests

- private session names and absolute or relative FITS paths never appear;
- ordered source tokens and stable codes are deterministic;
- the canonical payload reproduces the envelope digest;
- publication is byte exact and create-new;
- a second publication cannot modify the first report;
- the browser sends only the user-selected destination to Rust.
