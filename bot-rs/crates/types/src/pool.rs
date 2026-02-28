use serde::{Deserialize, Serialize};

/// Unique identifier for a pool across all DEXes.
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolId(pub String);

/// Which DEX a pool belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Dex {
    Cetus,
    Turbos,
    DeepBook,
    Aftermath,
    FlowxClmm,
    FlowxAmm,
}

impl std::fmt::Display for Dex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Dex::Cetus => write!(f, "Cetus"),
            Dex::Turbos => write!(f, "Turbos"),
            Dex::DeepBook => write!(f, "DeepBook"),
            Dex::Aftermath => write!(f, "Aftermath"),
            Dex::FlowxClmm => write!(f, "FlowX CLMM"),
            Dex::FlowxAmm => write!(f, "FlowX AMM"),
        }
    }
}

/// Normalized pool state — extracted from on-chain data, used by strategy scanner.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolState {
    /// Object ID of the pool on Sui.
    pub object_id: String,
    /// Which DEX this pool belongs to.
    pub dex: Dex,
    /// Fully-qualified type of coin A (e.g., "0x2::sui::SUI").
    pub coin_type_a: String,
    /// Fully-qualified type of coin B.
    pub coin_type_b: String,

    /// Current sqrt price (Q64.64 for CLMM pools, None for AMM/CLOB).
    pub sqrt_price: Option<u128>,
    /// Current tick index (CLMM only).
    pub tick_index: Option<i32>,
    /// Active liquidity at current tick (CLMM only).
    pub liquidity: Option<u128>,
    /// Fee rate in basis points (e.g., 3000 = 0.3%).
    pub fee_rate_bps: Option<u64>,

    /// Reserve of coin A (AMM pools / DeepBook vault).
    pub reserve_a: Option<u64>,
    /// Reserve of coin B.
    pub reserve_b: Option<u64>,

    /// Best bid price for CLOB (DeepBook) — None for AMMs.
    pub best_bid: Option<f64>,
    /// Best ask price for CLOB.
    pub best_ask: Option<f64>,

    /// Epoch timestamp of last update (ms since Unix epoch).
    pub last_updated_ms: u64,

    /// Extra type parameter required by the pool's DEX (e.g., Turbos fee tier type).
    /// Turbos pools have a `TurbosFee` phantom type in Pool<A, B, Fee>.
    /// Must be passed as an additional type argument in Move calls.
    pub fee_type: Option<String>,
}

impl PoolState {
    /// Minimum liquidity for a CLMM pool to be considered usable.
    /// Pools below this threshold return None from price_a_in_b(),
    /// effectively excluding them from the scanner.
    /// 10_000_000 ≈ negligible for any real swap.
    const MIN_CLMM_LIQUIDITY: u128 = 10_000_000;

    /// Compute the effective price of A in terms of B.
    /// For CLMM: derived from sqrt_price (only if liquidity is above minimum).
    /// For AMM: reserve_b / reserve_a.
    /// For CLOB: midpoint of bid/ask.
    pub fn price_a_in_b(&self) -> Option<f64> {
        match self.dex {
            Dex::Cetus | Dex::Turbos | Dex::FlowxClmm => {
                // Skip pools with zero or negligible liquidity — their sqrt_price
                // is meaningless and creates phantom spreads in the scanner.
                let liq = self.liquidity.unwrap_or(0);
                if liq < Self::MIN_CLMM_LIQUIDITY {
                    return None;
                }
                self.sqrt_price.map(|sp| {
                    let sp_f64 = sp as f64 / (1u128 << 64) as f64;
                    sp_f64 * sp_f64
                })
            }
            Dex::Aftermath | Dex::FlowxAmm => {
                match (self.reserve_a, self.reserve_b) {
                    (Some(a), Some(b)) if a > 0 => Some(b as f64 / a as f64),
                    _ => None,
                }
            }
            Dex::DeepBook => {
                // DeepBook is a CLOB — price comes from the order book, not vault reserves.
                // Vault balances (base_vault/quote_vault) represent total deposited tokens
                // from all limit orders and have NO relationship to market price.
                // Using reserve_b/reserve_a here would produce garbage prices that create
                // phantom million-percent spreads against accurate CLMM pool prices.
                // Only return a price if we have actual bid/ask data.
                match (self.best_bid, self.best_ask) {
                    (Some(bid), Some(ask)) => Some((bid + ask) / 2.0),
                    (Some(bid), None) => Some(bid),
                    (None, Some(ask)) => Some(ask),
                    _ => None,
                }
            }
        }
    }

    /// Returns true if this pool can be used as a flash swap source (hot-potato pattern).
    /// Returns true if this pool can be used as a flash swap source (hot-potato pattern).
    /// Aftermath and FlowX AMM do NOT support flash swaps (sell leg only).
    pub fn supports_flash_swap(&self) -> bool {
        matches!(self.dex, Dex::Cetus | Dex::Turbos | Dex::DeepBook | Dex::FlowxClmm)
    }

    /// How stale this data is (ms since last update).
    pub fn staleness_ms(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.last_updated_ms)
    }
}

/// A pair of pools trading the same token pair on different DEXes.
#[derive(Debug, Clone)]
pub struct PoolPair {
    pub pool_a: PoolState,
    pub pool_b: PoolState,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_pool(dex: Dex) -> PoolState {
        PoolState {
            object_id: "0x1".into(),
            dex,
            coin_type_a: "SUI".into(),
            coin_type_b: "USDC".into(),
            sqrt_price: None,
            tick_index: None,
            liquidity: None,
            fee_rate_bps: None,
            reserve_a: None,
            reserve_b: None,
            best_bid: None,
            best_ask: None,
            last_updated_ms: 1000,
            fee_type: None,
        }
    }

    // ── price_a_in_b tests ──

    #[test]
    fn test_clmm_price_from_sqrt_price() {
        let mut p = base_pool(Dex::Cetus);
        p.sqrt_price = Some(1u128 << 64); // sqrt_price = 1.0 in Q64.64
        p.liquidity = Some(1_000_000_000); // sufficient liquidity
        let price = p.price_a_in_b().unwrap();
        assert!((price - 1.0).abs() < 0.01, "price should be ~1.0, got {price}");
    }

    #[test]
    fn test_clmm_price_sqrt_2() {
        // sqrt(2) * 2^64 ≈ 26087635650665564424 → price ≈ 2.0
        let mut p = base_pool(Dex::Turbos);
        p.sqrt_price = Some(26_087_635_650_665_564_424);
        p.liquidity = Some(1_000_000_000); // sufficient liquidity
        let price = p.price_a_in_b().unwrap();
        assert!((price - 2.0).abs() < 0.01, "price should be ~2.0, got {price}");
    }

    #[test]
    fn test_clmm_price_none_when_no_sqrt() {
        let p = base_pool(Dex::FlowxClmm);
        assert!(p.price_a_in_b().is_none());
    }

    #[test]
    fn test_clmm_price_none_when_zero_liquidity() {
        let mut p = base_pool(Dex::Cetus);
        p.sqrt_price = Some(1u128 << 64);
        p.liquidity = Some(0); // zero liquidity
        assert!(p.price_a_in_b().is_none(), "zero-liquidity pool should return None");
    }

    #[test]
    fn test_clmm_price_none_when_low_liquidity() {
        let mut p = base_pool(Dex::FlowxClmm);
        p.sqrt_price = Some(1u128 << 64);
        p.liquidity = Some(100); // below MIN_CLMM_LIQUIDITY
        assert!(p.price_a_in_b().is_none(), "low-liquidity pool should return None");
    }

    #[test]
    fn test_amm_price_from_reserves() {
        let mut p = base_pool(Dex::Aftermath);
        p.reserve_a = Some(1_000_000_000);
        p.reserve_b = Some(3_000_000);
        let price = p.price_a_in_b().unwrap();
        assert!((price - 0.003).abs() < 0.0001, "got {price}");
    }

    #[test]
    fn test_amm_price_none_when_zero_reserve_a() {
        let mut p = base_pool(Dex::FlowxAmm);
        p.reserve_a = Some(0);
        p.reserve_b = Some(1_000);
        assert!(p.price_a_in_b().is_none());
    }

    #[test]
    fn test_amm_price_none_when_no_reserves() {
        assert!(base_pool(Dex::Aftermath).price_a_in_b().is_none());
    }

    #[test]
    fn test_deepbook_price_midpoint() {
        let mut p = base_pool(Dex::DeepBook);
        p.best_bid = Some(2.0);
        p.best_ask = Some(3.0);
        assert!((p.price_a_in_b().unwrap() - 2.5).abs() < 0.001);
    }

    #[test]
    fn test_deepbook_price_bid_only() {
        let mut p = base_pool(Dex::DeepBook);
        p.best_bid = Some(2.5);
        assert!((p.price_a_in_b().unwrap() - 2.5).abs() < 0.001);
    }

    #[test]
    fn test_deepbook_price_ask_only() {
        let mut p = base_pool(Dex::DeepBook);
        p.best_ask = Some(3.0);
        assert!((p.price_a_in_b().unwrap() - 3.0).abs() < 0.001);
    }

    #[test]
    fn test_deepbook_no_fallback_to_reserves() {
        // DeepBook is a CLOB — vault reserves don't reflect market price.
        // Without bid/ask data, price should be None (not vault ratio).
        let mut p = base_pool(Dex::DeepBook);
        p.reserve_a = Some(1_000);
        p.reserve_b = Some(2_000);
        assert!(
            p.price_a_in_b().is_none(),
            "DeepBook should NOT use vault reserves as price proxy"
        );
    }

    #[test]
    fn test_deepbook_price_none_no_data() {
        assert!(base_pool(Dex::DeepBook).price_a_in_b().is_none());
    }

    // ── supports_flash_swap ──

    #[test]
    fn test_flash_swap_support() {
        assert!(base_pool(Dex::Cetus).supports_flash_swap());
        assert!(base_pool(Dex::Turbos).supports_flash_swap());
        assert!(base_pool(Dex::DeepBook).supports_flash_swap());
        assert!(base_pool(Dex::FlowxClmm).supports_flash_swap());
        assert!(!base_pool(Dex::Aftermath).supports_flash_swap());
        assert!(!base_pool(Dex::FlowxAmm).supports_flash_swap());
    }

    // ── staleness_ms ──

    #[test]
    fn test_staleness_ms() {
        let p = base_pool(Dex::Cetus);
        assert_eq!(p.staleness_ms(5000), 4000);
        assert_eq!(p.staleness_ms(1000), 0);
        assert_eq!(p.staleness_ms(500), 0); // saturating_sub
    }

    // ── Dex Display ──

    #[test]
    fn test_dex_display() {
        assert_eq!(format!("{}", Dex::Cetus), "Cetus");
        assert_eq!(format!("{}", Dex::DeepBook), "DeepBook");
        assert_eq!(format!("{}", Dex::FlowxClmm), "FlowX CLMM");
        assert_eq!(format!("{}", Dex::FlowxAmm), "FlowX AMM");
    }
}
