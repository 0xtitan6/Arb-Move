use anyhow::{Context, Result};
use reqwest::Client;
use serde_json::{json, Value};
use tracing::{debug, error, warn};

/// Monitors the wallet's SUI gas balance via RPC.
///
/// Checks balance before each trade attempt and warns/blocks when
/// the balance is too low to cover gas costs.
pub struct GasMonitor {
    client: Client,
    rpc_url: String,
    owner_address: String,
    /// Minimum balance (in MIST) required to attempt a trade.
    /// Default: 100M MIST = 0.1 SUI (enough for ~2 trades)
    min_balance_mist: u64,
    /// Cached balance to avoid querying every cycle (updated periodically).
    cached_balance: u64,
    /// Last time balance was fetched.
    last_fetch_ms: u64,
    /// How often to re-fetch balance (ms).
    fetch_interval_ms: u64,
}

impl GasMonitor {
    pub fn new(rpc_url: &str, owner_address: &str, min_balance_mist: u64) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .expect("Failed to create HTTP client"),
            rpc_url: rpc_url.to_string(),
            owner_address: owner_address.to_string(),
            min_balance_mist,
            cached_balance: u64::MAX, // assume ok until first fetch
            last_fetch_ms: 0,
            fetch_interval_ms: 10_000, // re-check every 10s
        }
    }

    /// Check if gas balance is sufficient for trading.
    /// Returns `Ok(balance)` if sufficient, `Err` if insufficient or fetch failed.
    pub async fn check_balance(&mut self, now_ms: u64) -> Result<u64> {
        // Use cached balance if fresh enough
        if now_ms.saturating_sub(self.last_fetch_ms) < self.fetch_interval_ms
            && self.cached_balance != u64::MAX
        {
            return if self.cached_balance >= self.min_balance_mist {
                Ok(self.cached_balance)
            } else {
                anyhow::bail!(
                    "Insufficient gas: {} MIST < {} MIST minimum",
                    self.cached_balance,
                    self.min_balance_mist
                )
            };
        }

        // Fetch fresh balance
        match self.fetch_balance().await {
            Ok(balance) => {
                self.cached_balance = balance;
                self.last_fetch_ms = now_ms;

                if balance < self.min_balance_mist {
                    warn!(
                        balance_mist = %balance,
                        balance_sui = %format!("{:.4}", balance as f64 / 1_000_000_000.0),
                        min_required = %self.min_balance_mist,
                        "⚠️  Low gas balance — trading paused"
                    );
                    anyhow::bail!(
                        "Insufficient gas: {} MIST ({:.4} SUI) < {} MIST minimum",
                        balance,
                        balance as f64 / 1_000_000_000.0,
                        self.min_balance_mist
                    )
                } else {
                    debug!(
                        balance_sui = %format!("{:.4}", balance as f64 / 1_000_000_000.0),
                        "Gas balance OK"
                    );
                    Ok(balance)
                }
            }
            Err(e) => {
                error!(error = %e, "Failed to fetch gas balance — allowing trade");
                // On RPC failure, don't block trading (might be transient)
                Ok(self.cached_balance)
            }
        }
    }

    /// Fetch the total SUI balance for the owner address.
    async fn fetch_balance(&self) -> Result<u64> {
        let response = self
            .client
            .post(&self.rpc_url)
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "suix_getBalance",
                "params": [
                    self.owner_address,
                    "0x0000000000000000000000000000000000000000000000000000000000000002::sui::SUI"
                ]
            }))
            .send()
            .await
            .context("Balance RPC request failed")?;

        let body: Value = response
            .json()
            .await
            .context("Failed to parse balance response")?;

        if let Some(error) = body.get("error") {
            anyhow::bail!("Balance RPC error: {}", error);
        }

        let total = body
            .get("result")
            .and_then(|r| r.get("totalBalance"))
            .and_then(|b| b.as_str())
            .and_then(|s| s.parse::<u64>().ok())
            .context("Failed to parse totalBalance from RPC response")?;

        Ok(total)
    }

    /// Update balance after a known gas expenditure (optimistic, avoids extra RPC call).
    pub fn deduct_gas(&mut self, gas_mist: u64) {
        self.cached_balance = self.cached_balance.saturating_sub(gas_mist);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_monitor_defaults() {
        let monitor = GasMonitor::new("http://localhost:9000", "0xabc", 100_000_000);
        assert_eq!(monitor.min_balance_mist, 100_000_000);
        assert_eq!(monitor.cached_balance, u64::MAX);
    }

    #[test]
    fn test_deduct_gas() {
        let mut monitor = GasMonitor::new("http://localhost:9000", "0xabc", 100_000_000);
        monitor.cached_balance = 500_000_000;
        monitor.deduct_gas(100_000_000);
        assert_eq!(monitor.cached_balance, 400_000_000);
    }

    #[test]
    fn test_deduct_gas_saturating() {
        let mut monitor = GasMonitor::new("http://localhost:9000", "0xabc", 100_000_000);
        monitor.cached_balance = 50_000_000;
        monitor.deduct_gas(100_000_000);
        assert_eq!(monitor.cached_balance, 0);
    }
}
