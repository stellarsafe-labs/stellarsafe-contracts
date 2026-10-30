//! Pure policy logic: validation, approval tiers and accounting periods.
//! Nothing here touches storage, which keeps it easy to test exhaustively.

use crate::errors::Error;
use crate::types::{AssetPolicy, Budget, Period, MAX_FIXED_PERIOD, MAX_TIERS, MIN_FIXED_PERIOD};

pub fn validate_asset_policy(policy: &AssetPolicy) -> Result<(), Error> {
    if policy.max_amount <= 0 {
        return Err(Error::InvalidPolicy);
    }
    let n = policy.tiers.len();
    if n == 0 || n > MAX_TIERS {
        return Err(Error::InvalidPolicy);
    }
    let mut prev_min: i128 = -1;
    let mut prev_approvals: u32 = 0;
    for (i, tier) in policy.tiers.iter().enumerate() {
        if i == 0 && tier.min_amount != 0 {
            // Every positive amount must map to a tier.
            return Err(Error::InvalidPolicy);
        }
        if tier.min_amount <= prev_min
            || tier.approvals == 0
            || tier.approvals < prev_approvals
            || tier.min_amount > policy.max_amount
        {
            return Err(Error::InvalidPolicy);
        }
        prev_min = tier.min_amount;
        prev_approvals = tier.approvals;
    }
    Ok(())
}

/// Approvals required for `amount`: the highest tier whose `min_amount` is
/// at most `amount`. Assumes a validated policy.
pub fn required_approvals(policy: &AssetPolicy, amount: i128) -> u32 {
    let mut required = 0;
    for tier in policy.tiers.iter() {
        if amount >= tier.min_amount {
            required = tier.approvals;
        } else {
            break;
        }
    }
    required
}

/// Highest approval count any payment under this policy can require.
pub fn max_tier_approvals(policy: &AssetPolicy) -> u32 {
    policy.tiers.iter().map(|t| t.approvals).max().unwrap_or(0)
}

pub fn validate_budget(budget: &Budget) -> Result<(), Error> {
    if budget.limit <= 0 {
        return Err(Error::InvalidBudget);
    }
    if let Period::Fixed(len) = budget.period {
        if !(MIN_FIXED_PERIOD..=MAX_FIXED_PERIOD).contains(&len) {
            return Err(Error::InvalidBudget);
        }
    }
    Ok(())
}

/// Identifier of the accounting window containing `timestamp`.
///
/// * `CalendarMonth`: `year * 12 + (month - 1)` in UTC.
/// * `Fixed(len)`: `timestamp / len`.
pub fn window_id(period: &Period, timestamp: u64) -> u64 {
    match period {
        Period::CalendarMonth => {
            let (year, month, _) = civil_from_days(timestamp / 86_400);
            year * 12 + (month - 1)
        }
        Period::Fixed(len) => timestamp / len,
    }
}

/// Converts days since 1970-01-01 to a (year, month, day) civil date.
/// Howard Hinnant's algorithm, restricted to non-negative inputs.
pub fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}
