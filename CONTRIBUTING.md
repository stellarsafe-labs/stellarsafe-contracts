# Contributing

Thanks for helping! This repository holds the security-critical part of StellarSafe, so the bar for changes is high. That bar is also what makes contributions here valuable.

## Setup

```sh
rustup target add wasm32v1-none
cargo install --locked stellar-cli   # or a prebuilt release
make check                           # lint, tests, wasm build, wasm tests, size
```

## Rules for every PR

1. **Tests first.** Every behaviour change needs a test, and every bug fix needs a test that fails without the fix. Security-relevant changes also need a row in [docs/THREAT_MODEL.md](docs/THREAT_MODEL.md).
2. **ABI stability.** Do not renumber `Error` variants, rename events or event fields, or reorder `Action` variants. Add new variants at the end.
3. **Bounded everything.** No unbounded loops or storage growth in a single entry. New collections need a `MAX_*` limit and a stress test.
4. **Re-check at execution.** Any new policy must be enforced in `execute`, not only in `propose`.
5. **Fail closed.** If configuration is missing, the call must be rejected.
6. `make lint` must pass with no warnings.

## Good first issues

- Add a test: approving a proposal exactly at `expires_at - 1` across a config change.
- Property-test `policy::required_approvals` against a reference implementation.
- Add `Period::CalendarQuarter`.
- Add an optional timelock for payments above a configurable amount.
- Add more operations to the reference model in `src/test/model.rs` (member changes, budget edits).
- Extend `tests/wasm.rs` with a fee estimate report (`env.cost_estimate().fee()`).

Each issue should state its acceptance criteria and the tests it needs.

## Commit style

Use [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, `test:`, `docs:`) and keep each commit focused on one change.
