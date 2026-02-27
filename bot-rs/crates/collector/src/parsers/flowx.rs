use anyhow::{Context, Result};
use arb_types::pool::{Dex, PoolState};
use serde_json::Value;

use super::{field_u128, field_u64, PoolMeta};

/// Parse a FlowX CLMM v3 Pool object.
///
/// FlowX CLMM Pool<A, B> has the same structure as Cetus:
/// - sqrt_price: u128
/// - tick_index: I32
/// - liquidity: u128
/// - swap_fee_rate: u64
pub(crate) fn parse(content: &Value, meta: &PoolMeta, now_ms: u64) -> Result<PoolState> {
    let fields = content
        .get("fields")
        .context("Missing fields in FlowX pool")?;

    let sqrt_price = field_u128(fields, "sqrt_price").ok();
    let liquidity = field_u128(fields, "liquidity").ok();

    // Tick index stored as I32 { bits: u32 } (two's complement).
    let tick_index = fields
        .get("tick_index")
        .and_then(|v| v.get("fields"))
        .and_then(|f| f.get("bits"))
        .and_then(|b| {
            b.as_u64()
                .map(|bits| (bits as u32) as i32)
                .or_else(|| b.as_i64().map(|v| v as i32))
        });

    let fee_rate = field_u64(fields, "swap_fee_rate").ok();
    let fee_rate_bps = fee_rate.map(|f| f / 100);

    Ok(PoolState {
        object_id: meta.object_id.clone(),
        dex: Dex::FlowxClmm,
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
