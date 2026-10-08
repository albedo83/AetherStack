# Automatic Integration Contract

## Purpose

Automatic integration is a deterministic planning layer over AetherStack's
explicit, tested estimators. It is not a separate pixel algorithm and it never
passes an unresolved `auto` value into integration.

The contract identifier is `automatic-integration-plan-v1`.

## Inputs and decision table

Version 1 consumes only the number of identities in the sealed registration
plan. It does not infer quality weights or thresholds from filenames, camera
brands, incomplete metrics, or display previews.

| Registered frames | Population tier | Explicit estimator           | Reason |
| ----------------- | --------------- | ---------------------------- | ------ |
| 1–2               | `minimal`       | Strict compensated mean      | Reliable outlier classification is impossible. |
| 3–7               | `small`         | Exact median                 | The exact finite-sample median provides robust central tendency. |
| 8–14              | `medium`        | Winsorized sigma clipping    | The population supports stable iterative robust statistics. |
| 15 or more        | `large`         | Two-sided generalized ESD    | The estimator's finite-sample Student-t test is admitted. |

Every recommendation also resolves the numerical controls used by the chosen
estimator. Version 1 uses asymmetric sigma thresholds of 4.0 low and 3.0 high,
an ESD outlier ceiling of 0.30, ESD significance 0.05, at most eight clipping
passes, and a retained-support floor of `min(3, source_count)`. Large-scale
spatial expansion is disabled because enabling it requires an explicit
scientific choice about coherent structures.

An accepted-sample support map is always requested. Low/high rejection maps are
requested only for estimators that produce low/high classifications. The exact
median therefore publishes support but does not claim rejection attribution.

## Canonical identity

The planner hashes the following fields in fixed order with SHA-256:

1. length-prefixed algorithm identifier;
2. source count as big-endian `u32`;
3. length-prefixed population tier;
4. length-prefixed explicit estimator;
5. every floating-point control as big-endian IEEE-754 bits;
6. every integer control as big-endian bytes;
7. rejection-map and support-map booleans as single bytes.

The result is a 64-character lowercase hexadecimal `automaticPlanSha256`.
The registration-plan SHA-256 travels beside this seal so the UI can reject a
recommendation produced for an older identity set.

## Preview and execution

The desktop requests the plan from Rust and displays its population tier,
explicit estimator, rationale, and complete digest. Manual controls are hidden
while Automatic mode is active. Integration remains disabled until the returned
registration identity equals the currently reviewed registration plan.

At execution, Rust resolves the automatic plan again from the submitted source
population. It rejects the request unless all of the following match exactly:

- automatic-plan SHA-256;
- explicit estimator;
- bit representation of every floating-point control;
- every integer and boolean control;
- support and rejection product choices;
- disabled large-scale expansion and its canonical inactive defaults.

Manual execution remains available and does not require an automatic seal.

## Failure behavior

Empty populations, malformed digests, stale population counts, modified
settings, and unknown request fields fail closed before registered pixels are
integrated. No fallback estimator is selected. The user can retry Automatic
mode after rebuilding the registration plan or switch explicitly to Manual.

## Scope limits

Version 1 is population-aware rather than data-dependent. Future policies may
consume complete, sealed quality evidence, but they require a new algorithm
identifier, independent differential validation, documented missing-evidence
behavior, and tests proving that preview and execution resolve identically.
