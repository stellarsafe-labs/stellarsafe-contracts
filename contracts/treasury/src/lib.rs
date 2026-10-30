//! # StellarSafe Treasury
//!
//! A self-governing Soroban treasury. Members hold role bitmasks; every
//! outgoing payment and every configuration change is a *proposal* that must
//! collect enough approvals before an executor can run it.
//!
//! Payments are checked, at execution time, against:
//! * the asset policy (fail-closed: tokens without a policy cannot be spent),
//!   including a per-payment cap and amount-based approval tiers,
//! * the recipient allowlist (when enabled),
//! * the department budget for the current UTC accounting window,
//! * the pause switch.
//!
//! Configuration changes need `governance_threshold` approvals, then wait
//! out `governance_delay` (a timelock during which guardians can veto).
//!
//! Approvals are tied to an *approval epoch* so they stop counting when the
//! rules they were given under change: payments are invalidated only by
//! changes to their token's asset policy or budgets; governance proposals by
//! any executed governance action.
//! There is no admin key: the treasury governs itself.
#![no_std]

mod errors;
mod events;
mod policy;
mod storage;
mod types;

#[cfg(test)]
mod test;

pub use errors::Error;
pub use types::*;

use soroban_sdk::{
    contract, contractimpl, token, Address, BytesN, ContractExecutable, Env, Map, String, Vec,
};

use crate::policy::{
    max_tier_approvals, required_approvals, validate_asset_policy, validate_budget, window_id,
};
use crate::storage as st;

#[contract]
pub struct Treasury;

/// Result of checking a payment against current policy.
struct PaymentPlan {
    required: u32,
    window: u64,
    window_spent: i128,
}

#[contractimpl]
impl Treasury {
    /// Deploy-time initialization. Running it in the constructor means the
    /// contract can never be observed (or front-run) in an uninitialized state.
    pub fn __constructor(env: Env, init: InitConfig) -> Result<(), Error> {
        validate_ttl_cap(init.max_proposal_ttl)?;
        let config = Config {
            governance_threshold: init.governance_threshold,
            governance_delay: init.governance_delay,
            max_proposal_ttl: init.max_proposal_ttl,
            allowlist_enabled: init.allowlist_enabled,
            paused: false,
            version: 0,
        };

        let mut members = Map::new(&env);
        for (member, roles) in init.members.iter() {
            validate_member(&env, &member, roles)?;
            members.set(member, roles);
        }

        let mut assets = Vec::new(&env);
        for (token, policy) in init.asset_policies.iter() {
            put_asset_policy(&env, &mut assets, &token, &policy)?;
        }
        for b in init.budgets.iter() {
            validate_budget(&b.budget)?;
            st::set_budget(&env, b.department, &b.token, &b.budget);
        }
        for recipient in init.allowlist.iter() {
            st::set_allowed(&env, &recipient, true);
        }

        check_invariants(&env, &config, &members, &assets)?;
        st::set_config(&env, &config);
        st::set_members(&env, &members);
        st::set_policy_assets(&env, &assets);
        st::set_next_proposal_id(&env, 0);
        st::bump_instance(&env);
        Ok(())
    }

    // ------------------------------------------------------------------
    // Proposals
    // ------------------------------------------------------------------

    /// Create a proposal for any action. Returns the proposal id.
    /// `ttl` is in seconds and must be in `1..=max_proposal_ttl`.
    pub fn propose(
        env: Env,
        proposer: Address,
        action: Action,
        ttl: u64,
        metadata_hash: Option<BytesN<32>>,
    ) -> Result<u64, Error> {
        st::bump_instance(&env);
        let members = st::get_members(&env);
        require_role(&members, &proposer, ROLE_PROPOSER)?;
        let config = st::get_config(&env);
        if ttl == 0 || ttl > config.max_proposal_ttl {
            return Err(Error::InvalidTtl);
        }
        // A governance proposal must outlive its timelock to be executable.
        if !is_payment(&action) && ttl <= config.governance_delay {
            return Err(Error::InvalidTtl);
        }
        prevalidate(&env, &config, &action)?;

        let id = st::next_proposal_id(&env);
        let now = env.ledger().timestamp();
        let proposal = Proposal {
            id,
            proposer: proposer.clone(),
            action: action.clone(),
            metadata_hash: metadata_hash.clone(),
            created_at: now,
            expires_at: now.checked_add(ttl).ok_or(Error::Overflow)?,
            config_version: config.version,
            approvals: Map::new(&env),
            ready_at: None,
            ready_epoch: 0,
            status: ProposalStatus::Pending,
            closed_at: 0,
        };
        st::set_proposal(&env, &proposal);
        st::set_next_proposal_id(&env, id.checked_add(1).ok_or(Error::Overflow)?);

        events::ProposalCreated {
            id,
            proposer,
            action,
            expires_at: proposal.expires_at,
            config_version: config.version,
            metadata_hash,
        }
        .publish(&env);
        Ok(id)
    }

    /// Convenience wrapper around [`Treasury::propose`] for payments.
    #[allow(clippy::too_many_arguments)]
    pub fn propose_payment(
        env: Env,
        proposer: Address,
        token: Address,
        to: Address,
        amount: i128,
        department: u32,
        ttl: u64,
        metadata_hash: Option<BytesN<32>>,
    ) -> Result<u64, Error> {
        let action = Action::Payment(Payment {
            token,
            to,
            amount,
            department,
        });
        Self::propose(env, proposer, action, ttl, metadata_hash)
    }

    /// Approve a pending proposal. Returns the number of approvals that
    /// currently count. Re-approving after a config change refreshes a stale
    /// approval.
    pub fn approve(env: Env, approver: Address, id: u64) -> Result<u32, Error> {
        st::bump_instance(&env);
        let members = st::get_members(&env);
        require_role(&members, &approver, ROLE_APPROVER)?;
        let mut proposal = load_proposal(&env, id)?;
        ensure_open(&env, &proposal)?;
        let config = st::get_config(&env);
        let epoch = approval_epoch(&env, &proposal, &config);

        if proposal.approvals.get(approver.clone()) == Some(epoch) {
            return Err(Error::AlreadyApproved);
        }
        proposal.approvals.set(approver.clone(), epoch);
        let approvals = count_valid_approvals(&proposal, &members, epoch);

        // Start the timelock the first time a governance proposal reaches the
        // threshold in this epoch.
        let mut ready = None;
        if !is_payment(&proposal.action)
            && approvals >= config.governance_threshold
            && (proposal.ready_at.is_none() || proposal.ready_epoch != epoch)
        {
            let ready_at = env
                .ledger()
                .timestamp()
                .checked_add(config.governance_delay)
                .ok_or(Error::Overflow)?;
            proposal.ready_at = Some(ready_at);
            proposal.ready_epoch = epoch;
            ready = Some(ready_at);
        }
        st::set_proposal(&env, &proposal);

        events::ProposalApproved {
            id,
            approver,
            approvals,
        }
        .publish(&env);
        if let Some(ready_at) = ready {
            events::ProposalReady { id, ready_at }.publish(&env);
        }
        Ok(approvals)
    }

    /// Withdraw your approval from a pending proposal. Former members may
    /// also call this to clean up.
    pub fn revoke_approval(env: Env, approver: Address, id: u64) -> Result<(), Error> {
        st::bump_instance(&env);
        approver.require_auth();
        let mut proposal = load_proposal(&env, id)?;
        ensure_open(&env, &proposal)?;
        if proposal.approvals.remove(approver.clone()).is_none() {
            return Err(Error::NotApproved);
        }
        if !is_payment(&proposal.action) {
            let config = st::get_config(&env);
            let members = st::get_members(&env);
            let approvals = count_valid_approvals(&proposal, &members, config.version);
            if approvals < config.governance_threshold {
                // Dropping below the threshold restarts the timelock.
                proposal.ready_at = None;
            }
        }
        st::set_proposal(&env, &proposal);
        events::ApprovalRevoked { id, approver }.publish(&env);
        Ok(())
    }

    /// Cancel a pending (or expired) proposal. Allowed for:
    /// * the proposer;
    /// * guardians, as an emergency brake / timelock veto, on any proposal
    ///   **except** one that removes or changes the roles of a current
    ///   guardian. That way a rogue guardian can always be removed.
    pub fn cancel(env: Env, caller: Address, id: u64) -> Result<(), Error> {
        st::bump_instance(&env);
        caller.require_auth();
        let mut proposal = load_proposal(&env, id)?;
        if proposal.status != ProposalStatus::Pending {
            return Err(Error::ProposalNotPending);
        }
        if caller != proposal.proposer {
            let members = st::get_members(&env);
            let roles = members.get(caller.clone()).unwrap_or(0);
            if roles & ROLE_GUARDIAN == 0 || targets_guardian(&members, &proposal.action) {
                return Err(Error::Unauthorized);
            }
        }
        proposal.status = ProposalStatus::Cancelled;
        proposal.closed_at = env.ledger().timestamp();
        st::set_proposal(&env, &proposal);
        events::ProposalCancelled { id, by: caller }.publish(&env);
        Ok(())
    }

    /// Execute a fully approved proposal. Every policy is re-checked here;
    /// this is the only place funds leave the treasury.
    pub fn execute(env: Env, executor: Address, id: u64) -> Result<(), Error> {
        st::bump_instance(&env);
        let mut members = st::get_members(&env);
        require_role(&members, &executor, ROLE_EXECUTOR)?;
        let mut proposal = load_proposal(&env, id)?;
        ensure_open(&env, &proposal)?;
        let mut config = st::get_config(&env);
        let epoch = approval_epoch(&env, &proposal, &config);
        let approvals = count_valid_approvals(&proposal, &members, epoch);

        // Effects before interactions: mark executed first so the proposal
        // can never run twice, even if the token contract misbehaves.
        proposal.status = ProposalStatus::Executed;
        proposal.closed_at = env.ledger().timestamp();

        match proposal.action.clone() {
            Action::Payment(payment) => {
                let plan = payment_plan(&env, &config, &payment)?;
                if approvals < plan.required {
                    return Err(Error::InsufficientApprovals);
                }
                st::set_proposal(&env, &proposal);
                st::set_budget_usage(
                    &env,
                    payment.department,
                    &payment.token,
                    &BudgetUsage {
                        window: plan.window,
                        spent: plan.window_spent,
                    },
                );
                token::TokenClient::new(&env, &payment.token).transfer(
                    &env.current_contract_address(),
                    &payment.to,
                    &payment.amount,
                );
                events::PaymentExecuted {
                    id,
                    token: payment.token,
                    to: payment.to,
                    amount: payment.amount,
                    department: payment.department,
                    window: plan.window,
                    window_spent: plan.window_spent,
                }
                .publish(&env);
            }
            action => {
                if approvals < config.governance_threshold {
                    return Err(Error::InsufficientApprovals);
                }
                check_timelock(&env, &proposal, epoch)?;
                st::set_proposal(&env, &proposal);
                let mut assets = st::get_policy_assets(&env);
                apply_governance(&env, &action, &mut config, &mut members, &mut assets)?;
                config.version = config.version.checked_add(1).ok_or(Error::Overflow)?;
                check_invariants(&env, &config, &members, &assets)?;
                st::set_config(&env, &config);
                st::set_members(&env, &members);
                st::set_policy_assets(&env, &assets);
                events::ConfigUpdated {
                    id,
                    version: config.version,
                    action,
                }
                .publish(&env);
            }
        }

        events::ProposalExecuted { id, executor }.publish(&env);
        Ok(())
    }

    // ------------------------------------------------------------------
    // Emergency + funding
    // ------------------------------------------------------------------

    /// Guardians can halt all payment execution immediately. Unpausing
    /// requires a governance proposal (`Action::Unpause`). Governance keeps
    /// working while paused so a rogue guardian can be removed.
    pub fn pause(env: Env, guardian: Address) -> Result<(), Error> {
        st::bump_instance(&env);
        let members = st::get_members(&env);
        require_role(&members, &guardian, ROLE_GUARDIAN)?;
        let mut config = st::get_config(&env);
        config.paused = true;
        st::set_config(&env, &config);
        events::Paused { by: guardian }.publish(&env);
        Ok(())
    }

    /// Deposit tokens and emit an indexable `deposited` event. Plain token
    /// transfers to the contract address also work; they just are not
    /// announced by this contract. Indexers must only trust deposits of
    /// tokens they recognise, since any contract can claim to be a token.
    pub fn deposit(env: Env, from: Address, token: Address, amount: i128) -> Result<(), Error> {
        st::bump_instance(&env);
        from.require_auth();
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        token::TokenClient::new(&env, &token).transfer(
            &from,
            env.current_contract_address(),
            &amount,
        );
        events::Deposited {
            token,
            from,
            amount,
        }
        .publish(&env);
        Ok(())
    }

    // ------------------------------------------------------------------
    // Views
    // ------------------------------------------------------------------

    pub fn contract_version(env: Env) -> String {
        String::from_str(&env, env!("CARGO_PKG_VERSION"))
    }

    pub fn get_config(env: Env) -> Config {
        st::get_config(&env)
    }

    pub fn get_members(env: Env) -> Map<Address, u32> {
        st::get_members(&env)
    }

    /// Role bitmask of `who`, or 0 if not a member.
    pub fn get_roles(env: Env, who: Address) -> u32 {
        st::get_members(&env).get(who).unwrap_or(0)
    }

    pub fn get_policy_assets(env: Env) -> Vec<Address> {
        st::get_policy_assets(&env)
    }

    pub fn get_asset_policy(env: Env, token: Address) -> Option<AssetPolicy> {
        st::get_asset_policy(&env, &token)
    }

    /// Budget and spend for the window containing the current ledger time.
    pub fn get_budget(env: Env, department: u32, token: Address) -> Option<BudgetStatus> {
        let budget = st::get_budget(&env, department, &token)?;
        let window = window_id(&budget.period, env.ledger().timestamp());
        let spent = match st::get_budget_usage(&env, department, &token) {
            Some(u) if u.window == window => u.spent,
            _ => 0,
        };
        Some(BudgetStatus {
            limit: budget.limit,
            period: budget.period,
            window,
            spent,
            remaining: budget.limit.saturating_sub(spent).max(0),
        })
    }

    pub fn is_recipient_allowed(env: Env, recipient: Address) -> bool {
        st::is_allowed(&env, &recipient)
    }

    pub fn get_proposal(env: Env, id: u64) -> Result<Proposal, Error> {
        load_proposal(&env, id)
    }

    /// Number of proposals ever created (ids are `0..count`).
    pub fn proposal_count(env: Env) -> u64 {
        st::next_proposal_id(&env)
    }

    /// Proposals with ids in `start..start+limit` (limit 1..=50).
    pub fn get_proposals(env: Env, start: u64, limit: u32) -> Result<Vec<Proposal>, Error> {
        if limit == 0 || limit > MAX_PAGE {
            return Err(Error::InvalidPagination);
        }
        let end = start
            .saturating_add(limit as u64)
            .min(st::next_proposal_id(&env));
        let mut out = Vec::new(&env);
        let mut id = start;
        while id < end {
            if let Some(p) = st::get_proposal(&env, id) {
                out.push_back(p);
            }
            id += 1;
        }
        Ok(out)
    }

    /// Live status of a proposal under the current config: how many
    /// approvals count, how many are needed, and what (if anything) would
    /// make `execute` fail right now. Governance invariants are only checked
    /// by `execute` itself; simulate it for a definitive answer.
    pub fn get_proposal_state(env: Env, id: u64) -> Result<ProposalState, Error> {
        let proposal = load_proposal(&env, id)?;
        let config = st::get_config(&env);
        let members = st::get_members(&env);
        let epoch = approval_epoch(&env, &proposal, &config);
        let approvals = count_valid_approvals(&proposal, &members, epoch);
        let ready_at = proposal.ready_at.filter(|_| proposal.ready_epoch == epoch);

        let expired = env.ledger().timestamp() >= proposal.expires_at;
        let status = match proposal.status {
            ProposalStatus::Pending if expired => ProposalStatus::Expired,
            s => s,
        };

        let (required, mut blocked_by) = match status {
            ProposalStatus::Pending => match &proposal.action {
                Action::Payment(payment) => match payment_plan(&env, &config, payment) {
                    Ok(plan) => (plan.required, None),
                    Err(e) => (0, Some(e as u32)),
                },
                _ => (config.governance_threshold, None),
            },
            ProposalStatus::Expired => (0, Some(Error::ProposalExpired as u32)),
            _ => (0, Some(Error::ProposalNotPending as u32)),
        };
        if blocked_by.is_none() && approvals < required {
            blocked_by = Some(Error::InsufficientApprovals as u32);
        }
        if blocked_by.is_none() && !is_payment(&proposal.action) {
            blocked_by = check_timelock(&env, &proposal, epoch)
                .err()
                .map(|e| e as u32);
        }

        Ok(ProposalState {
            status,
            approvals,
            required,
            ready_at,
            blocked_by,
        })
    }
}

// ----------------------------------------------------------------------
// Internal helpers
// ----------------------------------------------------------------------

fn require_role(members: &Map<Address, u32>, who: &Address, role: u32) -> Result<(), Error> {
    who.require_auth();
    match members.get(who.clone()) {
        Some(roles) if roles & role != 0 => Ok(()),
        _ => Err(Error::Unauthorized),
    }
}

fn load_proposal(env: &Env, id: u64) -> Result<Proposal, Error> {
    st::get_proposal(env, id).ok_or(Error::ProposalNotFound)
}

fn ensure_open(env: &Env, proposal: &Proposal) -> Result<(), Error> {
    if proposal.status != ProposalStatus::Pending {
        return Err(Error::ProposalNotPending);
    }
    if env.ledger().timestamp() >= proposal.expires_at {
        return Err(Error::ProposalExpired);
    }
    Ok(())
}

fn is_payment(action: &Action) -> bool {
    matches!(action, Action::Payment(_))
}

/// Epoch that approvals on `proposal` must carry to count.
fn approval_epoch(env: &Env, proposal: &Proposal, config: &Config) -> u32 {
    match &proposal.action {
        Action::Payment(payment) => st::get_asset_epoch(env, &payment.token),
        _ => config.version,
    }
}

/// Governance proposals may only run once the timelock that started when
/// they reached the threshold, in the current epoch, has ended.
fn check_timelock(env: &Env, proposal: &Proposal, epoch: u32) -> Result<(), Error> {
    match proposal.ready_at {
        Some(ready_at) if proposal.ready_epoch == epoch => {
            if env.ledger().timestamp() < ready_at {
                Err(Error::TimelockActive)
            } else {
                Ok(())
            }
        }
        _ => Err(Error::InsufficientApprovals),
    }
}

/// True if `action` removes or changes the roles of a current guardian.
/// Guardians cannot veto these, so a rogue guardian can always be removed.
fn targets_guardian(members: &Map<Address, u32>, action: &Action) -> bool {
    let target = match action {
        Action::RemoveMember(m) | Action::UpdateMember(m, _) => m,
        _ => return false,
    };
    members
        .get(target.clone())
        .is_some_and(|roles| roles & ROLE_GUARDIAN != 0)
}

/// Approvals that count: given under the current approval epoch by an
/// address that is still an approver.
fn count_valid_approvals(proposal: &Proposal, members: &Map<Address, u32>, epoch: u32) -> u32 {
    let mut count = 0;
    for (approver, approved_at) in proposal.approvals.iter() {
        let is_approver = members
            .get(approver)
            .is_some_and(|roles| roles & ROLE_APPROVER != 0);
        if approved_at == epoch && is_approver {
            count += 1;
        }
    }
    count
}

fn validate_roles(roles: u32) -> Result<(), Error> {
    if roles == 0 || roles & !ALL_ROLES != 0 {
        return Err(Error::InvalidRoles);
    }
    Ok(())
}

fn validate_member(env: &Env, member: &Address, roles: u32) -> Result<(), Error> {
    if *member == env.current_contract_address() {
        return Err(Error::InvalidMember);
    }
    validate_roles(roles)
}

fn validate_ttl_cap(ttl: u64) -> Result<(), Error> {
    if !(MIN_PROPOSAL_TTL_CAP..=MAX_PROPOSAL_TTL_CAP).contains(&ttl) {
        return Err(Error::InvalidTtl);
    }
    Ok(())
}

fn put_asset_policy(
    env: &Env,
    assets: &mut Vec<Address>,
    token: &Address,
    policy: &AssetPolicy,
) -> Result<(), Error> {
    validate_asset_policy(policy)?;
    if !assets.contains(token) {
        if assets.len() >= MAX_ASSETS {
            return Err(Error::TooManyAssets);
        }
        assets.push_back(token.clone());
    }
    st::set_asset_policy(env, token, policy);
    Ok(())
}

/// Static checks at proposal time so obviously invalid proposals fail fast.
/// Everything is checked again, against the then-current state, on execute.
fn prevalidate(env: &Env, config: &Config, action: &Action) -> Result<(), Error> {
    match action {
        Action::Payment(payment) => validate_payment(env, config, payment).map(|_| ()),
        Action::AddMember(member, roles) | Action::UpdateMember(member, roles) => {
            validate_member(env, member, *roles)
        }
        Action::SetGovernanceThreshold(0) => Err(Error::InvalidThreshold),
        Action::SetMaxProposalTtl(ttl) => validate_ttl_cap(*ttl),
        Action::SetGovernanceDelay(delay) if *delay > MAX_GOVERNANCE_DELAY => {
            Err(Error::InvalidDelay)
        }
        Action::SetAssetPolicy(_, policy) => validate_asset_policy(policy),
        Action::SetBudget(_, _, budget) => validate_budget(budget),
        _ => Ok(()),
    }
}

fn validate_payment(
    env: &Env,
    config: &Config,
    payment: &Payment,
) -> Result<(AssetPolicy, Budget), Error> {
    if payment.amount <= 0 {
        return Err(Error::InvalidAmount);
    }
    if payment.to == env.current_contract_address() {
        return Err(Error::InvalidRecipient);
    }
    let policy = st::get_asset_policy(env, &payment.token).ok_or(Error::AssetNotAllowed)?;
    if payment.amount > policy.max_amount {
        return Err(Error::AmountExceedsCap);
    }
    if config.allowlist_enabled && !st::is_allowed(env, &payment.to) {
        return Err(Error::RecipientNotAllowed);
    }
    let budget =
        st::get_budget(env, payment.department, &payment.token).ok_or(Error::BudgetNotFound)?;
    if payment.amount > budget.limit {
        return Err(Error::BudgetExceeded);
    }
    Ok((policy, budget))
}

/// Full execution-time check of a payment, including the spend counter for
/// the current window.
fn payment_plan(env: &Env, config: &Config, payment: &Payment) -> Result<PaymentPlan, Error> {
    if config.paused {
        return Err(Error::Paused);
    }
    let (policy, budget) = validate_payment(env, config, payment)?;
    let window = window_id(&budget.period, env.ledger().timestamp());
    let spent = match st::get_budget_usage(env, payment.department, &payment.token) {
        Some(usage) if usage.window == window => usage.spent,
        _ => 0,
    };
    let window_spent = spent.checked_add(payment.amount).ok_or(Error::Overflow)?;
    if window_spent > budget.limit {
        return Err(Error::BudgetExceeded);
    }
    Ok(PaymentPlan {
        required: required_approvals(&policy, payment.amount),
        window,
        window_spent,
    })
}

fn apply_governance(
    env: &Env,
    action: &Action,
    config: &mut Config,
    members: &mut Map<Address, u32>,
    assets: &mut Vec<Address>,
) -> Result<(), Error> {
    match action {
        Action::Payment(_) => unreachable!("payments are not governance actions"),
        Action::AddMember(member, roles) => {
            validate_member(env, member, *roles)?;
            if members.contains_key(member.clone()) {
                return Err(Error::MemberExists);
            }
            members.set(member.clone(), *roles);
        }
        Action::UpdateMember(member, roles) => {
            validate_member(env, member, *roles)?;
            if !members.contains_key(member.clone()) {
                return Err(Error::MemberNotFound);
            }
            members.set(member.clone(), *roles);
        }
        Action::RemoveMember(member) => {
            if members.remove(member.clone()).is_none() {
                return Err(Error::MemberNotFound);
            }
        }
        Action::SetGovernanceThreshold(threshold) => config.governance_threshold = *threshold,
        Action::SetMaxProposalTtl(ttl) => {
            validate_ttl_cap(*ttl)?;
            config.max_proposal_ttl = *ttl;
        }
        // Changes to a token's rules invalidate approvals on pending
        // payments in that token (and only that token).
        Action::SetAssetPolicy(token, policy) => {
            put_asset_policy(env, assets, token, policy)?;
            st::bump_asset_epoch(env, token);
        }
        Action::RemoveAssetPolicy(token) => {
            let index = assets.first_index_of(token).ok_or(Error::NotConfigured)?;
            assets.remove(index);
            st::remove_asset_policy(env, token);
            st::bump_asset_epoch(env, token);
        }
        Action::SetBudget(department, token, budget) => {
            validate_budget(budget)?;
            st::set_budget(env, *department, token, budget);
            st::bump_asset_epoch(env, token);
        }
        Action::RemoveBudget(department, token) => {
            if st::get_budget(env, *department, token).is_none() {
                return Err(Error::NotConfigured);
            }
            st::remove_budget(env, *department, token);
            st::bump_asset_epoch(env, token);
        }
        Action::SetAllowlistEnabled(enabled) => config.allowlist_enabled = *enabled,
        Action::SetRecipientAllowed(recipient, allowed) => {
            st::set_allowed(env, recipient, *allowed)
        }
        Action::Unpause => config.paused = false,
        Action::Upgrade(wasm_hash) => env
            .deployer()
            .update_current_contract(ContractExecutable::Wasm(wasm_hash.clone())),
        Action::SetGovernanceDelay(delay) => config.governance_delay = *delay,
    }
    Ok(())
}

/// Invariants that must hold after initialization and after every
/// governance action, so the treasury can never lock itself out.
fn check_invariants(
    env: &Env,
    config: &Config,
    members: &Map<Address, u32>,
    assets: &Vec<Address>,
) -> Result<(), Error> {
    let n = members.len();
    if n == 0 || n > MAX_MEMBERS {
        return Err(Error::InvalidMemberCount);
    }
    let (mut proposers, mut approvers, mut executors) = (0u32, 0u32, 0u32);
    for (_, roles) in members.iter() {
        proposers += (roles & ROLE_PROPOSER != 0) as u32;
        approvers += (roles & ROLE_APPROVER != 0) as u32;
        executors += (roles & ROLE_EXECUTOR != 0) as u32;
    }
    if proposers == 0 || approvers == 0 || executors == 0 {
        return Err(Error::MissingRequiredRole);
    }
    if config.governance_threshold == 0 || config.governance_threshold > approvers {
        return Err(Error::InvalidThreshold);
    }
    if config.governance_delay > MAX_GOVERNANCE_DELAY
        || config.governance_delay >= config.max_proposal_ttl
    {
        return Err(Error::InvalidDelay);
    }
    if assets.len() > MAX_ASSETS {
        return Err(Error::TooManyAssets);
    }
    for token in assets.iter() {
        let policy = st::get_asset_policy(env, &token).ok_or(Error::InvalidPolicy)?;
        if max_tier_approvals(&policy) > approvers {
            return Err(Error::InvalidThreshold);
        }
    }
    Ok(())
}
