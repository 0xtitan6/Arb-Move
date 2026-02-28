use anyhow::{Context, Result};

/// Bot configuration loaded from environment variables.
#[derive(Debug, Clone)]
pub struct Config {
    // ── Network ──
    pub rpc_url: String,

    // ── Wallet ──
    pub private_key_hex: String,

    // ── Deployed package ──
    pub package_id: String,
    pub admin_cap_id: String,
    pub pause_flag_id: String,

    // ── DEX shared objects ──
    pub cetus_global_config: String,
    pub turbos_versioned: String,
    pub flowx_versioned: String,

    // ── Aftermath shared objects ──
    pub aftermath_registry: String,
    pub aftermath_fee_vault: String,
    pub aftermath_treasury: String,
    pub aftermath_insurance: String,
    pub aftermath_referral: String,

    // ── FlowX AMM ──
    /// FlowX AMM shared Container object.
    pub flowx_container: String,

    // ── DeepBook ──
    /// An owned Coin<DEEP> object ID for DeepBook fee payments.
    /// Required for any strategy involving DeepBook.
    pub deep_fee_coin_id: String,

    // ── Pool monitoring ──
    pub monitored_pools: Vec<PoolConfig>,

    // ── Strategy params ──
    pub min_profit_mist: u64,
    pub poll_interval_ms: u64,
    pub max_gas_budget: u64,
    pub dry_run_before_submit: bool,

    // ── Circuit breaker ──
    pub cb_max_consecutive_failures: u32,
    pub cb_max_cumulative_loss_mist: i64,
    pub cb_cooldown_ms: u64,
}

/// Configuration for a single monitored pool.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    pub dex: String,
    pub pool_id: String,
    pub coin_type_a: String,
    pub coin_type_b: String,
}

impl Config {
    /// Load configuration from environment variables.
    /// Call `dotenvy::dotenv().ok()` before calling this.
    pub fn from_env() -> Result<Self> {
        let monitored_pools = std::env::var("MONITORED_POOLS")
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.trim().is_empty())
            .filter_map(|entry| parse_pool_entry(entry.trim()))
            .collect();

        Ok(Config {
            rpc_url: env_var("SUI_RPC_URL")?,
            private_key_hex: env_var("SUI_PRIVATE_KEY")?,
            package_id: env_var("PACKAGE_ID")?,
            admin_cap_id: env_var("ADMIN_CAP_ID")?,
            pause_flag_id: env_var("PAUSE_FLAG_ID")?,
            cetus_global_config: env_var("CETUS_GLOBAL_CONFIG")?,
            turbos_versioned: env_var("TURBOS_VERSIONED")?,
            flowx_versioned: env_var_or("FLOWX_VERSIONED", ""),
            aftermath_registry: env_var_or("AFTERMATH_REGISTRY", ""),
            aftermath_fee_vault: env_var_or("AFTERMATH_FEE_VAULT", ""),
            aftermath_treasury: env_var_or("AFTERMATH_TREASURY", ""),
            aftermath_insurance: env_var_or("AFTERMATH_INSURANCE", ""),
            aftermath_referral: env_var_or("AFTERMATH_REFERRAL", ""),
            flowx_container: env_var_or("FLOWX_CONTAINER", ""),
            deep_fee_coin_id: env_var_or("DEEP_FEE_COIN_ID", ""),
            monitored_pools,
            min_profit_mist: env_var_or("MIN_PROFIT_MIST", "1000000")
                .parse()
                .context("Invalid MIN_PROFIT_MIST")?,
            poll_interval_ms: env_var_or("POLL_INTERVAL_MS", "500")
                .parse()
                .context("Invalid POLL_INTERVAL_MS")?,
            max_gas_budget: env_var_or("MAX_GAS_BUDGET", "50000000")
                .parse()
                .context("Invalid MAX_GAS_BUDGET")?,
            dry_run_before_submit: env_var_or("DRY_RUN_BEFORE_SUBMIT", "true")
                .parse()
                .unwrap_or(true),
            cb_max_consecutive_failures: env_var_or("CB_MAX_CONSECUTIVE_FAILURES", "5")
                .parse()
                .context("Invalid CB_MAX_CONSECUTIVE_FAILURES")?,
            cb_max_cumulative_loss_mist: env_var_or("CB_MAX_CUMULATIVE_LOSS_MIST", "1000000000")
                .parse()
                .context("Invalid CB_MAX_CUMULATIVE_LOSS_MIST")?,
            cb_cooldown_ms: env_var_or("CB_COOLDOWN_MS", "60000")
                .parse()
                .context("Invalid CB_COOLDOWN_MS")?,
        })
    }
}

/// Parse a single pool config entry.
///
/// Format: `DEX:POOL_ID:COIN_TYPE_A:COIN_TYPE_B`
///
/// Coin types use `::` as a Move path separator (e.g., `0x2::sui::SUI`),
/// so we can't naively split on `:`. Instead we:
///   1. Extract DEX (first `:`)
///   2. Extract POOL_ID (second `:`)
///   3. Split the remaining string at `:0x` to separate the two coin types,
///      since each coin type starts with a `0x` hex address.
fn parse_pool_entry(entry: &str) -> Option<PoolConfig> {
    // Step 1: DEX is before the first ':'
    let colon1 = entry.find(':')?;
    let dex = &entry[..colon1];

    // Step 2: POOL_ID is between the first and second ':'
    let rest1 = &entry[colon1 + 1..];
    let colon2 = rest1.find(':')?;
    let pool_id = &rest1[..colon2];

    // Step 3: Remaining is "COIN_TYPE_A:COIN_TYPE_B"
    // Both coin types start with "0x", so find ":0x" as the field boundary.
    // This works because "::" in Move paths never produces ":0x" (modules are
    // alphanumeric, not hex-prefixed).
    let rest2 = &rest1[colon2 + 1..];
    let boundary = rest2.find(":0x")?;
    let coin_type_a = &rest2[..boundary];
    let coin_type_b = &rest2[boundary + 1..]; // skip the ':'

    if dex.is_empty() || pool_id.is_empty() || coin_type_a.is_empty() || coin_type_b.is_empty() {
        eprintln!("WARN: Skipping malformed pool config: {entry}");
        return None;
    }

    Some(PoolConfig {
        dex: dex.to_string(),
        pool_id: pool_id.to_string(),
        coin_type_a: coin_type_a.to_string(),
        coin_type_b: coin_type_b.to_string(),
    })
}

fn env_var(name: &str) -> Result<String> {
    std::env::var(name).with_context(|| format!("Missing environment variable: {name}"))
}

fn env_var_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_config_parse_valid_full_types() {
        let entry = "cetus:0xcf994611fd4c48e277ce3ffd4d4364c914af2c3cbb05f7bf6facd371de688630:0x0000000000000000000000000000000000000000000000000000000000000002::sui::SUI:0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC";
        let pc = parse_pool_entry(entry).expect("Should parse valid entry");
        assert_eq!(pc.dex, "cetus");
        assert_eq!(pc.pool_id, "0xcf994611fd4c48e277ce3ffd4d4364c914af2c3cbb05f7bf6facd371de688630");
        assert_eq!(pc.coin_type_a, "0x0000000000000000000000000000000000000000000000000000000000000002::sui::SUI");
        assert_eq!(pc.coin_type_b, "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC");
    }

    #[test]
    fn test_pool_config_parse_deep_sui() {
        let entry = "turbos:0xbca476e3c744648c65b1fae5551b86be8ad7f482ca9c2268dad1d6b4fd0e2635:0xdeeb7a4662eec9f2f3def03fb937a663dddaa2e215b8078a284d026b7946c270::deep::DEEP:0x0000000000000000000000000000000000000000000000000000000000000002::sui::SUI";
        let pc = parse_pool_entry(entry).expect("Should parse DEEP/SUI");
        assert_eq!(pc.dex, "turbos");
        assert_eq!(pc.coin_type_a, "0xdeeb7a4662eec9f2f3def03fb937a663dddaa2e215b8078a284d026b7946c270::deep::DEEP");
        assert_eq!(pc.coin_type_b, "0x0000000000000000000000000000000000000000000000000000000000000002::sui::SUI");
    }

    #[test]
    fn test_pool_config_parse_reversed() {
        // The big Cetus pool has USDC/SUI ordering (reversed)
        let entry = "cetus:0xb8d7d9e66a60c239e7a60110efcf8de6c705580ed924d0dde141f4a0e2c90105:0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC:0x0000000000000000000000000000000000000000000000000000000000000002::sui::SUI";
        let pc = parse_pool_entry(entry).expect("Should parse reversed pair");
        assert_eq!(pc.coin_type_a, "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC");
        assert_eq!(pc.coin_type_b, "0x0000000000000000000000000000000000000000000000000000000000000002::sui::SUI");
    }

    #[test]
    fn test_pool_config_parse_malformed_skipped() {
        assert!(parse_pool_entry("bad_entry").is_none());
        assert!(parse_pool_entry("cetus:0xpool1").is_none());
        assert!(parse_pool_entry("cetus:0xpool1:onlyone").is_none());
        assert!(parse_pool_entry("").is_none());
    }

    #[test]
    fn test_pool_config_empty_string() {
        let entries = "";
        let parsed: Vec<&str> = entries
            .split(',')
            .filter(|s| !s.trim().is_empty())
            .collect();
        assert!(parsed.is_empty());
    }

    #[test]
    fn test_pool_config_multiple_valid() {
        let entries = "cetus:0x1:0x2::sui::SUI:0xdba3::usdc::USDC,turbos:0x2:0x2::sui::SUI:0xdba3::usdc::USDC,deepbook:0x3:0x2::sui::SUI:0xdba3::usdc::USDC";
        let parsed: Vec<PoolConfig> = entries
            .split(',')
            .filter(|s| !s.trim().is_empty())
            .filter_map(|entry| parse_pool_entry(entry.trim()))
            .collect();
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].coin_type_a, "0x2::sui::SUI");
        assert_eq!(parsed[0].coin_type_b, "0xdba3::usdc::USDC");
    }

    #[test]
    fn test_env_var_or_defaults() {
        let val = env_var_or("NONEXISTENT_TEST_VAR_12345", "default_value");
        assert_eq!(val, "default_value");
    }

    #[test]
    fn test_env_var_missing_errors() {
        assert!(env_var("NONEXISTENT_TEST_VAR_12345").is_err());
    }

    #[test]
    fn test_numeric_parse_defaults() {
        let min_profit: u64 = "1000000".parse().unwrap();
        assert_eq!(min_profit, 1_000_000);
        let poll_ms: u64 = "500".parse().unwrap();
        assert_eq!(poll_ms, 500);
        let gas_budget: u64 = "50000000".parse().unwrap();
        assert_eq!(gas_budget, 50_000_000);
        assert!("true".parse::<bool>().unwrap());
    }

    #[test]
    fn test_numeric_parse_invalid() {
        assert!("not_a_number".parse::<u64>().is_err());
        assert!("".parse::<u64>().is_err());
        assert!("-1".parse::<u64>().is_err());
    }
}
