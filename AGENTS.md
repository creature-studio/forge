# Forge agent guidelines

## Architecture: axiomatic design

Use MIT-originated axiomatic design (Nam P. Suh) as a practical design and
review heuristic, not a requirement to introduce architectural machinery.
See [Forge's FR-DP contracts](docs/axiomatic-design.md) for the current design,
evidence, and known limitations.

- **Independence axiom:** Before a feature or architectural change, identify
  the functional requirements (FRs), constraints, and design parameters (DPs).
  Prefer one clear owner for each independent requirement. Prefer an uncoupled
  (diagonal) design; when dependencies are necessary, prefer a decoupled design
  whose FR-DP matrix can be ordered triangularly (typically lower triangular).
  Avoid reciprocal control dependencies and unnecessary side effects.
- **Information axiom:** Among designs satisfying the independence axiom and
  requirements, favor the one with the greatest probability of satisfying the
  FRs within their acceptance ranges. Prefer the simplest sufficient design;
  use reliability, regression coverage, and operational evidence to compare
  alternatives. Do not equate information content with lines of code or claim
  a numerical information measure without probability data.

## Implementation rules

- Keep state with its owning module. Use Rust module privacy, narrowly scoped
  methods, explicit inputs and typed outcomes instead of cross-module writes.
  App coordinates necessary effects; domain logic should not depend on App.
- Share read-only snapshots or explicit commands/events at module boundaries.
  Preserve required domain consequences (e.g. a file save changes Git status).
  Do not remove functionality, hide coupling behind an event bus, or add traits,
  dependency-injection frameworks, duplicate state or fallback paths merely to
  make a design matrix look triangular.
- Keep presentation and semantic state distinct. Layout/render changes must
  not mutate repository or file contents; preserve document identity, source
  position and PTY sessions. Keyboard, paste and cursor share input ownership.
- Scope asynchronous results to the correct workspace, request and owner.
  Cancel or reject stale results; refresh dependents only when relevant inputs
  change. Preserve existing user-facing behavior unless a change is requested.

## Review and verification

- For nontrivial changes, identify affected FRs/DPs and check whether changing
  one DP unexpectedly affects another FR with external inputs held fixed.
  Record new dependencies and their rationale; update the design document when
  a contract changes. A full matrix review is unnecessary for a small fix.
- Add focused owner-level tests and cross-module invariant tests for new
  boundaries. Run `cargo test --workspace` and `git diff --check` for code
  changes; check formatting of changed Rust files. For interactive or major
  architecture changes, run `bash scripts/check-axiomatic-design.sh` on Linux/WSL
  where available (isolated real-terminal regression suite).
- Report actual evidence, skipped or failed checks, and remaining coupling.
  Passing tests or splitting files does not prove global FR independence or
  strict triangularity. Never mark unverified platforms or scenarios as passed.
