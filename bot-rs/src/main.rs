use anyhow::Result;
use arb_collector::{rpc_poller, DexPackage, PoolCache, RpcPoller, TxEffectStream, WsStream};
use arb_executor::{CoinMerger, GasMonitor, Signer, Submitter};
use arb_strategy::{CircuitBreaker, DryRunner, Scanner, build_local_simulator, ternary_search};
use arb_types::Config;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::signal;
use tracing::{debug, error, info, warn};

/// Maximum allowed staleness (ms) for pool data before strategy loop skips a cycle.
const MAX_POOL_STALENESS_MS: u64 = 10_000; // 10 seconds

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

    // ── Startup validation ──
    validate_startup(&config);

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

    // ── Spawn collector task(s) with supervision ──
    // Shared counter: collectors bump this on every successful update so the
    // strategy loop can detect when all collectors have died.
    let collector_heartbeat = Arc::new(AtomicU64::new(now_ms()));

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
            let tx_stream = TxEffectStream::new(&ws_url, &config.rpc_url, pool_metas);
            let ws_cache = cache.clone();
            let hb = collector_heartbeat.clone();
            info!(mode = "tx_effects", "Using WebSocket streaming");

            tokio::spawn(async move {
                loop {
                    match tx_stream.run(ws_cache.clone()).await {
                        Ok(()) => {
                            warn!("TX effect stream ended cleanly — restarting in 3s");
                        }
                        Err(e) => {
                            error!(error = %e, "TX effect stream failed — restarting in 3s");
                        }
                    }
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    hb.store(now_ms(), Ordering::Relaxed);
                }
            });
        } else {
            let dex_packages = build_dex_packages(&config);
            let ws = WsStream::new(&ws_url, &config.rpc_url, dex_packages, pool_metas);
            let ws_cache = cache.clone();
            let hb = collector_heartbeat.clone();
            info!(mode = "event", "Using WebSocket streaming");

            tokio::spawn(async move {
                loop {
                    match ws.run(ws_cache.clone()).await {
                        Ok(()) => {
                            warn!("WebSocket event stream ended cleanly — restarting in 3s");
                        }
                        Err(e) => {
                            error!(error = %e, "WebSocket event stream failed — restarting in 3s");
                        }
                    }
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    hb.store(now_ms(), Ordering::Relaxed);
                }
            });
        }

        // Also run RPC poller as supervised fallback
        let fallback_cache = cache.clone();
        let poller = RpcPoller::new(&config);
        let hb = collector_heartbeat.clone();
        info!("RPC poller running as fallback");

        tokio::spawn(async move {
            loop {
                if let Err(e) = poller.run(fallback_cache.clone()).await {
                    error!(error = %e, "Fallback poller failed — restarting in 5s");
                }
                tokio::time::sleep(Duration::from_secs(5)).await;
                hb.store(now_ms(), Ordering::Relaxed);
            }
        });
    } else {
        // Default: supervised RPC polling
        let collector_cache = cache.clone();
        let hb = collector_heartbeat.clone();
        info!("Using RPC polling (set USE_WEBSOCKET=true for streaming)");

        tokio::spawn(async move {
            loop {
                if let Err(e) = poller.run(collector_cache.clone()).await {
                    error!(error = %e, "Collector task failed — restarting in 5s");
                }
                tokio::time::sleep(Duration::from_secs(5)).await;
                hb.store(now_ms(), Ordering::Relaxed);
            }
        });
    }

    // ── Strategy loop ──
    let poll_interval = Duration::from_millis(config.poll_interval_ms);
    let dry_run_enabled = config.dry_run_before_submit;

    // Gas balance monitor (min 0.1 SUI = 100M MIST to allow trading)
    let min_gas_balance: u64 = env_var_or_default("MIN_GAS_BALANCE_MIST", 100_000_000);
    let mut gas_monitor = GasMonitor::new(&config.rpc_url, &sender_address, min_gas_balance);
    info!(
        min_balance_sui = %format!("{:.2}", min_gas_balance as f64 / 1_000_000_000.0),
        "Gas balance monitor initialized"
    );

    // Coin dust merger (consolidates fragmented Coin<SUI> objects)
    let mut coin_merger = CoinMerger::new(&config.rpc_url, &sender_address);
    info!("Coin merger initialized (threshold: 20 coins, check every ~50s)");

    // Circuit breaker
    let mut circuit_breaker = CircuitBreaker::new(
        config.cb_max_consecutive_failures,
        config.cb_max_cumulative_loss_mist,
        config.cb_cooldown_ms,
    );
    info!(
        max_consec = %config.cb_max_consecutive_failures,
        max_loss = %config.cb_max_cumulative_loss_mist,
        cooldown_ms = %config.cb_cooldown_ms,
        "Circuit breaker initialized"
    );

    let strategy_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(poll_interval);
        let mut total_trades = 0u64;
        let mut total_profit = 0i64;
        let mut total_gas = 0u64;

        info!("Strategy loop started ({}ms tick)", poll_interval.as_millis());

        loop {
            interval.tick().await;

            // 0a. Circuit breaker check
            if !circuit_breaker.is_trading_allowed(now_ms()) {
                continue;
            }

            // 0b. Gas balance check
            if let Err(e) = gas_monitor.check_balance(now_ms()).await {
                warn!(error = %e, "Gas balance insufficient — skipping cycle");
                continue;
            }

            // 0c. Periodic coin dust merge
            if let Ok(Some(merge_tx)) = coin_merger.maybe_merge().await {
                match signer.sign_transaction(&merge_tx) {
                    Ok(sig) => {
                        match submitter.submit(&merge_tx, &sig).await {
                            Ok(result) => {
                                if result.success {
                                    info!(
                                        digest = %result.digest,
                                        gas = %result.gas_cost_mist,
                                        "Coin merge successful"
                                    );
                                    gas_monitor.deduct_gas(result.gas_cost_mist);
                                } else {
                                    warn!(error = ?result.error_message, "Coin merge failed on-chain");
                                }
                            }
                            Err(e) => warn!(error = %e, "Coin merge submission failed"),
                        }
                    }
                    Err(e) => warn!(error = %e, "Failed to sign merge transaction"),
                }
            }

            // 0d. Check collector liveness via heartbeat
            let hb_age = now_ms().saturating_sub(
                collector_heartbeat.load(Ordering::Relaxed),
            );
            if hb_age > MAX_POOL_STALENESS_MS * 3 {
                warn!(
                    stale_ms = %hb_age,
                    "All collectors appear dead — skipping cycle"
                );
                continue;
            }

            // 1. Read pool states from cache
            let pools = cache.snapshot();
            if pools.is_empty() {
                continue;
            }

            // 1b. Staleness guard: skip if ALL pools are too old
            let now = now_ms();
            let fresh_count = pools
                .iter()
                .filter(|p| p.staleness_ms(now) <= MAX_POOL_STALENESS_MS)
                .count();
            if fresh_count == 0 {
                warn!("All pool data is stale — skipping cycle");
                continue;
            }

            // 2. Scan for opportunities (two-hop + tri-hop)
            let mut opportunities = scanner.scan_two_hop(&pools);
            let tri_opps = scanner.scan_tri_hop(&pools);
            opportunities.extend(tri_opps);

            if opportunities.is_empty() {
                continue;
            }

            // Re-sort combined opportunities by expected profit
            opportunities.sort_by(|a, b| b.expected_profit.cmp(&a.expected_profit));

            // 3. Process best opportunity (safe: we checked is_empty above)
            let mut best = match opportunities.into_iter().next() {
                Some(opp) => opp,
                None => continue,
            };

            // 4. Always run optimizer via ternary search (local simulation)
            {
                let flash_pool = pools.iter().find(|p| p.object_id == best.pool_ids[0]);
                let sell_pool = pools.iter().find(|p| p.object_id == best.pool_ids[1]);

                if let (Some(fp), Some(sp)) = (flash_pool, sell_pool) {
                    let (simulate, hi) = build_local_simulator(fp, sp);
                    let (optimal_amount, max_profit) =
                        ternary_search(1_000, hi, 100_000, &*simulate);

                    if max_profit > 0 {
                        debug!(
                            prev_amount = %best.amount_in,
                            new_amount = %optimal_amount,
                            prev_profit = %best.expected_profit,
                            new_profit = %max_profit,
                            "Ternary search optimized"
                        );
                        best.amount_in = optimal_amount;
                        best.expected_profit = max_profit;
                        best.net_profit = max_profit as i64 - best.estimated_gas as i64;
                    }
                }
            }

            // 4b. Post-optimization guards
            // Guard: skip if optimizer couldn't find a profitable trade
            if best.expected_profit == 0 {
                debug!("Optimizer found no profitable amount — skipping");
                continue;
            }

            // Guard: check opportunity staleness (prices may have moved)
            let opp_age_ms = now_ms().saturating_sub(best.detected_at_ms);
            if opp_age_ms > 3_000 {
                debug!(
                    age_ms = %opp_age_ms,
                    "Opportunity too stale (>3s) — skipping"
                );
                continue;
            }

            // Guard: net profit must still be positive after gas
            best.net_profit = best.expected_profit as i64 - best.estimated_gas as i64;
            if best.net_profit <= 0 {
                debug!(
                    expected_profit = %best.expected_profit,
                    estimated_gas = %best.estimated_gas,
                    "Net profit non-positive after optimization — skipping"
                );
                continue;
            }

            info!(
                strategy = ?best.strategy,
                amount = %best.amount_in,
                expected_profit = %best.expected_profit,
                net_profit = %best.net_profit,
                min_profit_onchain = %(best.expected_profit * 9 / 10).max(1),
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

                // 6b. Rebuild PTB with tighter min_profit from dry-run actuals
                let tx_bytes_final = match ptb_builder.build(&best).await {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        warn!(error = %e, "Failed to rebuild PTB after dry-run");
                        continue;
                    }
                };

                // 7. Sign and submit (dry-run path with rebuilt PTB)
                let signature = match signer.sign_transaction(&tx_bytes_final) {
                    Ok(sig) => sig,
                    Err(e) => {
                        error!(error = %e, "Failed to sign transaction");
                        continue;
                    }
                };

                match submitter.submit(&tx_bytes_final, &signature).await {
                    Ok(result) => {
                        total_trades += 1;
                        total_gas += result.gas_cost_mist;
                        gas_monitor.deduct_gas(result.gas_cost_mist);
                        log_trade_result(&result, &mut total_profit, total_trades, total_gas);
                        // Report to circuit breaker
                        if result.success {
                            let net = result.profit_mist.unwrap_or(0) as i64
                                - result.gas_cost_mist as i64;
                            circuit_breaker.record_success(net);
                        } else {
                            circuit_breaker
                                .record_failure(-(result.gas_cost_mist as i64), now_ms());
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "Transaction submission failed");
                        circuit_breaker.record_failure(0, now_ms());
                    }
                }
            } else {
                // 7. Sign and submit (no dry-run path)
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
                        total_gas += result.gas_cost_mist;
                        gas_monitor.deduct_gas(result.gas_cost_mist);
                        log_trade_result(&result, &mut total_profit, total_trades, total_gas);
                        // Report to circuit breaker
                        if result.success {
                            let net = result.profit_mist.unwrap_or(0) as i64
                                - result.gas_cost_mist as i64;
                            circuit_breaker.record_success(net);
                        } else {
                            circuit_breaker
                                .record_failure(-(result.gas_cost_mist as i64), now_ms());
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "Transaction submission failed");
                        circuit_breaker.record_failure(0, now_ms());
                    }
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

/// Read an environment variable with a default, parsing to the target type.
fn env_var_or_default<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Get current time in milliseconds since Unix epoch.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Log a trade result and update running totals.
fn log_trade_result(
    result: &arb_executor::SubmitResult,
    total_profit: &mut i64,
    total_trades: u64,
    total_gas: u64,
) {
    if result.success {
        let profit = result.profit_mist.unwrap_or(0);
        *total_profit += profit as i64 - result.gas_cost_mist as i64;

        info!(
            digest = %result.digest,
            profit = %profit,
            gas = %result.gas_cost_mist,
            total_trades = %total_trades,
            total_profit = %total_profit,
            total_gas = %total_gas,
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

/// Validate critical configuration at startup.
/// Warns on non-fatal issues, errors on blockers.
fn validate_startup(config: &Config) {
    let mut warnings = 0u32;
    let mut errors = 0u32;

    // 1. Package ID must be set (not placeholder)
    if config.package_id == "0x0" || config.package_id == "0x..." || config.package_id.is_empty() {
        error!(
            "PACKAGE_ID is not set ({}) — deploy the Move package first with `sui client publish`",
            config.package_id
        );
        errors += 1;
    }

    // 2. AdminCap and PauseFlag
    if config.admin_cap_id == "0x..." || config.admin_cap_id.is_empty() {
        error!("ADMIN_CAP_ID is not set — required for admin operations");
        errors += 1;
    }
    if config.pause_flag_id == "0x..." || config.pause_flag_id.is_empty() {
        error!("PAUSE_FLAG_ID is not set — required for all strategy calls");
        errors += 1;
    }

    // 3. Monitored pools
    if config.monitored_pools.is_empty() {
        error!("MONITORED_POOLS is empty — no pools to monitor. Add pool configs to start trading.");
        errors += 1;
    } else {
        // Validate pool config format
        for (i, pool) in config.monitored_pools.iter().enumerate() {
            let valid_dexes = ["cetus", "turbos", "deepbook", "aftermath", "flowx_clmm", "flowx_amm", "flowx"];
            if !valid_dexes.contains(&pool.dex.to_lowercase().as_str()) {
                warn!(
                    pool = %i,
                    dex = %pool.dex,
                    "Unknown DEX in pool config — may not be parseable"
                );
                warnings += 1;
            }
            if !pool.pool_id.starts_with("0x") {
                warn!(pool = %i, id = %pool.pool_id, "Pool ID doesn't start with 0x");
                warnings += 1;
            }
        }

        // Check for DeepBook pools without DEEP fee coin
        let has_deepbook = config
            .monitored_pools
            .iter()
            .any(|p| p.dex.to_lowercase() == "deepbook");
        if has_deepbook
            && (config.deep_fee_coin_id.is_empty()
                || config.deep_fee_coin_id == "0x..."
                || config.deep_fee_coin_id == "0x0")
        {
            warn!(
                "DeepBook pools configured but DEEP_FEE_COIN_ID is not set — \
                 DeepBook strategies will abort. Get a Coin<DEEP> object: \
                 `sui client gas --coin-type 0xdeeb...::deep::DEEP`"
            );
            warnings += 1;
        }
    }

    // 4. DEX shared objects
    if config.cetus_global_config.is_empty() {
        warn!("CETUS_GLOBAL_CONFIG not set — Cetus strategies will fail");
        warnings += 1;
    }
    if config.turbos_versioned.is_empty() {
        warn!("TURBOS_VERSIONED not set — Turbos strategies will fail");
        warnings += 1;
    }

    // 5. Strategy params sanity
    if config.min_profit_mist == 0 {
        warn!("MIN_PROFIT_MIST is 0 — bot will attempt tiny unprofitable trades");
        warnings += 1;
    }
    if config.max_gas_budget < 10_000_000 {
        warn!(
            budget = %config.max_gas_budget,
            "MAX_GAS_BUDGET is very low — transactions may run out of gas"
        );
        warnings += 1;
    }

    // Summary
    if errors > 0 {
        error!(
            errors = %errors,
            warnings = %warnings,
            "⛔ Startup validation found {} critical error(s) — bot will likely fail",
            errors
        );
    } else if warnings > 0 {
        warn!(
            warnings = %warnings,
            "⚠️  Startup validation passed with {} warning(s)",
            warnings
        );
    } else {
        info!("✅ Startup validation passed — all checks OK");
    }
}
