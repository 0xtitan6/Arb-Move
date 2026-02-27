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
            (Dex::Cetus, Dex::FlowxAmm, StrategyType::CetusToFlowxAmm),
            (Dex::Turbos, Dex::FlowxAmm, StrategyType::TurbosToFlowxAmm),
            (Dex::DeepBook, Dex::FlowxAmm, StrategyType::DeepBookToFlowxAmm),
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
        let mut big_other = make_pool("0x4", Dex::DeepBook, (1u128 << 64) * 120 / 100);

        // DeepBook needs reserves for price
        big_other.sqrt_price = None;
        big_other.reserve_a = Some(1_000_000);
        big_other.reserve_b = Some(1_440_000); // price ~1.44 vs 0.64

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
}
