//! Token decimal metadata for normalizing cross-DEX price comparisons.
//!
//! On Sui, different coins have different decimal precision:
//! - SUI: 9 decimals (1 SUI = 1_000_000_000 MIST)
//! - USDC: 6 decimals
//! - WETH: 8 decimals
//! - DEEP: 6 decimals
//! - USDT: 6 decimals
//!
//! When comparing prices from CLMM pools (sqrt_price in Q64.64) vs AMM pools
//! (reserve_b / reserve_a), the decimal difference between token A and B must
//! be factored in to get a real-world price comparison.

/// Known mainnet decimal counts, keyed by the last segment of the coin type.
/// e.g. `0x2::sui::SUI` → `SUI` → 9
///
/// Defaults to 9 (SUI-like) for unknown tokens.
pub fn decimals_for_coin_type(coin_type: &str) -> u8 {
    // Extract the last segment: "0x2::sui::SUI" → "SUI"
    let token_name = coin_type
        .rsplit("::")
        .next()
        .unwrap_or(coin_type)
        .to_uppercase();

    match token_name.as_str() {
        "SUI" => 9,
        "USDC" => 6,
        "USDT" => 6,
        "DEEP" => 6,
        "COIN" => {
            // "COIN" is used for wrapped tokens — try to infer from module name
            // e.g., "0xaf8cd...::coin::COIN" is wETH (8 decimals)
            if coin_type.contains("af8cd5edc19c4512") {
                8 // wETH on Sui
            } else if coin_type.contains("c060006111016b8a") {
                6 // USDT on Sui (wrapped)
            } else {
                9 // unknown wrapped — assume 9
            }
        }
        "WETH" | "ETH" => 8,
        "WBTC" | "BTC" => 8,
        "CETUS" => 9,
        "SCA" => 9,
        "TURBOS" => 9,
        "NAVX" => 9,
        "HASUI" | "AFSUI" | "VSUI" => 9, // liquid staking derivatives
        _ => 9, // default to 9 (SUI-standard)
    }
}

/// Compute the decimal adjustment factor for a price quoted as A-in-B.
///
/// If token A has `dec_a` decimals and token B has `dec_b` decimals,
/// the raw price ratio needs to be multiplied by `10^(dec_a - dec_b)`
/// to get the real-world price.
///
/// Returns the multiplier as `f64`. Values >1 mean B has fewer decimals
/// (price appears larger), <1 means A has fewer.
///
/// Example: SUI/USDC (9/6) → factor = 10^(9-6) = 1000
/// Raw price 0.003 → Real price 0.003 * 1000 = 3.0 USDC per SUI
pub fn decimal_adjustment_factor(coin_type_a: &str, coin_type_b: &str) -> f64 {
    let dec_a = decimals_for_coin_type(coin_type_a) as i32;
    let dec_b = decimals_for_coin_type(coin_type_b) as i32;
    let diff = dec_a - dec_b;
    10f64.powi(diff)
}

/// Normalize a raw price (from pool math) to a real-world price.
pub fn normalize_price(raw_price: f64, coin_type_a: &str, coin_type_b: &str) -> f64 {
    raw_price * decimal_adjustment_factor(coin_type_a, coin_type_b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sui_decimals() {
        assert_eq!(decimals_for_coin_type("0x0000000000000000000000000000000000000000000000000000000000000002::sui::SUI"), 9);
    }

    #[test]
    fn test_usdc_decimals() {
        assert_eq!(decimals_for_coin_type("0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC"), 6);
    }

    #[test]
    fn test_deep_decimals() {
        assert_eq!(decimals_for_coin_type("0xdeeb7a4662eec9f2f3def03fb937a663dddaa2e215b8078a284d026b7946c270::deep::DEEP"), 6);
    }

    #[test]
    fn test_weth_coin_wrapper() {
        assert_eq!(decimals_for_coin_type("0xaf8cd5edc19c4512f4259f0bee101a40d41ebed738ade5874359610ef8eeced5::coin::COIN"), 8);
    }

    #[test]
    fn test_unknown_defaults_to_9() {
        assert_eq!(decimals_for_coin_type("0xabc::unknown::UNKNOWN"), 9);
    }

    #[test]
    fn test_adjustment_factor_same_decimals() {
        let factor = decimal_adjustment_factor("0x2::sui::SUI", "0xabc::cetus::CETUS");
        assert!((factor - 1.0).abs() < 1e-10, "Same decimals → factor = 1.0");
    }

    #[test]
    fn test_adjustment_factor_sui_usdc() {
        let factor = decimal_adjustment_factor(
            "0x2::sui::SUI",
            "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC",
        );
        assert!((factor - 1000.0).abs() < 1e-10, "SUI(9) / USDC(6) → 1000, got {factor}");
    }

    #[test]
    fn test_adjustment_factor_usdc_sui() {
        let factor = decimal_adjustment_factor(
            "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC",
            "0x2::sui::SUI",
        );
        assert!((factor - 0.001).abs() < 1e-10, "USDC(6) / SUI(9) → 0.001, got {factor}");
    }

    #[test]
    fn test_normalize_price_sui_usdc() {
        // Raw sqrt_price-derived price for SUI/USDC is ~0.003 (before normalization)
        let raw = 0.003;
        let normalized = normalize_price(
            raw,
            "0x2::sui::SUI",
            "0xdba3::usdc::USDC",
        );
        assert!((normalized - 3.0).abs() < 1e-10, "Normalized should be ~3.0, got {normalized}");
    }

    #[test]
    fn test_normalize_price_same_decimals() {
        let raw = 1.5;
        let normalized = normalize_price(raw, "0x2::sui::SUI", "0xabc::cetus::CETUS");
        assert!((normalized - 1.5).abs() < 1e-10);
    }
}
