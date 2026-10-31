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

## Finding an issue

Every open issue has acceptance criteria, pointers to the files involved, and two kinds of label:

- **Area:** `area: contracts`, `area: testing`, `area: ci` or `area: docs`.
- **Difficulty:**
  - `difficulty: easy` is a small, well-scoped task. Issues also labelled `good first issue` are a good place to start.
  - `difficulty: medium` needs some context on the contract.
  - `difficulty: hard` needs deep context. Read [docs/THREAT_MODEL.md](docs/THREAT_MODEL.md) first.

Browse the [good first issues](../../issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22), or [all open issues](../../issues). If you want to work on something without an issue, open one first so the scope can be agreed.

## Contributing through Drips Wave

This repository is built to take part in the [Stellar Wave](https://www.drips.network/wave/stellar) on Drips. If you are joining through a Wave:

1. **Apply in the Drips Wave app, and wait to be assigned before coding.** Work on unassigned issues may not be accepted or rewarded. Comment on the issue if you need its scope clarified.
2. **Finish within the Wave.** Issues are scoped to fit one Wave cycle. If you get stuck, say so on the issue early, and it can be reassigned or split.
3. **One issue per PR.** Link it with `Closes #N` so the Wave tracks it.
4. **Meet every acceptance-criteria checkbox** in the issue, and say in the PR description how each one is met.
5. **Understand what you submit.** AI tools are fine, but you must be able to explain every line. Code that is untested or not understood will be closed.
6. **Security-sensitive changes** need a test that fails without your change. That means anything touching auth, execution, approvals, budgets or storage in `src/`.
7. **Expect a first review within 3 days.** Address comments with new commits, not force-pushes, so the review history stays readable.

Wave difficulty maps to the issue labels: `difficulty: easy` is Trivial, `difficulty: medium` is Medium, and `difficulty: hard` is High.

### PR checklist

- [ ] `make check` passes locally (lint, native tests, Wasm build and tests, size)
- [ ] New behaviour is covered by tests; bug fixes include a test that failed before
- [ ] ABI rules above are respected (error codes, events, `Action` order)
- [ ] Docs updated: README interface tables, [docs/INTEGRATION.md](docs/INTEGRATION.md), and [docs/THREAT_MODEL.md](docs/THREAT_MODEL.md) for security-relevant changes
- [ ] Storage layout changes follow [docs/UPGRADING.md](docs/UPGRADING.md)
- [ ] `CHANGELOG.md` entry under *Unreleased*

## Commit style

Use [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, `test:`, `docs:`) and keep each commit focused on one change.
