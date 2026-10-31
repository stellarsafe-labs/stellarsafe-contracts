//! Pruning closed proposals and migrating storage after upgrades.
extern crate std;

use soroban_sdk::{testutils::Events as _, vec, Vec};

use super::setup::*;
use crate::storage::DataKey;
use crate::{
    assert_contract_err, events, Action, Error, ProposalStatus, MAX_PAGE, PROPOSAL_RETENTION,
    STORAGE_VERSION,
};

fn emitted<E: soroban_sdk::Event>(f: &Fixture, event: &E) -> bool {
    f.env
        .events()
        .all()
        .filter_by_contract(&f.id)
        .events()
        .contains(&event.to_xdr(&f.env, &f.id))
}

fn set_storage_version(f: &Fixture, version: Option<u32>) {
    f.env.as_contract(&f.id, || {
        let store = f.env.storage().instance();
        match version {
            Some(v) => store.set(&DataKey::StorageVersion, &v),
            None => store.remove(&DataKey::StorageVersion),
        }
    });
}

// ----- pruning -----

#[test]
fn executed_proposal_can_be_pruned_after_retention() {
    let f = Fixture::new();
    let id = f.pay(100, OPS);
    let closed_at = f.client.get_proposal(&id).closed_at;

    f.set_time(closed_at + PROPOSAL_RETENTION - 1);
    assert_contract_err!(
        f.client.try_prune_proposals(&vec![&f.env, id]),
        Error::ProposalNotPrunable
    );

    f.set_time(closed_at + PROPOSAL_RETENTION);
    assert_eq!(f.client.prune_proposals(&vec![&f.env, id]), 1);
    assert!(emitted(&f, &events::ProposalPruned { id }));
    // Anyone may prune: no signature was needed.
    assert!(f.env.auths().is_empty());

    assert_contract_err!(f.client.try_get_proposal(&id), Error::ProposalNotFound);
    assert_eq!(f.client.proposal_count(), 1);
    assert_eq!(f.client.get_proposals(&0, &10).len(), 0);
    // Spending stays counted (in the window it was spent in).
    assert_eq!(f.balance(&f.vendor), 100);
}

#[test]
fn cancelled_and_expired_proposals_can_be_pruned() {
    let f = Fixture::new();
    let cancelled = f.propose_pay(100, OPS);
    f.client.cancel(&f.alice, &cancelled);
    let expired = f.propose_pay(100, OPS);
    let expires_at = f.client.get_proposal(&expired).expires_at;

    // Expired proposals are counted from `expires_at`, not `closed_at`.
    f.set_time(expires_at + PROPOSAL_RETENTION - 1);
    assert_eq!(f.client.prune_proposals(&vec![&f.env, cancelled]), 1);
    assert_contract_err!(
        f.client.try_prune_proposals(&vec![&f.env, expired]),
        Error::ProposalNotPrunable
    );
    f.set_time(expires_at + PROPOSAL_RETENTION);
    assert_eq!(f.client.prune_proposals(&vec![&f.env, expired]), 1);
    assert_eq!(f.client.get_proposals(&0, &10).len(), 0);
}

#[test]
fn open_proposals_cannot_be_pruned() {
    let f = Fixture::new();
    let id = f.propose_pay(100, OPS);
    assert_contract_err!(
        f.client.try_prune_proposals(&vec![&f.env, id]),
        Error::ProposalNotPrunable
    );
    assert_eq!(f.client.get_proposal(&id).status, ProposalStatus::Pending);
}

#[test]
fn pruning_is_all_or_nothing() {
    let f = Fixture::new();
    let old = f.pay(100, OPS);
    f.advance(PROPOSAL_RETENTION);
    let fresh = f.pay(100, OPS);

    assert_contract_err!(
        f.client.try_prune_proposals(&vec![&f.env, old, fresh]),
        Error::ProposalNotPrunable
    );
    assert_contract_err!(
        f.client.try_prune_proposals(&vec![&f.env, old, 99]),
        Error::ProposalNotFound
    );
    assert_contract_err!(
        f.client.try_prune_proposals(&vec![&f.env, old, old]),
        Error::ProposalNotFound
    );
    // Nothing was deleted by the failed calls.
    assert_eq!(f.client.get_proposals(&0, &10).len(), 2);
}

#[test]
fn prune_batch_size_is_bounded() {
    let f = Fixture::new();
    assert_contract_err!(
        f.client.try_prune_proposals(&Vec::new(&f.env)),
        Error::InvalidPagination
    );
    let mut too_many = Vec::new(&f.env);
    for id in 0..=MAX_PAGE as u64 {
        too_many.push_back(id);
    }
    assert_contract_err!(
        f.client.try_prune_proposals(&too_many),
        Error::InvalidPagination
    );
}

#[test]
fn pruned_proposals_reject_every_call() {
    let f = Fixture::new();
    let id = f.propose_pay(100, OPS);
    f.client.cancel(&f.alice, &id);
    f.advance(PROPOSAL_RETENTION);
    f.client.prune_proposals(&vec![&f.env, id]);

    let e = Error::ProposalNotFound;
    assert_contract_err!(f.client.try_approve(&f.bob, &id), e);
    assert_contract_err!(f.client.try_execute(&f.dave, &id), e);
    assert_contract_err!(f.client.try_cancel(&f.alice, &id), e);
    assert_contract_err!(f.client.try_revoke_approval(&f.bob, &id), e);
    assert_contract_err!(f.client.try_get_proposal_state(&id), e);
    // New proposals never reuse the id.
    assert_eq!(f.propose_pay(100, OPS), id + 1);
}

// ----- storage migration -----

#[test]
fn new_treasury_uses_current_storage_version() {
    let f = Fixture::new();
    // Written by the constructor, not just the default for a missing entry.
    let stored: Option<u32> = f.env.as_contract(&f.id, || {
        f.env.storage().instance().get(&DataKey::StorageVersion)
    });
    assert_eq!(stored, Some(STORAGE_VERSION));
    assert_eq!(f.client.storage_version(), STORAGE_VERSION);
    assert_eq!(f.client.migrate(), STORAGE_VERSION);
    let migrated = events::StorageMigrated {
        from: STORAGE_VERSION,
        to: STORAGE_VERSION,
    };
    assert!(!emitted(&f, &migrated));
}

#[test]
fn treasury_without_recorded_version_uses_first_layout() {
    // Treasuries deployed by 0.1.0 have no version entry.
    let f = Fixture::new();
    set_storage_version(&f, None);
    assert_eq!(f.client.storage_version(), crate::FIRST_STORAGE_VERSION);
    f.pay(100, OPS);
}

#[test]
fn writes_wait_for_migration_after_upgrade() {
    let f = Fixture::new();
    let pending = f.propose_pay(100, OPS);
    f.client.approve(&f.alice, &pending);
    let old = STORAGE_VERSION - 1;
    set_storage_version(&f, Some(old));

    let e = Error::MigrationRequired;
    let unpause = Action::Unpause;
    assert_contract_err!(f.client.try_propose(&f.alice, &unpause, &TTL, &None), e);
    assert_contract_err!(f.client.try_approve(&f.bob, &pending), e);
    assert_contract_err!(f.client.try_revoke_approval(&f.alice, &pending), e);
    assert_contract_err!(f.client.try_execute(&f.dave, &pending), e);
    assert_contract_err!(f.client.try_cancel(&f.alice, &pending), e);
    assert_contract_err!(f.client.try_pause(&f.guardian), e);
    assert_contract_err!(f.client.try_deposit(&f.alice, &f.usdc, &1), e);
    let ids = vec![&f.env, pending];
    assert_contract_err!(f.client.try_prune_proposals(&ids), e);
    // Views keep working.
    assert_eq!(f.client.storage_version(), old);
    assert_eq!(f.client.get_proposal(&pending).approvals.len(), 1);

    assert_eq!(f.client.migrate(), STORAGE_VERSION);
    assert!(emitted(
        &f,
        &events::StorageMigrated {
            from: old,
            to: STORAGE_VERSION
        }
    ));
    assert!(f.env.auths().is_empty());
    assert_eq!(f.client.storage_version(), STORAGE_VERSION);
    f.client.execute(&f.dave, &pending);
    assert_eq!(f.balance(&f.vendor), 100);
}

#[test]
fn newer_storage_cannot_be_migrated_but_does_not_brick_the_treasury() {
    let f = Fixture::new();
    set_storage_version(&f, Some(STORAGE_VERSION + 1));
    assert_contract_err!(f.client.try_migrate(), Error::UnsupportedStorageVersion);
    // Governance still works, so the treasury can upgrade forward again.
    f.govern(Action::SetAllowlistEnabled(false));
    assert!(!f.client.get_config().allowlist_enabled);
}
