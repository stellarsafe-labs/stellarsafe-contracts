//! Test suite. `setup` builds a realistic treasury used by every module:
//!
//! | member    | roles                          |
//! |-----------|--------------------------------|
//! | alice     | proposer, approver, executor   |
//! | bob       | proposer, approver             |
//! | carol     | approver                       |
//! | dave      | executor                       |
//! | guardian  | guardian                       |
//!
//! USDC policy: cap 10_000; 1 approval below 500, 2 from 500, 3 from 5_000.
//! Department 1: 8_000 USDC per calendar month. Department 2: 300 USDC per
//! UTC day. Governance threshold 2. Allowlist enforced; `vendor` allowed.

mod approvals;
mod budgets;
mod governance;
mod independence;
mod init;
mod maintenance;
mod model;
mod payments;
mod policy_math;
mod security;
mod setup;
mod stress;
mod timelock;
mod views;
