//! Storage layout and TTL management.
//!
//! * Instance storage (lives and dies with the contract instance): config,
//!   member set, list of assets with a policy, proposal counter, storage
//!   layout version. All of these are bounded (see `MAX_MEMBERS`, `MAX_ASSETS`).
//! * Persistent storage: asset policies, budgets, spend counters, allowlist
//!   entries and proposals. Persistent entries are archived (not deleted) when
//!   their TTL runs out and can be restored, so spend counters can never be
//!   silently reset. Temporary storage is deliberately not used.

use soroban_sdk::{contracttype, Address, Env, Map, Vec};

use crate::types::{AssetPolicy, Budget, BudgetUsage, Config, Proposal, FIRST_STORAGE_VERSION};

const DAY_IN_LEDGERS: u32 = 17_280;
const INSTANCE_BUMP: u32 = 120 * DAY_IN_LEDGERS;
const INSTANCE_THRESHOLD: u32 = INSTANCE_BUMP - 30 * DAY_IN_LEDGERS;
const PERSISTENT_BUMP: u32 = 120 * DAY_IN_LEDGERS;
const PERSISTENT_THRESHOLD: u32 = PERSISTENT_BUMP - 30 * DAY_IN_LEDGERS;

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    // instance
    Config,
    Members,
    PolicyAssets,
    NextProposalId,
    // persistent
    AssetPolicy(Address),
    Budget(u32, Address),
    BudgetUsage(u32, Address),
    Allowed(Address),
    Proposal(u64),
    AssetEpoch(Address),
    // instance (added in storage version 1; append new keys at the end)
    StorageVersion,
}

pub fn bump_instance(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(INSTANCE_THRESHOLD, INSTANCE_BUMP);
}

fn get_persistent<V: soroban_sdk::TryFromVal<Env, soroban_sdk::Val>>(
    env: &Env,
    key: &DataKey,
) -> Option<V> {
    let store = env.storage().persistent();
    let value = store.get::<DataKey, V>(key);
    if value.is_some() {
        store.extend_ttl(key, PERSISTENT_THRESHOLD, PERSISTENT_BUMP);
    }
    value
}

fn set_persistent<V: soroban_sdk::IntoVal<Env, soroban_sdk::Val>>(
    env: &Env,
    key: &DataKey,
    value: &V,
) {
    let store = env.storage().persistent();
    store.set(key, value);
    store.extend_ttl(key, PERSISTENT_THRESHOLD, PERSISTENT_BUMP);
}

// ----- config -----

pub fn get_config(env: &Env) -> Config {
    env.storage()
        .instance()
        .get(&DataKey::Config)
        .expect("not initialized")
}

pub fn set_config(env: &Env, config: &Config) {
    env.storage().instance().set(&DataKey::Config, config);
}

// ----- members -----

pub fn get_members(env: &Env) -> Map<Address, u32> {
    env.storage()
        .instance()
        .get(&DataKey::Members)
        .unwrap_or_else(|| Map::new(env))
}

pub fn set_members(env: &Env, members: &Map<Address, u32>) {
    env.storage().instance().set(&DataKey::Members, members);
}

// ----- asset policies -----

pub fn get_policy_assets(env: &Env) -> Vec<Address> {
    env.storage()
        .instance()
        .get(&DataKey::PolicyAssets)
        .unwrap_or_else(|| Vec::new(env))
}

pub fn set_policy_assets(env: &Env, assets: &Vec<Address>) {
    env.storage().instance().set(&DataKey::PolicyAssets, assets);
}

pub fn get_asset_policy(env: &Env, token: &Address) -> Option<AssetPolicy> {
    get_persistent(env, &DataKey::AssetPolicy(token.clone()))
}

pub fn set_asset_policy(env: &Env, token: &Address, policy: &AssetPolicy) {
    set_persistent(env, &DataKey::AssetPolicy(token.clone()), policy);
}

pub fn remove_asset_policy(env: &Env, token: &Address) {
    env.storage()
        .persistent()
        .remove(&DataKey::AssetPolicy(token.clone()));
}

// ----- budgets -----

pub fn get_budget(env: &Env, department: u32, token: &Address) -> Option<Budget> {
    get_persistent(env, &DataKey::Budget(department, token.clone()))
}

pub fn set_budget(env: &Env, department: u32, token: &Address, budget: &Budget) {
    set_persistent(env, &DataKey::Budget(department, token.clone()), budget);
}

pub fn remove_budget(env: &Env, department: u32, token: &Address) {
    env.storage()
        .persistent()
        .remove(&DataKey::Budget(department, token.clone()));
}

/// Spend counters are intentionally *not* removed with the budget: if a
/// budget is removed and re-added within the same window, the amount already
/// spent in that window still counts.
pub fn get_budget_usage(env: &Env, department: u32, token: &Address) -> Option<BudgetUsage> {
    get_persistent(env, &DataKey::BudgetUsage(department, token.clone()))
}

pub fn set_budget_usage(env: &Env, department: u32, token: &Address, usage: &BudgetUsage) {
    set_persistent(env, &DataKey::BudgetUsage(department, token.clone()), usage);
}

// ----- approval epochs -----

/// Approval epoch for payments in `token`. Bumped whenever the token's
/// asset policy or any of its budgets change, which invalidates approvals
/// on pending payments in that token only.
pub fn get_asset_epoch(env: &Env, token: &Address) -> u32 {
    get_persistent(env, &DataKey::AssetEpoch(token.clone())).unwrap_or(0)
}

pub fn bump_asset_epoch(env: &Env, token: &Address) {
    let next = get_asset_epoch(env, token).wrapping_add(1);
    set_persistent(env, &DataKey::AssetEpoch(token.clone()), &next);
}

// ----- allowlist -----

pub fn is_allowed(env: &Env, recipient: &Address) -> bool {
    get_persistent::<bool>(env, &DataKey::Allowed(recipient.clone())).unwrap_or(false)
}

pub fn set_allowed(env: &Env, recipient: &Address, allowed: bool) {
    let key = DataKey::Allowed(recipient.clone());
    if allowed {
        set_persistent(env, &key, &true);
    } else {
        env.storage().persistent().remove(&key);
    }
}

// ----- proposals -----

pub fn next_proposal_id(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey::NextProposalId)
        .unwrap_or(0)
}

pub fn set_next_proposal_id(env: &Env, id: u64) {
    env.storage().instance().set(&DataKey::NextProposalId, &id);
}

pub fn get_proposal(env: &Env, id: u64) -> Option<Proposal> {
    get_persistent(env, &DataKey::Proposal(id))
}

pub fn set_proposal(env: &Env, proposal: &Proposal) {
    set_persistent(env, &DataKey::Proposal(proposal.id), proposal);
}

pub fn remove_proposal(env: &Env, id: u64) {
    env.storage().persistent().remove(&DataKey::Proposal(id));
}

// ----- storage layout version -----

/// Layout version of the stored data. Treasuries deployed before the
/// version was recorded use the first layout.
pub fn get_storage_version(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::StorageVersion)
        .unwrap_or(FIRST_STORAGE_VERSION)
}

pub fn set_storage_version(env: &Env, version: u32) {
    env.storage()
        .instance()
        .set(&DataKey::StorageVersion, &version);
}
