use anyhow::{Context, Result};
use reqwest::Client;
use serde_json::{json, Value};
use tracing::{error, info, warn};

/// Submits signed transactions to the Sui network with retry logic.
pub struct Submitter {
    client: Client,
    rpc_url: String,
    max_retries: u32,
}

/// Result of a transaction submission.
#[derive(Debug)]
pub struct SubmitResult {
    pub digest: String,
    pub success: bool,
    pub gas_cost_mist: u64,
    pub profit_mist: Option<u64>,
    pub error_message: Option<String>,
}

impl Submitter {
    pub fn new(rpc_url: &str) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("Failed to create HTTP client"),
            rpc_url: rpc_url.to_string(),
            max_retries: 2,
        }
    }

    /// Submit a signed transaction and wait for execution.
    pub async fn submit(
        &self,
        tx_bytes: &str,
        signature: &str,
    ) -> Result<SubmitResult> {
        let mut last_error = String::new();

        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                warn!(attempt = %attempt, "Retrying transaction submission");
                tokio::time::sleep(std::time::Duration::from_millis(200 * attempt as u64)).await;
            }

            match self.submit_once(tx_bytes, signature).await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    last_error = e.to_string();
                    error!(attempt = %attempt, error = %last_error, "Submission failed");
                }
            }
        }

        anyhow::bail!("Transaction submission failed after {} retries: {}", self.max_retries, last_error)
    }

    async fn submit_once(
        &self,
        tx_bytes: &str,
        signature: &str,
    ) -> Result<SubmitResult> {
        let response = self
            .client
            .post(&self.rpc_url)
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "sui_executeTransactionBlock",
                "params": [
                    tx_bytes,
                    [signature],
                    {
                        "showEffects": true,
                        "showEvents": true,
                    },
                    "WaitForLocalExecution"
                ]
            }))
            .send()
            .await
            .context("Failed to submit transaction")?;

        let body: Value = response.json().await.context("Failed to parse submission response")?;

        if let Some(error) = body.get("error") {
            anyhow::bail!("RPC error: {}", error);
        }

        let result = body.get("result").context("Missing result")?;

        let digest = result
            .get("digest")
            .and_then(|d| d.as_str())
            .unwrap_or("unknown")
            .to_string();

        let effects = result.get("effects");
        let status = effects
            .and_then(|e| e.get("status"))
            .and_then(|s| s.get("status"))
            .and_then(|s| s.as_str())
            .unwrap_or("unknown");

        let gas_cost = effects
            .and_then(|e| e.get("gasUsed"))
            .map(|g| {
                let comp = g.get("computationCost")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(0);
                let storage = g.get("storageCost")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(0);
                let rebate = g.get("storageRebate")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(0);
                comp + storage - rebate.min(comp + storage)
            })
            .unwrap_or(0);

        // Parse ArbExecuted event for actual profit
        let profit = result
            .get("events")
            .and_then(|e| e.as_array())
            .and_then(|events| {
                events.iter().find_map(|ev| {
                    let event_type = ev.get("type")?.as_str()?;
                    if event_type.contains("ArbExecuted") {
                        ev.get("parsedJson")
                            .and_then(|p| p.get("profit"))
                            .and_then(|p| p.as_str())
                            .and_then(|s| s.parse::<u64>().ok())
                    } else {
                        None
                    }
                })
            });

        let success = status == "success";

        if success {
            info!(
                digest = %digest,
                gas = %gas_cost,
                profit = ?profit,
                "Transaction executed successfully"
            );
        } else {
            let error_msg = effects
                .and_then(|e| e.get("status"))
                .and_then(|s| s.get("error"))
                .and_then(|e| e.as_str())
                .unwrap_or("Unknown error");
            warn!(digest = %digest, error = %error_msg, "Transaction failed on-chain");
        }

        Ok(SubmitResult {
            digest,
            success,
            gas_cost_mist: gas_cost,
            profit_mist: profit,
            error_message: if success {
                None
            } else {
                Some(
                    effects
                        .and_then(|e| e.get("status"))
                        .and_then(|s| s.get("error"))
                        .and_then(|e| e.as_str())
                        .unwrap_or("Unknown error")
                        .to_string(),
                )
            },
        })
    }
}
