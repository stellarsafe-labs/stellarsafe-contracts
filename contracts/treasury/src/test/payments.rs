extern crate std;

use soroban_sdk::{
    testutils::{Address as _, Events as _},
    Address, BytesN, Event,
};

use super::setup::*;
use crate::{assert_contract_err, events, Action, Error, Payment, ProposalStatus};

#[test]
fn small_payment_needs_one_approval_and_moves_funds() {
    let f = Fixture::new();
    let id = f.propose_pay(100, OPS);
    assert_eq!(f.client.get_proposal_state(&id).required, 1);

    f.client.approve(&f.bob, &id);
    f.client.execute(&f.dave, &id);

    let event = events::PaymentExecuted {
        id,
        token: f.usdc.clone(),
        to: f.vendor.clone(),
        amount: 100,
        department: OPS,
        window: crate::policy::window_id(&crate::Period::CalendarMonth, OCT_15),
        window_spent: 100,
    };
    assert!(f
        .env
        .events()
        .all()
        .filter_by_contract(&f.id)
        .events()
        .contains(&event.to_xdr(&f.env, &f.id)));

    assert_eq!(f.balance(&f.vendor), 100);
    assert_eq!(f.balance(&f.id), TREASURY_FUNDS - 100);
    let p = f.client.get_proposal(&id);
    assert_eq!(p.status, ProposalStatus::Executed);
    assert_eq!(p.closed_at, OCT_15);
    assert_eq!(f.client.get_budget(&OPS, &f.usdc).unwrap().spent, 100);
}

#[test]
fn approval_tiers_apply_at_their_boundaries() {
    let f = Fixture::new();
    let cases = [
        (1, 1),
        (499, 1),
        (500, 2),
        (4_999, 2),
        (5_000, 3),
        (8_000, 3),
    ];
    for (amount, required) in cases {
        let id = f.propose_pay(amount, OPS);
        assert_eq!(
            f.client.get_proposal_state(&id).required,
            required,
            "amount {amount}"
        );
    }
}

#[test]
fn payment_of_500_needs_two_approvals() {
    let f = Fixture::new();
    let id = f.propose_pay(500, OPS);
    f.client.approve(&f.alice, &id);
    assert_contract_err!(
        f.client.try_execute(&f.dave, &id),
        Error::InsufficientApprovals
    );
    f.client.approve(&f.carol, &id);
    f.client.execute(&f.dave, &id);
    assert_eq!(f.balance(&f.vendor), 500);
}

#[test]
fn proposer_can_also_approve_their_own_proposal() {
    let f = Fixture::new();
    let id = f.propose_pay(10, OPS);
    f.client.approve(&f.alice, &id);
    f.client.execute(&f.alice, &id);
    assert_eq!(f.balance(&f.vendor), 10);
}

#[test]
fn rejects_non_positive_amounts() {
    let f = Fixture::new();
    for amount in [0, -1, i128::MIN] {
        assert_contract_err!(
            f.client
                .try_propose_payment(&f.alice, &f.usdc, &f.vendor, &amount, &OPS, &TTL, &None),
            Error::InvalidAmount
        );
    }
}

#[test]
fn rejects_amount_above_asset_cap() {
    let f = Fixture::new();
    assert_contract_err!(
        f.client
            .try_propose_payment(&f.alice, &f.usdc, &f.vendor, &10_001, &OPS, &TTL, &None),
        Error::AmountExceedsCap
    );
}

#[test]
fn rejects_tokens_without_policy() {
    let f = Fixture::new();
    let other = f
        .env
        .register_stellar_asset_contract_v2(Address::generate(&f.env))
        .address();
    assert_contract_err!(
        f.client
            .try_propose_payment(&f.alice, &other, &f.vendor, &1, &OPS, &TTL, &None),
        Error::AssetNotAllowed
    );
}

#[test]
fn rejects_recipients_not_on_allowlist() {
    let f = Fixture::new();
    let stranger = Address::generate(&f.env);
    assert_contract_err!(
        f.client
            .try_propose_payment(&f.alice, &f.usdc, &stranger, &1, &OPS, &TTL, &None),
        Error::RecipientNotAllowed
    );
}

#[test]
fn rejects_department_without_budget() {
    let f = Fixture::new();
    assert_contract_err!(
        f.client
            .try_propose_payment(&f.alice, &f.usdc, &f.vendor, &1, &42, &TTL, &None),
        Error::BudgetNotFound
    );
}

#[test]
fn rejects_payment_to_treasury_itself() {
    let f = Fixture::new();
    assert_contract_err!(
        f.client
            .try_propose_payment(&f.alice, &f.usdc, &f.id, &1, &OPS, &TTL, &None),
        Error::InvalidRecipient
    );
}

#[test]
fn rejects_single_payment_larger_than_budget() {
    let f = Fixture::new();
    assert_contract_err!(
        f.client
            .try_propose_payment(&f.alice, &f.usdc, &f.vendor, &301, &MARKETING, &TTL, &None),
        Error::BudgetExceeded
    );
}

#[test]
fn ttl_must_be_within_bounds() {
    let f = Fixture::new();
    for ttl in [0, MAX_TTL + 1] {
        assert_contract_err!(
            f.client
                .try_propose_payment(&f.alice, &f.usdc, &f.vendor, &1, &OPS, &ttl, &None),
            Error::InvalidTtl
        );
    }
    f.client
        .propose_payment(&f.alice, &f.usdc, &f.vendor, &1, &OPS, &MAX_TTL, &None);
}

#[test]
fn metadata_hash_is_stored() {
    let f = Fixture::new();
    let hash = BytesN::from_array(&f.env, &[7; 32]);
    let id = f.client.propose_payment(
        &f.alice,
        &f.usdc,
        &f.vendor,
        &1,
        &OPS,
        &TTL,
        &Some(hash.clone()),
    );
    let p = f.client.get_proposal(&id);
    assert_eq!(p.metadata_hash, Some(hash));
    assert_eq!(p.proposer, f.alice);
    assert_eq!(p.created_at, OCT_15);
    assert_eq!(p.expires_at, OCT_15 + TTL);
    assert_eq!(
        p.action,
        Action::Payment(Payment {
            token: f.usdc.clone(),
            to: f.vendor.clone(),
            amount: 1,
            department: OPS
        })
    );
}

#[test]
fn insufficient_treasury_balance_reverts_and_keeps_proposal_pending() {
    let f = Fixture::new();
    // Raise the budget so only the balance can be the limiting factor.
    f.govern(Action::SetBudget(
        OPS,
        f.usdc.clone(),
        crate::Budget {
            limit: 100_000,
            period: crate::Period::CalendarMonth,
        },
    ));
    f.pay(10_000, OPS);
    f.pay(9_000, OPS);
    let id = f.propose_pay(5_000, OPS);
    f.approve_by(id, &[&f.alice, &f.bob, &f.carol]);
    assert!(f.client.try_execute(&f.dave, &id).is_err());
    assert_eq!(f.client.get_proposal(&id).status, ProposalStatus::Pending);
    assert_eq!(f.client.get_budget(&OPS, &f.usdc).unwrap().spent, 19_000);

    // Top up and retry.
    f.usdc_admin.mint(&f.id, &10_000);
    f.client.execute(&f.dave, &id);
    assert_eq!(f.balance(&f.vendor), 24_000);
}

#[test]
fn deposit_moves_funds_and_emits_event() {
    let f = Fixture::new();
    let donor = Address::generate(&f.env);
    f.usdc_admin.mint(&donor, &1_000);
    f.client.deposit(&donor, &f.usdc, &250);
    let event = events::Deposited {
        token: f.usdc.clone(),
        from: donor.clone(),
        amount: 250,
    };
    assert!(f
        .env
        .events()
        .all()
        .filter_by_contract(&f.id)
        .events()
        .contains(&event.to_xdr(&f.env, &f.id)));
    assert_eq!(f.balance(&f.id), TREASURY_FUNDS + 250);
    assert_contract_err!(
        f.client.try_deposit(&donor, &f.usdc, &0),
        Error::InvalidAmount
    );
}
