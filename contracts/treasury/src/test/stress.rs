//! Worst-case sizes must fit within mainnet resource limits, which the test
//! environment enforces by default.
extern crate std;

use soroban_sdk::{testutils::Address as _, vec, Address};

use super::setup::*;
use crate::{
    Action, ApprovalTier, AssetPolicy, MAX_ASSETS, MAX_MEMBERS, ROLE_APPROVER, ROLE_PROPOSER,
};

#[test]
fn max_members_max_assets_full_approval_set_fits_limits() {
    let f = Fixture::new();
    // Grow to MAX_MEMBERS (5 already present).
    let mut extra = std::vec::Vec::new();
    for _ in 5..MAX_MEMBERS {
        let m = Address::generate(&f.env);
        f.govern(Action::AddMember(m.clone(), ROLE_PROPOSER | ROLE_APPROVER));
        extra.push(m);
    }
    assert_eq!(f.client.get_members().len(), MAX_MEMBERS);

    // Grow to MAX_ASSETS (1 already present).
    for _ in 1..MAX_ASSETS {
        let token = Address::generate(&f.env);
        let policy = AssetPolicy {
            max_amount: 1_000,
            tiers: vec![
                &f.env,
                ApprovalTier {
                    min_amount: 0,
                    approvals: 1,
                },
            ],
        };
        f.govern(Action::SetAssetPolicy(token, policy));
    }
    assert_eq!(f.client.get_policy_assets().len(), MAX_ASSETS);
    let one_more = Address::generate(&f.env);
    assert_eq!(
        f.try_govern(Action::SetAssetPolicy(one_more, usdc_policy(&f.env))),
        Err(crate::Error::TooManyAssets)
    );
    assert_eq!(
        f.try_govern(Action::AddMember(Address::generate(&f.env), ROLE_APPROVER)),
        Err(crate::Error::InvalidMemberCount)
    );

    // Every approver signs one governance proposal and one payment.
    let gov = f
        .client
        .propose(&f.alice, &Action::SetAllowlistEnabled(true), &TTL, &None);
    let pay = f.propose_pay(5_000, OPS);
    for a in [&f.alice, &f.bob, &f.carol].into_iter().chain(extra.iter()) {
        f.client.approve(a, &pay);
    }
    assert_eq!(f.client.get_proposal_state(&pay).approvals, 48);
    f.client.execute(&f.dave, &pay);

    for a in [&f.alice, &f.bob, &f.carol].into_iter().chain(extra.iter()) {
        f.client.approve(a, &gov);
    }
    f.client.execute(&f.alice, &gov);
    assert_eq!(f.client.get_proposals(&0, &crate::MAX_PAGE).len(), 50);
}
