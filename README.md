# stellarsafe-contracts

On-chain treasury and spending-policy engine for [Soroban](https://developers.stellar.org/docs/build/smart-contracts) (Stellar smart contracts).

StellarSafe lets an organization hold assets in a contract that **only pays out when every policy passes**:

- Payments of 500 USDC or more need two approvals (amount-based approval tiers, per asset).
- Only allowlisted addresses can receive treasury payments.
- Each department has a spending limit per UTC calendar month (or a fixed window).
- Proposals expire, and approvals given before a relevant rule change stop counting.
- Configuration changes wait out a timelock, during which a guardian can veto them.
- Every payment and configuration change emits an on-chain event.

There is **no admin key**. Adding members, changing policies, raising budgets and upgrading the contract all go through the same multi-approval process as payments.

> **Status: testnet only.** This code has not had an external security audit. Do not hold real assets with it until it has. See [SECURITY.md](SECURITY.md).

---

## Why StellarSafe

Soroban already has good building blocks for *who can sign*. Examples are [OpenZeppelin's smart-account policies](https://docs.openzeppelin.com/stellar-contracts/accounts/policies) (N-of-M and weighted thresholds, rolling ledger-based spending caps) and Stellar's [custom account example](https://developers.stellar.org/docs/build/smart-contracts/example-contracts/complex-account). Those check whether a transaction is signed correctly when it happens.

StellarSafe works one level higher. It is a **treasury workflow engine**: payments are on-chain *proposals* that collect approvals over time and are checked against organisation-level rules when they execute.

| | Signing-level policies (e.g. OZ smart accounts) | StellarSafe |
|---|---|---|
| Approvals | Signatures on a single transaction | On-chain proposals approved asynchronously, with expiry, revocation and an audit trail |
| Spending limits | Rolling window measured in ledgers, per signing rule | Per **department** and token, by **UTC calendar month** (or fixed window); predictable for accounting |
| Amount-based thresholds | Configure separate rules | Built-in approval tiers per asset (e.g. ≥ 500 USDC → 2 approvals) |
| Recipient control | – | Allowlist, checked again at execution |
| Rule changes | Thresholds can become unreachable when signers change; admins must fix them by hand | Invariants enforced on every change; affected approvals reset automatically |
| Governance | Account admin | Self-governing quorum, **timelock** and guardian veto, emergency pause |
| Off-chain integration | Generic | Typed events, `get_proposal_state` with the exact blocking error, metadata hashes for descriptions |

The two fit together. A StellarSafe member can itself be a smart account (for example a passkey wallet), which gives hardware-grade signing *and* treasury-level policy.

## Contents

- [How it works](#how-it-works)
- [Roles](#roles)
- [Policies](#policies)
- [Contract interface](#contract-interface)
- [Building and testing](#building-and-testing)
- [Deploying to testnet](#deploying-to-testnet)
- [Repository layout](#repository-layout)
- Further docs: [Threat model](docs/THREAT_MODEL.md) · [Integration guide (SDK / indexer / frontend)](docs/INTEGRATION.md) · [Contributing](CONTRIBUTING.md)

## How it works

```
             propose            approve (xN)           execute
 proposer ───────────▶ Pending ─────────────▶ Pending ──────────▶ Executed
                          │                      │
                 cancel   │      expires_at      │
                          ▼          passes      ▼
                      Cancelled               Expired
```

Everything is a **proposal** carrying an `Action`:

| Action                                    | Approvals needed                  |
|-------------------------------------------|-----------------------------------|
| `Payment { token, to, amount, department }` | the asset policy's tier for `amount` |
| `AddMember`, `UpdateMember`, `RemoveMember` | `governance_threshold`            |
| `SetGovernanceThreshold`, `SetMaxProposalTtl` | `governance_threshold`          |
| `SetAssetPolicy`, `RemoveAssetPolicy`     | `governance_threshold`            |
| `SetBudget`, `RemoveBudget`               | `governance_threshold`            |
| `SetAllowlistEnabled`, `SetRecipientAllowed` | `governance_threshold`         |
| `Unpause`                                 | `governance_threshold`            |
| `Upgrade(wasm_hash)`                      | `governance_threshold`            |
| `SetGovernanceDelay(seconds)`             | `governance_threshold`            |

Everything except payments is a **governance action**. Governance actions are timelocked (see below).

**All checks run again at execution time**, against the state at that moment. A payment that was valid when proposed fails if the budget has since been used up, the recipient was removed from the allowlist, or the asset policy was removed.

**Approval epochs.** Each approval records the *epoch* it was given under, and only approvals from the current epoch count. A rule change bumps the epoch of exactly the proposals it affects, and approvers must then confirm those again:

| Proposal kind | Its approvals stop counting when |
|---------------|----------------------------------|
| Payment in token X | X's asset policy or any of X's budgets changes |
| Governance | any governance action executes (`config.version` increments) |

Unrelated changes, such as adding a member, editing the allowlist, or touching another token's policy, leave pending payment approvals intact. Allowlist changes don't need to reset anything, because the allowlist is checked again at execution. An approval also stops counting if the approver is removed or loses the approver role.

**Governance timelock.** When a governance proposal reaches `governance_threshold` approvals, its `ready_at` is set to `now + governance_delay`. `execute` fails with `TimelockActive` until that time. If approvals drop below the threshold, or the epoch changes, the timer starts over. During the delay a guardian can **veto** (cancel) the proposal. The one exception: guardians cannot veto removing or changing the roles of a guardian, so a rogue guardian can always be removed. The delay is limited to 30 days and must be shorter than `max_proposal_ttl`. Governance proposals need a TTL longer than the delay. Payments are not timelocked.

**Safety invariants.** These are checked after construction and after every governance action. A change that would break one is rejected, so the treasury cannot lock itself out:

- 1 to 50 members; at least one proposer, one approver and one executor.
- `1 <= governance_threshold <= number of approvers`.
- `governance_delay <= 30 days` and `governance_delay < max_proposal_ttl`.
- No asset tier requires more approvals than there are approvers.
- At most 20 assets with a policy.

## Roles

Roles are a bitmask, so one address can hold several.

| Role       | Bit | Value | Can                                                                 |
|------------|-----|-------|----------------------------------------------------------------------|
| `PROPOSER` | 0   | 1     | Create proposals                                                    |
| `APPROVER` | 1   | 2     | Approve; their approvals count toward thresholds                     |
| `EXECUTOR` | 2   | 4     | Execute fully approved proposals                                    |
| `GUARDIAN` | 3   | 8     | `pause()` payments instantly; cancel (veto) pending proposals         |

Guardians cannot veto proposals that remove or change the roles of a guardian, and pausing does not block governance. A rogue guardian can therefore always be removed. Unpausing requires a governance proposal.

## Policies

### Asset policy (one per token, fail-closed)

```rust
AssetPolicy {
    max_amount: 100_000_0000000,          // hard cap per payment (base units)
    tiers: [
        ApprovalTier { min_amount: 0,            approvals: 1 },
        ApprovalTier { min_amount: 500_0000000,  approvals: 2 },  // >= 500 USDC
        ApprovalTier { min_amount: 5000_0000000, approvals: 3 },
    ],
}
```

The highest tier whose `min_amount <= amount` applies. Tiers must start at 0, increase strictly in `min_amount`, never decrease in `approvals`, and stay within `max_amount`. A token with no policy cannot be paid out at all. Amounts are in the token's base units (USDC on Stellar uses 7 decimals). For a strict "above 500" rule, set `min_amount` to `500_0000001`.

### Department budgets (one per department + token, fail-closed)

```rust
Budget { limit: 5000_0000000, period: Period::CalendarMonth }
Budget { limit:  300_0000000, period: Period::Fixed(86_400) }   // per UTC day
```

Departments are `u32` ids. Names live off-chain. Every payment names a department, and a department with no budget for the token cannot spend it.

- `CalendarMonth`: UTC months computed on-chain from the ledger timestamp, e.g. `2026-10-01T00:00:00Z` up to but not including `2026-11-01T00:00:00Z`. Leap years are handled.
- `Fixed(seconds)`: back-to-back windows aligned to the Unix epoch (1 hour to 366 days).

Spending is counted **in contract storage when a payment executes**, never by the backend. Only the current window's counter is kept, and it resets when the window changes. Counters live in persistent storage. That storage gets archived and can be restored, but it is never deleted, so letting an entry expire cannot reset the count. Removing and re-adding a budget within the same window keeps the amount already spent.

### Recipient allowlist

When `allowlist_enabled` is true, payments may only go to allowlisted addresses. Removing an address blocks payments to it that are already pending.

## Contract interface

How the functions in the original spec map to this contract:

| Spec function        | Contract function                                         |
|----------------------|------------------------------------------------------------|
| `initialize()`       | `__constructor(init: InitConfig)` (runs atomically at deploy, so it cannot be front-run) |
| `add_member()`       | `propose(Action::AddMember(addr, roles))` → approve → execute |
| `remove_member()`    | `propose(Action::RemoveMember(addr))` → approve → execute  |
| `set_policy()`       | `propose(Action::SetAssetPolicy / SetBudget / SetRecipientAllowed …)` |
| `propose_payment()`  | `propose_payment(proposer, token, to, amount, department, ttl, metadata_hash)` |
| `approve_payment()`  | `approve(approver, id)`                                    |
| `revoke_approval()`  | `revoke_approval(approver, id)`                            |
| `execute_payment()`  | `execute(executor, id)`                                    |
| `cancel_payment()`   | `cancel(caller, id)`                                       |

### Writes

| Function | Auth | Notes |
|----------|------|-------|
| `propose(proposer, action, ttl, metadata_hash) -> u64` | proposer | `1 <= ttl <= max_proposal_ttl` seconds; `metadata_hash` is an optional 32-byte hash of the off-chain description |
| `propose_payment(proposer, token, to, amount, department, ttl, metadata_hash) -> u64` | proposer | Shortcut for `propose(Action::Payment(..))` |
| `approve(approver, id) -> u32` | approver | Returns the number of approvals that count. Re-approving after an epoch change refreshes the approval. Starts the timelock when a governance proposal reaches its threshold |
| `revoke_approval(approver, id)` | approver | Former members can also revoke |
| `cancel(caller, id)` | proposer, or guardian (veto; not for actions targeting a guardian) | Also works on expired proposals (cleanup) |
| `execute(executor, id)` | executor | Re-checks everything; governance also needs `now >= ready_at`. The only path out of the treasury |
| `pause(guardian)` | guardian | Blocks payment execution |
| `deposit(from, token, amount)` | from | Optional; plain transfers to the contract address work too |

### Reads

`get_config`, `get_members`, `get_roles(addr)`, `get_policy_assets`, `get_asset_policy(token)`, `get_budget(department, token) -> BudgetStatus` (limit, spent, remaining in the current window), `is_recipient_allowed(addr)`, `get_proposal(id)`, `get_proposals(start, limit)`, `proposal_count`, `get_proposal_state(id)` (live status, counted approvals, required approvals, timelock end, and the error `execute` would return right now), `contract_version`.

Error codes are listed in [`errors.rs`](contracts/treasury/src/errors.rs) and in the [integration guide](docs/INTEGRATION.md#error-codes).

## Building and testing

Prerequisites: Rust stable with the `wasm32v1-none` target, and [stellar-cli](https://developers.stellar.org/docs/tools/cli) v25.2 or newer. soroban-sdk 29 refuses plain `cargo build` for Wasm.

```sh
make test        # 110 native tests, incl. the model-based fuzz test
make build       # stellar contract build -> target/wasm32v1-none/release/stellarsafe_treasury.wasm
make test-wasm   # tests against the compiled Wasm, metered against mainnet limits
make lint        # rustfmt + clippy -D warnings
make check       # everything above + Wasm size check (<= 128 KiB)
make fuzz        # long model-based run (PROPTEST_CASES=2000)
```

What the tests cover:

| Area | Examples |
|------|----------|
| Payments | tier boundaries (499/500/4,999/5,000), cap, allowlist, fail-closed asset and budget, self-payment, insufficient balance reverts cleanly |
| Approvals | duplicates, revocation, expiry boundary (`expires_at` is exclusive), per-token and governance epochs, unrelated changes keep approvals, removed or demoted approvers |
| Timelock | delay boundary to the second, timer restarts when approvals drop below threshold or the epoch changes, changing the delay is itself delayed, guardian veto and its limits, TTL must outlive the delay |
| Budgets | accumulation, check at execution time not proposal time, exact UTC month and year rollover, daily windows, per-department isolation, remove/re-add keeps spend |
| Governance | each action, threshold bounds, invariant violations rejected, failed execution reverts fully, upgrade |
| Security | double execution, closed proposals immutable, role checks on every entry point, signature impersonation, calls without auth, pause semantics, guardian limits, tier bypass via policy change |
| Limits | 50 members × 20 assets × 48 approvals within mainnet limits. On Wasm, executing with 50 approvals costs about 2.3M instructions (limit 100M) |
| Date math | calendar conversion checked day by day across 300 years |
| Model-based fuzzing | random sequences of propose / approve / revoke / cancel / execute / pause / unpause / policy changes / time jumps (including exact expiry and window edges, and exact-remaining-budget payments) checked step by step against a reference model: exact result or error code, balances, budget counters, proposal statuses. Validated by mutation: 8 of 8 deliberately introduced rule bugs are caught |

## Deploying to testnet

```sh
stellar keys generate alice --network testnet --fund
cp scripts/init.example.json init.json   # fill in real G... / C... addresses
SOURCE=alice ./scripts/deploy-testnet.sh init.json
```

The script refuses to target mainnet.

## Verifying a deployment

Releases are built from a pinned toolchain (`rust-toolchain.toml`, stellar-cli version in CI) and publish the Wasm's SHA-256. That hash is also the on-chain Wasm hash. Check that a deployed treasury runs a given commit with:

```sh
NETWORK=testnet ./scripts/verify-build.sh C...CONTRACT_ID
```

## Repository layout

```
contracts/treasury/
  src/lib.rs        entry points, execution and governance logic, invariants
  src/types.rs      public types (Action, AssetPolicy, Budget, Proposal, …) and limits
  src/policy.rs     pure policy math: tier lookup, validation, UTC calendar windows
  src/storage.rs    storage layout and TTL management
  src/events.rs     event definitions (the audit trail the indexer consumes)
  src/errors.rs     stable error codes
  src/test/         native test suite
  tests/wasm.rs     tests against the compiled Wasm (feature `wasm-tests`)
  src/test/model.rs reference model + property-based fuzz test
docs/               threat model, integration guide
scripts/            testnet deployment, build verification
.github/workflows/  CI (lint, tests, wasm, nightly fuzz, audit) and reproducible releases
```

## License

[Apache-2.0](LICENSE)
