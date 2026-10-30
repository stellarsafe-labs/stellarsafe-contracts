extern crate std;

use soroban_sdk::{vec, Env};

use crate::policy::{
    civil_from_days, max_tier_approvals, required_approvals, validate_budget, window_id,
};
use crate::{ApprovalTier, AssetPolicy, Budget, Period};

#[test]
fn civil_dates_match_known_values() {
    assert_eq!(civil_from_days(0), (1970, 1, 1));
    assert_eq!(civil_from_days(59), (1970, 3, 1));
    assert_eq!(civil_from_days(10_957), (2000, 1, 1));
    assert_eq!(civil_from_days(11_016), (2000, 2, 29)); // leap year divisible by 400
    assert_eq!(civil_from_days(1_792_065_600 / 86_400), (2026, 10, 15));
    assert_eq!(civil_from_days(47_541), (2100, 3, 1)); // 2100 is not a leap year
    assert_eq!(civil_from_days(47_540), (2100, 2, 28));
}

#[test]
fn civil_dates_are_continuous_for_three_centuries() {
    let days_in = |y: u64, m: u64| match m {
        2 if (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    let mut prev = civil_from_days(0);
    for d in 1..(300 * 366) {
        let cur = civil_from_days(d);
        let (y, m, day) = prev;
        let expected = if day < days_in(y, m) {
            (y, m, day + 1)
        } else if m < 12 {
            (y, m + 1, 1)
        } else {
            (y + 1, 1, 1)
        };
        assert_eq!(cur, expected, "day {d}");
        prev = cur;
    }
}

#[test]
fn month_windows_are_sequential() {
    let oct = window_id(&Period::CalendarMonth, 1_793_491_199);
    let nov = window_id(&Period::CalendarMonth, 1_793_491_200);
    assert_eq!(oct, 2026 * 12 + 9);
    assert_eq!(nov, oct + 1);
    let dec = window_id(&Period::CalendarMonth, 1_798_761_599);
    let jan = window_id(&Period::CalendarMonth, 1_798_761_600);
    assert_eq!(jan, dec + 1);
}

#[test]
fn fixed_windows_align_to_epoch() {
    assert_eq!(window_id(&Period::Fixed(86_400), 0), 0);
    assert_eq!(window_id(&Period::Fixed(86_400), 86_399), 0);
    assert_eq!(window_id(&Period::Fixed(86_400), 86_400), 1);
}

#[test]
fn tier_lookup_picks_highest_matching_tier() {
    let env = Env::default();
    let policy = AssetPolicy {
        max_amount: 1_000,
        tiers: vec![
            &env,
            ApprovalTier {
                min_amount: 0,
                approvals: 1,
            },
            ApprovalTier {
                min_amount: 100,
                approvals: 2,
            },
            ApprovalTier {
                min_amount: 900,
                approvals: 4,
            },
        ],
    };
    assert_eq!(required_approvals(&policy, 1), 1);
    assert_eq!(required_approvals(&policy, 99), 1);
    assert_eq!(required_approvals(&policy, 100), 2);
    assert_eq!(required_approvals(&policy, 899), 2);
    assert_eq!(required_approvals(&policy, 900), 4);
    assert_eq!(max_tier_approvals(&policy), 4);
}

#[test]
fn budget_validation() {
    let ok = |limit, period| validate_budget(&Budget { limit, period }).is_ok();
    assert!(ok(1, Period::CalendarMonth));
    assert!(ok(1, Period::Fixed(3_600)));
    assert!(ok(1, Period::Fixed(366 * 86_400)));
    assert!(!ok(0, Period::CalendarMonth));
    assert!(!ok(-5, Period::CalendarMonth));
    assert!(!ok(1, Period::Fixed(0)));
    assert!(!ok(1, Period::Fixed(3_599)));
    assert!(!ok(1, Period::Fixed(366 * 86_400 + 1)));
}
