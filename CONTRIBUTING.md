# Contributing to AetherStack

Thank you for helping improve AetherStack. The project handles scientific image
data, so correctness, traceability, and reproducibility take precedence over
micro-optimizations.

## Before opening a pull request

1. Keep the change focused and explain the scientific or engineering invariant
   it preserves.
2. Add unit tests for ordinary behavior, boundary values, and failures.
3. Add differential or property tests when implementing numerical algorithms.
4. Document public interfaces and any non-obvious safety or precision decision.
5. Do not commit private acquisition files or metadata.
6. Run the complete local quality gate:

```shell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps
```

## Code conventions

- Use English for code, documentation, commit messages, and pull requests.
- Prefer explicit domain types over loosely related primitive parameters.
- Avoid unchecked indexing and arithmetic in data-dependent code.
- Keep processing deterministic unless a documented execution profile states
  otherwise.
- Never hide malformed input, missing metadata, or evidence conflicts behind a
  default value.
- Comments should explain intent, invariants, numerical behavior, or format
  constraints. Do not narrate syntax that is already clear from the code.

## Tests and fixtures

Fixtures must be synthetic, minimal, redistributable, and stripped of private
metadata. A regression test should state which behavior it protects. Large
camera files belong in an external validation corpus, not in Git history.

## Commit history

Use concise imperative messages, preferably following Conventional Commits, for
example `feat(fits): validate mandatory card alignment`. Keep fixups out of the
published branch and do not add unrelated attribution trailers.
