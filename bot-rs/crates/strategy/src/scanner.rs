use arb_types::decimals::normalize_price;
use arb_types::opportunity::{ArbOpportunity, StrategyType};
use arb_types::pool::{Dex, PoolState};
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{debug, info};

/// Maximum spread (as fraction) considered realistic.
/// Anything above this is a price normalization bug, not a real arb.
/// Real cross-DEX arbs on Sui mainnet are typically 0.01%–5%.
const MAX_REALISTIC_SPREAD: f64 = 0.50; // 50%

/// Scans pool states for arbitrage opportunities.
/// Performs O(n²) pairwise comparison of pools sharing the same token pair.
pub struct Scanner {
    /// Minimum profit threshold in MIST.
    pub min_profit_mist: u64,
    /// Maximum staleness in ms — skip pools older than this.
    pub max_staleness_ms: u64,
    /// Cycle counter for periodic summary logging.
    scan_count: AtomicU64,
}

impl Scanner {
    pub fn new(min_profit_mist: u64) -> Self {
        Self {
            min_profit_mist,
            max_staleness_ms: 5_000, // 5 seconds default
            scan_count: AtomicU64::new(0),
        }
    }

    /// Scan all pool states for two-hop arbitrage opportunities.
    /// Returns opportunities sorted by expected profit (descending).
    pub fn scan_two_hop(&self, pools: &[PoolState]) -> Vec<ArbOpportunity> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let mut opportunities = Vec::new();
        let mut pairs_checked = 0u32;
        let mut divergences = 0u32;
        let mut near_misses = 0u32;
        let mut best_spread = 0.0f64;
        let mut best_pair_desc = String::new();

        // O(n²) pairwise comparison
        for i in 0..pools.len() {
            for j in (i + 1)..pools.len() {
                let pool_a = &pools[i];
                let pool_b = &pools[j];

                // Skip stale pools
                if pool_a.staleness_ms(now_ms) > self.max_staleness_ms
                    || pool_b.staleness_ms(now_ms) > self.max_staleness_ms
                {
                    continue;
                }

                // Check if pools share the same token pair
                if !same_pair(pool_a, pool_b) {
                    continue;
                }

                pairs_checked += 1;

                // Check for price divergence
                if let (Some(price_a), Some(price_b)) =
                    (pool_a.price_a_in_b(), pool_b.price_a_in_b())
                {
                    // Apply decimal normalization for cross-DEX-type comparison
                    let adj_a = normalize_price(
                        price_a,
                        &pool_a.coin_type_a,
                        &pool_a.coin_type_b,
                    );
                    let adj_b = normalize_price(
                        price_b,
                        &pool_b.coin_type_a,
                        &pool_b.coin_type_b,
                    );

                    // Ensure we compare A/B prices in the same direction
                    let (norm_a, norm_b) = if pool_a.coin_type_a == pool_b.coin_type_a {
                        (adj_a, adj_b)
                    } else {
                        // Pools have reversed ordering
                        (adj_a, 1.0 / adj_b)
                    };

                    let spread = (norm_a - norm_b).abs() / norm_a.min(norm_b);

                    // Track best spread for summary logging
                    if spread > best_spread {
                        best_spread = spread;
                        best_pair_desc = format!("{}/{}", pool_a.dex, pool_b.dex);
                    }

                    if spread > 0.001 {
                        // >0.1% spread — potential opportunity
                        divergences += 1;

                        // Sanity check: reject impossible spreads (normalization bugs)
                        if spread > MAX_REALISTIC_SPREAD {
                            debug!(
                                dex_a = %pool_a.dex,
                                dex_b = %pool_b.dex,
                                spread = %format!("{:.2}%", spread * 100.0),
                                pair = %format!("{}/{}",
                                    pool_a.coin_type_a.rsplit("::").next().unwrap_or("?"),
                                    pool_a.coin_type_b.rsplit("::").next().unwrap_or("?")),
                                "Bogus spread rejected (likely decimal mismatch)"
                            );
                            continue;
                        }

                        debug!(
                            dex_a = %pool_a.dex,
                            dex_b = %pool_b.dex,
                            spread = %format!("{:.4}%", spread * 100.0),
                            pair = %format!("{}/{}",
                                pool_a.coin_type_a.rsplit("::").next().unwrap_or("?"),
                                pool_a.coin_type_b.rsplit("::").next().unwrap_or("?")),
                            "Price divergence"
                        );

                        // Determine direction: buy cheap, sell expensive
                        let (flash_pool, sell_pool) = if norm_a < norm_b {
                            (pool_a, pool_b)
                        } else {
                            (pool_b, pool_a)
                        };

                        if let Some(strategy) =
                            resolve_strategy(flash_pool.dex, sell_pool.dex)
                        {
                            // Rough profit estimate (will be refined by optimizer)
                            let est_amount = 1_000_000_000u64; // 1 SUI as starting estimate
                            let est_profit =
                                (est_amount as f64 * spread * 0.5) as u64; // conservative

                            if est_profit > self.min_profit_mist {
                                debug!(
                                    strategy = ?strategy,
                                    spread = %format!("{:.4}%", spread * 100.0),
                                    est_profit = %est_profit,
                                    "Arb opportunity detected"
                                );

                                let mut type_args = vec![
                                    flash_pool.coin_type_a.clone(),
                                    flash_pool.coin_type_b.clone(),
                                ];
                                // Turbos pools need their fee tier type as an extra type arg
                                if let Some(ft) = find_turbos_fee_type(&[flash_pool, sell_pool]) {
                                    type_args.push(ft);
                                }

                                opportunities.push(ArbOpportunity {
                                    strategy,
                                    amount_in: est_amount,
                                    expected_profit: est_profit,
                                    estimated_gas: 5_000_000, // ~5M MIST default
                                    net_profit: est_profit as i64 - 5_000_000,
                                    pool_ids: vec![
                                        flash_pool.object_id.clone(),
                                        sell_pool.object_id.clone(),
                                    ],
                                    type_args,
                                    detected_at_ms: now_ms,
                                });
                            } else {
                                near_misses += 1;
                                debug!(
                                    dex_a = %flash_pool.dex,
                                    dex_b = %sell_pool.dex,
                                    spread = %format!("{:.4}%", spread * 100.0),
                                    est_profit = %est_profit,
                                    min_profit = %self.min_profit_mist,
                                    "Near miss — spread found but below threshold"
                                );
                            }
                        }
                    }
                }
            }
        }

        // Periodic summary log (every ~20 scans ≈ 30s at 1.5s interval)
        let cycle = self.scan_count.fetch_add(1, Ordering::Relaxed);
        if cycle % 20 == 0 {
            info!(
                cycle = cycle,
                pairs_checked = pairs_checked,
                divergences = divergences,
                near_misses = near_misses,
                opportunities = opportunities.len(),
                best_spread = %format!("{:.4}%", best_spread * 100.0),
                best_pair = %best_pair_desc,
                "📊 Scan summary"
            );
        }

        // Sort by expected profit descending
        opportunities.sort_by(|a, b| b.expected_profit.cmp(&a.expected_profit));
        opportunities
    }

    /// Scan for tri-hop (triangular) arbitrage opportunities: A→B→C→A.
    ///
    /// Finds loops where:
    /// - Pool 1 trades A/B (flash borrow A, get B)
    /// - Pool 2 trades B/C (swap B for C)
    /// - Pool 3 trades C/A (swap C for A, repay flash)
    ///
    /// Returns opportunities sorted by expected profit (descending).
    pub fn scan_tri_hop(&self, pools: &[PoolState]) -> Vec<ArbOpportunity> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let mut opportunities = Vec::new();

        // Filter to fresh pools only
        let fresh: Vec<&PoolState> = pools
            .iter()
            .filter(|p| p.staleness_ms(now_ms) <= self.max_staleness_ms)
            .collect();

        // O(n³) — fine for small pool counts (<50 pools)
        for p1 in &fresh {
            for p2 in &fresh {
                if std::ptr::eq(*p1, *p2) {
                    continue;
                }

                // Find shared token between p1 and p2 (the "B" in A→B→C)
                let shared = shared_token(p1, p2);
                if shared.is_none() {
                    continue;
                }
                let (token_b, token_a_from_p1, token_c_from_p2) = shared.unwrap();

                for p3 in &fresh {
                    if std::ptr::eq(*p1, *p3) || std::ptr::eq(*p2, *p3) {
                        continue;
                    }

                    // p3 must trade C/A to close the triangle
                    if !pool_has_pair(p3, &token_c_from_p2, &token_a_from_p1) {
                        continue;
                    }

                    // We have a triangle: A→B (p1) → C (p2) → A (p3)
                    // Check if price loop creates an arbitrage
                    let price_ab = pool_price_for_direction(p1, &token_a_from_p1, &token_b);
                    let price_bc = pool_price_for_direction(p2, &token_b, &token_c_from_p2);
                    let price_ca = pool_price_for_direction(p3, &token_c_from_p2, &token_a_from_p1);

                    if let (Some(pab), Some(pbc), Some(pca)) = (price_ab, price_bc, price_ca) {
                        // Cross-rate: if pab * pbc * pca > 1.0, there's an arb
                        let cross_rate = pab * pbc * pca;

                        if cross_rate > 1.01 && cross_rate < (1.0 + MAX_REALISTIC_SPREAD) {
                            // >1.0% edge for tri-hops (3 sequential swaps need larger edge
                            // Try to find a valid pool ordering + strategy for these 3 pools
                            if let Some((strategy, ordered_pools, mut type_args)) =
                                resolve_tri_with_ordering(p1, p2, p3)
                            {
                                let spread = cross_rate - 1.0;
                                let est_amount = 5_000_000_000u64; // 5 SUI
                                // Tri-hop slippage factor: use 0.15 (not 0.5) because
                                // 3 sequential swaps compound price impact significantly.
                                // 2-hop uses 0.5; tri-hop needs much more conservative estimate.
                                let est_profit =
                                    (est_amount as f64 * spread * 0.15) as u64;
                                let tri_gas_estimate: u64 = 4_000_000;

                                if est_profit > self.min_profit_mist {
                                    debug!(
                                        strategy = ?strategy,
                                        cross_rate = %format!("{:.6}", cross_rate),
                                        est_profit = %est_profit,
                                        path = %format!("{} → {} → {} → {}",
                                            type_args[0].rsplit("::").next().unwrap_or("?"),
                                            type_args[1].rsplit("::").next().unwrap_or("?"),
                                            type_args[2].rsplit("::").next().unwrap_or("?"),
                                            type_args[0].rsplit("::").next().unwrap_or("?")),
                                        "Tri-hop opportunity detected"
                                    );

                                    // Turbos pools need their fee tier type as extra type arg
                                    if let Some(ft) = find_turbos_fee_type(&[p1, p2, p3]) {
                                        type_args.push(ft);
                                    }

                                    opportunities.push(ArbOpportunity {
                                        strategy,
                                        amount_in: est_amount,
                                        expected_profit: est_profit,
                                        estimated_gas: tri_gas_estimate,
                                        net_profit: est_profit as i64 - tri_gas_estimate as i64,
                                        pool_ids: ordered_pools
                                            .iter()
                                            .map(|p| p.object_id.clone())
                                            .collect(),
                                        type_args,
                                        detected_at_ms: now_ms,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        // Deduplicate (same 3 pools in different order = same opportunity)
        opportunities.dedup_by(|a, b| {
            let mut ids_a = a.pool_ids.clone();
            let mut ids_b = b.pool_ids.clone();
            ids_a.sort();
            ids_b.sort();
            ids_a == ids_b
        });

        opportunities.sort_by(|a, b| b.expected_profit.cmp(&a.expected_profit));
        opportunities
    }
}

/// Check if two pools trade the same token pair (in either order).
fn same_pair(a: &PoolState, b: &PoolState) -> bool {
    (a.coin_type_a == b.coin_type_a && a.coin_type_b == b.coin_type_b)
        || (a.coin_type_a == b.coin_type_b && a.coin_type_b == b.coin_type_a)
}

/// Map a (flash_source_dex, sell_dex) pair to the correct StrategyType.
fn resolve_strategy(flash_dex: Dex, sell_dex: Dex) -> Option<StrategyType> {
    match (flash_dex, sell_dex) {
        (Dex::Cetus, Dex::Turbos) => Some(StrategyType::CetusToTurbos),
        (Dex::Turbos, Dex::Cetus) => Some(StrategyType::TurbosToCetus),
        (Dex::Cetus, Dex::DeepBook) => Some(StrategyType::CetusToDeepBook),
        (Dex::DeepBook, Dex::Cetus) => Some(StrategyType::DeepBookToCetus),
        (Dex::Turbos, Dex::DeepBook) => Some(StrategyType::TurbosToDeepBook),
        (Dex::DeepBook, Dex::Turbos) => Some(StrategyType::DeepBookToTurbos),
        (Dex::Cetus, Dex::Aftermath) => Some(StrategyType::CetusToAftermath),
        (Dex::Turbos, Dex::Aftermath) => Some(StrategyType::TurbosToAftermath),
        (Dex::DeepBook, Dex::Aftermath) => Some(StrategyType::DeepBookToAftermath),
        (Dex::Cetus, Dex::FlowxClmm) => Some(StrategyType::CetusToFlowxClmm),
        (Dex::FlowxClmm, Dex::Cetus) => Some(StrategyType::FlowxClmmToCetus),
        (Dex::Turbos, Dex::FlowxClmm) => Some(StrategyType::TurbosToFlowxClmm),
        (Dex::FlowxClmm, Dex::Turbos) => Some(StrategyType::FlowxClmmToTurbos),
        (Dex::DeepBook, Dex::FlowxClmm) => Some(StrategyType::DeepBookToFlowxClmm),
        (Dex::FlowxClmm, Dex::DeepBook) => Some(StrategyType::FlowxClmmToDeepBook),
        // FlowX AMM — NO on-chain Move implementation exists.
        // These would burn gas with MoveAbort. Disabled until Move code ships.
        (Dex::Cetus, Dex::FlowxAmm)
        | (Dex::Turbos, Dex::FlowxAmm)
        | (Dex::DeepBook, Dex::FlowxAmm) => {
            tracing::trace!("FlowX AMM strategy skipped — no on-chain code");
            None
        }
        // Aftermath and FlowX AMM can't be flash sources
        (Dex::Aftermath, _) | (Dex::FlowxAmm, _) => None,
        _ => None,
    }
}

/// Map a (dex1, dex2, dex3) triple to the correct tri-hop StrategyType.
fn resolve_tri_strategy(dex1: Dex, dex2: Dex, dex3: Dex) -> Option<StrategyType> {
    match (dex1, dex2, dex3) {
        (Dex::Cetus, Dex::Cetus, Dex::Cetus) => Some(StrategyType::TriCetusCetusCetus),
        (Dex::Cetus, Dex::Cetus, Dex::Turbos) => Some(StrategyType::TriCetusCetusTurbos),
        (Dex::Cetus, Dex::Turbos, Dex::DeepBook) => Some(StrategyType::TriCetusTurbosDeepBook),
        (Dex::Cetus, Dex::DeepBook, Dex::Turbos) => Some(StrategyType::TriCetusDeepBookTurbos),
        (Dex::DeepBook, Dex::Cetus, Dex::Turbos) => Some(StrategyType::TriDeepBookCetusTurbos),
        (Dex::Cetus, Dex::Cetus, Dex::Aftermath) => Some(StrategyType::TriCetusCetusAftermath),
        (Dex::Cetus, Dex::Turbos, Dex::Aftermath) => Some(StrategyType::TriCetusTurbosAftermath),
        (Dex::Cetus, Dex::Cetus, Dex::FlowxClmm) => Some(StrategyType::TriCetusCetusFlowxClmm),
        (Dex::Cetus, Dex::FlowxClmm, Dex::Turbos) => Some(StrategyType::TriCetusFlowxClmmTurbos),
        (Dex::FlowxClmm, Dex::Cetus, Dex::Turbos) => Some(StrategyType::TriFlowxClmmCetusTurbos),
        _ => None,
    }
}

/// Find the Turbos fee type from a set of pools.
/// Returns the fee_type of the first Turbos pool found (strategies have at most one).
fn find_turbos_fee_type(pools: &[&PoolState]) -> Option<String> {
    pools
        .iter()
        .find(|p| p.dex == Dex::Turbos)
        .and_then(|p| p.fee_type.clone())
}

/// Resolve tri-hop strategy with validated pool ordering.
///
/// Tries to find a valid assignment of 3 pools to move function parameters.
/// Returns `(strategy, ordered_pools, type_args)` if a valid ordering exists.
///
/// **a2b cycle** (v1): pool_ab<A,B> → pool_bc<B,C> → pool_ca<C,A> all using swap_a2b.
/// **b2a third leg** (v2): pool_ab<A,B> → pool_bc<B,C> → pool_ac<A,C> where third leg uses swap_b2a.
fn resolve_tri_with_ordering<'a>(
    p1: &'a PoolState,
    p2: &'a PoolState,
    p3: &'a PoolState,
) -> Option<(StrategyType, Vec<&'a PoolState>, Vec<String>)> {
    let pools = [p1, p2, p3];
    let perms: [[usize; 3]; 6] = [
        [0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0],
    ];

    // --- Try 1: a2b cycle (v1 strategies) ---
    // Constraints: p[0].coin_b == p[1].coin_a, p[1].coin_b == p[2].coin_a, p[2].coin_b == p[0].coin_a
    for perm in &perms {
        let pa = pools[perm[0]];
        let pb = pools[perm[1]];
        let pc = pools[perm[2]];

        if pa.coin_type_b == pb.coin_type_a
            && pb.coin_type_b == pc.coin_type_a
            && pc.coin_type_b == pa.coin_type_a
        {
            if let Some(strategy) = resolve_tri_strategy(pa.dex, pb.dex, pc.dex) {
                let type_args = vec![
                    pa.coin_type_a.clone(), // A
                    pa.coin_type_b.clone(), // B
                    pb.coin_type_b.clone(), // C
                ];
                return Some((strategy, vec![pa, pb, pc], type_args));
            }
        }
    }

    // --- Try 2: b2a third leg (v2 strategies) ---
    // Constraints: p[0].coin_b == p[1].coin_a (A→B chain, B→C chain)
    //              p[2].coin_a == p[0].coin_a (both are A)
    //              p[2].coin_b == p[1].coin_b (both are C)
    // Third pool is Pool<A, C>, swap direction is b2a (C→A).
    for perm in &perms {
        let pa = pools[perm[0]];
        let pb = pools[perm[1]];
        let pc = pools[perm[2]];

        if pa.coin_type_b == pb.coin_type_a
            && pc.coin_type_a == pa.coin_type_a
            && pc.coin_type_b == pb.coin_type_b
        {
            if let Some(strategy) = resolve_tri_strategy_v2(pa.dex, pb.dex, pc.dex) {
                let type_args = vec![
                    pa.coin_type_a.clone(), // A
                    pa.coin_type_b.clone(), // B
                    pb.coin_type_b.clone(), // C
                ];
                return Some((strategy, vec![pa, pb, pc], type_args));
            }
        }
    }

    None
}

/// Map a (dex1, dex2, dex3) triple to v2 strategy (third leg uses b2a).
fn resolve_tri_strategy_v2(dex1: Dex, dex2: Dex, dex3: Dex) -> Option<StrategyType> {
    match (dex1, dex2, dex3) {
        (Dex::Cetus, Dex::Cetus, Dex::Cetus) => Some(StrategyType::TriCetusCetusCetusV2),
        // Future: add more v2 variants as needed (e.g., TriCetusCetusTurbosV2)
        _ => None,
    }
}

/// Find the shared token between two pools.
/// Returns `(shared_token, other_from_p1, other_from_p2)` where:
/// - `shared_token` is the token both pools trade
/// - `other_from_p1` is the non-shared token from pool 1 (token A in the triangle)
/// - `other_from_p2` is the non-shared token from pool 2 (token C in the triangle)
fn shared_token(p1: &PoolState, p2: &PoolState) -> Option<(String, String, String)> {
    if p1.coin_type_a == p2.coin_type_a {
        // Shared: A1 == A2, others: B1 and B2
        Some((p1.coin_type_a.clone(), p1.coin_type_b.clone(), p2.coin_type_b.clone()))
    } else if p1.coin_type_a == p2.coin_type_b {
        // Shared: A1 == B2, others: B1 and A2
        Some((p1.coin_type_a.clone(), p1.coin_type_b.clone(), p2.coin_type_a.clone()))
    } else if p1.coin_type_b == p2.coin_type_a {
        // Shared: B1 == A2, others: A1 and B2
        Some((p1.coin_type_b.clone(), p1.coin_type_a.clone(), p2.coin_type_b.clone()))
    } else if p1.coin_type_b == p2.coin_type_b {
        // Shared: B1 == B2, others: A1 and A2
        Some((p1.coin_type_b.clone(), p1.coin_type_a.clone(), p2.coin_type_a.clone()))
    } else {
        None
    }
}

/// Check if a pool trades a specific pair (in either order).
fn pool_has_pair(pool: &PoolState, token_x: &str, token_y: &str) -> bool {
    (pool.coin_type_a == token_x && pool.coin_type_b == token_y)
        || (pool.coin_type_a == token_y && pool.coin_type_b == token_x)
}

/// Get the effective price for swapping `from` → `to` on a pool.
/// Returns None if the pool doesn't have price data or doesn't trade the pair.
fn pool_price_for_direction(pool: &PoolState, from: &str, to: &str) -> Option<f64> {
    let base_price = pool.price_a_in_b()?;
    let normalized = normalize_price(base_price, &pool.coin_type_a, &pool.coin_type_b);

    if pool.coin_type_a == from && pool.coin_type_b == to {
        // a→b: price is already A-in-B
        Some(normalized)
    } else if pool.coin_type_b == from && pool.coin_type_a == to {
        // b→a: invert
        if normalized > 0.0 {
            Some(1.0 / normalized)
        } else {
            None
        }
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pool(id: &str, dex: Dex, sqrt_price: u128) -> PoolState {
        PoolState {
            object_id: id.to_string(),
            dex,
            coin_type_a: "SUI".to_string(),
            coin_type_b: "USDC".to_string(),
            sqrt_price: Some(sqrt_price),
            tick_index: None,
            liquidity: Some(1_000_000_000),
            fee_rate_bps: Some(30),
            reserve_a: None,
            reserve_b: None,
            best_bid: None,
            best_ask: None,
            last_updated_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            fee_type: None,
        }
    }

    #[test]
    fn test_same_pair() {
        let a = make_pool("0x1", Dex::Cetus, 1 << 64);
        let b = make_pool("0x2", Dex::Turbos, 1 << 64);
        assert!(same_pair(&a, &b));
    }

    #[test]
    fn test_resolve_strategy() {
        assert_eq!(
            resolve_strategy(Dex::Cetus, Dex::Turbos),
            Some(StrategyType::CetusToTurbos)
        );
        assert_eq!(
            resolve_strategy(Dex::Cetus, Dex::FlowxClmm),
            Some(StrategyType::CetusToFlowxClmm)
        );
        // Aftermath can't be a flash source
        assert_eq!(resolve_strategy(Dex::Aftermath, Dex::Cetus), None);
    }

    // ── resolve_strategy exhaustive tests ──

    #[test]
    fn test_resolve_all_valid_strategies() {
        let cases = vec![
            (Dex::Cetus, Dex::Turbos, StrategyType::CetusToTurbos),
            (Dex::Turbos, Dex::Cetus, StrategyType::TurbosToCetus),
            (Dex::Cetus, Dex::DeepBook, StrategyType::CetusToDeepBook),
            (Dex::DeepBook, Dex::Cetus, StrategyType::DeepBookToCetus),
            (Dex::Turbos, Dex::DeepBook, StrategyType::TurbosToDeepBook),
            (Dex::DeepBook, Dex::Turbos, StrategyType::DeepBookToTurbos),
            (Dex::Cetus, Dex::Aftermath, StrategyType::CetusToAftermath),
            (Dex::Turbos, Dex::Aftermath, StrategyType::TurbosToAftermath),
            (Dex::DeepBook, Dex::Aftermath, StrategyType::DeepBookToAftermath),
            (Dex::Cetus, Dex::FlowxClmm, StrategyType::CetusToFlowxClmm),
            (Dex::FlowxClmm, Dex::Cetus, StrategyType::FlowxClmmToCetus),
            (Dex::Turbos, Dex::FlowxClmm, StrategyType::TurbosToFlowxClmm),
            (Dex::FlowxClmm, Dex::Turbos, StrategyType::FlowxClmmToTurbos),
            (Dex::DeepBook, Dex::FlowxClmm, StrategyType::DeepBookToFlowxClmm),
            (Dex::FlowxClmm, Dex::DeepBook, StrategyType::FlowxClmmToDeepBook),
            // FlowX AMM strategies are intentionally disabled (no on-chain code)
        ];

        for (flash, sell, expected) in cases {
            assert_eq!(
                resolve_strategy(flash, sell),
                Some(expected),
                "Failed for {flash:?} → {sell:?}"
            );
        }
    }

    #[test]
    fn test_resolve_no_flash_dexes() {
        // Aftermath and FlowxAmm cannot be flash sources
        for sell in [Dex::Cetus, Dex::Turbos, Dex::DeepBook, Dex::FlowxClmm, Dex::Aftermath, Dex::FlowxAmm] {
            assert_eq!(resolve_strategy(Dex::Aftermath, sell), None, "Aftermath as flash → {sell:?}");
            assert_eq!(resolve_strategy(Dex::FlowxAmm, sell), None, "FlowxAmm as flash → {sell:?}");
        }
    }

    #[test]
    fn test_resolve_flowx_amm_disabled() {
        // FlowX AMM has no on-chain Move code — must return None for all combos
        assert_eq!(resolve_strategy(Dex::Cetus, Dex::FlowxAmm), None);
        assert_eq!(resolve_strategy(Dex::Turbos, Dex::FlowxAmm), None);
        assert_eq!(resolve_strategy(Dex::DeepBook, Dex::FlowxAmm), None);
    }

    #[test]
    fn test_resolve_same_dex_returns_none() {
        // Same DEX can't be both flash and sell (no arb)
        assert_eq!(resolve_strategy(Dex::Cetus, Dex::Cetus), None);
        assert_eq!(resolve_strategy(Dex::Turbos, Dex::Turbos), None);
        assert_eq!(resolve_strategy(Dex::DeepBook, Dex::DeepBook), None);
        assert_eq!(resolve_strategy(Dex::FlowxClmm, Dex::FlowxClmm), None);
    }

    // ── same_pair tests ──

    #[test]
    fn test_same_pair_reversed() {
        let a = make_pool("0x1", Dex::Cetus, 1 << 64);
        let mut b = make_pool("0x2", Dex::Turbos, 1 << 64);
        b.coin_type_a = "USDC".to_string();
        b.coin_type_b = "SUI".to_string();
        assert!(same_pair(&a, &b), "Reversed pair should still match");
    }

    #[test]
    fn test_different_pair_no_match() {
        let a = make_pool("0x1", Dex::Cetus, 1 << 64);
        let mut b = make_pool("0x2", Dex::Turbos, 1 << 64);
        b.coin_type_b = "WETH".to_string();
        assert!(!same_pair(&a, &b));
    }

    // ── scan_two_hop integration tests ──

    #[test]
    fn test_scan_empty_pools() {
        let scanner = Scanner::new(1_000);
        assert!(scanner.scan_two_hop(&[]).is_empty());
    }

    #[test]
    fn test_scan_single_pool_no_opportunities() {
        let scanner = Scanner::new(1_000);
        let pools = vec![make_pool("0x1", Dex::Cetus, 1 << 64)];
        assert!(scanner.scan_two_hop(&pools).is_empty());
    }

    #[test]
    fn test_scan_same_price_no_opportunities() {
        let scanner = Scanner::new(1_000);
        let pools = vec![
            make_pool("0x1", Dex::Cetus, 1 << 64),
            make_pool("0x2", Dex::Turbos, 1 << 64),
        ];
        assert!(scanner.scan_two_hop(&pools).is_empty());
    }

    #[test]
    fn test_scan_detects_spread() {
        let scanner = Scanner::new(0); // zero min_profit to catch everything
        let pools = vec![
            make_pool("0x1", Dex::Cetus, (1u128 << 64) * 90 / 100), // price=0.81
            make_pool("0x2", Dex::Turbos, (1u128 << 64) * 110 / 100), // price=1.21
        ];
        let opps = scanner.scan_two_hop(&pools);
        assert!(!opps.is_empty(), "Should detect ~40% spread");
        assert_eq!(opps[0].pool_ids.len(), 2);
    }

    #[test]
    fn test_scan_skips_stale_pools() {
        let scanner = Scanner::new(0);
        let mut fresh = make_pool("0x1", Dex::Cetus, (1u128 << 64) * 80 / 100);
        let mut stale = make_pool("0x2", Dex::Turbos, (1u128 << 64) * 120 / 100);
        stale.last_updated_ms = 0; // epoch = very stale
        fresh.last_updated_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let opps = scanner.scan_two_hop(&[fresh, stale]);
        assert!(opps.is_empty(), "Should skip stale pool");
    }

    #[test]
    fn test_scan_different_pairs_no_match() {
        let scanner = Scanner::new(0);
        let mut a = make_pool("0x1", Dex::Cetus, (1u128 << 64) * 80 / 100);
        let mut b = make_pool("0x2", Dex::Turbos, (1u128 << 64) * 120 / 100);
        b.coin_type_b = "WETH".to_string();
        a.last_updated_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        b.last_updated_ms = a.last_updated_ms;
        assert!(scanner.scan_two_hop(&[a, b]).is_empty());
    }

    #[test]
    fn test_scan_sorted_by_profit() {
        let scanner = Scanner::new(0);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let mut small_spread = make_pool("0x1", Dex::Cetus, (1u128 << 64) * 98 / 100);
        let mut small_other = make_pool("0x2", Dex::Turbos, (1u128 << 64) * 103 / 100);
        let mut big_spread = make_pool("0x3", Dex::Cetus, (1u128 << 64) * 80 / 100);
        // Use Turbos (CLMM) instead of DeepBook since DeepBook no longer
        // falls back to vault reserves for price (it's a CLOB, not AMM).
        let mut big_other = make_pool("0x4", Dex::Turbos, (1u128 << 64) * 120 / 100);

        for p in [&mut small_spread, &mut small_other, &mut big_spread, &mut big_other] {
            p.last_updated_ms = now;
        }

        let opps = scanner.scan_two_hop(&[small_spread, small_other, big_spread, big_other]);
        if opps.len() >= 2 {
            assert!(
                opps[0].expected_profit >= opps[1].expected_profit,
                "Should be sorted descending by profit"
            );
        }
    }

    // ── Tri-hop helper tests ──

    fn make_tri_pool(id: &str, dex: Dex, coin_a: &str, coin_b: &str, price: f64) -> PoolState {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let is_clmm = matches!(dex, Dex::Cetus | Dex::Turbos | Dex::FlowxClmm);

        let (sqrt_price, liquidity, reserve_a, reserve_b) = if is_clmm {
            // CLMM: set sqrt_price from desired price (Q64.64)
            let sqrt_val = price.sqrt();
            let sp = (sqrt_val * (1u128 << 64) as f64) as u128;
            (Some(sp), Some(1_000_000_000_000u128), None, None)
        } else {
            // AMM: use reserves
            let ra = 1_000_000_000u64;
            let rb = (ra as f64 * price) as u64;
            (None, None, Some(ra), Some(rb))
        };

        PoolState {
            object_id: id.to_string(),
            dex,
            coin_type_a: coin_a.to_string(),
            coin_type_b: coin_b.to_string(),
            sqrt_price,
            tick_index: Some(0),
            liquidity,
            fee_rate_bps: Some(30),
            reserve_a,
            reserve_b,
            best_bid: None,
            best_ask: None,
            last_updated_ms: now,
            fee_type: None,
        }
    }

    #[test]
    fn test_shared_token_same_a() {
        let p1 = make_pool("0x1", Dex::Cetus, 1 << 64);
        let mut p2 = make_pool("0x2", Dex::Cetus, 1 << 64);
        p2.coin_type_b = "DEEP".to_string();
        let (shared, other1, other2) = shared_token(&p1, &p2).unwrap();
        assert_eq!(shared, "SUI");
        assert_eq!(other1, "USDC");
        assert_eq!(other2, "DEEP");
    }

    #[test]
    fn test_shared_token_cross() {
        let p1 = make_pool("0x1", Dex::Cetus, 1 << 64); // SUI/USDC
        let mut p2 = make_pool("0x2", Dex::Turbos, 1 << 64);
        p2.coin_type_a = "DEEP".to_string();
        p2.coin_type_b = "SUI".to_string(); // DEEP/SUI — shared is SUI
        let (shared, other1, other2) = shared_token(&p1, &p2).unwrap();
        assert_eq!(shared, "SUI");
        assert_eq!(other1, "USDC");
        assert_eq!(other2, "DEEP");
    }

    #[test]
    fn test_shared_token_none() {
        let p1 = make_pool("0x1", Dex::Cetus, 1 << 64); // SUI/USDC
        let mut p2 = make_pool("0x2", Dex::Turbos, 1 << 64);
        p2.coin_type_a = "DEEP".to_string();
        p2.coin_type_b = "WETH".to_string();
        assert!(shared_token(&p1, &p2).is_none());
    }

    #[test]
    fn test_pool_has_pair() {
        let p = make_pool("0x1", Dex::Cetus, 1 << 64); // SUI/USDC
        assert!(pool_has_pair(&p, "SUI", "USDC"));
        assert!(pool_has_pair(&p, "USDC", "SUI")); // reversed
        assert!(!pool_has_pair(&p, "SUI", "DEEP"));
    }

    #[test]
    fn test_resolve_tri_strategy_valid() {
        assert_eq!(
            resolve_tri_strategy(Dex::Cetus, Dex::Cetus, Dex::Cetus),
            Some(StrategyType::TriCetusCetusCetus)
        );
        assert_eq!(
            resolve_tri_strategy(Dex::Cetus, Dex::Turbos, Dex::DeepBook),
            Some(StrategyType::TriCetusTurbosDeepBook)
        );
        assert_eq!(
            resolve_tri_strategy(Dex::FlowxClmm, Dex::Cetus, Dex::Turbos),
            Some(StrategyType::TriFlowxClmmCetusTurbos)
        );
    }

    #[test]
    fn test_resolve_tri_strategy_invalid() {
        // No strategy for Turbos→Turbos→Turbos
        assert_eq!(resolve_tri_strategy(Dex::Turbos, Dex::Turbos, Dex::Turbos), None);
        // No strategy for Aftermath as flash source
        assert_eq!(resolve_tri_strategy(Dex::Aftermath, Dex::Cetus, Dex::Turbos), None);
    }

    #[test]
    fn test_scan_tri_hop_empty() {
        let scanner = Scanner::new(0);
        assert!(scanner.scan_tri_hop(&[]).is_empty());
    }

    #[test]
    fn test_scan_tri_hop_needs_three_pools() {
        let scanner = Scanner::new(0);
        // Use 9-decimal tokens to avoid normalization effects in tests
        let p1 = make_tri_pool("0x1", Dex::Cetus, "SUI", "CETUS", 3.0);
        let p2 = make_tri_pool("0x2", Dex::Cetus, "CETUS", "NAVX", 0.5);
        // Only 2 pools — can't form triangle
        assert!(scanner.scan_tri_hop(&[p1, p2]).is_empty());
    }

    #[test]
    fn test_scan_tri_hop_finds_triangle() {
        let scanner = Scanner::new(0);
        // Create a profitable triangle with same-decimal (9) tokens: SUI→CETUS→NAVX→SUI
        // Prices set so the cross-rate > 1.003
        let p1 = make_tri_pool("0x1", Dex::Cetus, "SUI", "CETUS", 3.5);   // 1 SUI = 3.5 CETUS
        let p2 = make_tri_pool("0x2", Dex::Cetus, "CETUS", "NAVX", 2.0);  // 1 CETUS = 2 NAVX
        let p3 = make_tri_pool("0x3", Dex::Cetus, "NAVX", "SUI", 0.2);    // 1 NAVX = 0.2 SUI
        // Cross rate: 3.5 * 2.0 * 0.2 = 1.4 (40% edge)

        let opps = scanner.scan_tri_hop(&[p1, p2, p3]);
        assert!(!opps.is_empty(), "Should find triangular arb");
        assert_eq!(opps[0].pool_ids.len(), 3);
        assert_eq!(opps[0].type_args.len(), 3);
    }

    #[test]
    fn test_scan_tri_hop_no_arb_balanced() {
        let scanner = Scanner::new(0);
        // Balanced triangle: cross-rate ≈ 1.0 (no arb after fees)
        let p1 = make_tri_pool("0x1", Dex::Cetus, "SUI", "CETUS", 3.0);
        let p2 = make_tri_pool("0x2", Dex::Cetus, "CETUS", "NAVX", 2.0);
        let p3 = make_tri_pool("0x3", Dex::Cetus, "NAVX", "SUI", 0.1667); // cross: 3*2*0.1667 ≈ 1.0
        let opps = scanner.scan_tri_hop(&[p1, p2, p3]);
        assert!(opps.is_empty(), "Balanced triangle should not produce arb");
    }
}
