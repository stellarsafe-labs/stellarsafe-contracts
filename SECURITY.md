# Security policy

## Status

StellarSafe contracts are **pre-audit** and intended for **Stellar testnet only**. Do not deposit real assets until an independent audit is complete and published here.

## Reporting a vulnerability

Do **not** open a public issue. Report privately through [GitHub Security Advisories](https://github.com/stellarsafe-labs/stellarsafe-contracts/security/advisories/new). Please include:

- the affected commit or version,
- a description and its impact,
- a reproduction, ideally a failing test in `contracts/treasury/src/test/`.

We aim to acknowledge reports within 72 hours and to agree on a disclosure timeline with you.

## Scope

In scope: everything under `contracts/`. Out of scope: issues that require a quorum of approver keys to be compromised (that is the trust model; see [docs/THREAT_MODEL.md](docs/THREAT_MODEL.md)), and bugs in the Soroban host or in token contracts.

## Before mainnet

- [ ] Independent audit of `contracts/treasury`
- [x] Model-based property tests of the proposal state machine (nightly in CI)
- [x] Governance timelock with guardian veto
- [x] Reproducible build: pinned toolchain, release workflow publishes the Wasm hash and build provenance
- [x] Public testnet deployment with demo transactions ([docs/TESTNET.md](docs/TESTNET.md))
- [x] CI green on GitHub with the official stellar-cli build
