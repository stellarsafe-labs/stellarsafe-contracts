# Threat model

## Assets protected

Tokens held by the treasury contract, and the integrity of its configuration: members, policies, budgets and allowlist.

## Trust assumptions

| Component | Trusted for |
|-----------|-------------|
| Soroban host and Stellar consensus | Correct execution, `require_auth` signature checks, ledger timestamp |
| Token contracts with an asset policy | Honest `transfer`. Governance chooses which tokens get a policy |
| A quorum of approvers (`governance_threshold`) | Configuration changes, including upgrades |
| Backend, indexer, frontend | **Nothing.** They can show stale or wrong data, but cannot move funds or change rules |

## Threats and mitigations

| # | Threat | Mitigation | Test(s) |
|---|--------|-----------|---------|
| T1 | A single compromised key drains funds | No admin key. Payments need tier approvals plus an executor. Governance needs a threshold | `governance_needs_threshold_approvals`, `payment_of_500_needs_two_approvals` |
| T2 | Someone approves in another member's name | `approver.require_auth()` on every write; auth is bound to the function and its arguments | `approval_requires_the_approvers_own_signature`, `every_state_changing_call_requires_caller_auth` |
| T3 | The same payment executes twice | Status is set to `Executed` before the transfer; closed proposals reject every write; Soroban forbids re-entrancy | `payment_cannot_be_executed_twice`, `closed_proposals_reject_all_mutations` |
| T4 | Approvals collected under looser rules are used after the rules tighten | Approvals are tagged with an epoch: per token for payments, global for governance. Rule changes reset the affected approvals, and every rule is checked again at execution anyway | `approvals_cannot_be_carried_across_policy_change_to_bypass_tiers`, `asset_policy_change_invalidates_payment_approvals`, `any_governance_execution_invalidates_governance_approvals` |
| T5 | A removed or demoted approver's earlier vote still counts | Approvals are counted only for current approvers | `removed_member_approval_does_not_count`, `approver_demoted_to_executor_loses_vote` |
| T6 | An old approval is used long after it was given | `expires_at` is enforced on approve, revoke and execute; max TTL is capped at 90 days | `expiry_is_exclusive_at_expires_at` |
| T7 | Overspending a budget with several proposals | The budget is checked and counted at execution using an on-chain counter | `budget_is_enforced_at_execution_not_proposal` |
| T8 | Resetting the spend counter by letting storage expire | Counters use persistent storage, which is archived rather than deleted. Temporary storage is never used | design; see `storage.rs` |
| T9 | Resetting the counter by removing and re-adding a budget | Counters are kept when a budget is removed | `removing_and_readding_budget_keeps_spend_in_same_window` |
| T10 | Paying an unexpected token or recipient | Fail-closed asset policies and budgets; allowlist re-checked at execution | `rejects_tokens_without_policy`, `allowlist_management`, `removing_policy_blocks_pending_payments` |
| T11 | Governance locks the treasury out (no executor, unreachable threshold) | Invariants checked after every governance action; the action is rejected if one breaks | `cannot_remove_last_executor`, `cannot_remove_approver_needed_by_highest_tier` |
| T12 | A rogue guardian blocks the treasury | Guardians cannot veto removing or changing the roles of any guardian, and pausing doesn't block governance. A rogue guardian *can* stall payments by re-pausing after every `Unpause` and vetoing `Unpause` proposals, but only until a `RemoveMember` proposal gets its approvals and waits out `governance_delay`. They cannot veto that proposal. So the worst case is a payment outage lasting about `governance_delay` plus the time to collect approvals. See residual risk below | `rogue_guardian_can_be_removed_while_paused`, `guardian_cannot_veto_actions_targeting_a_guardian`, `guardian_cannot_veto_removal_of_a_guardian` |
| T13 | Front-running initialization | Setup runs in `__constructor`, atomically at deploy | design |
| T14 | Storage or compute exhaustion (DoS) | Members ≤ 50, assets ≤ 20, tiers ≤ 8, page size ≤ 50. Worst case uses about 2% of mainnet instruction limits | `max_members_max_assets_full_approval_set_fits_limits`, `fifty_approvals_fit_mainnet_limits_on_wasm` |
| T15 | Arithmetic overflow | `checked_add` on spend counters, ids and timestamps; `overflow-checks = true` in release | – |
| T16 | Proposal spam | Only proposers can propose; each proposer pays storage rent for their proposals | `only_proposers_can_propose` |
| T17 | Malicious upgrade or config change by a captured quorum | Governance timelock: the change becomes visible (`proposal_ready`) `governance_delay` seconds before it can run, and an honest guardian can veto it. Changing the delay is itself delayed | `governance_waits_for_the_delay`, `guardian_can_veto_queued_governance`, `changing_the_delay_is_itself_timelocked` |
| T19 | A timelock skipped by reusing an old `ready_at` | `ready_at` is tied to the epoch it was set in, and dropping below the threshold clears it | `stale_ready_time_from_older_epoch_is_ignored`, `dropping_below_threshold_restarts_the_timelock` |
| T20 | Unknown state-machine bugs | Model-based fuzzing compares every call's result and the full state with a reference model; nightly CI runs 2,000 sequences | `contract_matches_reference_model` |
| T18 | Fake deposit events from fake tokens | Indexers must filter by known tokens (see INTEGRATION.md) | – |
| T21 | Pruning deletes a proposal that could still change, or rewrites history | Only proposals closed for 30 days can be pruned, and closed proposals never change again. Ids are never reused, spend counters are separate entries, and events stay the audit trail. Batches are all-or-nothing | `executed_proposal_can_be_pruned_after_retention`, `open_proposals_cannot_be_pruned`, `pruned_proposals_reject_every_call`, `contract_matches_reference_model` |
| T22 | Old or archived proposals break views, or storage grows without bound | An open proposal expires (within 90 days) before its storage entry can be archived (about 120 days untouched). Anyone can prune closed ones | `cancelled_and_expired_proposals_can_be_pruned` |
| T23 | Upgraded code runs on data in an old layout | Stored `storage_version`; every write refuses with `MigrationRequired` until `migrate()` converts the data. `migrate` is permissionless but only runs conversions built into code governance approved | `writes_wait_for_migration_after_upgrade` |
| T24 | A downgrade bricks the treasury | Newer-than-code data is not refused by writes (only by `migrate`), so governance can still upgrade forward | `newer_storage_cannot_be_migrated_but_does_not_brick_the_treasury` |

## Known limitations and residual risk

- **Quorum compromise.** If `governance_threshold` approver keys are compromised, the timelock and guardian veto slow the attacker down but don't stop them for good. They can first remove guardians (not vetoable by design) and then wait out the delay again. Payments are not timelocked, so a captured quorum can spend up to the current budgets. Use hardware wallets or smart-wallet signers, a majority threshold, and a delay of at least 24 hours, and monitor `proposal_ready`.
- **Ledger timestamp.** Validators set it within protocol bounds. A few seconds of skew around a month boundary or `expires_at` is possible.
- **Hostile token contracts.** A malicious token can do anything in `transfer` except re-enter the treasury. Only add policies for tokens you trust, such as Stellar Asset Contracts.
- **Changing a budget's period** (for example from monthly to daily) can make the current counter look like it belongs to a different window, which resets it. This requires governance approval.
- **Coarse epochs.** A change to any budget of token X resets approvals on every pending payment in X, even in other departments. Every governance execution resets approvals on all pending governance proposals. Both are deliberately conservative.
- **Upgrades don't reset payment approvals.** Pending payment approvals survive an `Upgrade`. The upgrade itself needs a quorum and goes through the timelock.
- **Storage migration on upgrade.** A layout change must bump `STORAGE_VERSION` and add a step to `migration::migrate`. A step must fit in one transaction, so unbounded data (proposals, allowlist entries) has to be converted lazily on read. Between the upgrade and `migrate()`, every write is blocked. Anyone can end that state immediately.
- **Downgrades.** Downgraded code can't read data stored in a newer layout correctly. Governance must only downgrade to code that can still read it.
- **Rogue guardian outage.** While a rogue guardian is being removed (T12), payments can stay paused. Have several guardians, keep `governance_delay` no longer than the outage you can tolerate, and alert on `paused` events.
- **Pruned proposals are gone from contract storage.** After pruning, the history lives only in events and indexers. RPC nodes keep events for a limited time, so run an indexer if you need records for longer.
- **Unaudited.** See SECURITY.md.
