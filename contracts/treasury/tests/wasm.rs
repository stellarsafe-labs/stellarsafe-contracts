//! Tests against the compiled Wasm, so VM instantiation and real storage
//! costs are metered against mainnet limits. Build the contract first:
//!
//! ```sh
//! stellar contract build
//! cargo test -p stellarsafe-treasury --features wasm-tests --test wasm
//! ```
#![cfg(feature = "wasm-tests")]

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token::{StellarAssetClient, TokenClient},
    vec, Address, Env, Map,
};

#[allow(clippy::too_many_arguments)]
mod treasury {
    soroban_sdk::contractimport!(
        file = "../../target/wasm32v1-none/release/stellarsafe_treasury.wasm"
    );
}

use treasury::{Action, ApprovalTier, AssetPolicy, Budget, BudgetInit, InitConfig, Period};

const TTL: u64 = 86_400;

struct Setup {
    env: Env,
    client: treasury::Client<'static>,
    id: Address,
    usdc: Address,
    members: std::vec::Vec<Address>,
    vendor: Address,
}

fn setup(member_count: usize) -> Setup {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_792_065_600);
    let usdc = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let vendor = Address::generate(&env);

    let mut members = Map::new(&env);
    let mut list = std::vec::Vec::new();
    for _ in 0..member_count {
        let m = Address::generate(&env);
        members.set(m.clone(), 0b0111); // proposer | approver | executor
        list.push(m);
    }
    let mut policies = Map::new(&env);
    policies.set(
        usdc.clone(),
        AssetPolicy {
            max_amount: 10_000,
            tiers: vec![
                &env,
                ApprovalTier {
                    min_amount: 0,
                    approvals: 1,
                },
                ApprovalTier {
                    min_amount: 500,
                    approvals: 2,
                },
            ],
        },
    );
    let init = InitConfig {
        members,
        governance_threshold: 2,
        governance_delay: 0,
        max_proposal_ttl: 30 * TTL,
        allowlist_enabled: true,
        allowlist: vec![&env, vendor.clone()],
        asset_policies: policies,
        budgets: vec![
            &env,
            BudgetInit {
                department: 1,
                token: usdc.clone(),
                budget: Budget {
                    limit: 8_000,
                    period: Period::CalendarMonth,
                },
            },
        ],
    };
    let id = env.register(treasury::WASM, (init,));
    StellarAssetClient::new(&env, &usdc).mint(&id, &20_000);
    let client = treasury::Client::new(&env, &id);
    Setup {
        env,
        client,
        id,
        usdc,
        members: list,
        vendor,
    }
}

#[test]
fn payment_flow_on_wasm() {
    let s = setup(3);
    let id = s
        .client
        .propose_payment(&s.members[0], &s.usdc, &s.vendor, &600, &1, &TTL, &None);
    s.client.approve(&s.members[0], &id);
    s.client.approve(&s.members[1], &id);
    s.client.execute(&s.members[2], &id);
    assert_eq!(TokenClient::new(&s.env, &s.usdc).balance(&s.vendor), 600);
    assert_eq!(TokenClient::new(&s.env, &s.usdc).balance(&s.id), 19_400);
}

#[test]
fn fifty_approvals_fit_mainnet_limits_on_wasm() {
    let s = setup(50);
    let id = s.client.propose(
        &s.members[0],
        &Action::SetAllowlistEnabled(false),
        &TTL,
        &None,
    );
    for m in &s.members {
        s.client.approve(m, &id);
    }
    s.client.execute(&s.members[0], &id);
    let res = s.env.cost_estimate().resources();
    std::println!("execute with 50 approvals: {res:#?}");
    assert!(
        res.instructions < 100_000_000,
        "instructions: {}",
        res.instructions
    );
}

#[test]
fn governance_can_upgrade_contract() {
    let s = setup(3);
    let hash = s.env.deployer().upload_contract_wasm(treasury::WASM);
    let id = s
        .client
        .propose(&s.members[0], &Action::Upgrade(hash), &TTL, &None);
    s.client.approve(&s.members[0], &id);
    s.client.approve(&s.members[1], &id);
    s.client.execute(&s.members[0], &id);
    // Storage survives the upgrade.
    assert_eq!(s.client.get_config().version, 1);
    assert_eq!(s.client.get_members().len(), 3);
}

#[test]
fn timelocked_governance_on_wasm() {
    let s = setup(3);
    let set_delay = s.client.propose(
        &s.members[0],
        &Action::SetGovernanceDelay(TTL),
        &(2 * TTL),
        &None,
    );
    s.client.approve(&s.members[0], &set_delay);
    s.client.approve(&s.members[1], &set_delay);
    s.client.execute(&s.members[0], &set_delay);

    let id = s
        .client
        .propose(&s.members[0], &Action::Unpause, &(2 * TTL), &None);
    s.client.approve(&s.members[0], &id);
    s.client.approve(&s.members[1], &id);
    assert_eq!(
        s.client.try_execute(&s.members[0], &id),
        Err(Ok(treasury::Error::TimelockActive))
    );
    let now = s.env.ledger().timestamp();
    s.env.ledger().set_timestamp(now + TTL);
    s.client.execute(&s.members[0], &id);
}
