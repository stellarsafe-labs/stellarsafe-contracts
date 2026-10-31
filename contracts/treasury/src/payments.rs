//! Payment checks against the asset policy, allowlist, budget and pause.

use soroban_sdk::Env;

use crate::errors::Error;
use crate::policy::{required_approvals, window_id};
use crate::storage as st;
use crate::types::{AssetPolicy, Budget, Config, Payment};

/// Result of checking a payment against current policy.
pub struct PaymentPlan {
    pub required: u32,
    pub window: u64,
    pub window_spent: i128,
}

/// Static checks that don't depend on how much has been spent. Used at
/// proposal time and again at execution.
pub fn validate_payment(
    env: &Env,
    config: &Config,
    payment: &Payment,
) -> Result<(AssetPolicy, Budget), Error> {
    if payment.amount <= 0 {
        return Err(Error::InvalidAmount);
    }
    if payment.to == env.current_contract_address() {
        return Err(Error::InvalidRecipient);
    }
    let policy = st::get_asset_policy(env, &payment.token).ok_or(Error::AssetNotAllowed)?;
    if payment.amount > policy.max_amount {
        return Err(Error::AmountExceedsCap);
    }
    if config.allowlist_enabled && !st::is_allowed(env, &payment.to) {
        return Err(Error::RecipientNotAllowed);
    }
    let budget =
        st::get_budget(env, payment.department, &payment.token).ok_or(Error::BudgetNotFound)?;
    if payment.amount > budget.limit {
        return Err(Error::BudgetExceeded);
    }
    Ok((policy, budget))
}

/// Full execution-time check of a payment, including the spend counter for
/// the current window.
pub fn payment_plan(env: &Env, config: &Config, payment: &Payment) -> Result<PaymentPlan, Error> {
    if config.paused {
        return Err(Error::Paused);
    }
    let (policy, budget) = validate_payment(env, config, payment)?;
    let window = window_id(&budget.period, env.ledger().timestamp());
    let spent = match st::get_budget_usage(env, payment.department, &payment.token) {
        Some(usage) if usage.window == window => usage.spent,
        _ => 0,
    };
    let window_spent = spent.checked_add(payment.amount).ok_or(Error::Overflow)?;
    if window_spent > budget.limit {
        return Err(Error::BudgetExceeded);
    }
    Ok(PaymentPlan {
        required: required_approvals(&policy, payment.amount),
        window,
        window_spent,
    })
}
