use anyhow::{Context, Result};
use arb_types::pool::{Dex, PoolState};
use serde_json::Value;

use super::PoolMeta;

/// Parse an Aftermath AMM Pool object.
///
/// Aftermath uses weighted/stable pools (Balancer-style).
/// Pool<LP> has normalized_balances and weights.
/// For 2-token pools, we extract reserve_a and reserve_b.
pub(crate) fn parse(content: &Value, meta: &PoolMeta, now_ms: u64) -> Result<PoolState> {
    let fields = content
        .get("fields")
        .context("Missing fields in Aftermath pool")?;

    // Aftermath pools store balances in a vector or table
    // The exact structure depends on the pool version
    // For now, extract what we can
    let reserve_a = extract_balance(fields, 0);
    let reserve_b = extract_balance(fields, 1);

    Ok(PoolState {
        object_id: meta.object_id.clone(),
        dex: Dex::Aftermath,
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

/// Extract balance for token at index from Aftermath pool fields.
fn extract_balance(fields: &Value, _index: usize) -> Option<u64> {
    // Aftermath stores balances in various formats depending on pool type.
    // This is a simplified extractor — production code should handle
    // weighted pools (with balances vector) and stable pools.
    fields
        .get("normalized_balances")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.get(_index))
        .and_then(|b| b.as_str())
        .and_then(|s| s.parse::<u64>().ok())
}
