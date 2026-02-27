use anyhow::{Context, Result};
use arb_types::config::Config;
use reqwest::Client;
use serde_json::{json, Value};
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
    /// This function runs forever (until the task is cancelled).
    pub async fn run(&self, cache: PoolCache) -> Result<()> {
        info!(
            "Starting RPC poller: {} pools, {}ms interval",
            self.pool_ids.len(),
            self.poll_interval.as_millis()
        );

        let mut interval = time::interval(self.poll_interval);

        loop {
            interval.tick().await;

            for meta in &self.pool_ids {
                match self.fetch_and_parse(meta).await {
                    Ok(state) => {
                        debug!(pool = %meta.object_id, dex = %meta.dex, "Updated pool state");
                        cache.upsert(meta.object_id.clone(), state);
                    }
                    Err(e) => {
                        warn!(pool = %meta.object_id, error = %e, "Failed to fetch pool state");
                    }
                }
            }
        }
    }

    /// Fetch a single pool object via `sui_getObject` and parse it.
    async fn fetch_and_parse(&self, meta: &PoolMeta) -> Result<arb_types::pool::PoolState> {
        let response = self
            .client
            .post(&self.rpc_url)
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "sui_getObject",
                "params": [
                    meta.object_id,
                    {
                        "showContent": true,
                        "showType": true,
                    }
                ]
            }))
            .send()
            .await
            .context("RPC request failed")?;

        let body: Value = response.json().await.context("Failed to parse RPC response")?;

        if let Some(error) = body.get("error") {
            anyhow::bail!("RPC error: {}", error);
        }

        let data = body
            .get("result")
            .and_then(|r| r.get("data"))
            .context("Missing result.data in response")?;

        let content = data
            .get("content")
            .context("Missing content in object data")?;

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        parsers::parse_pool_object(content, meta.dex.as_str(), meta, now_ms)
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

            if let Some(data) = obj.get("data") {
                if let Some(content) = data.get("content") {
                    match parsers::parse_pool_object(content, &meta.dex, &meta, now_ms) {
                        Ok(state) => {
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
