extern crate std;

use soroban_sdk::{testutils::Address as _, vec, Address, Env, Map};

use super::setup::*;
use crate::{
    ApprovalTier, AssetPolicy, Budget, BudgetInit, InitConfig, Period, ProposalStatus, Treasury,
    ALL_ROLES, ROLE_APPROVER, ROLE_EXECUTOR, ROLE_PROPOSER,
};

fn minimal(env: &Env) -> (InitConfig, Address) {
    let admin = Address::generate(env);
    let token = Address::generate(env);
    let mut members = Map::new(env);
    members.set(admin.clone(), ROLE_PROPOSER | ROLE_APPROVER | ROLE_EXECUTOR);
    let mut init = base_init(env, &members, &token, &Address::generate(env));
    init.governance_threshold = 1;
    // Single-approver treasury: tiers can need at most 1 approval.
    let mut policies = Map::new(env);
    policies.set(
        token.clone(),
        AssetPolicy {
            max_amount: 100,
            tiers: vec![
                env,
                ApprovalTier {
                    min_amount: 0,
                    approvals: 1,
                },
            ],
        },
    );
    init.asset_policies = policies;
    (init, admin)
}

#[test]
fn initial_state_is_exposed_by_views() {
    let f = Fixture::new();
    let config = f.client.get_config();
    assert_eq!(config.governance_threshold, 2);
    assert_eq!(config.max_proposal_ttl, MAX_TTL);
    assert!(config.allowlist_enabled);
    assert!(!config.paused);
    assert_eq!(config.version, 0);

    assert_eq!(f.client.get_members().len(), 5);
    assert_eq!(f.client.get_roles(&f.carol), ROLE_APPROVER);
    assert_eq!(f.client.get_roles(&f.vendor), 0);
    assert_eq!(f.client.get_policy_assets(), vec![&f.env, f.usdc.clone()]);
    assert_eq!(
        f.client.get_asset_policy(&f.usdc),
        Some(usdc_policy(&f.env))
    );
    assert!(f.client.is_recipient_allowed(&f.vendor));
    assert!(!f.client.is_recipient_allowed(&f.bob));
    assert_eq!(f.client.proposal_count(), 0);
    assert_eq!(
        f.client.contract_version(),
        soroban_sdk::String::from_str(&f.env, "0.1.0")
    );

    let ops = f.client.get_budget(&OPS, &f.usdc).unwrap();
    assert_eq!(ops.limit, 8_000);
    assert_eq!(ops.spent, 0);
    assert_eq!(ops.remaining, 8_000);
    assert_eq!(ops.period, Period::CalendarMonth);
    assert!(f.client.get_budget(&99, &f.usdc).is_none());
}

#[test]
fn minimal_single_member_treasury_works() {
    let env = Env::default();
    let (init, _) = minimal(&env);
    env.register(Treasury, (init,));
}

#[test]
#[should_panic(expected = "Error(Contract, #103)")]
fn rejects_threshold_above_approver_count() {
    let env = Env::default();
    let (mut init, _) = minimal(&env);
    init.governance_threshold = 2;
    env.register(Treasury, (init,));
}

#[test]
#[should_panic(expected = "Error(Contract, #103)")]
fn rejects_zero_threshold() {
    let env = Env::default();
    let (mut init, _) = minimal(&env);
    init.governance_threshold = 0;
    env.register(Treasury, (init,));
}

#[test]
#[should_panic(expected = "Error(Contract, #103)")]
fn rejects_tier_requiring_more_approvals_than_approvers() {
    let env = Env::default();
    let (mut init, _) = minimal(&env);
    let token = init.asset_policies.keys().get(0).unwrap();
    init.asset_policies.set(token, usdc_policy(&env)); // needs up to 3 approvals
    env.register(Treasury, (init,));
}

#[test]
#[should_panic(expected = "Error(Contract, #101)")]
fn rejects_empty_member_set() {
    let env = Env::default();
    let (mut init, _) = minimal(&env);
    init.members = Map::new(&env);
    env.register(Treasury, (init,));
}

#[test]
#[should_panic(expected = "Error(Contract, #102)")]
fn rejects_treasury_without_executor() {
    let env = Env::default();
    let (mut init, admin) = minimal(&env);
    init.members.set(admin, ROLE_PROPOSER | ROLE_APPROVER);
    env.register(Treasury, (init,));
}

#[test]
#[should_panic(expected = "Error(Contract, #100)")]
fn rejects_unknown_role_bits() {
    let env = Env::default();
    let (mut init, _) = minimal(&env);
    init.members.set(Address::generate(&env), ALL_ROLES + 1);
    env.register(Treasury, (init,));
}

#[test]
#[should_panic(expected = "Error(Contract, #100)")]
fn rejects_member_without_roles() {
    let env = Env::default();
    let (mut init, _) = minimal(&env);
    init.members.set(Address::generate(&env), 0);
    env.register(Treasury, (init,));
}

#[test]
#[should_panic(expected = "Error(Contract, #104)")]
fn rejects_ttl_cap_too_small() {
    let env = Env::default();
    let (mut init, _) = minimal(&env);
    init.max_proposal_ttl = 60;
    env.register(Treasury, (init,));
}

#[test]
#[should_panic(expected = "Error(Contract, #104)")]
fn rejects_ttl_cap_too_large() {
    let env = Env::default();
    let (mut init, _) = minimal(&env);
    init.max_proposal_ttl = 91 * DAY;
    env.register(Treasury, (init,));
}

#[test]
#[should_panic(expected = "Error(Contract, #105)")]
fn rejects_invalid_policy() {
    let env = Env::default();
    let (mut init, _) = minimal(&env);
    let token = init.asset_policies.keys().get(0).unwrap();
    init.asset_policies.set(
        token,
        AssetPolicy {
            max_amount: 0,
            tiers: vec![&env],
        },
    );
    env.register(Treasury, (init,));
}

#[test]
#[should_panic(expected = "Error(Contract, #106)")]
fn rejects_invalid_budget() {
    let env = Env::default();
    let (mut init, _) = minimal(&env);
    init.budgets.push_back(BudgetInit {
        department: 7,
        token: Address::generate(&env),
        budget: Budget {
            limit: 100,
            period: Period::Fixed(10),
        },
    });
    env.register(Treasury, (init,));
}

#[test]
#[should_panic(expected = "Error(Contract, #107)")]
fn rejects_too_many_assets() {
    let env = Env::default();
    let (mut init, _) = minimal(&env);
    let policy = init.asset_policies.values().get(0).unwrap();
    for _ in 0..crate::MAX_ASSETS {
        init.asset_policies
            .set(Address::generate(&env), policy.clone());
    }
    env.register(Treasury, (init,));
}

#[test]
#[should_panic(expected = "Error(Contract, #101)")]
fn rejects_too_many_members() {
    let env = Env::default();
    let (mut init, _) = minimal(&env);
    for _ in 0..crate::MAX_MEMBERS {
        init.members.set(Address::generate(&env), ROLE_APPROVER);
    }
    env.register(Treasury, (init,));
}

#[test]
fn proposal_status_enum_is_stable() {
    // These discriminants are part of the ABI consumed by the SDK/indexer.
    assert_eq!(ProposalStatus::Pending as u32, 0);
    assert_eq!(ProposalStatus::Executed as u32, 1);
    assert_eq!(ProposalStatus::Cancelled as u32, 2);
    assert_eq!(ProposalStatus::Expired as u32, 3);
}
