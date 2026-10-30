extern crate std;

use soroban_sdk::{
    testutils::{Address as _, AuthorizedFunction, AuthorizedInvocation, MockAuth, MockAuthInvoke},
    Address, IntoVal, Symbol,
};

use super::setup::*;
use crate::{assert_contract_err, Action, Error, ProposalStatus, ROLE_GUARDIAN};

#[test]
fn payment_cannot_be_executed_twice() {
    let f = Fixture::new();
    let id = f.pay(100, OPS);
    assert_contract_err!(
        f.client.try_execute(&f.dave, &id),
        Error::ProposalNotPending
    );
    assert_contract_err!(
        f.client.try_execute(&f.alice, &id),
        Error::ProposalNotPending
    );
    assert_eq!(f.balance(&f.vendor), 100);
    assert_eq!(f.client.get_budget(&OPS, &f.usdc).unwrap().spent, 100);
}

#[test]
fn governance_cannot_be_executed_twice() {
    let f = Fixture::new();
    let id = f.govern(Action::SetAllowlistEnabled(false));
    assert_contract_err!(
        f.client.try_execute(&f.alice, &id),
        Error::ProposalNotPending
    );
    assert_eq!(f.client.get_config().version, 1);
}

#[test]
fn closed_proposals_reject_all_mutations() {
    let f = Fixture::new();
    let id = f.propose_pay(10, OPS);
    f.client.approve(&f.alice, &id);
    f.client.cancel(&f.alice, &id);
    assert_contract_err!(
        f.client.try_execute(&f.dave, &id),
        Error::ProposalNotPending
    );
    assert_contract_err!(f.client.try_approve(&f.bob, &id), Error::ProposalNotPending);
    assert_contract_err!(
        f.client.try_revoke_approval(&f.alice, &id),
        Error::ProposalNotPending
    );
    assert_contract_err!(
        f.client.try_cancel(&f.alice, &id),
        Error::ProposalNotPending
    );
}

#[test]
fn only_executors_can_execute() {
    let f = Fixture::new();
    let id = f.propose_pay(10, OPS);
    f.client.approve(&f.alice, &id);
    for who in [&f.bob, &f.carol, &f.guardian, &f.vendor] {
        assert_contract_err!(f.client.try_execute(who, &id), Error::Unauthorized);
    }
}

#[test]
fn only_proposers_can_propose() {
    let f = Fixture::new();
    for who in [&f.carol, &f.dave, &f.guardian, &f.vendor] {
        assert_contract_err!(
            f.client
                .try_propose_payment(who, &f.usdc, &f.vendor, &1, &OPS, &TTL, &None),
            Error::Unauthorized
        );
        assert_contract_err!(
            f.client
                .try_propose(who, &Action::SetAllowlistEnabled(false), &TTL, &None),
            Error::Unauthorized
        );
    }
}

#[test]
fn approval_requires_the_approvers_own_signature() {
    let f = Fixture::new();
    let id = f.propose_pay(10, OPS);
    // Alice signs, but tries to approve in bob's name.
    f.env.mock_auths(&[MockAuth {
        address: &f.alice,
        invoke: &MockAuthInvoke {
            contract: &f.id,
            fn_name: "approve",
            args: (&f.bob, id).into_val(&f.env),
            sub_invokes: &[],
        },
    }]);
    assert!(f.client.try_approve(&f.bob, &id).is_err());
    assert_eq!(f.client.get_proposal(&id).approvals.len(), 0);
}

#[test]
fn every_state_changing_call_requires_caller_auth() {
    let f = Fixture::new();
    let id = f.propose_pay(10, OPS);
    f.client.approve(&f.bob, &id);
    assert_eq!(
        f.env.auths(),
        std::vec![(
            f.bob.clone(),
            AuthorizedInvocation {
                function: AuthorizedFunction::Contract((
                    f.id.clone(),
                    Symbol::new(&f.env, "approve"),
                    (&f.bob, id).into_val(&f.env),
                )),
                sub_invocations: std::vec![],
            }
        )]
    );

    f.client.execute(&f.dave, &id);
    let auths = f.env.auths();
    assert_eq!(auths.len(), 1);
    assert_eq!(auths[0].0, f.dave);
}

#[test]
fn calls_without_any_auth_fail() {
    let f = Fixture::new();
    let id = f.propose_pay(10, OPS);
    f.env.mock_auths(&[]);
    assert!(f.client.try_approve(&f.alice, &id).is_err());
    assert!(f.client.try_execute(&f.dave, &id).is_err());
    assert!(f.client.try_pause(&f.guardian).is_err());
    assert!(f.client.try_cancel(&f.alice, &id).is_err());
    assert!(f
        .client
        .try_propose_payment(&f.alice, &f.usdc, &f.vendor, &1, &OPS, &TTL, &None)
        .is_err());
}

#[test]
fn guardian_pause_blocks_payments_but_not_governance() {
    let f = Fixture::new();
    let id = f.propose_pay(10, OPS);
    f.client.approve(&f.alice, &id);

    f.client.pause(&f.guardian);
    assert!(f.client.get_config().paused);
    assert_contract_err!(f.client.try_execute(&f.dave, &id), Error::Paused);

    // Proposing and governance still work.
    f.propose_pay(10, OPS);
    f.govern(Action::Unpause);
    assert!(!f.client.get_config().paused);

    // Pausing and unpausing don't invalidate the payment's approval.
    f.client.execute(&f.dave, &id);
}

#[test]
fn only_guardians_can_pause() {
    let f = Fixture::new();
    for who in [&f.alice, &f.bob, &f.vendor] {
        assert_contract_err!(f.client.try_pause(who), Error::Unauthorized);
    }
}

#[test]
fn rogue_guardian_can_be_removed_while_paused() {
    let f = Fixture::new();
    f.client.pause(&f.guardian);
    f.govern(Action::RemoveMember(f.guardian.clone()));
    assert_contract_err!(f.client.try_pause(&f.guardian), Error::Unauthorized);
    f.govern(Action::Unpause);
    f.pay(10, OPS);
}

#[test]
fn guardian_cannot_veto_removal_of_a_guardian() {
    let f = Fixture::new();
    let pay = f.propose_pay(10, OPS);
    f.client.cancel(&f.guardian, &pay);
    assert_eq!(
        f.client.get_proposal(&pay).status,
        ProposalStatus::Cancelled
    );

    let gov = f.client.propose(
        &f.alice,
        &Action::RemoveMember(f.guardian.clone()),
        &TTL,
        &None,
    );
    assert_contract_err!(f.client.try_cancel(&f.guardian, &gov), Error::Unauthorized);
}

#[test]
fn other_members_cannot_cancel_someone_elses_proposal() {
    let f = Fixture::new();
    let id = f.propose_pay(10, OPS);
    for who in [&f.bob, &f.carol, &f.dave, &f.vendor] {
        assert_contract_err!(f.client.try_cancel(who, &id), Error::Unauthorized);
    }
    f.client.cancel(&f.alice, &id);
}

#[test]
fn approvals_cannot_be_carried_across_policy_change_to_bypass_tiers() {
    // Approvers sign a 400 payment (1 approval). Governance then tightens the
    // policy so 400 needs 2. The old approval must not be enough.
    let f = Fixture::new();
    let id = f.propose_pay(400, OPS);
    f.client.approve(&f.alice, &id);

    let mut policy = usdc_policy(&f.env);
    policy.tiers.set(
        1,
        crate::ApprovalTier {
            min_amount: 100,
            approvals: 2,
        },
    );
    f.govern(Action::SetAssetPolicy(f.usdc.clone(), policy));

    f.client.approve(&f.alice, &id);
    assert_eq!(f.client.get_proposal_state(&id).required, 2);
    assert_contract_err!(
        f.client.try_execute(&f.dave, &id),
        Error::InsufficientApprovals
    );
}

#[test]
fn approvals_record_the_current_epoch() {
    let f = Fixture::new();
    let erin = Address::generate(&f.env);
    let pay = f.propose_pay(600, OPS);
    let gov = f.client.propose(&f.alice, &Action::Unpause, &TTL, &None);
    f.govern(Action::AddMember(
        erin.clone(),
        crate::ROLE_APPROVER | ROLE_GUARDIAN,
    ));
    f.govern(Action::SetAssetPolicy(f.usdc.clone(), usdc_policy(&f.env)));
    f.client.approve(&erin, &pay);
    f.client.approve(&erin, &gov);
    // Payment epoch = USDC epoch (1); governance epoch = config version (2).
    assert_eq!(
        f.client.get_proposal(&pay).approvals.get(erin.clone()),
        Some(1)
    );
    assert_eq!(f.client.get_proposal(&gov).approvals.get(erin), Some(2));
}
