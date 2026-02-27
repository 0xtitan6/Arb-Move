use anyhow::Result;
use arb_collector::{rpc_poller, DexPackage, PoolCache, RpcPoller, TxEffectStream, WsStream};
use arb_executor::{Signer, Submitter};
use arb_strategy::{DryRunner, Scanner};
use arb_types::Config;
use std::time::Duration;
use tokio::signal;
use tracing::{error, info, warn};

#[tokio::main]
async fn main() -> Result<()> {
    // ── Setup ──
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(true)
        .with_thread_ids(false)
        .init();

    info!("╔══════════════════════════════════════╗");
    info!("║     ArbMove Bot v0.1.0 — Sui MEV    ║");
    info!("╚══════════════════════════════════════╝");

    let config = Config::from_env()?;
    let signer = Signer::from_hex(&config.private_key_hex)?;
    let sender_address = signer.address();

    info!(address = %sender_address, "Wallet loaded");
    info!(rpc = %config.rpc_url, "Connecting to Sui");
    info!(
        pools = %config.monitored_pools.len(),
        min_profit = %config.min_profit_mist,
        poll_ms = %config.poll_interval_ms,
        "Configuration loaded"
    );

    // ── Initialize components ──
    let cache = PoolCache::new();

    // Seed cache with initial pool states
    rpc_poller::seed_cache(&config, &cache).await?;
    info!(cached = %cache.len(), "Pool cache ready");

    // Create components
    let poller = RpcPoller::new(&config);
    let scanner = Scanner::new(config.min_profit_mist);
    let dry_runner = DryRunner::new(
        &config.rpc_url,
        &config.package_id,
        &sender_address,
        config.max_gas_budget,
    );
    let submitter = Submitter::new(&config.rpc_url);
    let ptb_builder = arb_executor::ptb_builder::PtbBuilder::new(&config, &sender_address);

    // ── Determine collector mode ──
    let use_ws = std::env::var("USE_WEBSOCKET")
        .unwrap_or_else(|_| "false".to_string())
        .parse::<bool>()
        .unwrap_or(false);

    let ws_mode = std::env::var("WS_MODE")
        .unwrap_or_else(|_| "event".to_string());

    // ── Spawn collector task(s) ──
    if use_ws {
        let ws_url = WsStream::ws_url_from_rpc(&config.rpc_url);
        let pool_metas: Vec<_> = config
            .monitored_pools
            .iter()
            .map(|p| arb_collector::rpc_poller::PoolMeta {
                object_id: p.pool_id.clone(),
                dex: p.dex.clone(),
                coin_type_a: p.coin_type_a.clone(),
                coin_type_b: p.coin_type_b.clone(),
            })
            .collect();

        if ws_mode == "tx" {
            // Mode 1: Subscribe to transaction effects on monitored pool objects
            // More reliable — triggers on ANY transaction that modifies the pool
            let tx_stream = TxEffectStream::new(&ws_url, &config.rpc_url, pool_metas);
            let ws_cache = cache.clone();
            info!(mode = "tx_effects", "Using WebSocket streaming");

            tokio::spawn(async move {
                if let Err(e) = tx_stream.run(ws_cache).await {
                    error!(error = %e, "TX effect stream failed");
                }
            });
        } else {
            // Mode 2: Subscribe to DEX package events
            // Lower latency but may miss some updates
            let dex_packages = build_dex_packages(&config);
            let ws = WsStream::new(&ws_url, &config.rpc_url, dex_packages, pool_metas);
            let ws_cache = cache.clone();
            info!(mode = "event", "Using WebSocket streaming");

            tokio::spawn(async move {
                if let Err(e) = ws.run(ws_cache).await {
                    error!(error = %e, "WebSocket event stream failed");
                }
            });
        }

        // Also run RPC poller as fallback (slower interval)
        let fallback_cache = cache.clone();
        let poller = RpcPoller::new(&config);
        info!("RPC poller running as fallback");

        tokio::spawn(async move {
            if let Err(e) = poller.run(fallback_cache).await {
                error!(error = %e, "Fallback poller failed");
            }
        });
    } else {
        // Default: RPC polling only
        let collector_cache = cache.clone();
        info!("Using RPC polling (set USE_WEBSOCKET=true for streaming)");

        tokio::spawn(async move {
            if let Err(e) = poller.run(collector_cache).await {
                error!(error = %e, "Collector task failed");
            }
        });
    }

    // ── Strategy loop ──
    let poll_interval = Duration::from_millis(config.poll_interval_ms);
    let dry_run_enabled = config.dry_run_before_submit;

    let strategy_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(poll_interval);
        let mut total_trades = 0u64;
        let mut total_profit = 0i64;
        let mut _total_gas = 0u64;

        info!("Strategy loop started ({}ms tick)", poll_interval.as_millis());

        loop {
            interval.tick().await;

            // 1. Read pool states from cache
            let pools = cache.snapshot();
            if pools.is_empty() {
                continue;
            }

            // 2. Scan for opportunities
            let opportunities = scanner.scan_two_hop(&pools);
            if opportunities.is_empty() {
                continue;
            }

            // 3. Process best opportunity (safe: we checked is_empty above)
            let mut best = match opportunities.into_iter().next() {
                Some(opp) => opp,
                None => continue,
            };

            // 4. Optimize amount via ternary search (if profitable enough to justify)
            if best.expected_profit > scanner.min_profit_mist * 2 {
                // TODO: Run ternary search with local simulation
                // For now, use the scanner's estimate
            }

            info!(
                strategy = ?best.strategy,
                amount = %best.amount_in,
                expected_profit = %best.expected_profit,
                net_profit = %best.net_profit,
                "Processing opportunity"
            );

            // 5. Build PTB
            let tx_bytes = match ptb_builder.build(&best).await {
                Ok(bytes) => bytes,
                Err(e) => {
                    warn!(error = %e, "Failed to build PTB");
                    continue;
                }
            };

            // 6. Dry-run validation
            if dry_run_enabled {
                match dry_runner.validate(&mut best, &tx_bytes).await {
                    Ok(true) => {
                        info!(
                            gas = %best.estimated_gas,
                            net_profit = %best.net_profit,
                            "Dry-run passed"
                        );
                    }
                    Ok(false) => {
                        warn!("Opportunity no longer profitable after dry-run");
                        continue;
                    }
                    Err(e) => {
                        warn!(error = %e, "Dry-run failed");
                        continue;
                    }
                }
            }

            // 7. Sign and submit
            let signature = match signer.sign_transaction(&tx_bytes) {
                Ok(sig) => sig,
                Err(e) => {
                    error!(error = %e, "Failed to sign transaction");
                    continue;
                }
            };

            match submitter.submit(&tx_bytes, &signature).await {
                Ok(result) => {
                    total_trades += 1;
                    _total_gas += result.gas_cost_mist;

                    if result.success {
                        let profit = result.profit_mist.unwrap_or(0);
                        total_profit += profit as i64 - result.gas_cost_mist as i64;

                        info!(
                            digest = %result.digest,
                            profit = %profit,
                            gas = %result.gas_cost_mist,
                            total_trades = %total_trades,
                            total_profit = %total_profit,
                            "✅ Arb executed successfully"
                        );
                    } else {
                        warn!(
                            digest = %result.digest,
                            error = ?result.error_message,
                            "❌ Transaction failed on-chain"
                        );
                    }
                }
                Err(e) => {
                    error!(error = %e, "Transaction submission failed");
                }
            }
        }
    });

    // ── Graceful shutdown ──
    info!("Bot running. Press Ctrl+C to stop.");

    signal::ctrl_c().await?;
    info!("\nShutting down...");

    strategy_handle.abort();

    info!("╔══════════════════════════════════════╗");
    info!("║         Session Summary              ║");
    info!("╚══════════════════════════════════════╝");
    info!("Bot stopped gracefully.");

    Ok(())
}

/// Build the list of DEX package IDs to subscribe to from config.
fn build_dex_packages(config: &Config) -> Vec<DexPackage> {
    let mut packages = Vec::new();

    // Add package IDs from environment if set
    let dex_names = ["CETUS", "TURBOS", "DEEPBOOK", "AFTERMATH", "FLOWX"];

    for name in &dex_names {
        let env_key = format!("{}_PACKAGE_ID", name);
        if let Ok(pkg_id) = std::env::var(&env_key) {
            if !pkg_id.is_empty() {
                packages.push(DexPackage {
                    package_id: pkg_id,
                    dex_name: name.to_lowercase(),
                });
            }
        }
    }

    // Always include the arb package itself for ArbExecuted events
    packages.push(DexPackage {
        package_id: config.package_id.clone(),
        dex_name: "arbmove".to_string(),
    });

    packages
}
