//! Model-based property test.
//!
//! Random sequences of operations run against both the contract and a small
//! reference model written straight from the spec. After each operation:
//! * the contract's result (value or exact error code) must equal the model's;
//! * token balances, budget counters and proposal statuses must match.
//!
//! Override the number of cases with `PROPTEST_CASES=<n>`.
extern crate std;

use std::collections::BTreeMap;
use std::vec;
use std::vec::Vec;

use proptest::prelude::*;
use soroban_sdk::{testutils::Address as _, Address};

use super::setup::*;
use crate::{policy::window_id, Action, Error, Period, ProposalStatus};

// Fixture members in index order.
const ALICE: usize = 0; // proposer | approver | executor
const BOB: usize = 1; // proposer | approver
const CAROL: usize = 2; // approver
const DAVE: usize = 3; // executor
const GUARDIAN: usize = 4;
const NUM_ACTORS: usize = 6; // index 5 = outsider

#[derive(Clone, Debug)]
enum Op {
    Propose {
        who: usize,
        amount: i128,
        dept: u32,
        to_vendor: bool,
        ttl: u64,
    },
    Approve {
        who: usize,
        id: u64,
    },
    Revoke {
        who: usize,
        id: u64,
    },
    Execute {
        who: usize,
        id: u64,
    },
    Cancel {
        who: usize,
        id: u64,
    },
    Advance {
        secs: u64,
    },
    /// Fully approved payment of exactly the department's remaining budget,
    /// or the treasury's remaining balance if lower, plus `extra` (0 or 1).
    FlowExact {
        dept: u32,
        extra: i128,
    },
    /// Jump to the latest pending proposal's `expires_at` (or one second before).
    ToExpiry {
        before: bool,
    },
    /// Jump to the next budget-window boundary of `dept` (or one second before).
    ToWindowEdge {
        dept: u32,
        before: bool,
    },
    Pause {
        who: usize,
    },
    Unpause,
    /// Re-set the USDC policy unchanged: bumps the USDC approval epoch.
    TouchPolicy,
    /// Propose a payment, approve it with `approvers`, then execute it.
    /// Expanded into the individual operations above, each checked.
    Flow {
        amount: i128,
        dept: u32,
        approvers: Vec<usize>,
        executor: usize,
    },
}

fn amount() -> impl Strategy<Value = i128> {
    prop_oneof![
        1 => prop_oneof![Just(0i128), Just(-1i128), Just(10_001i128)],
        3 => prop_oneof![
            Just(1i128), Just(299i128), Just(300i128), Just(301i128), Just(499i128), Just(500i128),
            Just(501i128), Just(4_999i128), Just(5_000i128), Just(8_000i128), Just(10_000i128),
        ],
        6 => 1i128..=600,
        2 => 1i128..=8_000,
    ]
}

/// Mostly actors that hold the role an operation needs, sometimes anyone.
fn actor(valid: &'static [usize]) -> impl Strategy<Value = usize> {
    prop_oneof![
        9 => proptest::sample::select(valid),
        1 => 0..NUM_ACTORS,
    ]
}

fn op() -> impl Strategy<Value = Op> {
    // Resolved against the live proposal count in `run`, so most picks hit
    // a real proposal (recent ones more often) and a few miss.
    let id = || 0u64..1_000;
    let ttl = prop_oneof![
        6 => Just(TTL),
        2 => 1u64..=(3 * DAY),
        1 => Just(0u64),
        1 => Just(MAX_TTL + 1),
    ];
    prop_oneof![
        5 => (actor(&[ALICE, BOB]), amount(), prop_oneof![6 => Just(OPS), 3 => Just(MARKETING), 1 => Just(3u32)],
              proptest::bool::weighted(0.95), ttl)
            .prop_map(|(who, amount, dept, to_vendor, ttl)| Op::Propose { who, amount, dept, to_vendor, ttl }),
        9 => (actor(&[ALICE, BOB, CAROL]), id()).prop_map(|(who, id)| Op::Approve { who, id }),
        1 => (actor(&[ALICE, BOB, CAROL]), id()).prop_map(|(who, id)| Op::Revoke { who, id }),
        5 => (actor(&[ALICE, DAVE]), id()).prop_map(|(who, id)| Op::Execute { who, id }),
        1 => (actor(&[ALICE, BOB, GUARDIAN]), id()).prop_map(|(who, id)| Op::Cancel { who, id }),
        2 => prop_oneof![0u64..=3_600, 0u64..=(2 * DAY), Just(20 * DAY)].prop_map(|secs| Op::Advance { secs }),
        1 => actor(&[GUARDIAN]).prop_map(|who| Op::Pause { who }),
        2 => Just(Op::Unpause),
        1 => any::<bool>().prop_map(|before| Op::ToExpiry { before }),
        2 => (prop_oneof![Just(OPS), Just(MARKETING)], 0i128..=1)
            .prop_map(|(dept, extra)| Op::FlowExact { dept, extra }),
        1 => (prop_oneof![Just(OPS), Just(MARKETING)], any::<bool>())
            .prop_map(|(dept, before)| Op::ToWindowEdge { dept, before }),
        1 => Just(Op::TouchPolicy),
        6 => (amount(), prop_oneof![3 => Just(OPS), 1 => Just(MARKETING)],
              proptest::sample::subsequence(std::vec![ALICE, BOB, CAROL, DAVE], 0..=4),
              actor(&[ALICE, DAVE]))
            .prop_map(|(amount, dept, approvers, executor)| Op::Flow { amount, dept, approvers, executor }),
    ]
}

/// Outcome of a call: `Ok(value)`, a contract error, or a host-level failure
/// (e.g. the token transfer failing for lack of funds).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Out {
    Ok(u64),
    Err(Error),
    Host,
}

fn out<T, C, H>(r: Result<Result<T, C>, Result<Error, H>>, val: impl Fn(T) -> u64) -> Out {
    match r {
        Ok(Ok(v)) => Out::Ok(val(v)),
        Ok(Err(_)) => panic!("conversion error"),
        Err(Ok(e)) => Out::Err(e),
        Err(Err(_)) => Out::Host,
    }
}

#[derive(Clone, Debug)]
struct MProposal {
    proposer: usize,
    payment: Option<(i128, u32, bool)>, // amount, dept, to_vendor
    expires_at: u64,
    approvals: BTreeMap<usize, u32>, // approver -> epoch
    status: ProposalStatus,
}

struct Model {
    now: u64,
    paused: bool,
    usdc_epoch: u32,
    proposals: Vec<MProposal>,
    treasury: i128,
    paid_vendor: i128,
    /// dept -> (window, spent)
    usage: BTreeMap<u32, (u64, i128)>,
}

const fn roles(who: usize) -> (bool, bool, bool, bool) {
    // (proposer, approver, executor, guardian)
    match who {
        ALICE => (true, true, true, false),
        BOB => (true, true, false, false),
        CAROL => (false, true, false, false),
        DAVE => (false, false, true, false),
        GUARDIAN => (false, false, false, true),
        _ => (false, false, false, false),
    }
}

fn budget(dept: u32) -> Option<(i128, Period)> {
    match dept {
        OPS => Some((8_000, Period::CalendarMonth)),
        MARKETING => Some((300, Period::Fixed(DAY))),
        _ => None,
    }
}

fn required(amount: i128) -> u32 {
    if amount >= 5_000 {
        3
    } else if amount >= 500 {
        2
    } else {
        1
    }
}

impl Model {
    fn spent(&self, dept: u32) -> i128 {
        let (_, period) = budget(dept).unwrap();
        let w = window_id(&period, self.now);
        match self.usage.get(&dept) {
            Some((uw, s)) if *uw == w => *s,
            _ => 0,
        }
    }

    fn validate_payment(&self, amount: i128, dept: u32, to_vendor: bool) -> Result<(), Error> {
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        if amount > 10_000 {
            return Err(Error::AmountExceedsCap);
        }
        if !to_vendor {
            return Err(Error::RecipientNotAllowed);
        }
        let (limit, _) = budget(dept).ok_or(Error::BudgetNotFound)?;
        if amount > limit {
            return Err(Error::BudgetExceeded);
        }
        Ok(())
    }

    fn open(&self, id: u64) -> Result<usize, Error> {
        let p = self
            .proposals
            .get(id as usize)
            .ok_or(Error::ProposalNotFound)?;
        if p.status != ProposalStatus::Pending {
            return Err(Error::ProposalNotPending);
        }
        if self.now >= p.expires_at {
            return Err(Error::ProposalExpired);
        }
        Ok(id as usize)
    }

    fn valid_approvals(&self, p: &MProposal) -> u32 {
        p.approvals
            .iter()
            .filter(|(who, e)| **e == self.usdc_epoch && roles(**who).1)
            .count() as u32
    }

    fn propose(&mut self, who: usize, amount: i128, dept: u32, to_vendor: bool, ttl: u64) -> Out {
        if !roles(who).0 {
            return Out::Err(Error::Unauthorized);
        }
        if ttl == 0 || ttl > MAX_TTL {
            return Out::Err(Error::InvalidTtl);
        }
        if let Err(e) = self.validate_payment(amount, dept, to_vendor) {
            return Out::Err(e);
        }
        self.proposals.push(MProposal {
            proposer: who,
            payment: Some((amount, dept, to_vendor)),
            expires_at: self.now + ttl,
            approvals: BTreeMap::new(),
            status: ProposalStatus::Pending,
        });
        Out::Ok(self.proposals.len() as u64 - 1)
    }

    fn approve(&mut self, who: usize, id: u64) -> Out {
        if !roles(who).1 {
            return Out::Err(Error::Unauthorized);
        }
        let i = match self.open(id) {
            Ok(i) => i,
            Err(e) => return Out::Err(e),
        };
        let epoch = self.usdc_epoch;
        if self.proposals[i].approvals.get(&who) == Some(&epoch) {
            return Out::Err(Error::AlreadyApproved);
        }
        self.proposals[i].approvals.insert(who, epoch);
        Out::Ok(self.valid_approvals(&self.proposals[i]) as u64)
    }

    fn revoke(&mut self, who: usize, id: u64) -> Out {
        let i = match self.open(id) {
            Ok(i) => i,
            Err(e) => return Out::Err(e),
        };
        match self.proposals[i].approvals.remove(&who) {
            Some(_) => Out::Ok(0),
            None => Out::Err(Error::NotApproved),
        }
    }

    fn cancel(&mut self, who: usize, id: u64) -> Out {
        let Some(p) = self.proposals.get_mut(id as usize) else {
            return Out::Err(Error::ProposalNotFound);
        };
        if p.status != ProposalStatus::Pending {
            return Out::Err(Error::ProposalNotPending);
        }
        // Only payments are pending in this model, so guardians may cancel.
        if who != p.proposer && !roles(who).3 {
            return Out::Err(Error::Unauthorized);
        }
        p.status = ProposalStatus::Cancelled;
        Out::Ok(0)
    }

    fn execute(&mut self, who: usize, id: u64) -> Out {
        if !roles(who).2 {
            return Out::Err(Error::Unauthorized);
        }
        let i = match self.open(id) {
            Ok(i) => i,
            Err(e) => return Out::Err(e),
        };
        if self.paused {
            return Out::Err(Error::Paused);
        }
        let (amount, dept, to_vendor) = self.proposals[i].payment.unwrap();
        if let Err(e) = self.validate_payment(amount, dept, to_vendor) {
            return Out::Err(e);
        }
        let (limit, period) = budget(dept).unwrap();
        let new_spent = self.spent(dept) + amount;
        if new_spent > limit {
            return Out::Err(Error::BudgetExceeded);
        }
        if self.valid_approvals(&self.proposals[i]) < required(amount) {
            return Out::Err(Error::InsufficientApprovals);
        }
        if self.treasury < amount {
            return Out::Host; // token transfer fails; whole call reverts
        }
        self.proposals[i].status = ProposalStatus::Executed;
        self.usage
            .insert(dept, (window_id(&period, self.now), new_spent));
        self.treasury -= amount;
        self.paid_vendor += amount;
        Out::Ok(0)
    }

    /// A governance proposal created and executed in one go by the fixture.
    fn record_governance(&mut self) {
        self.proposals.push(MProposal {
            proposer: ALICE,
            payment: None,
            expires_at: self.now + TTL,
            approvals: BTreeMap::new(),
            status: ProposalStatus::Executed,
        });
    }
}

/// Maps a raw selector to a proposal id: 1 in 10 picks an id that doesn't
/// exist yet, 6 in 10 one of the three most recent payment proposals, and
/// the rest any existing proposal.
fn resolve_ids(op: Op, model: &Model) -> Op {
    let count = model.proposals.len() as u64;
    let payments: Vec<u64> = (0..count)
        .filter(|i| model.proposals[*i as usize].payment.is_some())
        .collect();
    let pick = |sel: u64| {
        let bucket = sel % 10;
        let rest = sel / 10;
        if count == 0 || bucket == 0 {
            count + rest % 3
        } else if bucket < 7 && !payments.is_empty() {
            let recent = payments.len().min(3) as u64;
            payments[payments.len() - 1 - (rest % recent) as usize]
        } else {
            rest % count
        }
    };
    match op {
        Op::Approve { who, id } => Op::Approve { who, id: pick(id) },
        Op::Revoke { who, id } => Op::Revoke { who, id: pick(id) },
        Op::Execute { who, id } => Op::Execute { who, id: pick(id) },
        Op::Cancel { who, id } => Op::Cancel { who, id: pick(id) },
        other => other,
    }
}

/// First timestamp after `now` that falls in a different window.
fn next_window_start(period: &Period, now: u64) -> u64 {
    let w = window_id(period, now);
    let (mut lo, mut hi) = (now, now + 1);
    while window_id(period, hi) == w {
        lo = hi;
        hi = now + (hi - now) * 2;
    }
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if window_id(period, mid) == w {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    hi
}

/// Turns the targeted time jumps into plain `Advance`s.
fn resolve_time(op: Op, m: &Model) -> Op {
    let target = match op {
        Op::ToExpiry { before } => m
            .proposals
            .iter()
            .rev()
            .find(|p| p.status == ProposalStatus::Pending && p.expires_at > m.now)
            .map(|p| p.expires_at - before as u64),
        Op::ToWindowEdge { dept, before } => {
            let (_, period) = budget(dept).unwrap();
            Some(next_window_start(&period, m.now) - before as u64)
        }
        other => return other,
    };
    Op::Advance {
        secs: target.map_or(0, |t| t.saturating_sub(m.now)),
    }
}

fn run(ops: Vec<Op>) {
    let f = Fixture::new();
    let outsider = Address::generate(&f.env);
    let stranger = Address::generate(&f.env);
    let actors = [&f.alice, &f.bob, &f.carol, &f.dave, &f.guardian, &outsider];
    // Leave the treasury with less than a month's OPS budget so running out
    // of funds is reachable within a run.
    const START_FUNDS: i128 = 2_500;
    f.token()
        .transfer(&f.id, &outsider, &(TREASURY_FUNDS - START_FUNDS));
    let mut m = Model {
        now: OCT_15,
        paused: false,
        usdc_epoch: 0,
        proposals: Vec::new(),
        treasury: START_FUNDS,
        paid_vendor: 0,
        usage: BTreeMap::new(),
    };

    let mut expanded = Vec::new();
    for op in ops {
        match op {
            Op::Flow {
                amount,
                dept,
                approvers,
                executor,
            } => {
                expanded.push((
                    Op::Propose {
                        who: ALICE,
                        amount,
                        dept,
                        to_vendor: true,
                        ttl: TTL,
                    },
                    false,
                ));
                // The new proposal's id is only known at run time: `u64::MAX`
                // is replaced with the latest id below.
                for who in approvers {
                    expanded.push((Op::Approve { who, id: u64::MAX }, false));
                }
                expanded.push((
                    Op::Execute {
                        who: executor,
                        id: u64::MAX,
                    },
                    false,
                ));
            }
            Op::FlowExact { dept, extra } => {
                expanded.push((Op::FlowExact { dept, extra }, false));
                for who in [ALICE, BOB, CAROL] {
                    expanded.push((Op::Approve { who, id: u64::MAX }, false));
                }
                expanded.push((
                    Op::Execute {
                        who: ALICE,
                        id: u64::MAX,
                    },
                    false,
                ));
            }
            other => expanded.push((other, true)),
        }
    }

    for (step, (op, random_id)) in expanded.into_iter().enumerate() {
        let op = resolve_time(op, &m);
        let op = match op {
            Op::FlowExact { dept, extra } => {
                let (limit, _) = budget(dept).unwrap();
                let room = (limit - m.spent(dept)).min(m.treasury);
                Op::Propose {
                    who: ALICE,
                    amount: room + extra,
                    dept,
                    to_vendor: true,
                    ttl: TTL,
                }
            }
            other => other,
        };
        let latest = (m.proposals.len() as u64).wrapping_sub(1);
        let op = if random_id {
            resolve_ids(op, &m)
        } else {
            match op {
                Op::Approve { who, .. } => Op::Approve { who, id: latest },
                Op::Execute { who, .. } => Op::Execute { who, id: latest },
                other => other,
            }
        };
        let (got, want) = match op.clone() {
            Op::Propose {
                who,
                amount,
                dept,
                to_vendor,
                ttl,
            } => {
                let to = if to_vendor { &f.vendor } else { &stranger };
                let got = out(
                    f.client.try_propose_payment(
                        actors[who],
                        &f.usdc,
                        to,
                        &amount,
                        &dept,
                        &ttl,
                        &None,
                    ),
                    |id| id,
                );
                (got, m.propose(who, amount, dept, to_vendor, ttl))
            }
            Op::Approve { who, id } => (
                out(f.client.try_approve(actors[who], &id), |n| n as u64),
                m.approve(who, id),
            ),
            Op::Revoke { who, id } => (
                out(f.client.try_revoke_approval(actors[who], &id), |_| 0),
                m.revoke(who, id),
            ),
            Op::Execute { who, id } => (
                out(f.client.try_execute(actors[who], &id), |_| 0),
                m.execute(who, id),
            ),
            Op::Cancel { who, id } => (
                out(f.client.try_cancel(actors[who], &id), |_| 0),
                m.cancel(who, id),
            ),
            Op::Advance { secs } => {
                f.advance(secs);
                m.now += secs;
                (Out::Ok(0), Out::Ok(0))
            }
            Op::Pause { who } => {
                let want = if roles(who).3 {
                    m.paused = true;
                    Out::Ok(0)
                } else {
                    Out::Err(Error::Unauthorized)
                };
                (out(f.client.try_pause(actors[who]), |_| 0), want)
            }
            Op::Unpause => {
                f.govern(Action::Unpause);
                m.paused = false;
                m.record_governance();
                (Out::Ok(0), Out::Ok(0))
            }
            Op::TouchPolicy => {
                f.govern(Action::SetAssetPolicy(f.usdc.clone(), usdc_policy(&f.env)));
                m.usdc_epoch += 1;
                m.record_governance();
                (Out::Ok(0), Out::Ok(0))
            }
            Op::Flow { .. }
            | Op::FlowExact { .. }
            | Op::ToExpiry { .. }
            | Op::ToWindowEdge { .. } => {
                unreachable!("resolved above")
            }
        };
        assert_eq!(got, want, "step {step}: {op:?}");

        // Global invariants.
        assert_eq!(
            f.balance(&f.id),
            m.treasury,
            "treasury balance, step {step}"
        );
        assert_eq!(
            f.balance(&f.vendor),
            m.paid_vendor,
            "vendor balance, step {step}"
        );
        assert_eq!(f.balance(&stranger), 0, "allowlist bypass, step {step}");
        assert_eq!(f.client.get_config().paused, m.paused);
        for dept in [OPS, MARKETING] {
            let status = f.client.get_budget(&dept, &f.usdc).unwrap();
            assert_eq!(
                status.spent,
                m.spent(dept),
                "dept {dept} spent, step {step}"
            );
            assert!(status.spent <= status.limit);
        }
        assert_eq!(f.client.proposal_count(), m.proposals.len() as u64);
        for (id, p) in m.proposals.iter().enumerate() {
            let stored = f.client.get_proposal(&(id as u64)).status;
            assert_eq!(stored, p.status, "proposal {id} status, step {step}");
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(32),
        max_shrink_iters: 2_000,
        ..ProptestConfig::default()
    })]

    #[test]
    fn contract_matches_reference_model(ops in proptest::collection::vec(op(), 1..60)) {
        run(ops);
    }
}
