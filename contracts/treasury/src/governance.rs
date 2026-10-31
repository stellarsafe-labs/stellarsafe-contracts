//! Governance actions: validation, application and the invariants that keep
//! the treasury from locking itself out.

use soroban_sdk::{Address, ContractExecutable, Env, Map, Vec};

use crate::errors::Error;
use crate::payments::validate_payment;
use crate::policy::{max_tier_approvals, validate_asset_policy, validate_budget};
use crate::storage as st;
use crate::types::{
    Action, AssetPolicy, Config, ALL_ROLES, MAX_ASSETS, MAX_GOVERNANCE_DELAY, MAX_MEMBERS,
    MAX_PROPOSAL_TTL_CAP, MIN_PROPOSAL_TTL_CAP, ROLE_APPROVER, ROLE_EXECUTOR, ROLE_PROPOSER,
};

fn validate_roles(roles: u32) -> Result<(), Error> {
    if roles == 0 || roles & !ALL_ROLES != 0 {
        return Err(Error::InvalidRoles);
    }
    Ok(())
}

pub fn validate_member(env: &Env, member: &Address, roles: u32) -> Result<(), Error> {
    if *member == env.current_contract_address() {
        return Err(Error::InvalidMember);
    }
    validate_roles(roles)
}

pub fn validate_ttl_cap(ttl: u64) -> Result<(), Error> {
    if !(MIN_PROPOSAL_TTL_CAP..=MAX_PROPOSAL_TTL_CAP).contains(&ttl) {
        return Err(Error::InvalidTtl);
    }
    Ok(())
}

pub fn put_asset_policy(
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
pub fn prevalidate(env: &Env, config: &Config, action: &Action) -> Result<(), Error> {
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

pub fn apply_governance(
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
        // The new code takes over after this invocation. If it uses a newer
        // storage layout, writes are refused until `migrate` runs.
        Action::Upgrade(wasm_hash) => env
            .deployer()
            .update_current_contract(ContractExecutable::Wasm(wasm_hash.clone())),
        Action::SetGovernanceDelay(delay) => config.governance_delay = *delay,
    }
    Ok(())
}

/// Invariants that must hold after initialization and after every
/// governance action, so the treasury can never lock itself out.
pub fn check_invariants(
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
