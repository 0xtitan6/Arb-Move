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

    // ══════════════════════════════════════════════
    //  XY AMM edge cases
    // ══════════════════════════════════════════════

    #[test]
    fn test_xy_arb_zero_reserves() {
        assert_eq!(simulate_xy_arb(0, 1_000, 1_000, 2_000, 30, 30, 100), 0);
        assert_eq!(simulate_xy_arb(1_000, 0, 1_000, 2_000, 30, 30, 100), 0);
        assert_eq!(simulate_xy_arb(1_000, 2_000, 0, 2_000, 30, 30, 100), 0);
        assert_eq!(simulate_xy_arb(1_000, 2_000, 1_000, 0, 30, 30, 100), 0);
    }

    #[test]
    fn test_xy_arb_zero_input() {
        assert_eq!(simulate_xy_arb(1_000, 2_000, 1_000, 3_000, 30, 30, 0), 0);
    }

    #[test]
    fn test_xy_arb_100_pct_fee() {
        // 10000 bps = 100% fee → after_fee = 0
        assert_eq!(
            simulate_xy_arb(1_000_000, 2_000_000, 1_000_000, 3_000_000, 10_000, 0, 100_000),
            0
        );
    }

    #[test]
    fn test_xy_arb_reversed_price_no_profit() {
        // Pool 1 has HIGHER price, Pool 2 LOWER → loss
        let profit = simulate_xy_arb(
            10_000_000, 30_000_000,  // price = 3.0
            10_000_000, 20_000_000,  // price = 2.0
            30, 30,
            100_000,
        );
        assert_eq!(profit, 0, "Reversed arb direction should not profit");
    }

    #[test]
    fn test_xy_arb_profit_scales_with_spread() {
        let profit_small = simulate_xy_arb(
            10_000_000, 20_000_000, 10_000_000, 22_000_000, 30, 30, 100_000,
        );
        let profit_large = simulate_xy_arb(
            10_000_000, 20_000_000, 10_000_000, 30_000_000, 30, 30, 100_000,
        );
        assert!(profit_large > profit_small, "Wider spread = more profit");
    }

    // ══════════════════════════════════════════════
    //  CLMM simulator tests
    // ══════════════════════════════════════════════

    #[test]
    fn test_clmm_arb_zero_liquidity() {
        assert_eq!(simulate_clmm_arb(1 << 64, 0, 1 << 64, 1_000_000, 30, 30, 1_000), 0);
        assert_eq!(simulate_clmm_arb(1 << 64, 1_000_000, 1 << 64, 0, 30, 30, 1_000), 0);
    }

    #[test]
    fn test_clmm_arb_zero_sqrt_price() {
        assert_eq!(simulate_clmm_arb(0, 1_000_000, 1 << 64, 1_000_000, 30, 30, 1_000), 0);
        assert_eq!(simulate_clmm_arb(1 << 64, 1_000_000, 0, 1_000_000, 30, 30, 1_000), 0);
    }

    #[test]
    fn test_clmm_arb_zero_input() {
        assert_eq!(
            simulate_clmm_arb(1 << 64, 1_000_000_000, 1 << 64, 1_000_000_000, 30, 30, 0),
            0
        );
    }

    #[test]
    fn test_clmm_arb_same_price_no_profit() {
        let profit = simulate_clmm_arb(
            1u128 << 64, 100_000_000_000u128,
            1u128 << 64, 100_000_000_000u128,
            30, 30, 1_000_000,
        );
        assert_eq!(profit, 0, "Same prices with fees should not profit");
    }

    #[test]
    fn test_clmm_arb_profitable_with_price_divergence() {
        let sqrt_price_low = (1u128 << 64) * 95 / 100;
        let sqrt_price_high = (1u128 << 64) * 105 / 100;
        let liquidity = 1_000_000_000_000u128;

        let profit = simulate_clmm_arb(
            sqrt_price_low, liquidity, sqrt_price_high, liquidity, 30, 30, 1_000_000,
        );
        assert!(profit > 0, "10% price divergence should profit, got {profit}");
    }

    #[test]
    fn test_clmm_arb_exhausts_liquidity() {
        let profit = simulate_clmm_arb(
            1u128 << 64, 1_000u128, // tiny liquidity
            (1u128 << 64) * 2, 1_000u128,
            0, 0,
            1_000_000_000, // huge input against tiny pool
        );
        assert_eq!(profit, 0, "Should return 0 when exhausting liquidity");
    }

    #[test]
    fn test_clmm_arb_100_pct_fee() {
        assert_eq!(
            simulate_clmm_arb(1 << 64, 1_000_000_000, 1 << 64, 1_000_000_000, 10_000, 10_000, 1_000),
            0
        );
    }

    // ══════════════════════════════════════════════
    //  max_trade_amount tests
    // ══════════════════════════════════════════════

    fn make_pool_for_max(dex: Dex, ra: Option<u64>, rb: Option<u64>, liq: Option<u128>) -> PoolState {
        PoolState {
            object_id: "0x1".into(), dex,
            coin_type_a: "A".into(), coin_type_b: "B".into(),
            sqrt_price: Some(1 << 64), tick_index: Some(0),
            liquidity: liq, fee_rate_bps: Some(30),
            reserve_a: ra, reserve_b: rb,
            best_bid: None, best_ask: None, last_updated_ms: 0,
        }
    }

    #[test]
    fn test_max_trade_amm_with_reserves() {
        let pool = make_pool_for_max(Dex::Aftermath, Some(30_000_000_000), Some(60_000_000_000), None);
        assert_eq!(max_trade_amount(&pool), 10_000_000_000); // min(30B,60B)/3
    }

    #[test]
    fn test_max_trade_amm_no_reserves() {
        let pool = make_pool_for_max(Dex::FlowxAmm, None, None, None);
        assert_eq!(max_trade_amount(&pool), 10_000_000_000); // fallback
    }

    #[test]
    fn test_max_trade_clmm_with_liquidity() {
        // liquidity = 100 * (1 << 32) → liquidity >> 32 = 100
        let pool = make_pool_for_max(Dex::Cetus, None, None, Some(429_496_729_600));
        assert_eq!(max_trade_amount(&pool), 1_000); // 100 clamped to min 1000
    }

    #[test]
    fn test_max_trade_deepbook() {
        let pool = make_pool_for_max(Dex::DeepBook, Some(90_000_000_000), None, None);
        assert_eq!(max_trade_amount(&pool), 30_000_000_000); // 90B/3
    }

    #[test]
    fn test_max_trade_clamped_to_100_sui() {
        let pool = make_pool_for_max(Dex::Aftermath, Some(1_000_000_000_000), Some(1_000_000_000_000), None);
        assert_eq!(max_trade_amount(&pool), MAX_TRADE_MIST);
    }

    #[test]
    fn test_max_trade_clamped_to_min() {
        let pool = make_pool_for_max(Dex::DeepBook, Some(100), None, None); // 100/3=33
        assert_eq!(max_trade_amount(&pool), 1_000); // min clamp
    }

    // ══════════════════════════════════════════════
    //  build_local_simulator tests
    // ══════════════════════════════════════════════

    fn clmm_pool(dex: Dex, sp: u128, liq: u128) -> PoolState {
        PoolState {
            object_id: "0xclmm".into(), dex,
            coin_type_a: "SUI".into(), coin_type_b: "USDC".into(),
            sqrt_price: Some(sp), tick_index: Some(0), liquidity: Some(liq),
            fee_rate_bps: Some(30),
            reserve_a: None, reserve_b: None,
            best_bid: None, best_ask: None, last_updated_ms: 0,
        }
    }

    fn amm_pool(dex: Dex, ra: u64, rb: u64) -> PoolState {
        PoolState {
            object_id: "0xamm".into(), dex,
            coin_type_a: "SUI".into(), coin_type_b: "USDC".into(),
            sqrt_price: None, tick_index: None, liquidity: None,
            fee_rate_bps: Some(30),
            reserve_a: Some(ra), reserve_b: Some(rb),
            best_bid: None, best_ask: None, last_updated_ms: 0,
        }
    }

    #[test]
    fn test_build_simulator_both_amm() {
        let p1 = amm_pool(Dex::Aftermath, 10_000_000, 20_000_000);
        let p2 = amm_pool(Dex::FlowxAmm, 10_000_000, 25_000_000);
        let (sim, hi) = build_local_simulator(&p1, &p2);
        assert!(hi > 0);
        let profit = sim(100_000);
        assert!(profit > 0, "AMM→AMM arb should profit with price gap, got {profit}");
    }

    #[test]
    fn test_build_simulator_both_clmm() {
        let sp_low = (1u128 << 64) * 95 / 100;
        let sp_high = (1u128 << 64) * 105 / 100;
        let liq = 1_000_000_000_000u128;
        let p1 = clmm_pool(Dex::Cetus, sp_low, liq);
        let p2 = clmm_pool(Dex::Turbos, sp_high, liq);
        let (sim, hi) = build_local_simulator(&p1, &p2);
        assert!(hi > 0);
        let profit = sim(1_000_000);
        assert!(profit > 0, "CLMM→CLMM should profit with 10% divergence, got {profit}");
    }

    #[test]
    fn test_build_simulator_mixed_clmm_amm() {
        let flash = clmm_pool(Dex::Cetus, 1u128 << 64, 1_000_000_000_000u128);
        let sell = amm_pool(Dex::Aftermath, 10_000_000, 25_000_000);
        let (sim, hi) = build_local_simulator(&flash, &sell);
        assert!(hi > 0);
        let _profit = sim(100_000); // should not panic
    }

    #[test]
    fn test_build_simulator_hi_bound_uses_min() {
        let small = amm_pool(Dex::Aftermath, 3_000, 6_000); // max=1000 (min clamp)
        let big = amm_pool(Dex::FlowxAmm, 300_000_000_000, 600_000_000_000);
        let (_, hi) = build_local_simulator(&small, &big);
        assert_eq!(hi, 1_000, "Should use minimum of two pool limits");
    }

    // ══════════════════════════════════════════════
    //  Ternary search advanced
    // ══════════════════════════════════════════════

    #[test]
    fn test_ternary_search_flat_function() {
        let (_, profit) = ternary_search(0, 1_000, 1, |_| 42);
        assert_eq!(profit, 42);
    }

    #[test]
    fn test_ternary_search_peak_at_start() {
        let (optimal, _) = ternary_search(0, 100, 1, |x| 100u64.saturating_sub(x));
        assert!(optimal <= 5, "Peak at start, got {optimal}");
    }

    #[test]
    fn test_ternary_search_peak_at_end() {
        let (optimal, _) = ternary_search(0, 100, 1, |x| x);
        assert!(optimal >= 95, "Peak at end, got {optimal}");
    }

    #[test]
    fn test_ternary_search_with_real_amm() {
        let simulate = |amount: u64| {
            simulate_xy_arb(10_000_000, 20_000_000, 10_000_000, 25_000_000, 30, 30, amount)
        };
        let (optimal, max_profit) = ternary_search(1_000, 5_000_000, 10_000, simulate);
        assert!(max_profit > 0, "Should find profitable point");
        assert!(optimal > 1_000 && optimal < 5_000_000);

        // Verify it's near the peak
        let p_low = simulate(optimal.saturating_sub(100_000));
        let p_high = simulate(optimal + 100_000);
        assert!(max_profit >= p_low && max_profit >= p_high);
    }
}
