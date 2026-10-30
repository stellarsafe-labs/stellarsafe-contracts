extern crate std;

use soroban_sdk::testutils::Address as _;
use soroban_sdk::Address;

use super::setup::*;
use crate::{
    assert_contract_err, Action, Budget, Error, Period, ProposalStatus, ROLE_APPROVER,
    ROLE_EXECUTOR, ROLE_PROPOSER,
};

#[test]
fn approve_returns_running_count() {
    let f = Fixture::new();
    let id = f.propose_pay(600, OPS);
    assert_eq!(f.client.approve(&f.alice, &id), 1);
    assert_eq!(f.client.approve(&f.bob, &id), 2);
}

#[test]
fn duplicate_approval_is_rejected() {
    let f = Fixture::new();
    let id = f.propose_pay(600, OPS);
    f.client.approve(&f.alice, &id);
    assert_contract_err!(f.client.try_approve(&f.alice, &id), Error::AlreadyApproved);
    // Still only one approval counts.
    assert_eq!(f.client.get_proposal_state(&id).approvals, 1);
    assert_contract_err!(
        f.client.try_execute(&f.dave, &id),
        Error::InsufficientApprovals
    );
}

#[test]
fn non_approvers_cannot_approve() {
    let f = Fixture::new();
    let id = f.propose_pay(10, OPS);
    for who in [&f.dave, &f.guardian, &f.vendor] {
        assert_contract_err!(f.client.try_approve(who, &id), Error::Unauthorized);
    }
}

#[test]
fn revoked_approval_no_longer_counts() {
    let f = Fixture::new();
    let id = f.propose_pay(600, OPS);
    f.approve_by(id, &[&f.alice, &f.bob]);
    f.client.revoke_approval(&f.bob, &id);
    assert_eq!(f.client.get_proposal_state(&id).approvals, 1);
    assert_contract_err!(
        f.client.try_execute(&f.dave, &id),
        Error::InsufficientApprovals
    );
    // Can approve again after revoking.
    f.client.approve(&f.bob, &id);
    f.client.execute(&f.dave, &id);
}

#[test]
fn cannot_revoke_without_approving() {
    let f = Fixture::new();
    let id = f.propose_pay(600, OPS);
    assert_contract_err!(
        f.client.try_revoke_approval(&f.bob, &id),
        Error::NotApproved
    );
}

#[test]
fn unknown_proposal_is_reported() {
    let f = Fixture::new();
    assert_contract_err!(f.client.try_approve(&f.alice, &99), Error::ProposalNotFound);
    assert_contract_err!(f.client.try_execute(&f.alice, &99), Error::ProposalNotFound);
    assert_contract_err!(f.client.try_cancel(&f.alice, &99), Error::ProposalNotFound);
    assert_contract_err!(f.client.try_get_proposal(&99), Error::ProposalNotFound);
}

#[test]
fn expiry_is_exclusive_at_expires_at() {
    let f = Fixture::new();
    let id = f.propose_pay(10, OPS);
    f.client.approve(&f.alice, &id);

    f.set_time(OCT_15 + TTL - 1);
    f.client.approve(&f.bob, &id); // still open one second before expiry

    f.set_time(OCT_15 + TTL);
    assert_contract_err!(f.client.try_execute(&f.dave, &id), Error::ProposalExpired);
    assert_contract_err!(f.client.try_approve(&f.carol, &id), Error::ProposalExpired);
    assert_contract_err!(
        f.client.try_revoke_approval(&f.bob, &id),
        Error::ProposalExpired
    );
    assert_eq!(
        f.client.get_proposal_state(&id).status,
        ProposalStatus::Expired
    );
    assert_eq!(f.balance(&f.vendor), 0);
}

#[test]
fn expired_proposal_can_be_cancelled_for_cleanup() {
    let f = Fixture::new();
    let id = f.propose_pay(10, OPS);
    f.advance(TTL);
    f.client.cancel(&f.alice, &id);
    assert_eq!(f.client.get_proposal(&id).status, ProposalStatus::Cancelled);
}

#[test]
fn unrelated_config_change_keeps_payment_approvals() {
    let f = Fixture::new();
    let id = f.propose_pay(600, OPS);
    f.approve_by(id, &[&f.alice, &f.bob]);

    // Governance changes that don't touch USDC's policy or budgets.
    f.govern(Action::SetAllowlistEnabled(true));
    f.govern(Action::AddMember(Address::generate(&f.env), ROLE_APPROVER));
    assert_eq!(f.client.get_config().version, 2);

    assert_eq!(f.client.get_proposal_state(&id).approvals, 2);
    f.client.execute(&f.dave, &id);
    assert_eq!(f.balance(&f.vendor), 600);
}

#[test]
fn asset_policy_change_invalidates_payment_approvals() {
    let f = Fixture::new();
    let id = f.propose_pay(600, OPS);
    f.approve_by(id, &[&f.alice, &f.bob]);

    f.govern(Action::SetAssetPolicy(f.usdc.clone(), usdc_policy(&f.env)));
    assert_eq!(f.client.get_proposal_state(&id).approvals, 0);
    assert_contract_err!(
        f.client.try_execute(&f.dave, &id),
        Error::InsufficientApprovals
    );

    // Approvers re-confirm under the new rules.
    f.approve_by(id, &[&f.alice, &f.bob]);
    f.client.execute(&f.dave, &id);
}

#[test]
fn budget_change_invalidates_payment_approvals_in_that_token() {
    let f = Fixture::new();
    let id = f.propose_pay(10, OPS);
    f.client.approve(&f.alice, &id);
    // Changing a *different department's* budget for the same token still
    // counts: approvals are scoped per token.
    f.govern(Action::SetBudget(
        MARKETING,
        f.usdc.clone(),
        Budget {
            limit: 1_000,
            period: Period::Fixed(DAY),
        },
    ));
    assert_eq!(f.client.get_proposal_state(&id).approvals, 0);
}

#[test]
fn other_token_change_keeps_payment_approvals() {
    let f = Fixture::new();
    let id = f.propose_pay(10, OPS);
    f.client.approve(&f.alice, &id);
    let eurc = Address::generate(&f.env);
    f.govern(Action::SetAssetPolicy(eurc.clone(), usdc_policy(&f.env)));
    f.govern(Action::SetBudget(
        OPS,
        eurc,
        Budget {
            limit: 1,
            period: Period::CalendarMonth,
        },
    ));
    assert_eq!(f.client.get_proposal_state(&id).approvals, 1);
}

#[test]
fn any_governance_execution_invalidates_governance_approvals() {
    let f = Fixture::new();
    let pending = f
        .client
        .propose(&f.alice, &Action::SetAllowlistEnabled(false), &TTL, &None);
    f.approve_by(pending, &[&f.alice, &f.bob]);
    f.govern(Action::Unpause);
    assert_eq!(f.client.get_proposal_state(&pending).approvals, 0);
    assert_contract_err!(
        f.client.try_execute(&f.alice, &pending),
        Error::InsufficientApprovals
    );
    f.approve_by(pending, &[&f.alice, &f.bob]);
    f.client.execute(&f.alice, &pending);
}

#[test]
fn removed_member_approval_does_not_count() {
    let f = Fixture::new();
    // Add a fourth approver so carol can be removed without breaking invariants.
    let erin = Address::generate(&f.env);
    f.govern(Action::AddMember(erin.clone(), ROLE_APPROVER));

    let id = f.propose_pay(600, OPS);
    f.approve_by(id, &[&f.carol]);
    // Remove carol (re-approve with alice+bob after the version bump).
    let gov = f.client.propose(
        &f.alice,
        &Action::RemoveMember(f.carol.clone()),
        &TTL,
        &None,
    );
    f.approve_by(gov, &[&f.alice, &f.bob]);
    f.client.execute(&f.alice, &gov);

    // Even refreshing approvals: carol cannot approve anymore, and her stale
    // approval is ignored.
    assert_contract_err!(f.client.try_approve(&f.carol, &id), Error::Unauthorized);
    f.client.approve(&f.alice, &id);
    assert_eq!(f.client.get_proposal_state(&id).approvals, 1);
    f.client.approve(&erin, &id);
    f.client.execute(&f.dave, &id);
}

#[test]
fn approver_demoted_to_executor_loses_vote() {
    let f = Fixture::new();
    // Keep three approvers so the 3-approval tier stays reachable.
    f.govern(Action::AddMember(Address::generate(&f.env), ROLE_APPROVER));
    let id = f.propose_pay(600, OPS);
    // Demote bob: version bump also clears approvals, so approve after.
    f.govern(Action::UpdateMember(
        f.bob.clone(),
        ROLE_PROPOSER | ROLE_EXECUTOR,
    ));
    assert_contract_err!(f.client.try_approve(&f.bob, &id), Error::Unauthorized);
    f.approve_by(id, &[&f.alice, &f.carol]);
    f.client.execute(&f.bob, &id); // bob may now execute
    assert_eq!(f.client.get_roles(&f.bob), ROLE_PROPOSER | ROLE_EXECUTOR);
}
