extern crate std;

use soroban_sdk::{testutils::Address as _, vec, Address, BytesN};

use super::setup::*;
use crate::{
    assert_contract_err, Action, ApprovalTier, AssetPolicy, Error, ProposalStatus, ALL_ROLES,
    ROLE_APPROVER, ROLE_PROPOSER,
};

#[test]
fn governance_needs_threshold_approvals() {
    let f = Fixture::new();
    let id = f
        .client
        .propose(&f.alice, &Action::SetAllowlistEnabled(false), &TTL, &None);
    f.client.approve(&f.alice, &id);
    assert_contract_err!(
        f.client.try_execute(&f.alice, &id),
        Error::InsufficientApprovals
    );
    f.client.approve(&f.carol, &id);
    f.client.execute(&f.dave, &id);
    assert!(!f.client.get_config().allowlist_enabled);
}

#[test]
fn add_member_grants_roles() {
    let f = Fixture::new();
    let erin = Address::generate(&f.env);
    assert_contract_err!(
        f.client
            .try_propose_payment(&erin, &f.usdc, &f.vendor, &1, &OPS, &TTL, &None),
        Error::Unauthorized
    );
    f.govern(Action::AddMember(erin.clone(), ROLE_PROPOSER));
    assert_eq!(f.client.get_roles(&erin), ROLE_PROPOSER);
    f.client
        .propose_payment(&erin, &f.usdc, &f.vendor, &1, &OPS, &TTL, &None);
}

#[test]
fn add_existing_member_fails() {
    let f = Fixture::new();
    assert_eq!(
        f.try_govern(Action::AddMember(f.carol.clone(), ROLE_APPROVER)),
        Err(Error::MemberExists)
    );
}

#[test]
fn update_or_remove_unknown_member_fails() {
    let f = Fixture::new();
    let nobody = Address::generate(&f.env);
    assert_eq!(
        f.try_govern(Action::RemoveMember(nobody.clone())),
        Err(Error::MemberNotFound)
    );
    assert_eq!(
        f.try_govern(Action::UpdateMember(nobody, ROLE_APPROVER)),
        Err(Error::MemberNotFound)
    );
}

#[test]
fn invalid_roles_rejected_at_proposal_time() {
    let f = Fixture::new();
    let erin = Address::generate(&f.env);
    for roles in [0, ALL_ROLES + 1, 1 << 31] {
        assert_contract_err!(
            f.client.try_propose(
                &f.alice,
                &Action::AddMember(erin.clone(), roles),
                &TTL,
                &None
            ),
            Error::InvalidRoles
        );
    }
    assert_contract_err!(
        f.client.try_propose(
            &f.alice,
            &Action::AddMember(f.id.clone(), ROLE_APPROVER),
            &TTL,
            &None
        ),
        Error::InvalidMember
    );
}

#[test]
fn cannot_remove_approver_needed_by_highest_tier() {
    let f = Fixture::new();
    // USDC needs 3 approvals at the top tier and there are exactly 3 approvers.
    assert_eq!(
        f.try_govern(Action::RemoveMember(f.carol.clone())),
        Err(Error::InvalidThreshold)
    );
    assert_eq!(f.client.get_roles(&f.carol), ROLE_APPROVER);
}

#[test]
fn cannot_remove_last_executor() {
    let f = Fixture::new();
    f.govern(Action::RemoveMember(f.dave.clone()));
    assert_eq!(
        f.try_govern(Action::UpdateMember(
            f.alice.clone(),
            ROLE_PROPOSER | ROLE_APPROVER
        )),
        Err(Error::MissingRequiredRole)
    );
}

#[test]
fn failed_governance_execution_reverts_completely() {
    let f = Fixture::new();
    let id = f.client.propose(
        &f.alice,
        &Action::RemoveMember(f.carol.clone()),
        &TTL,
        &None,
    );
    f.approve_by(id, &[&f.alice, &f.bob]);
    assert_contract_err!(f.client.try_execute(&f.alice, &id), Error::InvalidThreshold);
    assert_eq!(f.client.get_proposal(&id).status, ProposalStatus::Pending);
    assert_eq!(f.client.get_config().version, 0);
}

#[test]
fn governance_threshold_bounds() {
    let f = Fixture::new();
    assert_contract_err!(
        f.client
            .try_propose(&f.alice, &Action::SetGovernanceThreshold(0), &TTL, &None),
        Error::InvalidThreshold
    );
    assert_eq!(
        f.try_govern(Action::SetGovernanceThreshold(4)),
        Err(Error::InvalidThreshold)
    );
    f.govern(Action::SetGovernanceThreshold(3));
    assert_eq!(f.client.get_config().governance_threshold, 3);

    // Now two approvals are no longer enough.
    let id = f
        .client
        .propose(&f.alice, &Action::SetAllowlistEnabled(false), &TTL, &None);
    f.approve_by(id, &[&f.alice, &f.bob]);
    assert_contract_err!(
        f.client.try_execute(&f.alice, &id),
        Error::InsufficientApprovals
    );
    f.client.approve(&f.carol, &id);
    f.client.execute(&f.alice, &id);
}

#[test]
fn max_proposal_ttl_can_be_changed() {
    let f = Fixture::new();
    assert_contract_err!(
        f.client
            .try_propose(&f.alice, &Action::SetMaxProposalTtl(10), &TTL, &None),
        Error::InvalidTtl
    );
    f.govern(Action::SetMaxProposalTtl(DAY));
    assert_contract_err!(
        f.client
            .try_propose_payment(&f.alice, &f.usdc, &f.vendor, &1, &OPS, &(DAY + 1), &None),
        Error::InvalidTtl
    );
}

#[test]
fn asset_policy_lifecycle() {
    let f = Fixture::new();
    let eurc = f
        .env
        .register_stellar_asset_contract_v2(Address::generate(&f.env))
        .address();
    let policy = AssetPolicy {
        max_amount: 50,
        tiers: vec![
            &f.env,
            ApprovalTier {
                min_amount: 0,
                approvals: 2,
            },
        ],
    };

    f.govern(Action::SetAssetPolicy(eurc.clone(), policy.clone()));
    assert_eq!(f.client.get_asset_policy(&eurc), Some(policy));
    assert_eq!(f.client.get_policy_assets().len(), 2);

    f.govern(Action::RemoveAssetPolicy(eurc.clone()));
    assert_eq!(f.client.get_asset_policy(&eurc), None);
    assert_eq!(f.client.get_policy_assets(), vec![&f.env, f.usdc.clone()]);
    assert_eq!(
        f.try_govern(Action::RemoveAssetPolicy(eurc)),
        Err(Error::NotConfigured)
    );
}

#[test]
fn removing_policy_blocks_pending_payments() {
    let f = Fixture::new();
    let id = f.propose_pay(10, OPS);
    f.govern(Action::RemoveAssetPolicy(f.usdc.clone()));
    f.client.approve(&f.alice, &id);
    assert_contract_err!(f.client.try_execute(&f.dave, &id), Error::AssetNotAllowed);
}

#[test]
fn invalid_policies_are_rejected() {
    let f = Fixture::new();
    let e = &f.env;
    let t = |min: i128, approvals: u32| ApprovalTier {
        min_amount: min,
        approvals,
    };
    let bad = [
        AssetPolicy {
            max_amount: 0,
            tiers: vec![e, t(0, 1)],
        }, // zero cap
        AssetPolicy {
            max_amount: 100,
            tiers: vec![e],
        }, // no tiers
        AssetPolicy {
            max_amount: 100,
            tiers: vec![e, t(1, 1)],
        }, // gap below first tier
        AssetPolicy {
            max_amount: 100,
            tiers: vec![e, t(0, 0)],
        }, // zero approvals
        AssetPolicy {
            max_amount: 100,
            tiers: vec![e, t(0, 2), t(50, 1)],
        }, // decreasing approvals
        AssetPolicy {
            max_amount: 100,
            tiers: vec![e, t(0, 1), t(0, 2)],
        }, // duplicate min
        AssetPolicy {
            max_amount: 100,
            tiers: vec![e, t(0, 1), t(101, 2)],
        }, // unreachable tier
    ];
    for policy in bad {
        assert_contract_err!(
            f.client.try_propose(
                &f.alice,
                &Action::SetAssetPolicy(f.usdc.clone(), policy),
                &TTL,
                &None
            ),
            Error::InvalidPolicy
        );
    }
}

#[test]
fn policy_requiring_more_approvals_than_approvers_fails_on_execute() {
    let f = Fixture::new();
    let policy = AssetPolicy {
        max_amount: 100,
        tiers: vec![
            &f.env,
            ApprovalTier {
                min_amount: 0,
                approvals: 4,
            },
        ],
    };
    assert_eq!(
        f.try_govern(Action::SetAssetPolicy(f.usdc.clone(), policy)),
        Err(Error::InvalidThreshold)
    );
}

#[test]
fn allowlist_management() {
    let f = Fixture::new();
    let new_vendor = Address::generate(&f.env);
    f.govern(Action::SetRecipientAllowed(new_vendor.clone(), true));
    assert!(f.client.is_recipient_allowed(&new_vendor));
    f.client
        .propose_payment(&f.alice, &f.usdc, &new_vendor, &1, &OPS, &TTL, &None);

    // Removing a recipient blocks payments that were already proposed.
    let id = f
        .client
        .propose_payment(&f.alice, &f.usdc, &new_vendor, &1, &OPS, &TTL, &None);
    f.govern(Action::SetRecipientAllowed(new_vendor.clone(), false));
    f.client.approve(&f.alice, &id);
    assert_contract_err!(
        f.client.try_execute(&f.dave, &id),
        Error::RecipientNotAllowed
    );

    // Disabling the allowlist lets anyone be paid.
    // Allowlist changes don't invalidate payment approvals.
    f.govern(Action::SetAllowlistEnabled(false));
    f.client.execute(&f.dave, &id);
    assert_eq!(f.balance(&new_vendor), 1);
}

#[test]
fn every_executed_governance_action_bumps_version() {
    let f = Fixture::new();
    f.govern(Action::SetAllowlistEnabled(false));
    f.govern(Action::SetAllowlistEnabled(true));
    f.govern(Action::Unpause);
    assert_eq!(f.client.get_config().version, 3);
}

#[test]
fn upgrade_to_unknown_wasm_reverts() {
    let f = Fixture::new();
    let hash = BytesN::from_array(&f.env, &[1; 32]);
    let id = f
        .client
        .propose(&f.alice, &Action::Upgrade(hash), &TTL, &None);
    f.approve_by(id, &[&f.alice, &f.bob]);
    assert!(f.client.try_execute(&f.alice, &id).is_err());
    assert_eq!(f.client.get_proposal(&id).status, ProposalStatus::Pending);
}
