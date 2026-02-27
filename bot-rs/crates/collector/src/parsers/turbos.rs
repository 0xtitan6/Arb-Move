use anyhow::{Context, Result};
use arb_types::pool::{Dex, PoolState};
use serde_json::Value;

use super::{field_u128, field_u64, PoolMeta};

/// Parse a Turbos CLMM Pool object.
///
/// Turbos Pool<A, B, Fee> fields:
/// - sqrt_price: u128
/// - tick_current_index: I32
/// - liquidity: u128
/// - fee: u64 (in 1e6 units, e.g. 3000 = 0.3%)
pub(crate) fn parse(content: &Value, meta: &PoolMeta, now_ms: u64) -> Result<PoolState> {
    let fields = content
        .get("fields")
        .context("Missing fields in Turbos pool")?;

    let sqrt_price = field_u128(fields, "sqrt_price").ok();
    let liquidity = field_u128(fields, "liquidity").ok();

    let tick_index = fields
        .get("tick_current_index")
        .and_then(|v| v.get("fields"))
        .and_then(|f| f.get("bits"))
        .and_then(|b| b.as_u64())
        .map(|bits| bits as i32);

    // On-chain field is "fee" (not "fee_rate"), in 1e6 units (e.g. 3000 = 0.3%)
    // Convert to bps: 3000 / 100 = 30 bps
    let fee = field_u64(fields, "fee").ok();
    let fee_rate_bps = fee.map(|f| f / 100);

    Ok(PoolState {
        object_id: meta.object_id.clone(),
        dex: Dex::Turbos,
        coin_type_a: meta.coin_type_a.clone(),
        coin_type_b: meta.coin_type_b.clone(),
        sqrt_price,
        tick_index,
        liquidity,
        fee_rate_bps,
        reserve_a: None,
        reserve_b: None,
        best_bid: None,
        best_ask: None,
        last_updated_ms: now_ms,
    })
}
