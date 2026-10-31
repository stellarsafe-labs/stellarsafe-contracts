//! The optional independent-executor rule: nobody may execute a proposal
//! their own approval counts on.
extern crate std;

use soroban_sdk::Address;

use super::setup::*;
use crate::{assert_contract_err, Action, Error};

/// Runs a governance action, executed by dave (who never approves).
fn govern_by_dave(f: &Fixture, action: Action) -> Result<(), Error> {
    let id = f.client.propose(&f.alice, &action, &TTL, &None);
    f.approve_by(id, &[&f.alice, &f.bob]);
    match f.client.try_execute(&f.dave, &id) {
        Ok(_) => Ok(()),
        Err(Ok(e)) => Err(e),
        Err(Err(e)) => panic!("host error: {e:?}"),
    }
}

fn independent() -> Fixture {
    let f = Fixture::new();
    f.govern(Action::SetIndependentExecutor(true));
    assert!(f.client.independent_executor());
    f
}

fn approved_payment(f: &Fixture, approver: &Address) -> u64 {
    let id = f.propose_pay(100, OPS);
    f.client.approve(approver, &id);
    id
}

#[test]
fn off_by_default_so_one_member_may_approve_and_execute() {
    let f = Fixture::new();
    assert!(!f.client.independent_executor());
    let id = approved_payment(&f, &f.alice);
    f.client.execute(&f.alice, &id);
    assert_eq!(f.balance(&f.vendor), 100);
}

#[test]
fn executor_whose_approval_counts_cannot_execute_a_payment() {
    let f = independent();
    let id = approved_payment(&f, &f.alice);
    assert_contract_err!(f.client.try_execute(&f.alice, &id), Error::ExecutorApproved);
    f.client.execute(&f.dave, &id);
    assert_eq!(f.balance(&f.vendor), 100);
}

#[test]
fn rule_applies_to_governance_too() {
    let f = independent();
    let id = f
        .client
        .propose(&f.alice, &Action::SetAllowlistEnabled(false), &TTL, &None);
    f.approve_by(id, &[&f.alice, &f.bob]);
    assert_contract_err!(f.client.try_execute(&f.alice, &id), Error::ExecutorApproved);
    f.client.execute(&f.dave, &id);
    assert!(!f.client.get_config().allowlist_enabled);
}

#[test]
fn stale_or_revoked_approval_does_not_block_the_executor() {
    let f = independent();
    // Stale: alice approved before the USDC rules changed.
    let stale = approved_payment(&f, &f.alice);
    govern_by_dave(
        &f,
        Action::SetAssetPolicy(f.usdc.clone(), usdc_policy(&f.env)),
    )
    .unwrap();
    f.client.approve(&f.bob, &stale);
    f.client.execute(&f.alice, &stale);

    // Revoked: alice withdraws her approval, carol approves instead.
    let revoked = approved_payment(&f, &f.alice);
    f.client.revoke_approval(&f.alice, &revoked);
    f.client.approve(&f.carol, &revoked);
    f.client.execute(&f.alice, &revoked);
    assert_eq!(f.balance(&f.vendor), 200);
}

#[test]
fn cannot_leave_no_executor_outside_the_needed_approvers() {
    // 3 approvers and a 3-approval tier: only dave (executor, not an
    // approver) can execute such a payment, so he can't be removed.
    let f = independent();
    assert_eq!(
        govern_by_dave(&f, Action::RemoveMember(f.dave.clone())),
        Err(Error::NoIndependentExecutor)
    );
    assert!(f.client.get_members().contains_key(f.dave.clone()));
}

#[test]
fn cannot_turn_on_without_an_independent_executor() {
    let f = Fixture::new();
    f.govern(Action::RemoveMember(f.dave.clone()));
    assert_eq!(
        f.try_govern(Action::SetIndependentExecutor(true)),
        Err(Error::NoIndependentExecutor)
    );
    // Lowering the top tier to 2 approvals leaves room for alice to execute.
    let mut policy = usdc_policy(&f.env);
    policy.tiers.pop_back();
    f.govern(Action::SetAssetPolicy(f.usdc.clone(), policy));
    f.govern(Action::SetIndependentExecutor(true));
    assert!(f.client.independent_executor());
}

#[test]
fn can_be_turned_off_again() {
    let f = independent();
    govern_by_dave(&f, Action::SetIndependentExecutor(false)).unwrap();
    let id = approved_payment(&f, &f.alice);
    f.client.execute(&f.alice, &id);
}
