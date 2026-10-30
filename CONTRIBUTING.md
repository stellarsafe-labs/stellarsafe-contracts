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

## Contributing through Drips Wave

This repository is built to take part in the [Stellar Drips Wave](https://www.drips.network/wave/stellar). If you are joining through a Wave:

1. **Claim before coding.** Comment on the issue, or apply in the Wave app, and wait to be assigned. Unassigned PRs may be closed.
2. **One issue per PR.** Reference it with `Closes #N`.
3. **Meet every acceptance-criteria checkbox** in the issue, and say in the PR how each one is met.
4. **Understand what you submit.** AI tools are fine, but you must be able to explain every line. Untested or not-understood generated code will be closed (see the Wave Terms & Rules).
5. **Security-sensitive changes** (anything in `lib.rs` that touches auth, execution or accounting) need a test that fails without your change.
6. Expect review within a few days. Address comments with new commits, not force-pushes, so the review history stays readable.

## Commit style

Use [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, `test:`, `docs:`) and keep each commit focused on one change.
