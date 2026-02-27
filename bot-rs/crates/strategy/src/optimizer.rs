/// Optimal trade sizing via ternary search.
///
/// The profit function f(amount_in) for AMM/CLMM arbitrage is **concave**:
/// it rises (bigger trade = more profit) then falls (price impact exceeds spread).
/// Ternary search finds the maximum of a concave function in O(log n) iterations.
///
/// For u64 precision (0..u64::MAX), ~64 iterations suffice.

/// Find the input amount that maximizes profit using ternary search.
///
/// # Arguments
/// * `lo` — Minimum amount to try (usually 1_000 MIST = 0.001 SUI)
/// * `hi` — Maximum amount to try (limited by pool liquidity)
/// * `precision` — Stop when hi - lo < precision (e.g., 1_000_000 = 0.001 SUI)
/// * `simulate` — Function that returns expected profit (or 0 if unprofitable)
///
/// # Returns
/// `(optimal_amount, max_profit)` — the amount that produces maximum profit.
pub fn ternary_search<F>(lo: u64, hi: u64, precision: u64, simulate: F) -> (u64, u64)
where
    F: Fn(u64) -> u64,
{
    let mut lo = lo;
    let mut hi = hi;
    let mut best_amount = lo;
    let mut best_profit = 0u64;

    // Guard: if range is trivially small, just evaluate endpoints
    if hi <= lo {
        let p = simulate(lo);
        return (lo, p);
    }

    let max_iterations = 100; // safety bound
    let mut iteration = 0;

    while hi - lo > precision && iteration < max_iterations {
        iteration += 1;

        let third = (hi - lo) / 3;
        let m1 = lo + third;
        let m2 = hi - third;

        let p1 = simulate(m1);
        let p2 = simulate(m2);

        // Track best seen
        if p1 > best_profit {
            best_profit = p1;
            best_amount = m1;
        }
        if p2 > best_profit {
            best_profit = p2;
            best_amount = m2;
        }

        if p1 < p2 {
            lo = m1;
        } else {
            hi = m2;
        }
    }

    // Final check at midpoint
    let mid = lo + (hi - lo) / 2;
    let p_mid = simulate(mid);
    if p_mid > best_profit {
        best_profit = p_mid;
        best_amount = mid;
    }

    (best_amount, best_profit)
}

/// Simulate profit for a constant-product AMM arbitrage (x * y = k).
///
/// Given two pools with the same pair but different prices:
/// - Pool 1 (buy): reserve_a1, reserve_b1
/// - Pool 2 (sell): reserve_a2, reserve_b2
///
/// Buy A with B on pool 1, sell A for B on pool 2.
/// Profit = amount_b_out - amount_b_in.
pub fn simulate_xy_arb(
    reserve_a1: u64,
    reserve_b1: u64,
    reserve_a2: u64,
    reserve_b2: u64,
    fee_bps_1: u64,
    fee_bps_2: u64,
    amount_b_in: u64,
) -> u64 {
    // Buy A on pool 1 (pay B, receive A)
    let fee_1 = amount_b_in * fee_bps_1 / 10_000;
    let b_after_fee = amount_b_in.saturating_sub(fee_1);

    if b_after_fee == 0 || reserve_a1 == 0 || reserve_b1 == 0 {
        return 0;
    }

    // x * y = k: amount_a_out = reserve_a * amount_b / (reserve_b + amount_b)
    let a_out = (reserve_a1 as u128 * b_after_fee as u128)
        / (reserve_b1 as u128 + b_after_fee as u128);

    if a_out == 0 || a_out >= reserve_a1 as u128 {
        return 0;
    }

    let a_out = a_out as u64;

    // Sell A on pool 2 (pay A, receive B)
    let fee_2 = a_out * fee_bps_2 / 10_000;
    let a_after_fee = a_out.saturating_sub(fee_2);

    if a_after_fee == 0 || reserve_a2 == 0 || reserve_b2 == 0 {
        return 0;
    }

    let b_out = (reserve_b2 as u128 * a_after_fee as u128)
        / (reserve_a2 as u128 + a_after_fee as u128);

    if b_out == 0 {
        return 0;
    }

    let b_out = b_out as u64;
    b_out.saturating_sub(amount_b_in)
}

/// Simulate profit for a CLMM arbitrage using sqrt_price approximation.
///
/// For concentrated liquidity pools, the price impact depends on:
/// - Current sqrt_price
/// - Active liquidity at current tick
/// - Swap direction
///
/// This is a simplified model — the real simulation would traverse ticks.
pub fn simulate_clmm_arb(
    sqrt_price_1: u128,
    liquidity_1: u128,
    _sqrt_price_2: u128,
    liquidity_2: u128,
    fee_bps_1: u64,
    fee_bps_2: u64,
    amount_in: u64,
) -> u64 {
    if liquidity_1 == 0 || liquidity_2 == 0 {
        return 0;
    }

    // Simplified: assume swap stays within single tick range
    // For a2b swap: delta_sqrt_price = amount_in / liquidity
    // amount_out = liquidity * delta_sqrt_price (in the other dimension)

    let fee_1 = amount_in as u128 * fee_bps_1 as u128 / 10_000;
    let after_fee_1 = (amount_in as u128).saturating_sub(fee_1);

    if after_fee_1 == 0 {
        return 0;
    }

    // Pool 1: buy (swap in one direction)
    // Simplified constant-liquidity model
    let delta_sqrt_1 = (after_fee_1 << 64) / liquidity_1;
    let new_sqrt_1 = sqrt_price_1.saturating_sub(delta_sqrt_1);

    if new_sqrt_1 == 0 {
        return 0;
    }

    // Amount out from pool 1 (other dimension)
    let amount_mid = liquidity_1
        .checked_mul(sqrt_price_1.saturating_sub(new_sqrt_1))
        .map(|v| v >> 64)
        .unwrap_or(0);

    if amount_mid == 0 {
        return 0;
    }

    // Pool 2: sell (swap in reverse direction)
    let fee_2 = amount_mid * fee_bps_2 as u128 / 10_000;
    let after_fee_2 = amount_mid.saturating_sub(fee_2);

    let delta_sqrt_2 = (after_fee_2 << 64) / liquidity_2;
    let amount_out = liquidity_2
        .checked_mul(delta_sqrt_2)
        .map(|v| v >> 64)
        .unwrap_or(0);

    if amount_out <= amount_in as u128 {
        return 0;
    }

    (amount_out - amount_in as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ternary_search_simple_concave() {
        // f(x) = -(x-50)^2 + 2500 — max at x=50, f(50) = 2500
        let simulate = |x: u64| {
            let diff = if x > 50 { x - 50 } else { 50 - x };
            2500u64.saturating_sub(diff * diff)
        };

        let (optimal, profit) = ternary_search(0, 100, 1, simulate);
        assert!((optimal as i64 - 50).abs() <= 2, "optimal should be ~50, got {optimal}");
        assert!(profit >= 2498, "profit should be ~2500, got {profit}");
    }

    #[test]
    fn test_ternary_search_zero_range() {
        let (amount, profit) = ternary_search(42, 42, 1, |x| x);
        assert_eq!(amount, 42);
        assert_eq!(profit, 42);
    }

    #[test]
    fn test_simulate_xy_arb_profitable() {
        // Pool 1: cheaper A (price = 2 B/A)
        // Pool 2: more expensive A (price = 2.2 B/A)
        // Use large reserves so 10k trade doesn't have excessive price impact
        let profit = simulate_xy_arb(
            10_000_000, 20_000_000,   // pool 1: reserve_a, reserve_b
            10_000_000, 22_000_000,   // pool 2: reserve_a, reserve_b
            30, 30,                    // 0.3% fee each
            100_000,                   // spend 100k B (~0.5% of pool)
        );
        assert!(profit > 0, "Should be profitable, got {profit}");
    }

    #[test]
    fn test_simulate_xy_arb_unprofitable() {
        // Same prices = no arb
        let profit = simulate_xy_arb(
            1_000_000, 2_000_000,
            1_000_000, 2_000_000,
            30, 30,
            100_000,
        );
        assert_eq!(profit, 0, "Same prices should not be profitable");
    }
}
