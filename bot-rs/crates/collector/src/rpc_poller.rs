use anyhow::{Context, Result};
use arb_types::config::Config;
use reqwest::Client;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time;
use tracing::{debug, error, info, warn};

use crate::parsers;
use crate::pool_cache::PoolCache;

/// Polls Sui RPC for pool object state at a configurable interval.
/// Parses the response into PoolState and updates the shared cache.
pub struct RpcPoller {
    client: Client,
    rpc_url: String,
    poll_interval: Duration,
    pool_ids: Vec<PoolMeta>,
}

/// Metadata for a pool to poll.
#[derive(Debug, Clone)]
pub struct PoolMeta {
    pub object_id: String,
    pub dex: String,
    pub coin_type_a: String,
    pub coin_type_b: String,
}

impl RpcPoller {
    pub fn new(config: &Config) -> Self {
        let pool_ids: Vec<PoolMeta> = config
            .monitored_pools
            .iter()
            .map(|p| PoolMeta {
                object_id: p.pool_id.clone(),
                dex: p.dex.clone(),
                coin_type_a: p.coin_type_a.clone(),
                coin_type_b: p.coin_type_b.clone(),
            })
            .collect();

        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .expect("Failed to create HTTP client"),
            rpc_url: config.rpc_url.clone(),
            poll_interval: Duration::from_millis(config.poll_interval_ms),
            pool_ids,
        }
    }

    /// Run the polling loop. Updates `cache` with fresh pool states.
    /// Bumps `heartbeat` on every successful fetch so the strategy loop knows we're alive.
    /// This function runs forever (until the task is cancelled).
    ///
    /// Uses `sui_multiGetObjects` to batch-fetch all pools in a single RPC call,
    /// dramatically reducing rate-limit pressure vs individual fetches.
    pub async fn run(&self, cache: PoolCache, heartbeat: Arc<AtomicU64>) -> Result<()> {
        info!(
            "Starting RPC poller: {} pools, {}ms interval (batch mode)",
            self.pool_ids.len(),
            self.poll_interval.as_millis()
        );

        let mut interval = time::interval(self.poll_interval);

        loop {
            interval.tick().await;

            match self.batch_fetch_all(&cache).await {
                Ok(updated) => {
                    if updated > 0 {
                        heartbeat.store(now_ms(), Ordering::Relaxed);
                    }
                    debug!(updated = updated, total = self.pool_ids.len(), "Batch poll cycle complete");
                }
                Err(e) => {
                    warn!(error = %e, "Batch fetch failed, will retry next cycle");
                }
            }
        }
    }

    /// Batch-fetch all pool objects in a single `sui_multiGetObjects` RPC call.
    /// Returns the number of pools successfully updated.
    async fn batch_fetch_all(&self, cache: &PoolCache) -> Result<usize> {
        let object_ids: Vec<&str> = self.pool_ids.iter().map(|m| m.object_id.as_str()).collect();

        let response = self
            .client
            .post(&self.rpc_url)
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "sui_multiGetObjects",
                "params": [
                    object_ids,
                    {
                        "showContent": true,
                        "showType": true,
                    }
                ]
            }))
            .send()
            .await
            .context("Batch RPC request failed")?;

        let body: Value = response.json().await.context("Failed to parse batch RPC response")?;

        if let Some(error) = body.get("error") {
            anyhow::bail!("RPC error: {}", error);
        }

        let results = body
            .get("result")
            .and_then(|r| r.as_array())
            .context("Invalid multiGetObjects response")?;

        let ts = now_ms();
        let mut updated = 0usize;

        for (i, obj) in results.iter().enumerate() {
            let meta = match self.pool_ids.get(i) {
                Some(m) => m,
                None => continue,
            };

            // Check for object-level error
            if let Some(obj_error) = obj.get("error") {
                let code = obj_error
                    .get("code")
                    .and_then(|c| c.as_str())
                    .unwrap_or("unknown");
                warn!(pool = %meta.object_id, dex = %meta.dex, error = %code, "Object error");
                continue;
            }

            let data = match obj.get("data") {
                Some(d) => d,
                None => continue,
            };

            let raw_content = match data.get("content") {
                Some(c) => c,
                None => continue,
            };

            // DeepBook V3 Versioned pools need a second RPC call
            let content = if meta.dex.to_lowercase() == "deepbook"
                && is_deepbook_versioned(raw_content)
            {
                match unwrap_deepbook_versioned(&self.client, &self.rpc_url, raw_content).await {
                    Ok(inner) => inner,
                    Err(e) => {
                        warn!(pool = %meta.object_id, error = %e, "DeepBook V3 unwrap failed");
                        continue;
                    }
                }
            } else {
                raw_content.clone()
            };

            match parsers::parse_pool_object(&content, &meta.dex, meta, ts) {
                Ok(mut state) => {
                    // Extract Turbos fee type from on-chain object type.
                    // Pool<A, B, Fee> → Fee is the 3rd type parameter.
                    if meta.dex.to_lowercase() == "turbos" {
                        if let Some(type_str) = data.get("type").and_then(|t| t.as_str()) {
                            state.fee_type = extract_third_type_param(type_str);
                        }
                    }
                    cache.upsert(meta.object_id.clone(), state);
                    updated += 1;
                }
                Err(e) => {
                    warn!(pool = %meta.object_id, dex = %meta.dex, error = %e, "Parse failed");
                }
            }
        }

        Ok(updated)
    }

}

/// Seed the cache with initial pool states via multi-get.
pub async fn seed_cache(config: &Config, cache: &PoolCache) -> Result<()> {
    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;

    let object_ids: Vec<&str> = config
        .monitored_pools
        .iter()
        .map(|p| p.pool_id.as_str())
        .collect();

    if object_ids.is_empty() {
        warn!("No pools configured for monitoring");
        return Ok(());
    }

    info!("Seeding pool cache with {} pools...", object_ids.len());

    // Use sui_multiGetObjects for batch fetching
    let response = client
        .post(&config.rpc_url)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sui_multiGetObjects",
            "params": [
                object_ids,
                {
                    "showContent": true,
                    "showType": true,
                }
            ]
        }))
        .send()
        .await
        .context("Failed to seed pool cache")?;

    let body: Value = response.json().await?;
    let results = body
        .get("result")
        .and_then(|r| r.as_array())
        .context("Invalid multiGetObjects response")?;

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    for (i, obj) in results.iter().enumerate() {
        if let Some(pool_config) = config.monitored_pools.get(i) {
            let meta = PoolMeta {
                object_id: pool_config.pool_id.clone(),
                dex: pool_config.dex.clone(),
                coin_type_a: pool_config.coin_type_a.clone(),
                coin_type_b: pool_config.coin_type_b.clone(),
            };

            // Check for object-level error (e.g. notExists)
            if let Some(obj_error) = obj.get("error") {
                let code = obj_error
                    .get("code")
                    .and_then(|c| c.as_str())
                    .unwrap_or("unknown");
                error!(
                    pool = %meta.object_id,
                    dex = %meta.dex,
                    error = %code,
                    "Pool object does not exist on-chain — check pool ID"
                );
                continue;
            }

            if let Some(data) = obj.get("data") {
                if let Some(raw_content) = data.get("content") {
                    // DeepBook V3 wraps pool data in Versioned — needs second fetch
                    let content = if meta.dex.to_lowercase() == "deepbook"
                        && is_deepbook_versioned(raw_content)
                    {
                        debug!(pool = %meta.object_id, "DeepBook V3 Versioned detected, fetching inner object");
                        match unwrap_deepbook_versioned(&client, &config.rpc_url, raw_content)
                            .await
                        {
                            Ok(inner) => inner,
                            Err(e) => {
                                error!(
                                    pool = %meta.object_id,
                                    error = %e,
                                    "Failed to unwrap DeepBook V3 Versioned pool"
                                );
                                continue;
                            }
                        }
                    } else {
                        raw_content.clone()
                    };

                    match parsers::parse_pool_object(&content, &meta.dex, &meta, now_ms) {
                        Ok(mut state) => {
                            // Extract Turbos fee type from on-chain object type
                            if meta.dex.to_lowercase() == "turbos" {
                                if let Some(type_str) = data.get("type").and_then(|t| t.as_str()) {
                                    state.fee_type = extract_third_type_param(type_str);
                                    if let Some(ref ft) = state.fee_type {
                                        debug!(pool = %meta.object_id, fee_type = %ft, "Turbos fee type extracted");
                                    }
                                }
                            }
                            info!(
                                pool = %meta.object_id,
                                dex = %meta.dex,
                                "Seeded pool state"
                            );
                            cache.upsert(meta.object_id.clone(), state);
                        }
                        Err(e) => {
                            error!(pool = %meta.object_id, error = %e, "Failed to parse pool");
                        }
                    }
                }
            }
        }
    }

    info!("Pool cache seeded: {} pools", cache.len());
    Ok(())
}

/// Extract the 3rd type parameter from a Sui Move type string.
///
/// For Turbos pools, the object type looks like:
///   `0x91bfbc...::pool::Pool<CoinA, CoinB, 0x91bfbc...::fee3000bps::FEE3000BPS>`
///
/// This function returns the 3rd parameter (the fee type).
/// Splits on `, ` which works for non-nested generic types.
fn extract_third_type_param(type_str: &str) -> Option<String> {
    let open = type_str.find('<')?;
    let close = type_str.rfind('>')?;
    let inner = &type_str[open + 1..close];
    // Split on ", " to separate type parameters
    let parts: Vec<&str> = inner.split(", ").collect();
    if parts.len() >= 3 {
        Some(parts[2].to_string())
    } else {
        None
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Check if a DeepBook content object is a V3 Versioned wrapper.
/// V3 pools have an `inner` field (the Versioned object) but no direct `base_vault`.
fn is_deepbook_versioned(content: &Value) -> bool {
    content
        .get("fields")
        .map(|f| f.get("inner").is_some() && f.get("base_vault").is_none())
        .unwrap_or(false)
}

/// For DeepBook V3 pools wrapped in `0x2::versioned::Versioned`:
/// extract the inner object ID and fetch the PoolInner via `suix_getDynamicFieldObject`.
///
/// The outer pool has: content.fields.inner.fields.id.id → inner versioned object ID
/// The PoolInner is stored as a dynamic field on that inner object with key {type: "u64", value: "1"}.
/// The dynamic field response wraps the actual data: content.fields.value = PoolInner { fields: ... }
async fn unwrap_deepbook_versioned(
    client: &Client,
    rpc_url: &str,
    content: &Value,
) -> Result<Value> {
    let inner_id = content
        .get("fields")
        .and_then(|f| f.get("inner"))
        .and_then(|i| i.get("fields"))
        .and_then(|f| f.get("id"))
        .and_then(|id| id.get("id"))
        .and_then(|id| id.as_str())
        .context("Missing inner versioned object ID in DeepBook V3 pool")?;

    debug!(inner_id = %inner_id, "Fetching DeepBook V3 PoolInner dynamic field");

    let response = client
        .post(rpc_url)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "suix_getDynamicFieldObject",
            "params": [
                inner_id,
                {
                    "type": "u64",
                    "value": "1"
                }
            ]
        }))
        .send()
        .await
        .context("Failed to fetch DeepBook V3 inner object")?;

    let body: Value = response.json().await?;

    if let Some(error) = body.get("error") {
        anyhow::bail!("RPC error fetching DeepBook V3 inner: {}", error);
    }

    let result = body
        .get("result")
        .context("Missing result for DeepBook V3 inner")?;

    if let Some(obj_error) = result.get("error") {
        let code = obj_error
            .get("code")
            .and_then(|c| c.as_str())
            .unwrap_or("unknown");
        anyhow::bail!("DeepBook V3 inner object error: {}", code);
    }

    let data = result
        .get("data")
        .context("Missing result.data for DeepBook V3 inner")?;

    let inner_content = data
        .get("content")
        .context("Missing content in DeepBook V3 inner")?;

    // Dynamic field wraps as: content.fields.value = PoolInner { type: "...", fields: { base_vault, ... } }
    // Return fields.value as the new "content" for the parser (it has a `fields` key)
    inner_content
        .get("fields")
        .and_then(|f| f.get("value"))
        .cloned()
        .context("Missing value in DeepBook V3 dynamic field response")
}
