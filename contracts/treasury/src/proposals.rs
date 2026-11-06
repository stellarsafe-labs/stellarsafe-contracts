//! Proposal lifecycle helpers: role checks, open/closed state, approval
//! epochs and counting, the governance timelock and pruning rules.

use soroban_sdk::{Address, Env, Map};

use crate::errors::Error;
use crate::policy::required_approvals;
use crate::storage as st;
use crate::types::{
    Action, Config, Payment, Proposal, ProposalStatus, PROPOSAL_RETENTION, ROLE_APPROVER,
    ROLE_GUARDIAN,
};

pub fn require_role(members: &Map<Address, u32>, who: &Address, role: u32) -> Result<(), Error> {
    who.require_auth();
    match members.get(who.clone()) {
        Some(roles) if roles & role != 0 => Ok(()),
        _ => Err(Error::Unauthorized),
    }
}

pub fn load_proposal(env: &Env, id: u64) -> Result<Proposal, Error> {
    st::get_proposal(env, id).ok_or(Error::ProposalNotFound)
}

pub fn ensure_open(env: &Env, proposal: &Proposal) -> Result<(), Error> {
    if proposal.status != ProposalStatus::Pending {
        return Err(Error::ProposalNotPending);
    }
    if env.ledger().timestamp() >= proposal.expires_at {
        return Err(Error::ProposalExpired);
    }
    Ok(())
}

/// Epoch that approvals on `proposal` must carry to count.
pub fn approval_epoch(env: &Env, proposal: &Proposal, config: &Config) -> u32 {
    match &proposal.action {
        Action::Payment(payment) => st::get_asset_epoch(env, &payment.token),
        _ => config.version,
    }
}

/// Payment timelock that applies to `payment` under its token's current
/// rules, or `None` if the payment is below `delay_from` or the token has no
/// timelock.
pub fn payment_delay(env: &Env, payment: &Payment) -> Option<u64> {
    st::get_payment_timelock(env, &payment.token)
        .filter(|timelock| payment.amount >= timelock.delay_from)
        .map(|timelock| timelock.delay)
}

/// Timelock that starts when `action` reaches its required approvals:
/// `governance_delay` for governance, the token's payment timelock for
/// large payments, and `None` for payments that are not delayed.
pub fn action_delay(env: &Env, action: &Action, config: &Config) -> Option<u64> {
    match action {
        Action::Payment(payment) => payment_delay(env, payment),
        _ => Some(config.governance_delay),
    }
}

/// Approvals `action` needs under the current rules. `None` for a payment
/// whose token has no asset policy (it cannot execute at all).
pub fn required_now(env: &Env, action: &Action, config: &Config) -> Option<u32> {
    match action {
        Action::Payment(payment) => st::get_asset_policy(env, &payment.token)
            .map(|policy| required_approvals(&policy, payment.amount)),
        _ => Some(config.governance_threshold),
    }
}

/// Timelocked proposals may only run once the timelock that started when
/// they reached their required approvals, in the current epoch, has ended.
pub fn check_timelock(env: &Env, proposal: &Proposal, epoch: u32) -> Result<(), Error> {
    match proposal.ready_at {
        Some(ready_at) if proposal.ready_epoch == epoch => {
            if env.ledger().timestamp() < ready_at {
                Err(Error::TimelockActive)
            } else {
                Ok(())
            }
        }
        _ => Err(Error::InsufficientApprovals),
    }
}

/// True if `action` removes or changes the roles of a current guardian.
/// Guardians cannot veto these, so a rogue guardian can always be removed.
pub fn targets_guardian(members: &Map<Address, u32>, action: &Action) -> bool {
    let target = match action {
        Action::RemoveMember(m) | Action::UpdateMember(m, _) => m,
        _ => return false,
    };
    members
        .get(target.clone())
        .is_some_and(|roles| roles & ROLE_GUARDIAN != 0)
}

/// Approvals that count: given under the current approval epoch by an
/// address that is still an approver.
pub fn count_valid_approvals(proposal: &Proposal, members: &Map<Address, u32>, epoch: u32) -> u32 {
    let mut count = 0;
    for (approver, approved_at) in proposal.approvals.iter() {
        let is_approver = members
            .get(approver)
            .is_some_and(|roles| roles & ROLE_APPROVER != 0);
        if approved_at == epoch && is_approver {
            count += 1;
        }
    }
    count
}

/// True if `who` holds an approval on `proposal` that currently counts.
pub fn counts_approval(
    proposal: &Proposal,
    members: &Map<Address, u32>,
    who: &Address,
    epoch: u32,
) -> bool {
    proposal.approvals.get(who.clone()) == Some(epoch)
        && members
            .get(who.clone())
            .is_some_and(|roles| roles & ROLE_APPROVER != 0)
}

/// A proposal can be pruned once it has been closed for `PROPOSAL_RETENTION`:
/// executed or cancelled (counted from `closed_at`), or expired while still
/// pending (counted from `expires_at`). Closed proposals can never change
/// again, so deleting them loses nothing that events don't already record.
pub fn ensure_prunable(env: &Env, proposal: &Proposal) -> Result<(), Error> {
    let closed_at = match proposal.status {
        ProposalStatus::Pending => proposal.expires_at,
        _ => proposal.closed_at,
    };
    if env.ledger().timestamp() < closed_at.saturating_add(PROPOSAL_RETENTION) {
        return Err(Error::ProposalNotPrunable);
    }
    Ok(())
}
