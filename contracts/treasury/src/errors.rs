use soroban_sdk::contracterror;

/// Every failure the treasury can return. Codes are stable and part of the
/// public ABI: SDKs and frontends map them to user-facing messages, so never
/// renumber an existing variant.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    // ----- configuration (1xx) -----
    /// A member was given an empty or unknown role bitmask.
    InvalidRoles = 100,
    /// The treasury must keep between 1 and `MAX_MEMBERS` members.
    InvalidMemberCount = 101,
    /// At least one member must hold each of PROPOSER, APPROVER and EXECUTOR.
    MissingRequiredRole = 102,
    /// A threshold is 0 or larger than the number of approvers.
    InvalidThreshold = 103,
    /// Proposal TTL bounds are out of range.
    InvalidTtl = 104,
    /// Asset policy is malformed (tiers unsorted, zero caps, ...).
    InvalidPolicy = 105,
    /// Budget is malformed (non-positive limit, bad period).
    InvalidBudget = 106,
    /// More than `MAX_ASSETS` assets would have a policy.
    TooManyAssets = 107,
    /// Tried to add an address that is already a member.
    MemberExists = 108,
    /// Tried to modify or remove an address that is not a member.
    MemberNotFound = 109,
    /// Tried to remove a policy or budget that does not exist.
    NotConfigured = 110,
    /// The treasury contract itself cannot be a member.
    InvalidMember = 111,
    /// Governance delay is above `MAX_GOVERNANCE_DELAY` or not shorter than
    /// `max_proposal_ttl` (proposals would expire before they could run).
    InvalidDelay = 112,
    /// With independent execution on, every executor would be needed as an
    /// approver to reach the governance threshold or an approval tier.
    NoIndependentExecutor = 113,

    // ----- authorization (2xx) -----
    /// Caller is not a member or lacks the role needed for this call.
    Unauthorized = 200,

    // ----- proposals (3xx) -----
    ProposalNotFound = 300,
    /// Proposal was already executed or cancelled.
    ProposalNotPending = 301,
    ProposalExpired = 302,
    /// Caller already holds a valid approval on this proposal.
    AlreadyApproved = 303,
    /// Caller has no approval to revoke.
    NotApproved = 304,
    InsufficientApprovals = 305,
    /// Pagination limit out of range.
    InvalidPagination = 306,
    /// Governance proposal is approved but its timelock has not ended.
    TimelockActive = 307,
    /// Proposal is still open, or closed less than `PROPOSAL_RETENTION` ago.
    ProposalNotPrunable = 308,
    /// Independent execution is on and the executor's own approval counts
    /// on this proposal. Another executor must run it.
    ExecutorApproved = 309,
    /// The governance timelock would end at or after the proposal expires,
    /// so the proposal could never execute. Propose it again with a longer TTL.
    TimelockOutlivesProposal = 310,

    // ----- payments (4xx) -----
    InvalidAmount = 400,
    /// No asset policy exists for this token (fail-closed).
    AssetNotAllowed = 401,
    /// Amount is above the asset's per-payment cap.
    AmountExceedsCap = 402,
    /// Recipient is not on the allowlist while the allowlist is enforced.
    RecipientNotAllowed = 403,
    /// No budget exists for this department and token (fail-closed).
    BudgetNotFound = 404,
    /// Payment would exceed the department's budget for the current period.
    BudgetExceeded = 405,
    /// Payments are paused by a guardian.
    Paused = 406,
    /// The treasury cannot pay itself.
    InvalidRecipient = 407,

    // ----- arithmetic (5xx) -----
    Overflow = 500,

    // ----- storage layout (6xx) -----
    /// The contract was upgraded and its storage has not been migrated yet.
    /// Anyone can call `migrate` to fix this.
    MigrationRequired = 600,
    /// Stored data uses a newer layout than this code understands (the
    /// contract was downgraded), so it cannot be migrated.
    UnsupportedStorageVersion = 601,
}
