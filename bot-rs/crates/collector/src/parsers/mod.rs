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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_meta() -> PoolMeta {
        PoolMeta {
            object_id: "0xpool".to_string(),
            dex: "test".to_string(),
            coin_type_a: "0x2::sui::SUI".to_string(),
            coin_type_b: "0xusdc::usdc::USDC".to_string(),
        }
    }

    // ── field_u64 / field_u128 helper tests ──

    #[test]
    fn test_field_u64_numeric() {
        let fields = json!({"fee": 3000});
        assert_eq!(field_u64(&fields, "fee").unwrap(), 3000);
    }

    #[test]
    fn test_field_u64_string() {
        let fields = json!({"fee": "3000"});
        assert_eq!(field_u64(&fields, "fee").unwrap(), 3000);
    }

    #[test]
    fn test_field_u64_missing() {
        assert!(field_u64(&json!({}), "fee").is_err());
    }

    #[test]
    fn test_field_u64_invalid_string() {
        assert!(field_u64(&json!({"fee": "not_a_number"}), "fee").is_err());
    }

    #[test]
    fn test_field_u64_null() {
        assert!(field_u64(&json!({"fee": null}), "fee").is_err());
    }

    #[test]
    fn test_field_u128_string() {
        let fields = json!({"sqrt_price": "18446744073709551616"});
        assert_eq!(field_u128(&fields, "sqrt_price").unwrap(), 1u128 << 64);
    }

    #[test]
    fn test_field_u128_missing() {
        assert!(field_u128(&json!({}), "sqrt_price").is_err());
    }

    #[test]
    fn test_field_u128_max_value() {
        let fields = json!({"val": u128::MAX.to_string()});
        assert_eq!(field_u128(&fields, "val").unwrap(), u128::MAX);
    }

    // ── Cetus parser tests ──

    #[test]
    fn test_cetus_parse_full() {
        let content = json!({
            "fields": {
                "current_sqrt_price": "18446744073709551616",
                "liquidity": "1000000000",
                "current_tick_index": {
                    "fields": { "bits": 4294967196u64 }
                },
                "fee_rate": 2500
            }
        });
        let pool = cetus::parse(&content, &test_meta(), 12345).unwrap();
        assert_eq!(pool.dex, arb_types::pool::Dex::Cetus);
        assert_eq!(pool.sqrt_price, Some(1u128 << 64));
        assert_eq!(pool.liquidity, Some(1_000_000_000));
        // 4294967196 as u32 = 0xFFFFFF9C → as i32 = -100
        assert_eq!(pool.tick_index, Some(-100));
        assert_eq!(pool.fee_rate_bps, Some(25)); // 2500/100
        assert_eq!(pool.last_updated_ms, 12345);
    }

    #[test]
    fn test_cetus_parse_positive_tick() {
        let content = json!({
            "fields": {
                "current_sqrt_price": "18446744073709551616",
                "liquidity": "1000",
                "current_tick_index": { "fields": { "bits": 100u64 } },
                "fee_rate": 3000
            }
        });
        let pool = cetus::parse(&content, &test_meta(), 0).unwrap();
        assert_eq!(pool.tick_index, Some(100));
    }

    #[test]
    fn test_cetus_parse_missing_fields() {
        assert!(cetus::parse(&json!({}), &test_meta(), 0).is_err());
    }

    #[test]
    fn test_cetus_parse_partial_optional_none() {
        let content = json!({ "fields": { "fee_rate": 2500 } });
        let pool = cetus::parse(&content, &test_meta(), 0).unwrap();
        assert_eq!(pool.sqrt_price, None);
        assert_eq!(pool.liquidity, None);
        assert_eq!(pool.tick_index, None);
        assert_eq!(pool.fee_rate_bps, Some(25));
    }

    // ── Turbos parser tests ──

    #[test]
    fn test_turbos_parse_full() {
        let content = json!({
            "fields": {
                "sqrt_price": "18446744073709551616",
                "liquidity": "500000",
                "tick_current_index": { "fields": { "bits": 42u64 } },
                "fee": 3000
            }
        });
        let pool = turbos::parse(&content, &test_meta(), 999).unwrap();
        assert_eq!(pool.dex, arb_types::pool::Dex::Turbos);
        assert_eq!(pool.sqrt_price, Some(1u128 << 64));
        assert_eq!(pool.tick_index, Some(42));
        assert_eq!(pool.fee_rate_bps, Some(30));
    }

    #[test]
    fn test_turbos_parse_negative_tick() {
        let content = json!({
            "fields": {
                "sqrt_price": "1000",
                "liquidity": "1000",
                "tick_current_index": { "fields": { "bits": 4294967295u64 } },
                "fee": 500
            }
        });
        let pool = turbos::parse(&content, &test_meta(), 0).unwrap();
        assert_eq!(pool.tick_index, Some(-1));
    }

    // ── DeepBook parser tests ──

    #[test]
    fn test_deepbook_parse_with_balances() {
        let content = json!({
            "fields": {
                "base_vault": { "fields": { "balance": 1000000u64 } },
                "quote_vault": { "fields": { "balance": "2000000" } }
            }
        });
        let pool = deepbook::parse(&content, &test_meta(), 0).unwrap();
        assert_eq!(pool.dex, arb_types::pool::Dex::DeepBook);
        assert_eq!(pool.reserve_a, Some(1_000_000));
        assert_eq!(pool.reserve_b, Some(2_000_000));
        assert_eq!(pool.sqrt_price, None);
    }

    #[test]
    fn test_deepbook_parse_v3_value_field() {
        // V3 PoolInner uses Balance<T> which serializes as { fields: { value: "..." } }
        let content = json!({
            "fields": {
                "base_vault": { "fields": { "value": "921627040035451" } },
                "quote_vault": { "fields": { "value": "943352018975" } },
                "taker_fee": "1000"
            }
        });
        let pool = deepbook::parse(&content, &test_meta(), 0).unwrap();
        assert_eq!(pool.dex, arb_types::pool::Dex::DeepBook);
        assert_eq!(pool.reserve_a, Some(921_627_040_035_451));
        assert_eq!(pool.reserve_b, Some(943_352_018_975));
        assert_eq!(pool.fee_rate_bps, Some(1000));
    }

    #[test]
    fn test_deepbook_parse_missing_vaults() {
        let content = json!({ "fields": {} });
        let pool = deepbook::parse(&content, &test_meta(), 0).unwrap();
        assert_eq!(pool.reserve_a, None);
        assert_eq!(pool.reserve_b, None);
    }

    // ── Aftermath parser tests ──

    #[test]
    fn test_aftermath_parse_with_balances() {
        // Aftermath normalized_balances are 18-decimal fixed-point strings.
        // Parser derives synthetic reserves preserving the price ratio.
        let content = json!({
            "fields": { "normalized_balances": ["5000000", "10000000"] }
        });
        let pool = aftermath::parse(&content, &test_meta(), 0).unwrap();
        assert_eq!(pool.dex, arb_types::pool::Dex::Aftermath);
        // ratio = 10M / 5M = 2.0 → reserve_a = 1B (virtual depth), reserve_b = 2B
        assert_eq!(pool.reserve_a, Some(1_000_000_000));
        assert_eq!(pool.reserve_b, Some(2_000_000_000));
    }

    #[test]
    fn test_aftermath_parse_large_balances() {
        // Real mainnet values: u128-scale that overflow u64
        let content = json!({
            "fields": {
                "normalized_balances": [
                    "27968666076858000000000000000000",
                    "104839831283000000000000000000000"
                ],
                "fees_swap_in": ["2500000000000000"]
            }
        });
        let pool = aftermath::parse(&content, &test_meta(), 0).unwrap();
        assert_eq!(pool.reserve_a, Some(1_000_000_000));
        // ratio ≈ 3.749 → reserve_b ≈ 3.749B
        assert!(pool.reserve_b.unwrap() > 3_000_000_000);
        assert!(pool.reserve_b.unwrap() < 4_000_000_000);
        // 2500000000000000 / 1e18 * 10000 = 25 bps
        assert_eq!(pool.fee_rate_bps, Some(25));
    }

    #[test]
    fn test_aftermath_parse_empty_balances() {
        let content = json!({ "fields": { "normalized_balances": [] } });
        let pool = aftermath::parse(&content, &test_meta(), 0).unwrap();
        assert_eq!(pool.reserve_a, None);
        assert_eq!(pool.reserve_b, None);
    }

    // ── FlowX CLMM parser tests ──

    #[test]
    fn test_flowx_parse_full() {
        let content = json!({
            "fields": {
                "sqrt_price": "18446744073709551616",
                "liquidity": "999999",
                "tick_index": { "fields": { "bits": 50u64 } },
                "swap_fee_rate": 2000
            }
        });
        let pool = flowx::parse(&content, &test_meta(), 0).unwrap();
        assert_eq!(pool.dex, arb_types::pool::Dex::FlowxClmm);
        assert_eq!(pool.sqrt_price, Some(1u128 << 64));
        assert_eq!(pool.tick_index, Some(50));
        assert_eq!(pool.fee_rate_bps, Some(20));
    }

    // ── FlowX AMM parser tests ──

    #[test]
    fn test_flowx_amm_parse_direct() {
        let content = json!({
            "fields": {
                "reserve_x": 1000000u64,
                "reserve_y": "2000000",
                "fee_rate": 30
            }
        });
        let pool = flowx_amm::parse(&content, &test_meta(), 0).unwrap();
        assert_eq!(pool.dex, arb_types::pool::Dex::FlowxAmm);
        assert_eq!(pool.reserve_a, Some(1_000_000));
        assert_eq!(pool.reserve_b, Some(2_000_000));
        assert_eq!(pool.fee_rate_bps, Some(30));
    }

    #[test]
    fn test_flowx_amm_parse_dynamic_field_wrapper() {
        let content = json!({
            "fields": {
                "name": {"type": "SomeType"},
                "value": {
                    "fields": {
                        "reserve_x": 500000u64,
                        "reserve_y": 1000000u64,
                        "fee_rate": 25
                    }
                }
            }
        });
        let pool = flowx_amm::parse(&content, &test_meta(), 0).unwrap();
        assert_eq!(pool.reserve_a, Some(500_000));
        assert_eq!(pool.reserve_b, Some(1_000_000));
        assert_eq!(pool.fee_rate_bps, Some(25));
    }

    // ── parse_pool_object router tests ──

    #[test]
    fn test_route_to_cetus() {
        let content = json!({
            "fields": {
                "current_sqrt_price": "1000",
                "liquidity": "1000",
                "current_tick_index": { "fields": { "bits": 0u64 } },
                "fee_rate": 3000
            }
        });
        let pool = parse_pool_object(&content, "cetus", &test_meta(), 0).unwrap();
        assert_eq!(pool.dex, arb_types::pool::Dex::Cetus);
    }

    #[test]
    fn test_route_case_insensitive() {
        let content = json!({
            "fields": {
                "current_sqrt_price": "1000",
                "liquidity": "1000",
                "current_tick_index": { "fields": { "bits": 0u64 } },
                "fee_rate": 3000
            }
        });
        let pool = parse_pool_object(&content, "CETUS", &test_meta(), 0).unwrap();
        assert_eq!(pool.dex, arb_types::pool::Dex::Cetus);
    }

    #[test]
    fn test_route_unknown_dex() {
        assert!(parse_pool_object(&json!({"fields": {}}), "unknown_dex", &test_meta(), 0).is_err());
    }

    #[test]
    fn test_route_flowx_alias() {
        let content = json!({
            "fields": {
                "sqrt_price": "1000", "liquidity": "1000",
                "tick_index": { "fields": { "bits": 0u64 } },
                "swap_fee_rate": 1000
            }
        });
        let p1 = parse_pool_object(&content, "flowx", &test_meta(), 0).unwrap();
        let p2 = parse_pool_object(&content, "flowx_clmm", &test_meta(), 0).unwrap();
        assert_eq!(p1.dex, arb_types::pool::Dex::FlowxClmm);
        assert_eq!(p2.dex, arb_types::pool::Dex::FlowxClmm);
    }
}
