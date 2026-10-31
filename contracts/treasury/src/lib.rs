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
mod governance;
mod migration;
mod payments;
mod policy;
mod proposals;
mod storage;
mod types;

#[cfg(test)]
mod test;

pub use errors::Error;
pub use types::*;

use soroban_sdk::{contract, contractimpl, token, Address, BytesN, Env, Map, String, Vec};

use crate::governance::{
    apply_governance, check_invariants, prevalidate, put_asset_policy, validate_member,
    validate_ttl_cap,
};
use crate::migration::enter;
use crate::payments::payment_plan;
use crate::policy::{validate_budget, window_id};
use crate::proposals::{
    approval_epoch, check_timelock, count_valid_approvals, counts_approval, ensure_open,
    ensure_prunable, is_payment, load_proposal, require_role, targets_guardian,
};
use crate::storage as st;

#[contract]
pub struct Treasury;

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
        st::set_storage_version(&env, STORAGE_VERSION);
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
        enter(&env)?;
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
        enter(&env)?;
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
            // Execution needs `ready_at <= now < expires_at`.
            if ready_at >= proposal.expires_at {
                return Err(Error::TimelockOutlivesProposal);
            }
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
        enter(&env)?;
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
        enter(&env)?;
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
        enter(&env)?;
        let mut members = st::get_members(&env);
        require_role(&members, &executor, ROLE_EXECUTOR)?;
        let mut proposal = load_proposal(&env, id)?;
        ensure_open(&env, &proposal)?;
        let mut config = st::get_config(&env);
        let epoch = approval_epoch(&env, &proposal, &config);
        let approvals = count_valid_approvals(&proposal, &members, epoch);
        if st::get_independent_executor(&env)
            && counts_approval(&proposal, &members, &executor, epoch)
        {
            return Err(Error::ExecutorApproved);
        }

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
        enter(&env)?;
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
        enter(&env)?;
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
    // Maintenance
    // ------------------------------------------------------------------

    /// Delete closed proposals from storage. Anyone may call this. Each id
    /// must belong to a proposal that was executed, cancelled or expired at
    /// least `PROPOSAL_RETENTION` (30 days) ago; otherwise nothing is pruned.
    /// Takes 1..=50 ids and returns how many were pruned. Ids are never
    /// reused, and the events keep the full history.
    pub fn prune_proposals(env: Env, ids: Vec<u64>) -> Result<u32, Error> {
        enter(&env)?;
        if ids.is_empty() || ids.len() > MAX_PAGE {
            return Err(Error::InvalidPagination);
        }
        for id in ids.iter() {
            let proposal = load_proposal(&env, id)?;
            ensure_prunable(&env, &proposal)?;
            st::remove_proposal(&env, id);
            events::ProposalPruned { id }.publish(&env);
        }
        Ok(ids.len())
    }

    /// Bring stored data up to the layout this code uses, after an upgrade.
    /// Anyone may call this; it only runs conversions built into the code
    /// governance approved. Returns the current storage version. Does
    /// nothing if the data is already current.
    pub fn migrate(env: Env) -> Result<u32, Error> {
        if let Some(from) = migration::migrate(&env)? {
            events::StorageMigrated {
                from,
                to: STORAGE_VERSION,
            }
            .publish(&env);
        }
        Ok(STORAGE_VERSION)
    }

    // ------------------------------------------------------------------
    // Views
    // ------------------------------------------------------------------

    pub fn contract_version(env: Env) -> String {
        String::from_str(&env, env!("CARGO_PKG_VERSION"))
    }

    /// True if a proposal cannot be executed by a member whose approval
    /// counts on it (`Action::SetIndependentExecutor`).
    pub fn independent_executor(env: Env) -> bool {
        st::get_independent_executor(&env)
    }

    /// Layout version of the stored data. If it is lower than the code's
    /// `STORAGE_VERSION`, call `migrate` before anything else.
    pub fn storage_version(env: Env) -> u32 {
        st::get_storage_version(&env)
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

    /// Proposals with ids in `start..start+limit` (limit 1..=50). Pruned
    /// proposals are skipped.
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
    /// make `execute` fail right now. Governance invariants and the
    /// independent-executor rule (which depends on who executes) are only
    /// checked by `execute` itself; simulate it for a definitive answer.
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
            // A governance proposal that would reach its threshold now could
            // still never run if the timelock outlasts it.
            let too_late = !is_payment(&proposal.action)
                && env
                    .ledger()
                    .timestamp()
                    .saturating_add(config.governance_delay)
                    >= proposal.expires_at;
            blocked_by = Some(if too_late {
                Error::TimelockOutlivesProposal
            } else {
                Error::InsufficientApprovals
            } as u32);
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
