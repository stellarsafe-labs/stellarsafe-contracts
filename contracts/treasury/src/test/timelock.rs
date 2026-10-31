extern crate std;

use soroban_sdk::{
    testutils::{Address as _, Events as _},
    Address, Event,
};

use super::setup::*;
use crate::{
    assert_contract_err, events, Action, Error, ProposalStatus, Treasury, ROLE_APPROVER,
    ROLE_GUARDIAN,
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
