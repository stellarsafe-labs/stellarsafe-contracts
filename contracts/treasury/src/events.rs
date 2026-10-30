//! Contract events. These are the authoritative audit trail that the
//! StellarSafe indexer consumes; field names and topics are part of the ABI.

use soroban_sdk::{contractevent, Address, BytesN};

use crate::types::Action;

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProposalCreated {
    #[topic]
    pub id: u64,
    #[topic]
    pub proposer: Address,
    pub action: Action,
    pub expires_at: u64,
    pub config_version: u32,
    pub metadata_hash: Option<BytesN<32>>,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProposalApproved {
    #[topic]
    pub id: u64,
    #[topic]
    pub approver: Address,
    /// Approvals that count after this one.
    pub approvals: u32,
}

/// A governance proposal reached its threshold; it can execute at `ready_at`.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProposalReady {
    #[topic]
    pub id: u64,
    pub ready_at: u64,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovalRevoked {
    #[topic]
    pub id: u64,
    #[topic]
    pub approver: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProposalCancelled {
    #[topic]
    pub id: u64,
    pub by: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProposalExecuted {
    #[topic]
    pub id: u64,
    pub executor: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaymentExecuted {
    #[topic]
    pub id: u64,
    #[topic]
    pub token: Address,
    pub to: Address,
    pub amount: i128,
    pub department: u32,
    pub window: u64,
    /// Total spent by the department in `window` including this payment.
    pub window_spent: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigUpdated {
    #[topic]
    pub id: u64,
    pub version: u32,
    pub action: Action,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Paused {
    pub by: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Deposited {
    #[topic]
    pub token: Address,
    #[topic]
    pub from: Address,
    pub amount: i128,
}
