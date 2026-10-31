use soroban_sdk::{contracttype, Address, BytesN, Map, Vec};

/// May create proposals.
pub const ROLE_PROPOSER: u32 = 1 << 0;
/// Approval counts toward thresholds.
pub const ROLE_APPROVER: u32 = 1 << 1;
/// May execute fully approved proposals.
pub const ROLE_EXECUTOR: u32 = 1 << 2;
/// May pause payments and cancel pending *payment* proposals in an emergency.
pub const ROLE_GUARDIAN: u32 = 1 << 3;
pub const ALL_ROLES: u32 = ROLE_PROPOSER | ROLE_APPROVER | ROLE_EXECUTOR | ROLE_GUARDIAN;

/// Hard limits that keep every storage entry and loop bounded.
pub const MAX_MEMBERS: u32 = 50;
pub const MAX_ASSETS: u32 = 20;
pub const MAX_TIERS: u32 = 8;
pub const MAX_PAGE: u32 = 50;
/// Upper bound for `max_proposal_ttl` (90 days).
pub const MAX_PROPOSAL_TTL_CAP: u64 = 90 * 24 * 60 * 60;
/// Lower bound for `max_proposal_ttl` (1 hour).
pub const MIN_PROPOSAL_TTL_CAP: u64 = 60 * 60;
/// Fixed budget periods must be between 1 hour and 366 days.
pub const MIN_FIXED_PERIOD: u64 = 60 * 60;
pub const MAX_FIXED_PERIOD: u64 = 366 * 24 * 60 * 60;
/// Upper bound for the governance timelock (30 days).
pub const MAX_GOVERNANCE_DELAY: u64 = 30 * 24 * 60 * 60;
/// How long a closed proposal stays in storage before anyone may prune it
/// (30 days after execution, cancellation or expiry). Events keep the full
/// history after that.
pub const PROPOSAL_RETENTION: u64 = 30 * 24 * 60 * 60;

/// Storage layout this code reads and writes. Bump it (and add a step to
/// `migration::migrate`) whenever a stored type or key changes shape.
pub const STORAGE_VERSION: u32 = 1;
/// Layout of treasuries deployed before the version was stored (0.1.0).
pub const FIRST_STORAGE_VERSION: u32 = 1;

/// Payments of at least `min_amount` need at least `approvals` approvals.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovalTier {
    pub min_amount: i128,
    pub approvals: u32,
}

/// Rules for paying out one token. A token without a policy cannot be spent.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetPolicy {
    /// Hard cap for a single payment, in the token's base units.
    pub max_amount: i128,
    /// Sorted by `min_amount` (strictly increasing, first tier starts at 0),
    /// with non-decreasing `approvals`. The highest matching tier applies.
    pub tiers: Vec<ApprovalTier>,
}

/// Accounting period for department budgets. All periods are UTC and derived
/// purely from the ledger timestamp, so they are deterministic on-chain.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Period {
    /// Calendar month in UTC (e.g. 2026-10-01T00:00:00Z .. 2026-11-01T00:00:00Z).
    CalendarMonth,
    /// Consecutive windows of this many seconds, aligned to the Unix epoch.
    /// `86400` gives UTC days; `604800` gives weeks starting Thursday 00:00 UTC.
    Fixed(u64),
}

/// Spending limit for one (department, token) pair.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Budget {
    pub limit: i128,
    pub period: Period,
}

/// On-chain spend counter for one (department, token) pair. Only the current
/// window is stored; it resets automatically when the window changes.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BudgetUsage {
    pub window: u64,
    pub spent: i128,
}

/// Read-only summary of a budget in the current window.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BudgetStatus {
    pub limit: i128,
    pub period: Period,
    pub window: u64,
    pub spent: i128,
    pub remaining: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Payment {
    pub token: Address,
    pub to: Address,
    pub amount: i128,
    pub department: u32,
}

/// Everything a proposal can do. Payments use the asset policy's approval
/// tiers; every other action needs `governance_threshold` approvals and then
/// waits out `governance_delay` before it can execute.
///
/// Append new variants at the end only: the discriminant order is ABI.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    Payment(Payment),
    AddMember(Address, u32),
    UpdateMember(Address, u32),
    RemoveMember(Address),
    SetGovernanceThreshold(u32),
    SetMaxProposalTtl(u64),
    SetAssetPolicy(Address, AssetPolicy),
    RemoveAssetPolicy(Address),
    /// (department, token, budget)
    SetBudget(u32, Address, Budget),
    /// (department, token)
    RemoveBudget(u32, Address),
    SetAllowlistEnabled(bool),
    SetRecipientAllowed(Address, bool),
    Unpause,
    /// Replace the contract code. Storage is preserved.
    Upgrade(BytesN<32>),
    /// Timelock in seconds between reaching the governance threshold and
    /// being able to execute. Changing it is itself subject to the current delay.
    SetGovernanceDelay(u64),
    /// When on, a proposal cannot be executed by a member whose approval
    /// counts on it, so no single key can both approve and execute.
    SetIndependentExecutor(bool),
}

#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ProposalStatus {
    Pending = 0,
    Executed = 1,
    Cancelled = 2,
    /// Never stored: reported by views once a pending proposal passes `expires_at`.
    Expired = 3,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Proposal {
    pub id: u64,
    pub proposer: Address,
    pub action: Action,
    /// Optional hash committing to off-chain metadata (description, invoice, ...).
    pub metadata_hash: Option<BytesN<32>>,
    pub created_at: u64,
    /// Approvals and execution must happen strictly before this timestamp.
    pub expires_at: u64,
    /// Approval epoch when the proposal was created (informational).
    pub config_version: u32,
    /// approver -> approval epoch the approval was given under. Only
    /// approvals from the current epoch by current approvers count.
    ///
    /// * Payments use the epoch of their token, which changes when that
    ///   token's asset policy or any of its budgets changes.
    /// * Governance proposals use `Config::version`, which changes on every
    ///   executed governance action.
    pub approvals: Map<Address, u32>,
    /// Governance only: earliest execution time, set when the proposal
    /// reaches the threshold (`now + governance_delay`). Cleared when it
    /// drops below the threshold again.
    pub ready_at: Option<u64>,
    /// Epoch `ready_at` was computed under; a stale `ready_at` is ignored.
    pub ready_epoch: u32,
    pub status: ProposalStatus,
    /// Timestamp of execution or cancellation (0 while pending).
    pub closed_at: u64,
}

/// Live view of a proposal evaluated against the current config.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProposalState {
    pub status: ProposalStatus,
    /// Approvals that currently count.
    pub approvals: u32,
    /// Approvals required under the current policy (0 if a policy check fails).
    pub required: u32,
    /// Governance only: when the timelock ends (if the threshold was reached
    /// in the current epoch).
    pub ready_at: Option<u64>,
    /// `None` if the proposal could be executed right now, otherwise the
    /// error code `execute` would fail with.
    pub blocked_by: Option<u32>,
}

/// Global configuration held in instance storage.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    pub governance_threshold: u32,
    /// Seconds a governance proposal must wait after reaching the threshold.
    pub governance_delay: u64,
    pub max_proposal_ttl: u64,
    pub allowlist_enabled: bool,
    pub paused: bool,
    /// Bumped by every executed governance action. Invalidates approvals on
    /// pending *governance* proposals (payments use per-token epochs).
    pub version: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BudgetInit {
    pub department: u32,
    pub token: Address,
    pub budget: Budget,
}

/// Constructor arguments.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitConfig {
    /// member -> role bitmask
    pub members: Map<Address, u32>,
    pub governance_threshold: u32,
    pub governance_delay: u64,
    pub max_proposal_ttl: u64,
    pub allowlist_enabled: bool,
    pub allowlist: Vec<Address>,
    pub asset_policies: Map<Address, AssetPolicy>,
    pub budgets: Vec<BudgetInit>,
}
