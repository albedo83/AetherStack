# Session manifest contract

The session manifest is the durable boundary between FITS ingestion and later
processing stages. Version 1 records source identity, canonical metadata,
classification evidence and resolution, and exact group membership. It does not
yet describe calibration masters, processing parameters, cache artifacts, or
requested outputs; those fields will be introduced only with their owning
pipeline stages and will require a deliberate schema evolution.

## Versioning and decoding

`schema_version` is mandatory and currently equals `1`. Readers reject an
unsupported version before interpreting version-specific fields. Every object
rejects unknown fields so misspellings and accidental schema drift cannot be
silently ignored.

The in-memory JSON decoder accepts at most 128 MiB. This bound covers the current
large corpus while preventing an untrusted manifest from requesting unbounded
parser storage. The default `serde_json` nesting limit supplies an independent
depth bound.

## Source records

The manifest also records the FITS acceptance mode (`strict` or `tolerant`) used
for the complete session. Error-level conformance diagnostics are incompatible
with a strict manifest even if a caller attempts to assemble one manually.

Each source record contains:

- a UTF-8 path relative to the session root, using `/` separators;
- the exact file length and a lowercase SHA-256 digest;
- two or three non-zero FITS axes in FITS order;
- canonical metadata, including each selected source keyword and confidence;
- every FITS conformance diagnostic in detection order;
- all frame-classification evidence and its conflict state;
- the resolution produced by the manifest-wide classification policy.

Paths reject absolute forms, traversal components, backslashes, control
characters, drive prefixes, trailing spaces or dots, and Windows device names.
ASCII case-insensitive collisions are rejected so a session cannot change
meaning when moved between common filesystems.

`analyze_fits_source` parses the bounded primary header, applies the selected
acceptance policy, rewinds the source, and computes the length and SHA-256 using
64 KiB of fixed scratch storage. It never retains the pixel payload. Processing
stages must still re-check the length and digest if files may have changed after
manifest generation.

## Exact groups

Each group has a portable identifier, one `StrictGroupingKey`, and one or more
source paths. Validation guarantees that:

- every member exists in the manifest and belongs to only one group;
- a group key is not duplicated under another identifier;
- every member has a resolved frame classification;
- rebuilding the exact key from a member's metadata, axes, and resolved frame
  type produces the stored group key;
- every absent grouping field is listed exactly once in
  `accepted_missing_fields`;
- grouping with missing metadata includes a non-empty human rationale.

Missing values therefore remain visible policy decisions. They are never
silently converted into defaults or treated as equal to known values.

## Deterministic output

Before serialization, source records are ordered by path, groups by identifier,
group members by path, and accepted missing fields by their stable declaration
order. Pretty JSON always ends with one newline. Given the same validated
content, discovery order does not change the encoded result.

Floating-point grouping values are finite and retain exact binary64 identity.
Positive and negative zero are canonicalized to the same value. JSON round-trip
tests protect that rule, while NaN and infinity are rejected before encoding.

`generate_manifest` groups only resolved sources whose exact keys have no
missing fields. It leaves unresolved or incomplete sources unassigned for
review. Automatic group identifiers are full lowercase SHA-256 digests of the
versioned canonical binary grouping-key representation.
