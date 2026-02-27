use arb_types::opportunity::{ArbOpportunity, StrategyType};
use arb_types::pool::{Dex, PoolState};
use tracing::{debug, trace};

/// Scans pool states for arbitrage opportunities.
/// Performs O(n²) pairwise comparison of pools sharing the same token pair.
pub struct Scanner {
    /// Minimum profit threshold in MIST.
    pub min_profit_mist: u64,
    /// Maximum staleness in ms — skip pools older than this.
    pub max_staleness_ms: u64,
}

impl Scanner {
    pub fn new(min_profit_mist: u64) -> Self {
        Self {
            min_profit_mist,
            max_staleness_ms: 5_000, // 5 seconds default
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

                // Check for price divergence
                if let (Some(price_a), Some(price_b)) =
                    (pool_a.price_a_in_b(), pool_b.price_a_in_b())
                {
                    // Normalize: ensure we compare A/B prices correctly
                    let (norm_a, norm_b) = if pool_a.coin_type_a == pool_b.coin_type_a {
                        (price_a, price_b)
                    } else {
                        // Pools have reversed ordering
                        (price_a, 1.0 / price_b)
                    };

                    let spread = (norm_a - norm_b).abs() / norm_a.min(norm_b);

                    if spread > 0.001 {
                        // >0.1% spread — potential opportunity
                        trace!(
                            pool_a = %pool_a.object_id,
                            pool_b = %pool_b.object_id,
                            dex_a = %pool_a.dex,
                            dex_b = %pool_b.dex,
                            spread = %format!("{:.4}%", spread * 100.0),
                            "Price divergence detected"
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
                                    type_args: vec![
                                        flash_pool.coin_type_a.clone(),
                                        flash_pool.coin_type_b.clone(),
                                    ],
                                    detected_at_ms: now_ms,
                                });
                            }
                        }
                    }
                }
            }
        }

        // Sort by expected profit descending
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
        // FlowX AMM (sell leg only, like Aftermath)
        (Dex::Cetus, Dex::FlowxAmm) => Some(StrategyType::CetusToFlowxAmm),
        (Dex::Turbos, Dex::FlowxAmm) => Some(StrategyType::TurbosToFlowxAmm),
        (Dex::DeepBook, Dex::FlowxAmm) => Some(StrategyType::DeepBookToFlowxAmm),
        // Aftermath and FlowX AMM can't be flash sources
        (Dex::Aftermath, _) | (Dex::FlowxAmm, _) => None,
        _ => None,
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
            liquidity: Some(1_000_000),
            fee_rate_bps: Some(30),
            reserve_a: None,
            reserve_b: None,
            best_bid: None,
            best_ask: None,
            last_updated_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
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
}
