use anyhow::{Context, Result};
use arb_types::pool::{Dex, PoolState};
use serde_json::Value;

use super::{field_u128, field_u64, PoolMeta};

/// Parse a Cetus CLMM Pool object from `sui_getObject` content.
///
/// Cetus Pool<A, B> fields:
/// - current_sqrt_price: u128 (string in JSON)
/// - current_tick_index: i32 (via I32 wrapper)
/// - liquidity: u128
/// - fee_rate: u64 (in 1e6 units, divide by 10000 for bps)
pub(crate) fn parse(content: &Value, meta: &PoolMeta, now_ms: u64) -> Result<PoolState> {
    let fields = content
        .get("fields")
        .context("Missing fields in Cetus pool")?;

    let sqrt_price = field_u128(fields, "current_sqrt_price").ok();
    let liquidity = field_u128(fields, "liquidity").ok();

    // Cetus tick_index is stored as an I32 struct { bits: u32 } (two's complement).
    // Handle both unsigned u64 JSON (reinterpret as u32 → i32) and signed i64 JSON.
    let tick_index = fields
        .get("current_tick_index")
        .and_then(|v| v.get("fields"))
        .and_then(|f| f.get("bits"))
        .and_then(|b| {
            b.as_u64()
                .map(|bits| (bits as u32) as i32)
                .or_else(|| b.as_i64().map(|v| v as i32))
        });

    let fee_rate = field_u64(fields, "fee_rate").ok();
    // Cetus fee_rate is in 1e6 units (e.g., 2500 = 0.25%)
    // Convert to bps: fee_rate / 100
    let fee_rate_bps = fee_rate.map(|f| f / 100);

    Ok(PoolState {
        object_id: meta.object_id.clone(),
        dex: Dex::Cetus,
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
        fee_type: None,
    })
}
