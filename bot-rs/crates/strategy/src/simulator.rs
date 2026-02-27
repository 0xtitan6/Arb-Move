use anyhow::{Context, Result};
use arb_types::opportunity::ArbOpportunity;
use reqwest::Client;
use serde_json::{json, Value};
use tracing::{debug, warn};

/// Validates arbitrage opportunities via Sui dry-run RPC.
/// This catches issues that local simulation misses:
/// - Actual price impact across tick boundaries
/// - Pool liquidity changes between detection and execution
/// - Gas cost estimation
#[allow(dead_code)]
pub struct DryRunner {
    client: Client,
    rpc_url: String,
    package_id: String,
    sender: String,
    gas_budget: u64,
}

impl DryRunner {
    pub fn new(rpc_url: &str, package_id: &str, sender: &str, gas_budget: u64) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .expect("Failed to create HTTP client"),
            rpc_url: rpc_url.to_string(),
            package_id: package_id.to_string(),
            sender: sender.to_string(),
            gas_budget,
        }
    }

    /// Dry-run a transaction to validate profitability and get gas estimate.
    /// Returns (is_success, gas_cost_mist, error_message).
    pub async fn dry_run_tx(
        &self,
        tx_bytes: &str,
    ) -> Result<DryRunResult> {
        let response = self
            .client
            .post(&self.rpc_url)
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "sui_dryRunTransactionBlock",
                "params": [tx_bytes]
            }))
            .send()
            .await
            .context("Dry-run RPC request failed")?;

        let body: Value = response.json().await.context("Failed to parse dry-run response")?;

        if let Some(error) = body.get("error") {
            return Ok(DryRunResult {
                success: false,
                gas_cost_mist: 0,
                error_message: Some(format!("RPC error: {}", error)),
                events: vec![],
            });
        }

        let result = body.get("result").context("Missing result in dry-run response")?;

        let status = result
            .get("effects")
            .and_then(|e| e.get("status"))
            .and_then(|s| s.get("status"))
            .and_then(|s| s.as_str())
            .unwrap_or("unknown");

        let gas_cost = extract_gas_cost(result);

        let events: Vec<Value> = result
            .get("events")
            .and_then(|e| e.as_array())
            .cloned()
            .unwrap_or_default();

        if status == "success" {
            debug!(gas = %gas_cost, events = %events.len(), "Dry-run succeeded");
            Ok(DryRunResult {
                success: true,
                gas_cost_mist: gas_cost,
                error_message: None,
                events,
            })
        } else {
            let error_msg = result
                .get("effects")
                .and_then(|e| e.get("status"))
                .and_then(|s| s.get("error"))
                .and_then(|e| e.as_str())
                .unwrap_or("Unknown error")
                .to_string();

            warn!(error = %error_msg, "Dry-run failed");
            Ok(DryRunResult {
                success: false,
                gas_cost_mist: gas_cost,
                error_message: Some(error_msg),
                events,
            })
        }
    }

    /// Validate an opportunity by building and dry-running the transaction.
    /// Updates the opportunity with actual gas cost and returns whether it's still profitable.
    pub async fn validate(&self, opp: &mut ArbOpportunity, tx_bytes: &str) -> Result<bool> {
        let result = self.dry_run_tx(tx_bytes).await?;

        opp.estimated_gas = result.gas_cost_mist;
        opp.net_profit = opp.expected_profit as i64 - result.gas_cost_mist as i64;

        if !result.success {
            debug!(
                strategy = ?opp.strategy,
                error = ?result.error_message,
                "Opportunity failed dry-run"
            );
            return Ok(false);
        }

        // Parse ArbExecuted event to get actual profit
        for event in &result.events {
            if let Some(event_type) = event.get("type").and_then(|t| t.as_str()) {
                if event_type.contains("ArbExecuted") {
                    if let Some(parsed) = event.get("parsedJson") {
                        if let Some(profit) = parsed.get("profit").and_then(|p| p.as_str()) {
                            if let Ok(actual_profit) = profit.parse::<u64>() {
                                opp.expected_profit = actual_profit;
                                opp.net_profit =
                                    actual_profit as i64 - result.gas_cost_mist as i64;
                            }
                        }
                    }
                }
            }
        }

        Ok(opp.is_profitable())
    }
}

/// Result of a dry-run execution.
#[derive(Debug)]
pub struct DryRunResult {
    pub success: bool,
    pub gas_cost_mist: u64,
    pub error_message: Option<String>,
    pub events: Vec<Value>,
}

/// Extract total gas cost from dry-run effects.
fn extract_gas_cost(result: &Value) -> u64 {
    let effects = match result.get("effects") {
        Some(e) => e,
        None => return 0,
    };

    let gas_used = match effects.get("gasUsed") {
        Some(g) => g,
        None => return 0,
    };

    let computation = gas_used
        .get("computationCost")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    let storage = gas_used
        .get("storageCost")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    let rebate = gas_used
        .get("storageRebate")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    computation + storage - rebate.min(computation + storage)
}
