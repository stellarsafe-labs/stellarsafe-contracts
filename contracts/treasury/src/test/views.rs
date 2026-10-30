extern crate std;

use super::setup::*;
use crate::{assert_contract_err, Error, ProposalStatus};

#[test]
fn proposal_state_explains_what_blocks_execution() {
    let f = Fixture::new();
    let id = f.propose_pay(600, OPS);

    let s = f.client.get_proposal_state(&id);
    assert_eq!(
        (s.status, s.approvals, s.required),
        (ProposalStatus::Pending, 0, 2)
    );
    assert_eq!(s.blocked_by, Some(Error::InsufficientApprovals as u32));

    f.approve_by(id, &[&f.alice, &f.bob]);
    assert_eq!(f.client.get_proposal_state(&id).blocked_by, None);

    f.client.pause(&f.guardian);
    assert_eq!(
        f.client.get_proposal_state(&id).blocked_by,
        Some(Error::Paused as u32)
    );

    f.advance(TTL);
    let s = f.client.get_proposal_state(&id);
    assert_eq!(s.status, ProposalStatus::Expired);
    assert_eq!(s.blocked_by, Some(Error::ProposalExpired as u32));
}

#[test]
fn proposal_state_for_closed_and_budget_blocked_proposals() {
    let f = Fixture::new();
    let done = f.pay(8_000, OPS);
    let s = f.client.get_proposal_state(&done);
    assert_eq!(s.status, ProposalStatus::Executed);
    assert_eq!(s.blocked_by, Some(Error::ProposalNotPending as u32));

    let over = f.propose_pay(1, OPS);
    assert_eq!(
        f.client.get_proposal_state(&over).blocked_by,
        Some(Error::BudgetExceeded as u32)
    );
}

#[test]
fn governance_proposal_state_uses_governance_threshold() {
    let f = Fixture::new();
    let id = f
        .client
        .propose(&f.alice, &crate::Action::Unpause, &TTL, &None);
    assert_eq!(f.client.get_proposal_state(&id).required, 2);
}

#[test]
fn proposals_are_paginated() {
    let f = Fixture::new();
    for _ in 0..3 {
        f.propose_pay(1, OPS);
    }
    assert_eq!(f.client.proposal_count(), 3);
    let page = f.client.get_proposals(&0, &2);
    assert_eq!(page.len(), 2);
    assert_eq!(page.get(1).unwrap().id, 1);
    assert_eq!(f.client.get_proposals(&2, &50).len(), 1);
    assert_eq!(f.client.get_proposals(&10, &5).len(), 0);
    assert_eq!(f.client.get_proposals(&u64::MAX, &5).len(), 0);
    assert_contract_err!(f.client.try_get_proposals(&0, &0), Error::InvalidPagination);
    assert_contract_err!(
        f.client.try_get_proposals(&0, &51),
        Error::InvalidPagination
    );
}
