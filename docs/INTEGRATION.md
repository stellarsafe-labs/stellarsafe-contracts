# Integration guide

This guide is for `stellarsafe-sdk`, `stellarsafe-api` (indexer) and `stellarsafe-web`. **The contract is the only source of truth.** The backend stores descriptions and notifications. It cannot approve, execute or bypass anything.

## Generating typed bindings

```sh
stellar contract build
stellar contract bindings typescript \
  --wasm target/wasm32v1-none/release/stellarsafe_treasury.wasm \
  --output-dir ../stellarsafe-sdk/src/generated --overwrite
```

The contract spec embedded in the Wasm describes every function, type, error and event.

## Recommended client flows

**Before signing anything, simulate.** Every write function returns a typed error (see below), so simulation tells the user exactly why a call would fail.

**Show "can this execute?"** Call `get_proposal_state(id)`. It returns:

```ts
{ status: Pending | Executed | Cancelled | Expired,
  approvals: number,         // approvals that count right now
  required: number,          // under the current policy
  ready_at?: number,         // governance: when the timelock ends
  blocked_by?: number }      // error code `execute` would hit, if any
```

For governance proposals, `blocked_by` does not cover the invariant checks. Simulate `execute` for a definitive answer.

**Metadata.** Store the proposal description off-chain, hash it with SHA-256, and pass the hash as `metadata_hash`. The UI can then prove the description shown matches what approvers signed.

**Amounts** are `i128` in the token's base units. USDC and other Stellar classic assets use 7 decimals.

## Events

Each event's first topic is its snake_case name. Topic fields follow, in order. Data is a map of the remaining fields. `None` values are left out of the map.

| Event | Topics | Data |
|-------|--------|------|
| `proposal_created` | `id: u64`, `proposer: Address` | `action`, `expires_at: u64`, `config_version: u32`, `metadata_hash?: BytesN<32>` |
| `proposal_approved` | `id`, `approver` | `approvals: u32` (count after this approval) |
| `proposal_ready` | `id` | `ready_at: u64` (governance proposal reached its threshold; timelock started) |
| `approval_revoked` | `id`, `approver` | – |
| `proposal_cancelled` | `id` | `by: Address` |
| `proposal_executed` | `id` | `executor: Address` |
| `payment_executed` | `id`, `token: Address` | `to`, `amount: i128`, `department: u32`, `window: u64`, `window_spent: i128` |
| `config_updated` | `id` | `version: u32`, `action` |
| `paused` | – | `by: Address` |
| `deposited` | `token`, `from` | `amount: i128` |
| `proposal_pruned` | `id` | – (closed proposal deleted from storage) |
| `storage_migrated` | – | `from: u32`, `to: u32` |

Executing a payment emits `payment_executed`, then the token's own `transfer` event, then `proposal_executed`. Executing a governance action emits `config_updated` followed by `proposal_executed`.

### Indexer rules

1. Filter events by the treasury **contract id**. Never trust an event by its topics alone.
2. For `deposited`, accept only tokens you recognise, such as tokens with an asset policy. Any contract can pretend to be a token.
3. Checkpoint on ledger sequence. Stellar has immediate finality, so there are no reorgs to handle.
4. Approvals are invalidated **implicitly**. When `config_updated` fires, payment approvals in the affected token reset (policy or budget changes), and so do approvals on all governance proposals. Recompute pending approval counts from `get_proposal_state`, not by summing events.
5. Use `proposal_ready` to notify members that a governance change is scheduled, and guardians that they can still veto it.
6. RPC nodes keep events for a limited time, so run the indexer continuously and backfill from your own database.
7. On `proposal_pruned`, keep your copy of the proposal. The contract no longer has it: `get_proposal` returns `ProposalNotFound`.

## Error codes

| Code | Name | Meaning |
|------|------|---------|
| 100 | `InvalidRoles` | Role mask is 0 or has unknown bits |
| 101 | `InvalidMemberCount` | Must be between 1 and 50 members |
| 102 | `MissingRequiredRole` | Needs at least one proposer, approver and executor |
| 103 | `InvalidThreshold` | Threshold or a tier exceeds the number of approvers, or is 0 |
| 104 | `InvalidTtl` | TTL out of bounds |
| 105 | `InvalidPolicy` | Malformed asset policy |
| 106 | `InvalidBudget` | Malformed budget |
| 107 | `TooManyAssets` | More than 20 assets with a policy |
| 108 | `MemberExists` | |
| 109 | `MemberNotFound` | |
| 110 | `NotConfigured` | Removing a policy or budget that does not exist |
| 111 | `InvalidMember` | The treasury cannot be its own member |
| 112 | `InvalidDelay` | Governance delay > 30 days, or not shorter than `max_proposal_ttl` |
| 200 | `Unauthorized` | Caller lacks the required role |
| 300 | `ProposalNotFound` | |
| 301 | `ProposalNotPending` | Already executed or cancelled |
| 302 | `ProposalExpired` | |
| 303 | `AlreadyApproved` | |
| 304 | `NotApproved` | Nothing to revoke |
| 305 | `InsufficientApprovals` | |
| 306 | `InvalidPagination` | `limit` must be between 1 and 50 |
| 307 | `TimelockActive` | Governance proposal approved, but `ready_at` not reached |
| 308 | `ProposalNotPrunable` | Still open, or closed less than 30 days ago |
| 400 | `InvalidAmount` | Must be > 0 |
| 401 | `AssetNotAllowed` | Token has no policy |
| 402 | `AmountExceedsCap` | Above the asset's `max_amount` |
| 403 | `RecipientNotAllowed` | Not on the allowlist |
| 404 | `BudgetNotFound` | Department has no budget for this token |
| 405 | `BudgetExceeded` | Would exceed this window's budget |
| 406 | `Paused` | |
| 407 | `InvalidRecipient` | The treasury cannot pay itself |
| 500 | `Overflow` | |
| 600 | `MigrationRequired` | Contract was upgraded; call `migrate()` (anyone can) |
| 601 | `UnsupportedStorageVersion` | Stored data is newer than the code (after a downgrade) |

Codes are stable. New errors get new numbers, and existing numbers are never reused.

## Storage and TTL

The instance entry (config, members, asset list, proposal counter) and every persistent entry the contract touches get their TTL extended to about 120 days whenever less than about 90 days remain. Entries nobody touches for a long time get archived. Archived entries are not deleted and can be restored with `stellar contract restore`. Simulation reports when a restore is needed.

Open proposals can't be archived: their TTL (at most 90 days) is shorter than the storage TTL. Closed proposals can. Once one has been closed for 30 days, anyone can delete it with `prune_proposals`. Run a keeper that prunes closed proposals before they go untouched for about 90 days. That keeps `get_proposals` pages from hitting archived entries, and stops storage from growing forever.

### Upgrades and migrations

`storage_version()` reports the layout of the stored data. If an upgrade raises it, every write call fails with `MigrationRequired` (600) until someone calls `migrate()`. Whoever executes an `Upgrade` should call `migrate()` straight away; clients that see error 600 can call it themselves. Views keep working throughout.
