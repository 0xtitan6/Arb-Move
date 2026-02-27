use anyhow::{Context, Result};
use reqwest::Client;
use serde_json::{json, Value};
use tracing::{debug, error, info};

/// Periodically merges fragmented `Coin<SUI>` objects to prevent
/// hitting Sui's per-transaction object limits.
///
/// After many trades, gas rebates and profit transfers create numerous
/// small coin objects. This merger consolidates them via `unsafe_payAllSui`.
pub struct CoinMerger {
    client: Client,
    rpc_url: String,
    owner_address: String,
    /// Merge when coin count exceeds this threshold.
    merge_threshold: usize,
    /// Track cycles to only check periodically.
    cycle_count: u64,
    /// How often (in strategy cycles) to check coin count.
    check_interval_cycles: u64,
    /// Gas budget for merge transaction (MIST).
    merge_gas_budget: u64,
}

impl CoinMerger {
    pub fn new(rpc_url: &str, owner_address: &str) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("Failed to create HTTP client"),
            rpc_url: rpc_url.to_string(),
            owner_address: owner_address.to_string(),
            merge_threshold: 20,
            cycle_count: 0,
            check_interval_cycles: 100, // ~50s at 500ms tick
            merge_gas_budget: 10_000_000, // 0.01 SUI
        }
    }

    /// Call this every strategy cycle. Returns `Some(tx_bytes_base64)` when
    /// a merge is needed, or `None` if no action required.
    ///
    /// The caller is responsible for signing and submitting the returned tx.
    pub async fn maybe_merge(&mut self) -> Result<Option<String>> {
        self.cycle_count += 1;

        // Only check periodically to avoid spamming RPC
        if self.cycle_count % self.check_interval_cycles != 0 {
            return Ok(None);
        }

        // Query coin count
        let coins = self.fetch_sui_coins().await?;
        let coin_count = coins.len();

        if coin_count <= self.merge_threshold {
            debug!(
                coin_count = %coin_count,
                threshold = %self.merge_threshold,
                "Coin count OK — no merge needed"
            );
            return Ok(None);
        }

        info!(
            coin_count = %coin_count,
            threshold = %self.merge_threshold,
            "Too many Coin<SUI> objects — merging"
        );

        // Collect all coin object IDs
        let coin_ids: Vec<String> = coins
            .iter()
            .filter_map(|c| c.get("coinObjectId").and_then(|id| id.as_str()))
            .map(|s| s.to_string())
            .collect();

        if coin_ids.is_empty() {
            return Ok(None);
        }

        // Build merge transaction via unsafe_payAllSui
        match self.build_merge_tx(&coin_ids).await {
            Ok(tx_bytes) => Ok(Some(tx_bytes)),
            Err(e) => {
                error!(error = %e, "Failed to build merge transaction");
                Err(e)
            }
        }
    }

    /// Fetch all Coin<SUI> objects owned by the wallet.
    async fn fetch_sui_coins(&self) -> Result<Vec<Value>> {
        let mut all_coins = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let params = if let Some(ref c) = cursor {
                json!([
                    self.owner_address,
                    "0x0000000000000000000000000000000000000000000000000000000000000002::sui::SUI",
                    c,
                    50
                ])
            } else {
                json!([
                    self.owner_address,
                    "0x0000000000000000000000000000000000000000000000000000000000000002::sui::SUI",
                    null,
                    50
                ])
            };

            let response = self
                .client
                .post(&self.rpc_url)
                .json(&json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "suix_getCoins",
                    "params": params
                }))
                .send()
                .await
                .context("suix_getCoins request failed")?;

            let body: Value = response
                .json()
                .await
                .context("Failed to parse getCoins response")?;

            if let Some(error) = body.get("error") {
                anyhow::bail!("suix_getCoins error: {}", error);
            }

            let result = body.get("result").context("Missing result in getCoins")?;

            if let Some(data) = result.get("data").and_then(|d| d.as_array()) {
                all_coins.extend(data.clone());
            }

            // Check for pagination
            let has_next = result
                .get("hasNextPage")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if has_next {
                cursor = result
                    .get("nextCursor")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
            } else {
                break;
            }
        }

        Ok(all_coins)
    }

    /// Build a merge transaction using unsafe_payAllSui.
    /// Returns base64-encoded tx_bytes ready for signing.
    async fn build_merge_tx(&self, coin_ids: &[String]) -> Result<String> {
        let response = self
            .client
            .post(&self.rpc_url)
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "unsafe_payAllSui",
                "params": [
                    self.owner_address,       // signer
                    coin_ids,                 // input_coins (all SUI coins)
                    self.owner_address,       // recipient (self — just merging)
                    self.merge_gas_budget     // gas_budget
                ]
            }))
            .send()
            .await
            .context("unsafe_payAllSui request failed")?;

        let body: Value = response
            .json()
            .await
            .context("Failed to parse payAllSui response")?;

        if let Some(error) = body.get("error") {
            anyhow::bail!("unsafe_payAllSui error: {}", error);
        }

        let tx_bytes = body
            .get("result")
            .and_then(|r| r.get("txBytes"))
            .and_then(|b| b.as_str())
            .context("Missing txBytes in payAllSui response")?
            .to_string();

        Ok(tx_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_defaults() {
        let merger = CoinMerger::new("http://localhost:9000", "0xabc");
        assert_eq!(merger.merge_threshold, 20);
        assert_eq!(merger.check_interval_cycles, 100);
        assert_eq!(merger.merge_gas_budget, 10_000_000);
        assert_eq!(merger.cycle_count, 0);
    }

    #[tokio::test]
    async fn test_skips_non_interval_cycles() {
        let mut merger = CoinMerger::new("http://invalid:9999", "0xabc");

        // Cycles 1-99 should all return Ok(None) without any RPC call
        for i in 1..=99 {
            let result = merger.maybe_merge().await;
            assert!(result.is_ok(), "cycle {} should succeed", i);
            assert!(result.unwrap().is_none(), "cycle {} should skip", i);
        }
        assert_eq!(merger.cycle_count, 99);
    }

    #[tokio::test]
    async fn test_rpc_failure_on_interval_cycle() {
        let mut merger = CoinMerger::new("http://invalid:9999", "0xabc");
        // Jump to cycle 99 so next call is cycle 100 (triggers RPC)
        merger.cycle_count = 99;

        // Cycle 100 triggers RPC which fails because URL is invalid
        let result = merger.maybe_merge().await;
        assert!(result.is_err(), "should fail on invalid RPC URL");
        assert_eq!(merger.cycle_count, 100);
    }

    #[test]
    fn test_cycle_interval_logic() {
        let merger = CoinMerger::new("http://localhost:9000", "0xabc");
        // Verify that check_interval_cycles divides evenly
        assert_eq!(100 % merger.check_interval_cycles, 0);
        assert_eq!(200 % merger.check_interval_cycles, 0);
        assert_ne!(99 % merger.check_interval_cycles, 0);
    }
}
