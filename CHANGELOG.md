# Changelog

## [0.1.0] - Unreleased

### Added
- `prune_proposals(ids)`: anyone can delete proposals closed for at least 30 days (`PROPOSAL_RETENTION`); `proposal_pruned` event; error `ProposalNotPrunable` (308).
- Storage layout versioning: `storage_version()`, permissionless `migrate()`, `storage_migrated` event, errors `MigrationRequired` (600) and `UnsupportedStorageVersion` (601). Writes are refused until stored data matches the code's layout.
- Treasury contract with roles (proposer, approver, executor, guardian) and self-governance; no admin key.
- Per-asset policies: per-payment cap and amount-based approval tiers (fail-closed).
- Department budgets per token, with UTC calendar-month or fixed-window accounting stored on-chain.
- Recipient allowlist.
- Proposal lifecycle: propose, approve, revoke, cancel and execute, with expiry and config-version approval invalidation.
- Guardian pause; governance-only unpause.
- Governance-controlled contract upgrade.
- Typed events for indexing; stable error codes.
- Governance timelock (`governance_delay`, `SetGovernanceDelay`) with `proposal_ready` event and guardian veto. Guardians cannot veto actions that target a guardian.
- Approval epochs: payment approvals reset only when their token's policy or budgets change; governance approvals reset on any governance execution.
- `ProposalState.ready_at`; errors `InvalidDelay` (112) and `TimelockActive` (307).
- Model-based property test (proptest) checked by mutation testing; nightly fuzz CI job.
- Pinned toolchain; release workflow publishing the Wasm SHA-256 and build provenance; `scripts/verify-build.sh`.
- Native and Wasm test suites; CI; testnet deploy script; threat model.

### Changed
- Split contract internals out of `lib.rs` into `proposals`, `payments`, `governance` and `migration` modules. No behavior change.
- Threat model: documents the rogue-guardian re-pause outage and its bound.
