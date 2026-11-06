extern crate std;

use soroban_sdk::{
    testutils::{Address as _, Events as _},
    Address, Event,
};

use super::setup::*;
use crate::{
    assert_contract_err, events, Action, Budget, Error, PaymentTimelock, Period, ProposalStatus,
    Treasury, ROLE_APPROVER, ROLE_GUARDIAN, ROLE_PROPOSER,
};

const DELAY: u64 = 2 * DAY;

/// Fixture with a 2-day governance timelock.
fn timelocked() -> Fixture {
    let f = Fixture::new();
    f.govern(Action::SetGovernanceDelay(DELAY)); // delay is 0 until this runs
    assert_eq!(f.client.get_config().governance_delay, DELAY);
    f
}

fn approved_gov(f: &Fixture, action: Action) -> u64 {
    let id = f.client.propose(&f.alice, &action, &TTL, &None);
    f.approve_by(id, &[&f.alice, &f.bob]);
    id
}

#[test]
fn governance_waits_for_the_delay() {
    let f = timelocked();
    let id = approved_gov(&f, Action::SetAllowlistEnabled(false));
    let ready_at = OCT_15 + DELAY;
    assert_eq!(f.client.get_proposal(&id).ready_at, Some(ready_at));

    let state = f.client.get_proposal_state(&id);
    assert_eq!(state.ready_at, Some(ready_at));
    assert_eq!(state.blocked_by, Some(Error::TimelockActive as u32));
    assert_contract_err!(f.client.try_execute(&f.alice, &id), Error::TimelockActive);

    f.set_time(ready_at - 1);
    assert_contract_err!(f.client.try_execute(&f.alice, &id), Error::TimelockActive);
    f.set_time(ready_at);
    assert_eq!(f.client.get_proposal_state(&id).blocked_by, None);
    f.client.execute(&f.alice, &id);
    assert!(!f.client.get_config().allowlist_enabled);
}

#[test]
fn reaching_threshold_emits_ready_event_once() {
    let f = timelocked();
    let id = f.client.propose(&f.alice, &Action::Unpause, &TTL, &None);
    f.client.approve(&f.alice, &id);
    assert_eq!(f.client.get_proposal(&id).ready_at, None);
    f.client.approve(&f.bob, &id);
    let ready = events::ProposalReady {
        id,
        ready_at: OCT_15 + DELAY,
    };
    let emitted = f.env.events().all().filter_by_contract(&f.id);
    assert!(emitted.events().contains(&ready.to_xdr(&f.env, &f.id)));

    // A third approval later doesn't restart the clock.
    f.advance(DAY);
    f.client.approve(&f.carol, &id);
    assert_eq!(f.client.get_proposal(&id).ready_at, Some(OCT_15 + DELAY));
}

#[test]
fn dropping_below_threshold_restarts_the_timelock() {
    let f = timelocked();
    let id = approved_gov(&f, Action::Unpause);
    f.advance(DAY);
    f.client.revoke_approval(&f.bob, &id);
    assert_eq!(f.client.get_proposal(&id).ready_at, None);

    f.advance(DAY);
    f.client.approve(&f.bob, &id);
    let restarted = OCT_15 + 2 * DAY + DELAY;
    assert_eq!(f.client.get_proposal(&id).ready_at, Some(restarted));
    f.set_time(OCT_15 + DELAY);
    assert_contract_err!(f.client.try_execute(&f.alice, &id), Error::TimelockActive);
    f.set_time(restarted);
    f.client.execute(&f.alice, &id);
}

#[test]
fn revoking_above_threshold_keeps_the_timelock() {
    let f = timelocked();
    let id = approved_gov(&f, Action::Unpause);
    f.client.approve(&f.carol, &id);
    f.client.revoke_approval(&f.carol, &id);
    assert_eq!(f.client.get_proposal(&id).ready_at, Some(OCT_15 + DELAY));
}

#[test]
fn stale_ready_time_from_older_epoch_is_ignored() {
    let f = timelocked();
    let slow = approved_gov(&f, Action::SetAllowlistEnabled(false));
    let fast = approved_gov(&f, Action::Unpause);
    f.set_time(OCT_15 + DELAY);
    f.client.execute(&f.alice, &fast); // bumps the governance epoch

    // `slow` had a ready time, but from the old epoch: it needs fresh
    // approvals and a fresh delay.
    assert_eq!(f.client.get_proposal_state(&slow).ready_at, None);
    assert_contract_err!(
        f.client.try_execute(&f.alice, &slow),
        Error::InsufficientApprovals
    );
    f.approve_by(slow, &[&f.alice, &f.bob]);
    assert_contract_err!(f.client.try_execute(&f.alice, &slow), Error::TimelockActive);
    f.advance(DELAY);
    f.client.execute(&f.alice, &slow);
}

#[test]
fn payments_are_not_timelocked() {
    let f = timelocked();
    f.pay(600, OPS);
    assert_eq!(f.balance(&f.vendor), 600);
}

#[test]
fn changing_the_delay_is_itself_timelocked() {
    let f = timelocked();
    let id = approved_gov(&f, Action::SetGovernanceDelay(0));
    assert_contract_err!(f.client.try_execute(&f.alice, &id), Error::TimelockActive);
    f.advance(DELAY);
    f.client.execute(&f.alice, &id);
    assert_eq!(f.client.get_config().governance_delay, 0);
}

#[test]
fn guardian_can_veto_queued_governance() {
    let f = timelocked();
    let hash = soroban_sdk::BytesN::from_array(&f.env, &[9; 32]);
    let id = approved_gov(&f, Action::Upgrade(hash));
    f.client.cancel(&f.guardian, &id);
    assert_eq!(f.client.get_proposal(&id).status, ProposalStatus::Cancelled);
}

#[test]
fn guardian_cannot_veto_actions_targeting_a_guardian() {
    let f = timelocked();
    let second_guardian = Address::generate(&f.env);
    let add = approved_gov(
        &f,
        Action::AddMember(second_guardian.clone(), ROLE_GUARDIAN),
    );
    f.advance(DELAY);
    f.client.execute(&f.alice, &add);

    for action in [
        Action::RemoveMember(f.guardian.clone()),
        Action::UpdateMember(f.guardian.clone(), ROLE_APPROVER),
        Action::RemoveMember(second_guardian.clone()),
    ] {
        let id = f.client.propose(&f.alice, &action, &TTL, &None);
        assert_contract_err!(f.client.try_cancel(&f.guardian, &id), Error::Unauthorized);
        assert_contract_err!(
            f.client.try_cancel(&second_guardian, &id),
            Error::Unauthorized
        );
    }
    // Non-guardian member targets remain vetoable.
    let id = f
        .client
        .propose(&f.alice, &Action::RemoveMember(f.dave.clone()), &TTL, &None);
    f.client.cancel(&f.guardian, &id);
}

#[test]
fn governance_ttl_must_outlive_the_delay() {
    let f = timelocked();
    assert_contract_err!(
        f.client
            .try_propose(&f.alice, &Action::Unpause, &DELAY, &None),
        Error::InvalidTtl
    );
    f.client
        .propose(&f.alice, &Action::Unpause, &(DELAY + 1), &None);
    // Payments may use short TTLs regardless.
    f.client
        .propose_payment(&f.alice, &f.usdc, &f.vendor, &1, &OPS, &60, &None);
}

#[test]
fn delay_bounds_are_enforced() {
    let f = Fixture::new();
    assert_contract_err!(
        f.client
            .try_propose(&f.alice, &Action::SetGovernanceDelay(31 * DAY), &TTL, &None),
        Error::InvalidDelay
    );
    // Delay must be shorter than the max proposal TTL (30 days here).
    assert_eq!(
        f.try_govern(Action::SetGovernanceDelay(MAX_TTL)),
        Err(Error::InvalidDelay)
    );
    f.govern(Action::SetGovernanceDelay(DAY));
    // ...and the TTL cap can't be lowered below the delay.
    let id = f
        .client
        .propose(&f.alice, &Action::SetMaxProposalTtl(3_600), &TTL, &None);
    f.approve_by(id, &[&f.alice, &f.bob]);
    f.advance(DAY);
    assert_contract_err!(f.client.try_execute(&f.alice, &id), Error::InvalidDelay);
}

#[test]
#[should_panic(expected = "Error(Contract, #112)")]
fn constructor_rejects_delay_not_below_ttl_cap() {
    let f = Fixture::new();
    let mut init = base_init(&f.env, &f.client.get_members(), &f.usdc, &f.vendor);
    init.governance_delay = init.max_proposal_ttl;
    f.env.register(Treasury, (init,));
}

#[test]
fn approval_that_would_start_a_too_late_timelock_is_rejected() {
    let f = timelocked();
    let ttl = 3 * DAY;
    let id = f.client.propose(&f.alice, &Action::Unpause, &ttl, &None);
    let expires_at = f.client.get_proposal(&id).expires_at;
    f.client.approve(&f.alice, &id);

    // At this moment the timelock would end exactly at expiry: too late.
    f.set_time(expires_at - DELAY);
    let blocked = Some(Error::TimelockOutlivesProposal as u32);
    assert_eq!(f.client.get_proposal_state(&id).blocked_by, blocked);
    assert_contract_err!(
        f.client.try_approve(&f.bob, &id),
        Error::TimelockOutlivesProposal
    );
    assert_eq!(f.client.get_proposal(&id).ready_at, None);

    // One second earlier it still fits.
    f.set_time(expires_at - DELAY - 1);
    let state = f.client.get_proposal_state(&id);
    assert_eq!(state.blocked_by, Some(Error::InsufficientApprovals as u32));
    f.client.approve(&f.bob, &id);
    assert_eq!(f.client.get_proposal(&id).ready_at, Some(expires_at - 1));
    f.set_time(expires_at - 1);
    f.client.execute(&f.dave, &id);
}

#[test]
fn restarted_timelock_must_also_fit_before_expiry() {
    let f = timelocked();
    let id = f
        .client
        .propose(&f.alice, &Action::Unpause, &(3 * DAY), &None);
    let expires_at = f.client.get_proposal(&id).expires_at;
    f.approve_by(id, &[&f.alice, &f.bob]);
    // Dropping below the threshold and coming back restarts the clock.
    f.set_time(expires_at - DELAY);
    f.client.revoke_approval(&f.bob, &id);
    assert_contract_err!(
        f.client.try_approve(&f.bob, &id),
        Error::TimelockOutlivesProposal
    );
}

// ----------------------------------------------------------------------
// Payment timelock
// ----------------------------------------------------------------------

const PAY_DELAY: u64 = DAY;
/// Smallest delayed USDC payment. It needs 2 approvals (500..5_000 tier).
const DELAY_FROM: i128 = 1_000;

fn usdc_timelock() -> PaymentTimelock {
    PaymentTimelock {
        delay_from: DELAY_FROM,
        delay: PAY_DELAY,
    }
}

/// Fixture where USDC payments of at least 1_000 wait one day.
fn payment_timelocked() -> Fixture {
    let f = Fixture::new();
    f.govern(Action::SetPaymentTimelock(f.usdc.clone(), usdc_timelock()));
    f
}

/// Proposes a 1_000 USDC payment and approves it with alice and bob.
fn approved_large_payment(f: &Fixture) -> u64 {
    let id = f.propose_pay(DELAY_FROM, OPS);
    f.approve_by(id, &[&f.alice, &f.bob]);
    id
}

#[test]
fn payments_below_the_timelock_threshold_run_at_once() {
    let f = payment_timelocked();
    let id = f.propose_pay(DELAY_FROM - 1, OPS);
    f.approve_by(id, &[&f.alice, &f.bob]);
    assert_eq!(f.client.get_proposal(&id).ready_at, None);
    let state = f.client.get_proposal_state(&id);
    assert_eq!((state.ready_at, state.blocked_by), (None, None));
    f.client.execute(&f.dave, &id);
    assert_eq!(f.balance(&f.vendor), DELAY_FROM - 1);
    // Short TTLs stay fine for payments that are not delayed.
    f.client
        .propose_payment(&f.alice, &f.usdc, &f.vendor, &1, &OPS, &60, &None);
}

#[test]
fn payment_at_the_threshold_waits_to_the_second() {
    let f = payment_timelocked();
    let id = f.propose_pay(DELAY_FROM, OPS);
    f.client.approve(&f.alice, &id);
    assert_eq!(f.client.get_proposal(&id).ready_at, None);
    f.advance(10);
    f.client.approve(&f.bob, &id);
    let ready_at = OCT_15 + 10 + PAY_DELAY;
    let ready = events::ProposalReady { id, ready_at };
    let emitted = f.env.events().all().filter_by_contract(&f.id);
    assert!(emitted.events().contains(&ready.to_xdr(&f.env, &f.id)));
    assert_eq!(f.client.get_proposal(&id).ready_at, Some(ready_at));

    let state = f.client.get_proposal_state(&id);
    assert_eq!(state.ready_at, Some(ready_at));
    assert_eq!(state.blocked_by, Some(Error::TimelockActive as u32));
    assert_contract_err!(f.client.try_execute(&f.dave, &id), Error::TimelockActive);

    f.set_time(ready_at - 1);
    assert_contract_err!(f.client.try_execute(&f.dave, &id), Error::TimelockActive);
    f.set_time(ready_at);
    assert_eq!(f.client.get_proposal_state(&id).blocked_by, None);
    f.client.execute(&f.dave, &id);
    assert_eq!(f.balance(&f.vendor), DELAY_FROM);
}

#[test]
fn extra_payment_approval_does_not_restart_the_clock() {
    let f = payment_timelocked();
    let id = approved_large_payment(&f);
    f.advance(DAY / 2);
    f.client.approve(&f.carol, &id);
    assert_eq!(
        f.client.get_proposal(&id).ready_at,
        Some(OCT_15 + PAY_DELAY)
    );
}

#[test]
fn dropping_below_required_approvals_restarts_the_payment_timelock() {
    let f = payment_timelocked();
    let id = approved_large_payment(&f);
    f.advance(DAY / 2);
    f.client.revoke_approval(&f.bob, &id);
    assert_eq!(f.client.get_proposal(&id).ready_at, None);

    f.client.approve(&f.bob, &id);
    let restarted = OCT_15 + DAY / 2 + PAY_DELAY;
    assert_eq!(f.client.get_proposal(&id).ready_at, Some(restarted));
    f.set_time(OCT_15 + PAY_DELAY);
    assert_contract_err!(f.client.try_execute(&f.dave, &id), Error::TimelockActive);
    f.set_time(restarted - 1);
    assert_contract_err!(f.client.try_execute(&f.dave, &id), Error::TimelockActive);
    f.set_time(restarted);
    f.client.execute(&f.dave, &id);
}

#[test]
fn revoking_above_required_approvals_keeps_the_payment_timelock() {
    let f = payment_timelocked();
    let id = approved_large_payment(&f);
    f.client.approve(&f.carol, &id);
    f.client.revoke_approval(&f.carol, &id);
    assert_eq!(
        f.client.get_proposal(&id).ready_at,
        Some(OCT_15 + PAY_DELAY)
    );
}

#[test]
fn losing_an_approver_restarts_the_payment_timelock() {
    let f = payment_timelocked();
    // A fourth approver keeps the 3-approval tier reachable without bob.
    f.govern(Action::AddMember(Address::generate(&f.env), ROLE_APPROVER));
    let id = approved_large_payment(&f);
    // Demoting bob doesn't change USDC's epoch, but his approval stops
    // counting, so the payment drops below its requirement.
    f.govern(Action::UpdateMember(f.bob.clone(), ROLE_PROPOSER));
    f.set_time(OCT_15 + PAY_DELAY);
    assert_contract_err!(
        f.client.try_execute(&f.dave, &id),
        Error::InsufficientApprovals
    );

    // Carol brings it back to 2: the old ready time must not be reused.
    f.client.approve(&f.carol, &id);
    let restarted = OCT_15 + 2 * PAY_DELAY;
    assert_eq!(f.client.get_proposal(&id).ready_at, Some(restarted));
    assert_contract_err!(f.client.try_execute(&f.dave, &id), Error::TimelockActive);
    f.set_time(restarted);
    f.client.execute(&f.dave, &id);
}

#[test]
fn token_epoch_change_resets_the_payment_timelock() {
    let f = payment_timelocked();
    let id = approved_large_payment(&f);
    f.advance(PAY_DELAY / 2);
    let budget = Budget {
        limit: 9_000,
        period: Period::CalendarMonth,
    };
    f.govern(Action::SetBudget(OPS, f.usdc.clone(), budget)); // bumps USDC's epoch

    f.set_time(OCT_15 + PAY_DELAY);
    assert_eq!(f.client.get_proposal_state(&id).ready_at, None);
    assert_contract_err!(
        f.client.try_execute(&f.dave, &id),
        Error::InsufficientApprovals
    );
    f.approve_by(id, &[&f.alice, &f.bob]);
    let restarted = OCT_15 + 2 * PAY_DELAY;
    assert_eq!(f.client.get_proposal(&id).ready_at, Some(restarted));
    assert_contract_err!(f.client.try_execute(&f.dave, &id), Error::TimelockActive);
    f.set_time(restarted);
    f.client.execute(&f.dave, &id);
}

#[test]
fn payment_approved_before_the_timelock_must_still_wait() {
    let f = Fixture::new();
    let id = approved_large_payment(&f);
    f.govern(Action::SetPaymentTimelock(f.usdc.clone(), usdc_timelock()));
    // Its approvals were given under the old rules and no longer count.
    assert_contract_err!(
        f.client.try_execute(&f.dave, &id),
        Error::InsufficientApprovals
    );
    f.approve_by(id, &[&f.alice, &f.bob]);
    assert_contract_err!(f.client.try_execute(&f.dave, &id), Error::TimelockActive);
    f.advance(PAY_DELAY);
    f.client.execute(&f.dave, &id);
}

#[test]
fn removing_the_payment_timelock_lifts_the_delay() {
    let f = payment_timelocked();
    f.govern(Action::RemovePaymentTimelock(f.usdc.clone()));
    assert_eq!(f.client.get_payment_timelock(&f.usdc), None);
    f.pay(DELAY_FROM, OPS);
    assert_eq!(f.balance(&f.vendor), DELAY_FROM);
    assert_eq!(
        f.try_govern(Action::RemovePaymentTimelock(f.usdc.clone())),
        Err(Error::NotConfigured)
    );
}

#[test]
fn payment_timelock_is_per_token() {
    let f = payment_timelocked();
    assert_eq!(
        f.client.get_payment_timelock(&f.usdc),
        Some(usdc_timelock())
    );
    let issuer = Address::generate(&f.env);
    let eurc = f.env.register_stellar_asset_contract_v2(issuer).address();
    soroban_sdk::token::StellarAssetClient::new(&f.env, &eurc).mint(&f.id, &TREASURY_FUNDS);
    f.govern(Action::SetAssetPolicy(eurc.clone(), usdc_policy(&f.env)));
    let budget = Budget {
        limit: 8_000,
        period: Period::CalendarMonth,
    };
    f.govern(Action::SetBudget(OPS, eurc.clone(), budget));
    assert_eq!(f.client.get_payment_timelock(&eurc), None);

    let id = f
        .client
        .propose_payment(&f.alice, &eurc, &f.vendor, &DELAY_FROM, &OPS, &TTL, &None);
    f.approve_by(id, &[&f.alice, &f.bob]);
    f.client.execute(&f.dave, &id);
}

#[test]
fn guardian_can_veto_a_delayed_payment() {
    let f = payment_timelocked();
    let id = approved_large_payment(&f);
    f.client.cancel(&f.guardian, &id);
    assert_eq!(f.client.get_proposal(&id).status, ProposalStatus::Cancelled);
    f.advance(PAY_DELAY);
    assert_contract_err!(
        f.client.try_execute(&f.dave, &id),
        Error::ProposalNotPending
    );
    assert_eq!(f.balance(&f.vendor), 0);
}

#[test]
fn payment_timelock_bounds_are_enforced() {
    let f = Fixture::new();
    let bad = |delay_from: i128, delay: u64| {
        let timelock = PaymentTimelock { delay_from, delay };
        let action = Action::SetPaymentTimelock(f.usdc.clone(), timelock);
        f.client.try_propose(&f.alice, &action, &TTL, &None)
    };
    assert_contract_err!(bad(0, DAY), Error::InvalidPolicy);
    assert_contract_err!(bad(-1, DAY), Error::InvalidPolicy);
    assert_contract_err!(bad(1, 0), Error::InvalidDelay);
    assert_contract_err!(bad(1, 30 * DAY + 1), Error::InvalidDelay);

    // The delay must be shorter than the max proposal TTL (30 days here)...
    let at_cap = PaymentTimelock {
        delay_from: 1,
        delay: MAX_TTL,
    };
    assert_eq!(
        f.try_govern(Action::SetPaymentTimelock(f.usdc.clone(), at_cap)),
        Err(Error::InvalidDelay)
    );
    let below_cap = PaymentTimelock {
        delay_from: 1,
        delay: MAX_TTL - 1,
    };
    f.govern(Action::SetPaymentTimelock(f.usdc.clone(), below_cap));
    // ...and the TTL cap can't be lowered to the delay.
    assert_eq!(
        f.try_govern(Action::SetMaxProposalTtl(MAX_TTL - 1)),
        Err(Error::InvalidDelay)
    );
}

#[test]
fn delayed_payment_ttl_must_outlive_the_delay() {
    let f = payment_timelocked();
    let propose = |amount: i128, ttl: u64| {
        f.client
            .try_propose_payment(&f.alice, &f.usdc, &f.vendor, &amount, &OPS, &ttl, &None)
    };
    assert_contract_err!(propose(DELAY_FROM, PAY_DELAY), Error::InvalidTtl);
    assert!(propose(DELAY_FROM, PAY_DELAY + 1).is_ok());
    assert!(propose(DELAY_FROM - 1, PAY_DELAY).is_ok());
}

#[test]
fn approval_that_would_start_a_too_late_payment_timelock_is_rejected() {
    let f = payment_timelocked();
    let id = f.client.propose_payment(
        &f.alice,
        &f.usdc,
        &f.vendor,
        &DELAY_FROM,
        &OPS,
        &(3 * DAY),
        &None,
    );
    let expires_at = f.client.get_proposal(&id).expires_at;
    f.client.approve(&f.alice, &id);

    f.set_time(expires_at - PAY_DELAY);
    let blocked = Some(Error::TimelockOutlivesProposal as u32);
    assert_eq!(f.client.get_proposal_state(&id).blocked_by, blocked);
    assert_contract_err!(
        f.client.try_approve(&f.bob, &id),
        Error::TimelockOutlivesProposal
    );

    f.set_time(expires_at - PAY_DELAY - 1);
    f.client.approve(&f.bob, &id);
    assert_eq!(f.client.get_proposal(&id).ready_at, Some(expires_at - 1));
    f.set_time(expires_at - 1);
    f.client.execute(&f.dave, &id);
}
