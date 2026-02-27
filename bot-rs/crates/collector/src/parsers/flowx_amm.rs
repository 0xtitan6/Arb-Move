use anyhow::{Context, Result};
use arb_types::pool::{Dex, PoolState};
use serde_json::Value;

use super::{field_u64, PoolMeta};

/// Parse a FlowX AMM v2 Pool object (constant-product / xy=k).
///
/// On-chain, FlowX AMM pools are stored as dynamic fields inside the
/// shared `Container` object. When fetched via `sui_getObject`, they
/// may appear wrapped in `dynamic_field::Field`:
///
///   { "fields": { "name": {...}, "value": { "fields": { ... } } } }
///
/// Pool fields (PairMetadata<X, Y>):
/// - reserve_x: u64
/// - reserve_y: u64
/// - k_last: u128 (unused by us)
/// - fee_rate: u64
pub(crate) fn parse(content: &Value, meta: &PoolMeta, now_ms: u64) -> Result<PoolState> {
    let fields = content
        .get("fields")
        .context("Missing fields in FlowX AMM pool")?;

    // Handle dynamic_field::Field wrapper — check if fields has a "value" subobject
    let inner_fields = if let Some(value) = fields.get("value") {
        // Wrapped in dynamic_field::Field { name, value: PairMetadata { ... } }
        value
            .get("fields")
            .unwrap_or(value)
    } else {
        // Direct PairMetadata fields
        fields
    };

    let reserve_a = field_u64(inner_fields, "reserve_x").ok();
    let reserve_b = field_u64(inner_fields, "reserve_y").ok();
    let fee_rate = field_u64(inner_fields, "fee_rate").ok();
    // FlowX AMM fee_rate is typically in bps (e.g. 30 = 0.3%)
    let fee_rate_bps = fee_rate;

    Ok(PoolState {
        object_id: meta.object_id.clone(),
        dex: Dex::FlowxAmm,
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
    })
}
