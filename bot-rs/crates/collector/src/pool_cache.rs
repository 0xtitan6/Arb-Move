use arb_types::pool::PoolState;
use dashmap::DashMap;
use std::sync::Arc;

/// Thread-safe cache of pool states, keyed by pool object ID.
/// Updated by the collector, read by the strategy scanner.
#[derive(Debug, Clone)]
pub struct PoolCache {
    inner: Arc<DashMap<String, PoolState>>,
}

impl PoolCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(DashMap::new()),
        }
    }

    /// Insert or update a pool state.
    pub fn upsert(&self, pool_id: String, state: PoolState) {
        self.inner.insert(pool_id, state);
    }

    /// Get a snapshot of a specific pool's state.
    pub fn get(&self, pool_id: &str) -> Option<PoolState> {
        self.inner.get(pool_id).map(|r| r.value().clone())
    }

    /// Get a snapshot of all pool states.
    pub fn snapshot(&self) -> Vec<PoolState> {
        self.inner.iter().map(|r| r.value().clone()).collect()
    }

    /// Number of pools in the cache.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Remove a pool from the cache.
    pub fn remove(&self, pool_id: &str) -> Option<PoolState> {
        self.inner.remove(pool_id).map(|(_, v)| v)
    }

    /// Get all pools for a specific token pair (in either order).
    pub fn pools_for_pair(&self, coin_a: &str, coin_b: &str) -> Vec<PoolState> {
        self.inner
            .iter()
            .filter(|r| {
                let p = r.value();
                (p.coin_type_a == coin_a && p.coin_type_b == coin_b)
                    || (p.coin_type_a == coin_b && p.coin_type_b == coin_a)
            })
            .map(|r| r.value().clone())
            .collect()
    }
}

impl Default for PoolCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arb_types::pool::Dex;

    fn make_pool(id: &str, dex: Dex, coin_a: &str, coin_b: &str) -> PoolState {
        PoolState {
            object_id: id.to_string(),
            dex,
            coin_type_a: coin_a.to_string(),
            coin_type_b: coin_b.to_string(),
            sqrt_price: Some(1u128 << 64),
            tick_index: Some(0),
            liquidity: Some(1_000_000),
            fee_rate_bps: Some(3000),
            reserve_a: None,
            reserve_b: None,
            best_bid: None,
            best_ask: None,
            last_updated_ms: 0,
        }
    }

    #[test]
    fn test_upsert_and_get() {
        let cache = PoolCache::new();
        let pool = make_pool("0xabc", Dex::Cetus, "SUI", "USDC");
        cache.upsert("0xabc".to_string(), pool);
        assert_eq!(cache.len(), 1);
        let got = cache.get("0xabc").unwrap();
        assert_eq!(got.object_id, "0xabc");
    }

    #[test]
    fn test_pools_for_pair() {
        let cache = PoolCache::new();
        cache.upsert(
            "0x1".to_string(),
            make_pool("0x1", Dex::Cetus, "SUI", "USDC"),
        );
        cache.upsert(
            "0x2".to_string(),
            make_pool("0x2", Dex::Turbos, "SUI", "USDC"),
        );
        cache.upsert(
            "0x3".to_string(),
            make_pool("0x3", Dex::Cetus, "SUI", "WETH"),
        );

        let pairs = cache.pools_for_pair("SUI", "USDC");
        assert_eq!(pairs.len(), 2);

        // Reverse order also works
        let pairs_rev = cache.pools_for_pair("USDC", "SUI");
        assert_eq!(pairs_rev.len(), 2);
    }
}
