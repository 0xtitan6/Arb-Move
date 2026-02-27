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
}

impl PoolState {
    /// Compute the effective price of A in terms of B.
    /// For CLMM: derived from sqrt_price.
    /// For AMM: reserve_b / reserve_a.
    /// For CLOB: midpoint of bid/ask.
    pub fn price_a_in_b(&self) -> Option<f64> {
        match self.dex {
            Dex::Cetus | Dex::Turbos | Dex::FlowxClmm => {
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
                match (self.best_bid, self.best_ask) {
                    (Some(bid), Some(ask)) => Some((bid + ask) / 2.0),
                    (Some(bid), None) => Some(bid),
                    (None, Some(ask)) => Some(ask),
                    _ => {
                        // Fallback: use vault reserves as rough price proxy
                        match (self.reserve_a, self.reserve_b) {
                            (Some(a), Some(b)) if a > 0 => Some(b as f64 / a as f64),
                            _ => None,
                        }
                    }
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
