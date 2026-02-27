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
            .filter_map(|entry| {
                let parts: Vec<&str> = entry.trim().split(':').collect();
                if parts.len() >= 4 {
                    Some(PoolConfig {
                        dex: parts[0].to_string(),
                        pool_id: parts[1].to_string(),
                        coin_type_a: parts[2].to_string(),
                        coin_type_b: parts[3].to_string(),
                    })
                } else {
                    eprintln!("WARN: Skipping malformed pool config: {entry}");
                    None
                }
            })
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
        })
    }
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
    fn test_pool_config_parse_valid() {
        let entry = "cetus:0xpool1:0x2::sui::SUI:0xusdc::usdc::USDC";
        let parts: Vec<&str> = entry.split(':').collect();
        assert!(parts.len() >= 4);
        let pc = PoolConfig {
            dex: parts[0].to_string(),
            pool_id: parts[1].to_string(),
            coin_type_a: parts[2].to_string(),
            coin_type_b: parts[3].to_string(),
        };
        assert_eq!(pc.dex, "cetus");
        assert_eq!(pc.pool_id, "0xpool1");
    }

    #[test]
    fn test_pool_config_parse_malformed_skipped() {
        let entries = "cetus:0xpool1,bad_entry,turbos:0xpool2:SUI:USDC";
        let parsed: Vec<PoolConfig> = entries
            .split(',')
            .filter(|s| !s.trim().is_empty())
            .filter_map(|entry| {
                let parts: Vec<&str> = entry.trim().split(':').collect();
                if parts.len() >= 4 {
                    Some(PoolConfig {
                        dex: parts[0].to_string(),
                        pool_id: parts[1].to_string(),
                        coin_type_a: parts[2].to_string(),
                        coin_type_b: parts[3].to_string(),
                    })
                } else {
                    None
                }
            })
            .collect();

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].dex, "turbos");
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
        let entries = "cetus:0x1:SUI:USDC,turbos:0x2:SUI:USDC,deepbook:0x3:SUI:USDC";
        let parsed: Vec<PoolConfig> = entries
            .split(',')
            .filter(|s| !s.trim().is_empty())
            .filter_map(|entry| {
                let parts: Vec<&str> = entry.trim().split(':').collect();
                if parts.len() >= 4 {
                    Some(PoolConfig {
                        dex: parts[0].to_string(),
                        pool_id: parts[1].to_string(),
                        coin_type_a: parts[2].to_string(),
                        coin_type_b: parts[3].to_string(),
                    })
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(parsed.len(), 3);
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
