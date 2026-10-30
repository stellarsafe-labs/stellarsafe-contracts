extern crate std;

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token::{StellarAssetClient, TokenClient},
    vec, Address, Env, Map,
};

use crate::{
    Action, ApprovalTier, AssetPolicy, Budget, BudgetInit, InitConfig, Period, Treasury,
    TreasuryClient, ROLE_APPROVER, ROLE_EXECUTOR, ROLE_GUARDIAN, ROLE_PROPOSER,
};

/// 2026-10-15T12:00:00Z
pub const OCT_15: u64 = 1_792_065_600;
/// 2026-10-31T23:59:59Z
pub const OCT_31_LAST_SECOND: u64 = 1_793_491_199;
/// 2026-11-01T00:00:00Z
pub const NOV_1: u64 = 1_793_491_200;
/// 2026-12-31T23:59:59Z
pub const DEC_31_LAST_SECOND: u64 = 1_798_761_599;
/// 2027-01-01T00:00:00Z
pub const JAN_1_2027: u64 = 1_798_761_600;

pub const DAY: u64 = 86_400;
pub const TTL: u64 = 7 * DAY;
pub const MAX_TTL: u64 = 30 * DAY;

pub const OPS: u32 = 1;
pub const MARKETING: u32 = 2;

pub const TREASURY_FUNDS: i128 = 20_000;

#[macro_export]
macro_rules! assert_contract_err {
    ($call:expr, $err:expr) => {
        assert_eq!($call, Err(Ok($err)))
    };
}

pub struct Fixture {
    pub env: Env,
    pub id: Address,
    pub client: TreasuryClient<'static>,
    pub usdc: Address,
    pub usdc_admin: StellarAssetClient<'static>,
    pub alice: Address,
    pub bob: Address,
    pub carol: Address,
    pub dave: Address,
    pub guardian: Address,
    pub vendor: Address,
}

pub fn usdc_policy(env: &Env) -> AssetPolicy {
    AssetPolicy {
        max_amount: 10_000,
        tiers: vec![
            env,
            ApprovalTier {
                min_amount: 0,
                approvals: 1,
            },
            ApprovalTier {
                min_amount: 500,
                approvals: 2,
            },
            ApprovalTier {
                min_amount: 5_000,
                approvals: 3,
            },
        ],
    }
}

pub fn base_init(
    f_env: &Env,
    members: &Map<Address, u32>,
    usdc: &Address,
    vendor: &Address,
) -> InitConfig {
    let mut policies = Map::new(f_env);
    policies.set(usdc.clone(), usdc_policy(f_env));
    InitConfig {
        members: members.clone(),
        governance_threshold: 2,
        governance_delay: 0,
        max_proposal_ttl: MAX_TTL,
        allowlist_enabled: true,
        allowlist: vec![f_env, vendor.clone()],
        asset_policies: policies,
        budgets: vec![
            f_env,
            BudgetInit {
                department: OPS,
                token: usdc.clone(),
                budget: Budget {
                    limit: 8_000,
                    period: Period::CalendarMonth,
                },
            },
            BudgetInit {
                department: MARKETING,
                token: usdc.clone(),
                budget: Budget {
                    limit: 300,
                    period: Period::Fixed(DAY),
                },
            },
        ],
    }
}

impl Fixture {
    pub fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(OCT_15);

        let alice = Address::generate(&env);
        let bob = Address::generate(&env);
        let carol = Address::generate(&env);
        let dave = Address::generate(&env);
        let guardian = Address::generate(&env);
        let vendor = Address::generate(&env);

        let issuer = Address::generate(&env);
        let sac = env.register_stellar_asset_contract_v2(issuer);
        let usdc = sac.address();
        let usdc_admin = StellarAssetClient::new(&env, &usdc);

        let mut members = Map::new(&env);
        members.set(alice.clone(), ROLE_PROPOSER | ROLE_APPROVER | ROLE_EXECUTOR);
        members.set(bob.clone(), ROLE_PROPOSER | ROLE_APPROVER);
        members.set(carol.clone(), ROLE_APPROVER);
        members.set(dave.clone(), ROLE_EXECUTOR);
        members.set(guardian.clone(), ROLE_GUARDIAN);

        let init = base_init(&env, &members, &usdc, &vendor);
        let id = env.register(Treasury, (init,));
        let client = TreasuryClient::new(&env, &id);
        usdc_admin.mint(&id, &TREASURY_FUNDS);

        Fixture {
            env,
            id,
            client,
            usdc,
            usdc_admin,
            alice,
            bob,
            carol,
            dave,
            guardian,
            vendor,
        }
    }

    pub fn token(&self) -> TokenClient<'static> {
        TokenClient::new(&self.env, &self.usdc)
    }

    pub fn balance(&self, who: &Address) -> i128 {
        self.token().balance(who)
    }

    pub fn set_time(&self, ts: u64) {
        self.env.ledger().set_timestamp(ts);
    }

    pub fn advance(&self, secs: u64) {
        let now = self.env.ledger().timestamp();
        self.env.ledger().set_timestamp(now + secs);
    }

    /// Proposes a payment from alice to the vendor.
    pub fn propose_pay(&self, amount: i128, department: u32) -> u64 {
        self.client.propose_payment(
            &self.alice,
            &self.usdc,
            &self.vendor,
            &amount,
            &department,
            &TTL,
            &None,
        )
    }

    /// Approves `id` with each approver in order.
    pub fn approve_by(&self, id: u64, approvers: &[&Address]) {
        for a in approvers {
            self.client.approve(a, &id);
        }
    }

    /// Proposes, approves with alice/bob/carol as needed, and executes.
    pub fn pay(&self, amount: i128, department: u32) -> u64 {
        let id = self.propose_pay(amount, department);
        let state = self.client.get_proposal_state(&id);
        let approvers = [&self.alice, &self.bob, &self.carol];
        self.approve_by(id, &approvers[..state.required as usize]);
        self.client.execute(&self.dave, &id);
        id
    }

    /// Runs a governance action through propose -> 2 approvals -> execute.
    pub fn govern(&self, action: Action) -> u64 {
        let id = self.client.propose(&self.alice, &action, &TTL, &None);
        self.approve_by(id, &[&self.alice, &self.bob]);
        self.client.execute(&self.alice, &id);
        id
    }

    /// Like `govern` but returns the execute result.
    pub fn try_govern(&self, action: Action) -> Result<(), crate::Error> {
        let id = self.client.propose(&self.alice, &action, &TTL, &None);
        self.approve_by(id, &[&self.alice, &self.bob]);
        match self.client.try_execute(&self.alice, &id) {
            Ok(_) => Ok(()),
            Err(Ok(e)) => Err(e),
            Err(Err(e)) => panic!("host error: {e:?}"),
        }
    }
}
