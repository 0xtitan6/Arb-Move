use anyhow::{Context, Result};
use arb_types::pool::{Dex, PoolState};
use serde_json::Value;

use super::PoolMeta;

/// Parse a DeepBook V3 Pool object.
///
/// DeepBook uses a CLOB model — no sqrt_price or liquidity.
/// Key fields: base_vault, quote_vault balances, and order book state.
/// For price estimation, we'd need to query the order book (bids/asks).
/// For now, we store vault reserves as a rough proxy.
pub(crate) fn parse(content: &Value, meta: &PoolMeta, now_ms: u64) -> Result<PoolState> {
    let fields = content
        .get("fields")
        .context("Missing fields in DeepBook pool")?;

    // DeepBook V3 stores vault balances as nested objects
    // The exact field structure depends on the version
    let reserve_a = extract_vault_balance(fields, "base_vault");
    let reserve_b = extract_vault_balance(fields, "quote_vault");

    Ok(PoolState {
        object_id: meta.object_id.clone(),
        dex: Dex::DeepBook,
        coin_type_a: meta.coin_type_a.clone(),
        coin_type_b: meta.coin_type_b.clone(),
        sqrt_price: None,
        tick_index: None,
        liquidity: None,
        fee_rate_bps: None,
        reserve_a,
        reserve_b,
        best_bid: None,
        best_ask: None,
        last_updated_ms: now_ms,
    })
}

/// Try to extract vault balance from nested Move object fields.
/// Handles both string-encoded and numeric JSON balance values.
fn extract_vault_balance(fields: &Value, vault_name: &str) -> Option<u64> {
    let b = fields.get(vault_name)?.get("fields")?.get("balance")?;
    b.as_u64()
        .or_else(|| b.as_str().and_then(|s| s.parse::<u64>().ok()))
}
