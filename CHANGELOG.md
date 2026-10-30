# Changelog

## [0.1.0] - Unreleased

### Added
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
