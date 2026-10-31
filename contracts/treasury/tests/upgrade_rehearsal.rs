//! Upgrade rehearsal: runs a real upgrade, then `migrate`, against a
//! snapshot of a live treasury, metered against mainnet limits. Run it
//! before proposing any `Upgrade` on a real network; a migration that fails
//! or doesn't fit in one transaction would leave the treasury stuck.
//!
//! ```sh
//! ./scripts/upgrade-rehearsal.sh C...CONTRACT_ID   # NETWORK=testnet by default
//! ```
//!
//! The script builds the new Wasm, takes the snapshot and runs this test
//! with `STELLARSAFE_SNAPSHOT` (snapshot path) and `STELLARSAFE_CONTRACT`.
#![cfg(feature = "upgrade-rehearsal")]

use soroban_sdk::{testutils::Ledger as _, Address, Env, Vec};

#[allow(clippy::too_many_arguments)]
mod treasury {
    soroban_sdk::contractimport!(
        file = "../../target/wasm32v1-none/release/stellarsafe_treasury.wasm"
    );
}

use treasury::{Action, Error};

const ROLE_APPROVER: u32 = 1 << 1;
const ROLE_EXECUTOR: u32 = 1 << 2;
const ROLE_PROPOSER: u32 = 1 << 0;
/// Mainnet per-transaction instruction limit.
const MAX_INSTRUCTIONS: i64 = 100_000_000;

fn var(name: &str) -> std::string::String {
    std::env::var(name).unwrap_or_else(|_| panic!("set {name}; see scripts/upgrade-rehearsal.sh"))
}

fn assert_fits(env: &Env, what: &str) {
    let res = env.cost_estimate().resources();
    std::println!("{what}: {res:#?}");
    assert!(
        res.instructions < MAX_INSTRUCTIONS,
        "{what} needs {} instructions; the mainnet limit is {MAX_INSTRUCTIONS}",
        res.instructions
    );
}

#[test]
fn upgrade_and_migrate_live_treasury() {
    let env = Env::from_ledger_snapshot_file(var("STELLARSAFE_SNAPSHOT"));
    // Every test Env starts from the same PRNG seed, so mocked auth nonces
    // could collide with nonces already in a snapshot made by a test. Seed
    // from the snapshot's ledger instead.
    let mut seed = [0x5au8; 32];
    seed[..4].copy_from_slice(&env.ledger().sequence().to_be_bytes());
    seed[4..12].copy_from_slice(&env.ledger().timestamp().to_be_bytes());
    env.host().set_base_prng_seed(seed).unwrap();
    env.mock_all_auths();
    let id = Address::from_str(&env, &var("STELLARSAFE_CONTRACT"));
    let client = treasury::Client::new(&env, &id);

    // State before, read through the old code. Only functions that exist
    // in every released version are called until the upgrade has run.
    let config = client.get_config();
    let members = client.get_members();
    let assets = client.get_policy_assets();
    let policies: std::vec::Vec<_> = assets.iter().map(|t| client.get_asset_policy(&t)).collect();
    let count = client.proposal_count();
    let mut proposals = std::vec::Vec::new();
    let mut start = 0;
    while start < count {
        proposals.extend(client.get_proposals(&start, &50).iter());
        start += 50;
    }

    // Pick members for the governance flow. Approvers that can't execute go
    // first, so an executor who hasn't approved is left if there is one.
    let has = |roles: u32, role: u32| roles & role != 0;
    let proposer = members
        .iter()
        .find(|(_, r)| has(*r, ROLE_PROPOSER))
        .expect("no proposer")
        .0;
    let mut approvers: std::vec::Vec<(Address, u32)> = members
        .iter()
        .filter(|(_, r)| has(*r, ROLE_APPROVER))
        .collect();
    approvers.sort_by_key(|(_, r)| has(*r, ROLE_EXECUTOR));
    approvers.truncate(config.governance_threshold as usize);
    let executor = members
        .iter()
        .filter(|(_, r)| has(*r, ROLE_EXECUTOR))
        .map(|(a, _)| a)
        .find(|a| !approvers.iter().any(|(b, _)| b == a))
        .or_else(|| {
            members
                .iter()
                .find(|(_, r)| has(*r, ROLE_EXECUTOR))
                .map(|(a, _)| a)
        })
        .expect("no executor");

    // Upgrade through governance, exactly as it would run on-chain.
    let hash = env.deployer().upload_contract_wasm(treasury::WASM);
    let ttl = config.max_proposal_ttl;
    let upgrade = client.propose(&proposer, &Action::Upgrade(hash), &ttl, &None);
    for (approver, _) in &approvers {
        client.approve(approver, &upgrade);
    }
    let now = env.ledger().timestamp();
    env.ledger().set_timestamp(now + config.governance_delay);
    client.execute(&executor, &upgrade);
    assert_fits(&env, "execute(Upgrade)");

    // New code from here on.
    let current = client.migrate();
    assert_fits(&env, "migrate");
    assert_eq!(client.storage_version(), current);
    assert_eq!(client.migrate(), current, "migrate must be idempotent");

    // Nothing was lost or changed by the upgrade and migration.
    let after = client.get_config();
    assert_eq!(after.version, config.version + 1);
    assert_eq!(after.governance_threshold, config.governance_threshold);
    assert_eq!(after.governance_delay, config.governance_delay);
    assert_eq!(after.max_proposal_ttl, config.max_proposal_ttl);
    assert_eq!(after.allowlist_enabled, config.allowlist_enabled);
    assert_eq!(after.paused, config.paused);
    assert_eq!(client.get_members(), members);
    assert_eq!(client.get_policy_assets(), assets);
    for (token, policy) in assets.iter().zip(policies) {
        assert_eq!(client.get_asset_policy(&token), policy);
    }
    assert_eq!(client.proposal_count(), count + 1);
    for p in proposals {
        assert_eq!(client.get_proposal(&p.id), p, "proposal {}", p.id);
        client.get_proposal_state(&p.id);
    }

    // Writes work again: a new governance proposal can be created and
    // approved under the new code.
    let noop = Action::SetAllowlistEnabled(after.allowlist_enabled);
    let id = client.propose(&proposer, &noop, &ttl, &None);
    for (approver, _) in &approvers {
        client.approve(approver, &id);
    }
    assert_eq!(
        client.get_proposal_state(&id).approvals,
        config.governance_threshold
    );
    assert_eq!(
        client.try_prune_proposals(&Vec::new(&env)),
        Err(Ok(Error::InvalidPagination))
    );
}
