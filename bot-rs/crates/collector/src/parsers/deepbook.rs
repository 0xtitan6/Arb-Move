use anyhow::{Context, Result};
use arb_types::pool::{Dex, PoolState};
use serde_json::Value;

use super::PoolMeta;

/// Parse a DeepBook V3 PoolInner object.
///
/// DeepBook uses a CLOB model — no sqrt_price or liquidity.
/// Key fields: base_vault, quote_vault balances, and order book state.
///
/// Note: DeepBook V3 wraps pool data in `0x2::versioned::Versioned`.
/// The RPC poller handles the two-step unwrap — by the time we get here,
/// `content` is the PoolInner with direct vault fields.
pub(crate) fn parse(content: &Value, meta: &PoolMeta, now_ms: u64) -> Result<PoolState> {
    let fields = content
        .get("fields")
        .context("Missing fields in DeepBook pool")?;

    // DeepBook V3 stores vault balances as Balance<T> { value: "..." }
    // Older versions might use { balance: "..." }
    let reserve_a = extract_vault_balance(fields, "base_vault");
    let reserve_b = extract_vault_balance(fields, "quote_vault");

    // Extract taker fee in basis points if available
    let fee_rate_bps = extract_fee_bps(fields);

    Ok(PoolState {
        object_id: meta.object_id.clone(),
        dex: Dex::DeepBook,
        coin_type_a: meta.coin_type_a.clone(),
        coin_type_b: meta.coin_type_b.clone(),
        sqrt_price: None,
        tick_index: None,
        liquidity: None,
        fee_rate_bps,
        reserve_a,
        reserve_b,
        best_bid: None,
        best_ask: None,
        last_updated_ms: now_ms,
        fee_type: None,
    })
}

/// Try to extract vault balance from nested Move object fields.
/// Handles multiple possible structures:
///   - vault.fields.balance (string or u64)
///   - vault.fields.value (Sui Balance struct)
fn extract_vault_balance(fields: &Value, vault_name: &str) -> Option<u64> {
    let vault_fields = fields.get(vault_name)?.get("fields")?;

    // Try "balance" first (some versions), then "value" (Sui Balance<T> struct)
    let b = vault_fields
        .get("balance")
        .or_else(|| vault_fields.get("value"))?;

    b.as_u64()
        .or_else(|| b.as_str().and_then(|s| s.parse::<u64>().ok()))
}

/// Extract taker fee in basis points from DeepBook V3 PoolInner.
/// DeepBook V3 stores taker_fee as a raw integer (e.g. 100 = 1 bps, 1000 = 10 bps).
fn extract_fee_bps(fields: &Value) -> Option<u64> {
    // Try taker_fee first (V3 PoolInner)
    let fee = fields.get("taker_fee").or_else(|| fields.get("fee_rate"));
    fee.and_then(|v| {
        v.as_u64()
            .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
    })
}
