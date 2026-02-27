pub mod aftermath;
pub mod cetus;
pub mod deepbook;
pub mod flowx;
pub mod flowx_amm;
pub mod turbos;

use anyhow::{Context, Result};
use arb_types::pool::PoolState;
use serde_json::Value;

/// Route to the correct parser based on DEX name.
pub(crate) fn parse_pool_object(
    content: &Value,
    dex: &str,
    meta: &PoolMeta,
    now_ms: u64,
) -> Result<PoolState> {
    match dex.to_lowercase().as_str() {
        "cetus" => cetus::parse(content, meta, now_ms),
        "turbos" => turbos::parse(content, meta, now_ms),
        "deepbook" => deepbook::parse(content, meta, now_ms),
        "aftermath" => aftermath::parse(content, meta, now_ms),
        "flowx_clmm" | "flowx" => flowx::parse(content, meta, now_ms),
        "flowx_amm" => flowx_amm::parse(content, meta, now_ms),
        _ => anyhow::bail!("Unknown DEX: {dex}"),
    }
}

/// Helper: extract a u64 field from Move struct fields.
/// Handles both string-encoded ("12345") and numeric JSON values.
pub(crate) fn field_u64(fields: &Value, name: &str) -> Result<u64> {
    let v = fields
        .get(name)
        .with_context(|| format!("Missing field: {name}"))?;
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
        .with_context(|| format!("Invalid u64 field: {name}"))
}

/// Helper: extract a u128 field from Move struct fields.
pub(crate) fn field_u128(fields: &Value, name: &str) -> Result<u128> {
    fields
        .get(name)
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u128>().ok())
        .with_context(|| format!("Missing or invalid u128 field: {name}"))
}

/// Helper: extract a string field.
#[allow(dead_code)]
pub(crate) fn field_str<'a>(fields: &'a Value, name: &str) -> Result<&'a str> {
    fields
        .get(name)
        .and_then(|v| v.as_str())
        .with_context(|| format!("Missing string field: {name}"))
}

// Re-export PoolMeta for parser modules
pub(crate) use crate::rpc_poller::PoolMeta;
