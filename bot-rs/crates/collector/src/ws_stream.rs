use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info, warn};

use crate::parsers;
use crate::pool_cache::PoolCache;
use crate::rpc_poller::PoolMeta;

/// Streams real-time pool state updates via Sui WebSocket subscriptions.
///
/// Uses `suix_subscribeEvent` to listen for swap events from monitored DEX
/// packages. When events arrive, we re-fetch the affected pool object via
/// RPC to get the latest state, then update the cache.
///
/// This provides ~400ms latency (Sui finality) vs ~500ms+ with polling.
pub struct WsStream {
    ws_url: String,
    rpc_url: String,
    /// DEX package IDs to subscribe to swap events from
    dex_packages: Vec<DexPackage>,
    /// Pool metadata indexed by object ID for quick lookup
    pool_metas: Vec<PoolMeta>,
}

/// A DEX package to subscribe to events from.
#[derive(Debug, Clone)]
pub struct DexPackage {
    pub package_id: String,
    pub dex_name: String,
}

impl WsStream {
    pub fn new(
        ws_url: &str,
        rpc_url: &str,
        dex_packages: Vec<DexPackage>,
        pool_metas: Vec<PoolMeta>,
    ) -> Self {
        Self {
            ws_url: ws_url.to_string(),
            rpc_url: rpc_url.to_string(),
            dex_packages,
            pool_metas,
        }
    }

    /// Derive the WebSocket URL from an HTTP RPC URL.
    /// e.g., `https://fullnode.mainnet.sui.io:443` → `wss://fullnode.mainnet.sui.io:443`
    pub fn ws_url_from_rpc(rpc_url: &str) -> String {
        rpc_url
            .replace("https://", "wss://")
            .replace("http://", "ws://")
    }

    /// Run the WebSocket event stream. Updates `cache` with fresh pool states.
    /// Automatically reconnects on disconnect.
    pub async fn run(&self, cache: PoolCache) -> Result<()> {
        info!(
            ws_url = %self.ws_url,
            packages = %self.dex_packages.len(),
            pools = %self.pool_metas.len(),
            "Starting WebSocket event stream"
        );

        loop {
            match self.connect_and_stream(&cache).await {
                Ok(()) => {
                    info!("WebSocket stream ended normally");
                    break;
                }
                Err(e) => {
                    error!(error = %e, "WebSocket stream error, reconnecting in 3s...");
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                }
            }
        }

        Ok(())
    }

    /// Connect to the WebSocket and process events until disconnected.
    async fn connect_and_stream(&self, cache: &PoolCache) -> Result<()> {
        let (ws_stream, _response) = connect_async(&self.ws_url)
            .await
            .context("Failed to connect to WebSocket")?;

        info!("WebSocket connected");

        let (mut write, mut read) = ws_stream.split();

        // Subscribe to events from each DEX package
        for (i, pkg) in self.dex_packages.iter().enumerate() {
            let subscribe_msg = json!({
                "jsonrpc": "2.0",
                "id": i + 1,
                "method": "suix_subscribeEvent",
                "params": [{
                    "Package": pkg.package_id
                }]
            });

            write
                .send(Message::Text(subscribe_msg.to_string().into()))
                .await
                .context("Failed to send subscribe message")?;

            info!(
                package = %pkg.package_id,
                dex = %pkg.dex_name,
                "Subscribed to events"
            );
        }

        // Create HTTP client for re-fetching pool objects
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()?;

        // Process incoming events
        let mut event_count = 0u64;

        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    let text_str: &str = &text;
                    match serde_json::from_str::<Value>(text_str) {
                        Ok(value) => {
                            // Check if it's a subscription confirmation
                            if value.get("result").is_some() && value.get("id").is_some() {
                                debug!("Subscription confirmed");
                                continue;
                            }

                            // Process event notification
                            if let Some(params) = value.get("params") {
                                if let Some(result) = params.get("result") {
                                    event_count += 1;
                                    self.handle_event(
                                        result,
                                        cache,
                                        &http_client,
                                        event_count,
                                    )
                                    .await;
                                }
                            }
                        }
                        Err(e) => {
                            warn!(error = %e, "Failed to parse WebSocket message");
                        }
                    }
                }
                Ok(Message::Ping(data)) => {
                    write.send(Message::Pong(data)).await.ok();
                }
                Ok(Message::Close(_)) => {
                    info!("WebSocket closed by server");
                    break;
                }
                Err(e) => {
                    error!(error = %e, "WebSocket read error");
                    break;
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Handle a single event from the WebSocket stream.
    ///
    /// When a DEX event is received, we identify the affected pool
    /// and re-fetch its state via RPC to update the cache.
    async fn handle_event(
        &self,
        event: &Value,
        cache: &PoolCache,
        http_client: &reqwest::Client,
        event_count: u64,
    ) {
        // Extract the event type to identify which DEX and pool
        let event_type = match event.get("type").and_then(|t| t.as_str()) {
            Some(t) => t,
            None => return,
        };

        // Extract the pool object ID from event fields
        // DEX events typically include the pool ID in parsedJson
        let pool_id = self.extract_pool_id(event);

        let pool_id = match pool_id {
            Some(id) => id,
            None => {
                // Can't identify pool — check if any monitored pool is affected
                // by looking at object changes
                if let Some(id) = self.match_pool_from_event(event) {
                    id
                } else {
                    debug!(
                        event_type = %event_type,
                        count = %event_count,
                        "Event doesn't match monitored pools"
                    );
                    return;
                }
            }
        };

        // Find the pool metadata
        let meta = match self.pool_metas.iter().find(|m| m.object_id == pool_id) {
            Some(m) => m.clone(),
            None => {
                debug!(pool_id = %pool_id, "Event for unmonitored pool");
                return;
            }
        };

        debug!(
            pool = %pool_id,
            dex = %meta.dex,
            event_type = %event_type,
            count = %event_count,
            "Pool update event received"
        );

        // Re-fetch the pool object to get latest state
        match self
            .fetch_pool_state(http_client, &meta)
            .await
        {
            Ok(state) => {
                cache.upsert(pool_id, state);
                debug!(
                    pool = %meta.object_id,
                    dex = %meta.dex,
                    "Pool state updated from event"
                );
            }
            Err(e) => {
                warn!(
                    pool = %meta.object_id,
                    error = %e,
                    "Failed to re-fetch pool after event"
                );
            }
        }
    }

    /// Try to extract the pool object ID from an event's parsed JSON.
    fn extract_pool_id(&self, event: &Value) -> Option<String> {
        let parsed = event.get("parsedJson")?;

        // Different DEXes use different field names for pool ID
        // Try common patterns
        for field in &["pool", "pool_id", "poolId", "pool_address"] {
            if let Some(id) = parsed.get(field).and_then(|v| v.as_str()) {
                return Some(id.to_string());
            }
        }

        // Some events put pool ID in the object change sender
        None
    }

    /// Try to match a pool from the event's package/module info.
    fn match_pool_from_event(&self, event: &Value) -> Option<String> {
        let package_id = event.get("packageId").and_then(|v| v.as_str())?;

        // If the event is from a monitored package, check if any pool
        // matches the transaction sender or changed objects
        let is_monitored = self
            .dex_packages
            .iter()
            .any(|p| p.package_id == package_id);

        if !is_monitored {
            return None;
        }

        // For events without explicit pool IDs, check the transaction's
        // object changes (not always available in event stream)
        // Fallback: refresh all pools from this DEX
        None
    }

    /// Fetch a single pool's current state via RPC.
    async fn fetch_pool_state(
        &self,
        client: &reqwest::Client,
        meta: &PoolMeta,
    ) -> Result<arb_types::pool::PoolState> {
        let response = client
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

        let content = body
            .get("result")
            .and_then(|r| r.get("data"))
            .and_then(|d| d.get("content"))
            .context("Missing result.data.content in response")?;

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        parsers::parse_pool_object(content, &meta.dex, meta, now_ms)
    }
}

/// Subscribe to transaction effects for specific object IDs.
/// This is an alternative subscription mode that watches for any
/// transaction that modifies a monitored pool object.
pub struct TxEffectStream {
    ws_url: String,
    rpc_url: String,
    pool_metas: Vec<PoolMeta>,
}

impl TxEffectStream {
    pub fn new(ws_url: &str, rpc_url: &str, pool_metas: Vec<PoolMeta>) -> Self {
        Self {
            ws_url: ws_url.to_string(),
            rpc_url: rpc_url.to_string(),
            pool_metas,
        }
    }

    /// Run the transaction effect stream using `suix_subscribeTransaction`.
    /// Watches for transactions that modify any monitored pool object.
    pub async fn run(&self, cache: PoolCache) -> Result<()> {
        info!(
            ws_url = %self.ws_url,
            pools = %self.pool_metas.len(),
            "Starting transaction effect stream"
        );

        loop {
            match self.connect_and_stream(&cache).await {
                Ok(()) => break,
                Err(e) => {
                    error!(error = %e, "TX stream error, reconnecting in 3s...");
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                }
            }
        }

        Ok(())
    }

    async fn connect_and_stream(&self, cache: &PoolCache) -> Result<()> {
        let (ws_stream, _) = connect_async(&self.ws_url)
            .await
            .context("Failed to connect to WebSocket")?;

        info!("TX effect WebSocket connected");

        let (mut write, mut read) = ws_stream.split();

        // Subscribe to transactions that change any monitored pool
        // Use ChangedObject filter with pool IDs
        let pool_ids: Vec<&str> = self
            .pool_metas
            .iter()
            .map(|m| m.object_id.as_str())
            .collect();

        // Sui supports `TransactionFilter::ChangedObject` filter
        // We subscribe once per pool for precise filtering
        for (i, pool_id) in pool_ids.iter().enumerate() {
            let subscribe_msg = json!({
                "jsonrpc": "2.0",
                "id": i + 1,
                "method": "suix_subscribeTransaction",
                "params": [{
                    "ChangedObject": pool_id
                }]
            });

            write
                .send(Message::Text(subscribe_msg.to_string().into()))
                .await
                .context("Failed to send subscribe message")?;

            debug!(pool = %pool_id, "Subscribed to object changes");
        }

        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()?;

        // Process incoming transaction notifications
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    let text_str: &str = &text;
                    if let Ok(value) = serde_json::from_str::<Value>(text_str) {
                        // Skip subscription confirmations
                        if value.get("result").is_some() && value.get("id").is_some() {
                            continue;
                        }

                        // Handle transaction notification
                        if let Some(params) = value.get("params") {
                            if let Some(result) = params.get("result") {
                                self.handle_tx_effect(result, cache, &http_client).await;
                            }
                        }
                    }
                }
                Ok(Message::Ping(data)) => {
                    write.send(Message::Pong(data)).await.ok();
                }
                Ok(Message::Close(_)) => break,
                Err(e) => {
                    error!(error = %e, "WebSocket error");
                    break;
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// When a transaction affecting a monitored pool is detected,
    /// identify which pools changed and re-fetch their state.
    async fn handle_tx_effect(
        &self,
        tx_result: &Value,
        cache: &PoolCache,
        http_client: &reqwest::Client,
    ) {
        // Extract the digest for logging
        let digest = tx_result
            .get("digest")
            .and_then(|d| d.as_str())
            .unwrap_or("unknown");

        // Extract changed object IDs from effects
        let changed_ids = self.extract_changed_objects(tx_result);

        for pool_id in changed_ids {
            if let Some(meta) = self.pool_metas.iter().find(|m| m.object_id == pool_id) {
                debug!(
                    pool = %pool_id,
                    dex = %meta.dex,
                    tx = %digest,
                    "Pool changed by transaction"
                );

                // Re-fetch pool state
                match fetch_pool(http_client, &self.rpc_url, meta).await {
                    Ok(state) => {
                        cache.upsert(pool_id, state);
                    }
                    Err(e) => {
                        warn!(pool = %meta.object_id, error = %e, "Failed to re-fetch pool");
                    }
                }
            }
        }
    }

    /// Extract changed object IDs from transaction effects.
    fn extract_changed_objects(&self, tx_result: &Value) -> Vec<String> {
        let mut ids = Vec::new();

        // Check effects.mutated and effects.created
        if let Some(effects) = tx_result.get("effects") {
            for key in &["mutated", "created", "unwrapped"] {
                if let Some(objects) = effects.get(key).and_then(|v| v.as_array()) {
                    for obj in objects {
                        if let Some(id) = obj
                            .get("reference")
                            .or(obj.get("objectId"))
                            .and_then(|r| r.get("objectId").or(Some(r)))
                            .and_then(|id| id.as_str())
                        {
                            // Only include if it's a monitored pool
                            if self.pool_metas.iter().any(|m| m.object_id == id) {
                                ids.push(id.to_string());
                            }
                        }
                    }
                }
            }
        }

        ids
    }
}

/// Fetch a single pool's current state via RPC (shared helper).
async fn fetch_pool(
    client: &reqwest::Client,
    rpc_url: &str,
    meta: &PoolMeta,
) -> Result<arb_types::pool::PoolState> {
    let response = client
        .post(rpc_url)
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

    let body: Value = response.json().await?;

    if let Some(error) = body.get("error") {
        anyhow::bail!("RPC error: {}", error);
    }

    let content = body
        .get("result")
        .and_then(|r| r.get("data"))
        .and_then(|d| d.get("content"))
        .context("Missing content")?;

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    parsers::parse_pool_object(content, &meta.dex, meta, now_ms)
}
