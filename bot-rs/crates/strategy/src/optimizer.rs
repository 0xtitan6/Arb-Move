use arb_types::pool::{Dex, PoolState};

/// Optimal trade sizing via ternary search.
///
/// The profit function f(amount_in) for AMM/CLMM arbitrage is **concave**:
/// it rises (bigger trade = more profit) then falls (price impact exceeds spread).
/// Ternary search finds the maximum of a concave function in O(log n) iterations.
///
/// For u64 precision (0..u64::MAX), ~64 iterations suffice.
///
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
/// - Current sqrt_price (Q64.64 fixed-point)
/// - Active liquidity at current tick
/// - Swap direction
///
/// This is a simplified single-tick model. For a2b swaps:
///   amount_in  (token A) moves sqrt_price DOWN  → delta_sqrt = amount_in / L
///   amount_out (token B) = L * delta_sqrt_price
/// For b2a swaps: sqrt_price moves UP, reversed dimensions.
///
/// Pool 1 = flash/buy leg (a2b: we send A, receive B)
/// Pool 2 = sell leg (b2a: we send B back, receive A)
pub fn simulate_clmm_arb(
    sqrt_price_1: u128,
    liquidity_1: u128,
    sqrt_price_2: u128,
    liquidity_2: u128,
    fee_bps_1: u64,
    fee_bps_2: u64,
    amount_in: u64,
) -> u64 {
    if liquidity_1 == 0 || liquidity_2 == 0 || sqrt_price_1 == 0 || sqrt_price_2 == 0 {
        return 0;
    }

    // === Pool 1: a2b swap (send token A, receive token B) ===
    // Fee on input
    let fee_1 = amount_in as u128 * fee_bps_1 as u128 / 10_000;
    let after_fee_1 = (amount_in as u128).saturating_sub(fee_1);

    if after_fee_1 == 0 {
        return 0;
    }

    // a2b: token A goes in, sqrt_price decreases
    // delta_sqrt = amount_a_in / L  (in Q64.64 space)
    let delta_sqrt_1 = (after_fee_1 << 64) / liquidity_1;
    let new_sqrt_1 = sqrt_price_1.saturating_sub(delta_sqrt_1);

    if new_sqrt_1 == 0 {
        return 0; // exhausted all liquidity at this tick
    }

    // amount_b_out = L * (sqrt_price_old - sqrt_price_new)  (shift back from Q64.64)
    let amount_b_mid = liquidity_1
        .checked_mul(sqrt_price_1 - new_sqrt_1)
        .map(|v| v >> 64)
        .unwrap_or(0);

    if amount_b_mid == 0 {
        return 0;
    }

    // === Pool 2: b2a swap (send token B, receive token A) ===
    // Fee on input
    let fee_2 = amount_b_mid * fee_bps_2 as u128 / 10_000;
    let after_fee_2 = amount_b_mid.saturating_sub(fee_2);

    if after_fee_2 == 0 {
        return 0;
    }

    // b2a: token B goes in, sqrt_price increases.
    // Exact CLMM single-tick formula:
    //   new_sqrt = L * old_sqrt / (L - delta_b * old_sqrt >> 64)
    //   amount_a_out = L * (new_sqrt - old_sqrt) >> 64
    //
    // Compute: b_times_sqrt = after_fee_2 * sqrt_price_2 / 2^64
    // using split shifts to avoid overflow.
    let b_times_sqrt = after_fee_2
        .checked_mul(sqrt_price_2 >> 32)
        .map(|v| v >> 32)
        .unwrap_or(u128::MAX);

    if b_times_sqrt >= liquidity_2 {
        return 0; // exceeds single-tick capacity
    }

    let denom = liquidity_2 - b_times_sqrt;

    // new_sqrt = L * old_sqrt / denom (using split multiply to manage overflow)
    let new_sqrt_2 = liquidity_2
        .checked_mul(sqrt_price_2 >> 32)
        .map(|v| v / denom)
        .map(|v| v << 32)
        .unwrap_or(0);

    if new_sqrt_2 <= sqrt_price_2 {
        return 0; // price must increase for b2a
    }

    // amount_a_out = L * (new_sqrt - old_sqrt) >> 64
    let delta_sqrt_2 = new_sqrt_2 - sqrt_price_2;
    let amount_a_out = liquidity_2
        .checked_mul(delta_sqrt_2)
        .map(|v| v >> 64)
        .unwrap_or(0);

    if amount_a_out <= amount_in as u128 {
        return 0;
    }

    (amount_a_out - amount_in as u128) as u64
}

/// Hard cap on trade size (100 SUI).
const MAX_TRADE_MIST: u64 = 100_000_000_000;

/// Compute the upper bound for ternary search based on pool type.
fn max_trade_amount(pool: &PoolState) -> u64 {
    let raw = match pool.dex {
        // AMM: don't consume more than 30% of the smaller reserve
        Dex::Aftermath | Dex::FlowxAmm => {
            match (pool.reserve_a, pool.reserve_b) {
                (Some(a), Some(b)) => a.min(b) / 3,
                (Some(a), None) => a / 3,
                (None, Some(b)) => b / 3,
                _ => 10_000_000_000, // 10 SUI fallback
            }
        }
        // CLMM: conservative cap from liquidity at current tick
        Dex::Cetus | Dex::Turbos | Dex::FlowxClmm => {
            pool.liquidity
                .map(|l| u64::try_from(l >> 32).unwrap_or(MAX_TRADE_MIST))
                .unwrap_or(10_000_000_000)
        }
        // DeepBook CLOB: use vault reserves or fallback
        Dex::DeepBook => {
            pool.reserve_a.unwrap_or(10_000_000_000) / 3
        }
    };
    raw.clamp(1_000, MAX_TRADE_MIST) // [1000 MIST, 100 SUI]
}

/// Build a local simulation closure for ternary search optimization.
///
/// Returns `(simulate_fn, hi_bound)` where:
/// - `simulate_fn` takes `amount_in: u64` and returns `profit: u64`
/// - `hi_bound` is the maximum amount to search
///
/// The closure captures pool state and uses the appropriate model
/// (constant-product for AMMs, sqrt_price for CLMMs).
pub fn build_local_simulator(
    flash_pool: &PoolState,
    sell_pool: &PoolState,
) -> (Box<dyn Fn(u64) -> u64>, u64) {
    let hi = max_trade_amount(flash_pool).min(max_trade_amount(sell_pool));
    let fee1 = flash_pool.fee_rate_bps.unwrap_or(30);
    let fee2 = sell_pool.fee_rate_bps.unwrap_or(30);

    let is_amm = |dex: Dex| matches!(dex, Dex::Aftermath | Dex::FlowxAmm);
    let is_clmm = |dex: Dex| matches!(dex, Dex::Cetus | Dex::Turbos | Dex::FlowxClmm);

    // Both AMM pools — use constant-product model
    if is_amm(flash_pool.dex) && is_amm(sell_pool.dex) {
        let ra1 = flash_pool.reserve_a.unwrap_or(0);
        let rb1 = flash_pool.reserve_b.unwrap_or(0);
        let ra2 = sell_pool.reserve_a.unwrap_or(0);
        let rb2 = sell_pool.reserve_b.unwrap_or(0);
        return (
            Box::new(move |amount| simulate_xy_arb(ra1, rb1, ra2, rb2, fee1, fee2, amount)),
            hi,
        );
    }

    // Both CLMM pools — use sqrt_price model
    if is_clmm(flash_pool.dex) && is_clmm(sell_pool.dex) {
        let sp1 = flash_pool.sqrt_price.unwrap_or(0);
        let l1 = flash_pool.liquidity.unwrap_or(0);
        let sp2 = sell_pool.sqrt_price.unwrap_or(0);
        let l2 = sell_pool.liquidity.unwrap_or(0);
        return (
            Box::new(move |amount| simulate_clmm_arb(sp1, l1, sp2, l2, fee1, fee2, amount)),
            hi,
        );
    }

    // Mixed: CLMM flash → AMM sell (or DeepBook)
    // Use the AMM constant-product model for AMM legs and CLMM model for CLMM legs.
    // For DeepBook without order book data, fall back to reserve-based AMM model.
    // Simplification: treat the whole thing as xy=k using effective reserves derived from price.
    let price1 = flash_pool.price_a_in_b().unwrap_or(1.0);
    let price2 = sell_pool.price_a_in_b().unwrap_or(1.0);

    // Synthesize virtual reserves from prices: reserve_b / reserve_a = price
    // Use 1B as virtual pool depth (cancels out in ratio — only relative matters)
    let virtual_depth: u64 = 1_000_000_000;
    let ra1 = virtual_depth;
    let rb1 = (virtual_depth as f64 * price1) as u64;
    let ra2 = virtual_depth;
    let rb2 = (virtual_depth as f64 * price2) as u64;

    (
        Box::new(move |amount| simulate_xy_arb(ra1, rb1, ra2, rb2, fee1, fee2, amount)),
        hi,
    )
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
