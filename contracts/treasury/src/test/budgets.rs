extern crate std;

use super::setup::*;
use crate::{assert_contract_err, policy::window_id, Action, Budget, Error, Period};

#[test]
fn monthly_budget_accumulates_and_blocks_overspend() {
    let f = Fixture::new();
    f.pay(4_000, OPS);
    f.pay(3_500, OPS);
    let status = f.client.get_budget(&OPS, &f.usdc).unwrap();
    assert_eq!(status.spent, 7_500);
    assert_eq!(status.remaining, 500);

    // 501 fits the asset cap and the total budget size but not what's left.
    let id = f.propose_pay(501, OPS);
    f.approve_by(id, &[&f.alice, &f.bob]);
    assert_contract_err!(f.client.try_execute(&f.dave, &id), Error::BudgetExceeded);

    // Exactly the remainder is fine.
    f.pay(500, OPS);
    assert_eq!(f.client.get_budget(&OPS, &f.usdc).unwrap().remaining, 0);
}

#[test]
fn budget_is_enforced_at_execution_not_proposal() {
    let f = Fixture::new();
    // Two proposals that each fit, but not together.
    let a = f.propose_pay(5_000, OPS);
    let b = f.propose_pay(5_000, OPS);
    f.approve_by(a, &[&f.alice, &f.bob, &f.carol]);
    f.approve_by(b, &[&f.alice, &f.bob, &f.carol]);
    f.client.execute(&f.dave, &a);
    assert_contract_err!(f.client.try_execute(&f.dave, &b), Error::BudgetExceeded);
}

#[test]
fn monthly_budget_resets_exactly_at_utc_month_boundary() {
    let f = Fixture::new();
    f.pay(8_000, OPS);
    f.set_time(OCT_31_LAST_SECOND);
    let id = f.propose_pay(1, OPS);
    f.client.approve(&f.alice, &id);
    assert_contract_err!(f.client.try_execute(&f.dave, &id), Error::BudgetExceeded);

    f.set_time(NOV_1);
    f.client.execute(&f.dave, &id);
    let status = f.client.get_budget(&OPS, &f.usdc).unwrap();
    assert_eq!(status.spent, 1);
    assert_eq!(status.window, window_id(&Period::CalendarMonth, NOV_1));
}

#[test]
fn monthly_budget_rolls_over_year_end() {
    let f = Fixture::new();
    f.set_time(DEC_31_LAST_SECOND);
    f.pay(8_000, OPS);
    f.set_time(JAN_1_2027);
    f.pay(8_000, OPS);
    assert_eq!(f.client.get_budget(&OPS, &f.usdc).unwrap().spent, 8_000);
}

#[test]
fn fixed_daily_budget_resets_each_utc_day() {
    let f = Fixture::new();
    f.pay(300, MARKETING);
    let id = f.propose_pay(1, MARKETING);
    f.client.approve(&f.alice, &id);
    assert_contract_err!(f.client.try_execute(&f.dave, &id), Error::BudgetExceeded);

    // OCT_15 is noon; midnight is 12h away.
    f.advance(DAY / 2);
    f.client.execute(&f.dave, &id);
    assert_eq!(f.client.get_budget(&MARKETING, &f.usdc).unwrap().spent, 1);
}

#[test]
fn departments_are_accounted_separately() {
    let f = Fixture::new();
    f.pay(300, MARKETING);
    f.pay(8_000, OPS);
    assert_eq!(
        f.client.get_budget(&MARKETING, &f.usdc).unwrap().remaining,
        0
    );
    assert_eq!(f.client.get_budget(&OPS, &f.usdc).unwrap().remaining, 0);
}

#[test]
fn removing_and_readding_budget_keeps_spend_in_same_window() {
    let f = Fixture::new();
    f.pay(8_000, OPS);
    f.govern(Action::RemoveBudget(OPS, f.usdc.clone()));
    assert!(f.client.get_budget(&OPS, &f.usdc).is_none());
    assert_contract_err!(
        f.client
            .try_propose_payment(&f.alice, &f.usdc, &f.vendor, &1, &OPS, &TTL, &None),
        Error::BudgetNotFound
    );
    f.govern(Action::SetBudget(
        OPS,
        f.usdc.clone(),
        Budget {
            limit: 8_000,
            period: Period::CalendarMonth,
        },
    ));
    assert_eq!(f.client.get_budget(&OPS, &f.usdc).unwrap().spent, 8_000);
}

#[test]
fn lowering_budget_below_spent_blocks_further_payments() {
    let f = Fixture::new();
    f.pay(4_000, OPS);
    f.govern(Action::SetBudget(
        OPS,
        f.usdc.clone(),
        Budget {
            limit: 3_000,
            period: Period::CalendarMonth,
        },
    ));
    let status = f.client.get_budget(&OPS, &f.usdc).unwrap();
    assert_eq!(status.remaining, 0);
    let id = f.propose_pay(1, OPS);
    f.client.approve(&f.alice, &id);
    assert_contract_err!(f.client.try_execute(&f.dave, &id), Error::BudgetExceeded);
}
