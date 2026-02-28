use anyhow::{Context, Result};
use arb_types::pool::{Dex, PoolState};
use serde_json::Value;

use super::PoolMeta;

/// Parse an Aftermath AMM Pool object.
///
/// Aftermath uses weighted/stable pools (Balancer-style).
/// Pool<LP> has normalized_balances and weights.
/// For 2-token pools, we extract reserve_a and reserve_b.
///
/// Aftermath's `normalized_balances` are stored as very large strings
/// (scaled to 18 decimal fixed-point) so they overflow u64.
/// We parse them as f64 and derive synthetic reserves that preserve
/// the correct price ratio while fitting in u64.
pub(crate) fn parse(content: &Value, meta: &PoolMeta, now_ms: u64) -> Result<PoolState> {
    let fields = content
        .get("fields")
        .context("Missing fields in Aftermath pool")?;

    // Parse normalized_balances as f64 (too large for u64)
    let norm_a = extract_normalized_balance(fields, 0);
    let norm_b = extract_normalized_balance(fields, 1);

    // Derive synthetic reserves that preserve the price ratio.
    // Scale down so they fit in u64 as "virtual reserves".
    let (reserve_a, reserve_b) = match (norm_a, norm_b) {
        (Some(a), Some(b)) if a > 0.0 => {
            // Use 1B as virtual depth — ratio is what matters for price_a_in_b()
            let virtual_depth = 1_000_000_000u64;
            let price = b / a; // price of A in terms of B
            let rb = (virtual_depth as f64 * price) as u64;
            (Some(virtual_depth), Some(rb.max(1)))
        }
        _ => (None, None),
    };

    // Extract fee rate from fees_swap_in (Aftermath uses 18-decimal fixed-point bps)
    // e.g. "2500000000000000" = 0.0025 = 25 bps
    let fee_rate_bps = extract_fee_bps(fields);

    Ok(PoolState {
        object_id: meta.object_id.clone(),
        dex: Dex::Aftermath,
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

/// Extract normalized balance at index as f64 (values are too large for u64).
fn extract_normalized_balance(fields: &Value, index: usize) -> Option<f64> {
    fields
        .get("normalized_balances")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.get(index))
        .and_then(|b| b.as_str())
        .and_then(|s| s.parse::<f64>().ok())
}

/// Extract swap fee in basis points from Aftermath's fees_swap_in field.
/// Aftermath stores fees as 18-decimal fixed-point: 2500000000000000 = 0.25% = 25 bps.
fn extract_fee_bps(fields: &Value) -> Option<u64> {
    fields
        .get("fees_swap_in")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|f| f.as_str())
        .and_then(|s| s.parse::<f64>().ok())
        .map(|fee_18d| {
            // Convert from 18-decimal to bps: fee / 1e18 * 10000
            (fee_18d / 1e18 * 10_000.0) as u64
        })
}
