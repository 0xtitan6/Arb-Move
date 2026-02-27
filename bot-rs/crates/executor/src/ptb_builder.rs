use anyhow::{Context, Result};
use arb_types::config::Config;
use arb_types::opportunity::{ArbOpportunity, StrategyType};
use reqwest::Client;
use serde_json::{json, Value};
use tracing::debug;

/// Builds Programmable Transaction Blocks (PTBs) for arb strategies.
///
/// Each strategy maps to a specific Move entry function call with
/// the correct object IDs, type arguments, and value arguments.
pub struct PtbBuilder {
    client: Client,
    rpc_url: String,
    package_id: String,
    admin_cap_id: String,
    pause_flag_id: String,
    sender: String,
    gas_budget: u64,
    // DEX shared objects
    cetus_global_config: String,
    turbos_versioned: String,
    flowx_versioned: String,
    // Aftermath shared objects
    aftermath_registry: String,
    aftermath_fee_vault: String,
    aftermath_treasury: String,
    aftermath_insurance: String,
    aftermath_referral: String,
    // FlowX AMM
    flowx_container: String,
    // DeepBook fee coin
    deep_fee_coin_id: String,
}

impl PtbBuilder {
    pub fn new(config: &Config, sender: &str) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .expect("Failed to create HTTP client"),
            rpc_url: config.rpc_url.clone(),
            package_id: config.package_id.clone(),
            admin_cap_id: config.admin_cap_id.clone(),
            pause_flag_id: config.pause_flag_id.clone(),
            sender: sender.to_string(),
            gas_budget: config.max_gas_budget,
            cetus_global_config: config.cetus_global_config.clone(),
            turbos_versioned: config.turbos_versioned.clone(),
            flowx_versioned: config.flowx_versioned.clone(),
            flowx_container: config.flowx_container.clone(),
            aftermath_registry: config.aftermath_registry.clone(),
            aftermath_fee_vault: config.aftermath_fee_vault.clone(),
            aftermath_treasury: config.aftermath_treasury.clone(),
            aftermath_insurance: config.aftermath_insurance.clone(),
            aftermath_referral: config.aftermath_referral.clone(),
            deep_fee_coin_id: config.deep_fee_coin_id.clone(),
        }
    }

    /// Build a transaction for the given opportunity.
    /// Returns the serialized transaction bytes (base64).
    pub async fn build(&self, opp: &ArbOpportunity) -> Result<String> {
        let module = opp.strategy.move_module();
        let function = opp.strategy.move_function_name();

        let (args, type_args) = self.build_args(opp)?;

        debug!(
            module = %module,
            function = %function,
            amount = %opp.amount_in,
            "Building PTB"
        );

        // Use unsafe_moveCall to build the transaction
        let response = self
            .client
            .post(&self.rpc_url)
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "unsafe_moveCall",
                "params": [
                    self.sender,
                    self.package_id,
                    module,
                    function,
                    type_args,
                    args,
                    null,  // gas object (auto-select)
                    self.gas_budget.to_string(),
                ]
            }))
            .send()
            .await
            .context("Failed to build PTB via RPC")?;

        let body: Value = response.json().await?;

        if let Some(error) = body.get("error") {
            anyhow::bail!("PTB build error: {}", error);
        }

        let tx_bytes = body
            .get("result")
            .and_then(|r| r.get("txBytes"))
            .and_then(|t| t.as_str())
            .context("Missing txBytes in response")?
            .to_string();

        Ok(tx_bytes)
    }

    // ── Argument helpers ──

    /// Common prefix: admin_cap, pause_flag
    fn base_args(&self) -> Vec<Value> {
        vec![json!(self.admin_cap_id), json!(self.pause_flag_id)]
    }

    /// Aftermath shared object arguments (6 objects).
    fn aftermath_args(&self, aftermath_pool_id: &str) -> Vec<Value> {
        vec![
            json!(aftermath_pool_id),
            json!(self.aftermath_registry),
            json!(self.aftermath_fee_vault),
            json!(self.aftermath_treasury),
            json!(self.aftermath_insurance),
            json!(self.aftermath_referral),
        ]
    }

    /// Tail arguments: amount, min_profit, clock.
    fn tail_args(&self, amount: &str, min_profit: &str) -> Vec<Value> {
        vec![json!(amount), json!(min_profit), json!("0x6")]
    }

    /// Build the argument list for a specific strategy.
    fn build_args(&self, opp: &ArbOpportunity) -> Result<(Vec<Value>, Vec<String>)> {
        // Validate pool_ids length matches strategy requirements
        let expected_pools = if opp.strategy.move_module() == "tri_hop" { 3 } else { 2 };
        anyhow::ensure!(
            opp.pool_ids.len() >= expected_pools,
            "Strategy {:?} requires {} pool IDs, got {}",
            opp.strategy,
            expected_pools,
            opp.pool_ids.len()
        );

        let amount = opp.amount_in.to_string();
        // Use 90% of expected_profit as min_profit guard (tight but allows for minor slippage).
        // Floor at 1 MIST so the on-chain assert_profit() check is never no-op.
        let min_profit_raw = opp.expected_profit * 9 / 10;
        let min_profit = min_profit_raw.max(1).to_string();

        debug!(
            amount = %amount,
            min_profit = %min_profit,
            expected_profit = %opp.expected_profit,
            "PTB min_profit guard"
        );

        let args = match opp.strategy {
            // ═══════════════════════════════════════
            //  Two-hop: Cetus ↔ Turbos
            // ═══════════════════════════════════════
            StrategyType::CetusToTurbos | StrategyType::CetusToTurbosRev => {
                let mut a = self.base_args();
                a.push(json!(self.cetus_global_config));
                a.push(json!(opp.pool_ids[0])); // cetus_pool
                a.push(json!(opp.pool_ids[1])); // turbos_pool
                a.push(json!(self.turbos_versioned));
                a.extend(self.tail_args(&amount, &min_profit));
                a
            }

            StrategyType::TurbosToCetus => {
                let mut a = self.base_args();
                a.push(json!(self.cetus_global_config));
                a.push(json!(opp.pool_ids[1])); // cetus_pool
                a.push(json!(opp.pool_ids[0])); // turbos_pool (flash source)
                a.push(json!(self.turbos_versioned));
                a.extend(self.tail_args(&amount, &min_profit));
                a
            }

            // ═══════════════════════════════════════
            //  Two-hop: Cetus ↔ DeepBook
            // ═══════════════════════════════════════
            StrategyType::CetusToDeepBook => {
                let mut a = self.base_args();
                a.push(json!(self.cetus_global_config));
                a.push(json!(opp.pool_ids[0])); // cetus_pool
                a.push(json!(opp.pool_ids[1])); // deepbook_pool
                a.push(json!(self.deep_fee_coin_id));
                a.extend(self.tail_args(&amount, &min_profit));
                a
            }

            StrategyType::DeepBookToCetus => {
                let mut a = self.base_args();
                a.push(json!(self.cetus_global_config));
                a.push(json!(opp.pool_ids[1])); // cetus_pool
                a.push(json!(opp.pool_ids[0])); // deepbook_pool (flash source)
                a.push(json!(self.deep_fee_coin_id));
                a.extend(self.tail_args(&amount, &min_profit));
                a
            }

            // ═══════════════════════════════════════
            //  Two-hop: Turbos ↔ DeepBook
            // ═══════════════════════════════════════
            StrategyType::TurbosToDeepBook => {
                let mut a = self.base_args();
                a.push(json!(opp.pool_ids[0])); // turbos_pool
                a.push(json!(self.turbos_versioned));
                a.push(json!(opp.pool_ids[1])); // deepbook_pool
                a.push(json!(self.deep_fee_coin_id));
                a.extend(self.tail_args(&amount, &min_profit));
                a
            }

            StrategyType::DeepBookToTurbos => {
                let mut a = self.base_args();
                a.push(json!(opp.pool_ids[1])); // turbos_pool
                a.push(json!(self.turbos_versioned));
                a.push(json!(opp.pool_ids[0])); // deepbook_pool (flash source)
                a.push(json!(self.deep_fee_coin_id));
                a.extend(self.tail_args(&amount, &min_profit));
                a
            }

            // ═══════════════════════════════════════
            //  Two-hop: Cetus → Aftermath
            // ═══════════════════════════════════════
            StrategyType::CetusToAftermath | StrategyType::CetusToAftermathRev => {
                let mut a = self.base_args();
                a.push(json!(self.cetus_global_config));
                a.push(json!(opp.pool_ids[0])); // cetus_pool
                a.extend(self.aftermath_args(&opp.pool_ids[1]));
                a.extend(self.tail_args(&amount, &min_profit));
                a
            }

            // ═══════════════════════════════════════
            //  Two-hop: Turbos → Aftermath
            // ═══════════════════════════════════════
            StrategyType::TurbosToAftermath => {
                let mut a = self.base_args();
                a.push(json!(opp.pool_ids[0])); // turbos_pool
                a.push(json!(self.turbos_versioned));
                a.extend(self.aftermath_args(&opp.pool_ids[1]));
                a.extend(self.tail_args(&amount, &min_profit));
                a
            }

            // ═══════════════════════════════════════
            //  Two-hop: DeepBook → Aftermath
            // ═══════════════════════════════════════
            StrategyType::DeepBookToAftermath => {
                let mut a = self.base_args();
                a.push(json!(opp.pool_ids[0])); // deepbook_pool
                a.push(json!(self.deep_fee_coin_id));
                a.extend(self.aftermath_args(&opp.pool_ids[1]));
                a.extend(self.tail_args(&amount, &min_profit));
                a
            }

            // ═══════════════════════════════════════
            //  Two-hop: Cetus ↔ FlowX CLMM
            // ═══════════════════════════════════════
            StrategyType::CetusToFlowxClmm => {
                let mut a = self.base_args();
                a.push(json!(self.cetus_global_config));
                a.push(json!(opp.pool_ids[0])); // cetus_pool
                a.push(json!(opp.pool_ids[1])); // flowx_pool
                a.push(json!(self.flowx_versioned));
                a.extend(self.tail_args(&amount, &min_profit));
                a
            }

            StrategyType::FlowxClmmToCetus => {
                let mut a = self.base_args();
                a.push(json!(self.cetus_global_config));
                a.push(json!(opp.pool_ids[1])); // cetus_pool
                a.push(json!(opp.pool_ids[0])); // flowx_pool (flash source)
                a.push(json!(self.flowx_versioned));
                a.extend(self.tail_args(&amount, &min_profit));
                a
            }

            // ═══════════════════════════════════════
            //  Two-hop: Turbos ↔ FlowX CLMM
            // ═══════════════════════════════════════
            StrategyType::TurbosToFlowxClmm => {
                let mut a = self.base_args();
                a.push(json!(opp.pool_ids[0])); // turbos_pool
                a.push(json!(self.turbos_versioned));
                a.push(json!(opp.pool_ids[1])); // flowx_pool
                a.push(json!(self.flowx_versioned));
                a.extend(self.tail_args(&amount, &min_profit));
                a
            }

            StrategyType::FlowxClmmToTurbos => {
                let mut a = self.base_args();
                a.push(json!(opp.pool_ids[1])); // turbos_pool
                a.push(json!(self.turbos_versioned));
                a.push(json!(opp.pool_ids[0])); // flowx_pool (flash source)
                a.push(json!(self.flowx_versioned));
                a.extend(self.tail_args(&amount, &min_profit));
                a
            }

            // ═══════════════════════════════════════
            //  Two-hop: DeepBook ↔ FlowX CLMM
            // ═══════════════════════════════════════
            StrategyType::DeepBookToFlowxClmm => {
                let mut a = self.base_args();
                a.push(json!(opp.pool_ids[0])); // deepbook_pool
                a.push(json!(self.deep_fee_coin_id));
                a.push(json!(opp.pool_ids[1])); // flowx_pool
                a.push(json!(self.flowx_versioned));
                a.extend(self.tail_args(&amount, &min_profit));
                a
            }

            StrategyType::FlowxClmmToDeepBook => {
                let mut a = self.base_args();
                a.push(json!(opp.pool_ids[1])); // deepbook_pool
                a.push(json!(self.deep_fee_coin_id));
                a.push(json!(opp.pool_ids[0])); // flowx_pool (flash source)
                a.push(json!(self.flowx_versioned));
                a.extend(self.tail_args(&amount, &min_profit));
                a
            }

            // ═══════════════════════════════════════
            //  Two-hop: Cetus → FlowX AMM
            // ═══════════════════════════════════════
            StrategyType::CetusToFlowxAmm => {
                let mut a = self.base_args();
                a.push(json!(self.cetus_global_config));
                a.push(json!(opp.pool_ids[0])); // cetus_pool
                a.push(json!(self.flowx_container)); // flowx container
                a.extend(self.tail_args(&amount, &min_profit));
                a
            }

            // ═══════════════════════════════════════
            //  Two-hop: Turbos → FlowX AMM
            // ═══════════════════════════════════════
            StrategyType::TurbosToFlowxAmm => {
                let mut a = self.base_args();
                a.push(json!(opp.pool_ids[0])); // turbos_pool
                a.push(json!(self.turbos_versioned));
                a.push(json!(self.flowx_container)); // flowx container
                a.extend(self.tail_args(&amount, &min_profit));
                a
            }

            // ═══════════════════════════════════════
            //  Two-hop: DeepBook → FlowX AMM
            // ═══════════════════════════════════════
            StrategyType::DeepBookToFlowxAmm => {
                let mut a = self.base_args();
                a.push(json!(opp.pool_ids[0])); // deepbook_pool
                a.push(json!(self.deep_fee_coin_id));
                a.push(json!(self.flowx_container)); // flowx container
                a.extend(self.tail_args(&amount, &min_profit));
                a
            }

            // ═══════════════════════════════════════
            //  Tri-hop: Cetus × Cetus × Cetus
            // ═══════════════════════════════════════
            StrategyType::TriCetusCetusCetus => {
                let mut a = self.base_args();
                a.push(json!(self.cetus_global_config));
                a.push(json!(opp.pool_ids[0])); // pool_ab
                a.push(json!(opp.pool_ids[1])); // pool_bc
                a.push(json!(opp.pool_ids[2])); // pool_ca
                a.extend(self.tail_args(&amount, &min_profit));
                a
            }

            // ═══════════════════════════════════════
            //  Tri-hop: Cetus × Cetus × Turbos
            // ═══════════════════════════════════════
            StrategyType::TriCetusCetusTurbos => {
                let mut a = self.base_args();
                a.push(json!(self.cetus_global_config));
                a.push(json!(opp.pool_ids[0])); // pool_ab (cetus)
                a.push(json!(opp.pool_ids[1])); // pool_bc (cetus)
                a.push(json!(opp.pool_ids[2])); // turbos_pool_ca
                a.push(json!(self.turbos_versioned));
                a.extend(self.tail_args(&amount, &min_profit));
                a
            }

            // ═══════════════════════════════════════
            //  Tri-hop: Cetus × Turbos × DeepBook
            // ═══════════════════════════════════════
            StrategyType::TriCetusTurbosDeepBook => {
                let mut a = self.base_args();
                a.push(json!(self.cetus_global_config));
                a.push(json!(opp.pool_ids[0])); // cetus_pool_ab
                a.push(json!(opp.pool_ids[1])); // turbos_pool_bc
                a.push(json!(self.turbos_versioned));
                a.push(json!(opp.pool_ids[2])); // deepbook_pool_ca
                a.push(json!(self.deep_fee_coin_id));
                a.extend(self.tail_args(&amount, &min_profit));
                a
            }

            // ═══════════════════════════════════════
            //  Tri-hop: Cetus × DeepBook × Turbos
            // ═══════════════════════════════════════
            StrategyType::TriCetusDeepBookTurbos => {
                let mut a = self.base_args();
                a.push(json!(self.cetus_global_config));
                a.push(json!(opp.pool_ids[0])); // cetus_pool_ab
                a.push(json!(opp.pool_ids[1])); // deepbook_pool_bc
                a.push(json!(self.deep_fee_coin_id));
                a.push(json!(opp.pool_ids[2])); // turbos_pool_ca
                a.push(json!(self.turbos_versioned));
                a.extend(self.tail_args(&amount, &min_profit));
                a
            }

            // ═══════════════════════════════════════
            //  Tri-hop: DeepBook × Cetus × Turbos
            // ═══════════════════════════════════════
            StrategyType::TriDeepBookCetusTurbos => {
                let mut a = self.base_args();
                a.push(json!(self.cetus_global_config));
                a.push(json!(opp.pool_ids[0])); // deepbook_pool_ac
                a.push(json!(self.deep_fee_coin_id));
                a.push(json!(opp.pool_ids[1])); // cetus_pool_ab
                a.push(json!(opp.pool_ids[2])); // turbos_pool_bc
                a.push(json!(self.turbos_versioned));
                a.extend(self.tail_args(&amount, &min_profit));
                a
            }

            // ═══════════════════════════════════════
            //  Tri-hop: Cetus × Cetus × Aftermath
            // ═══════════════════════════════════════
            StrategyType::TriCetusCetusAftermath => {
                let mut a = self.base_args();
                a.push(json!(self.cetus_global_config));
                a.push(json!(opp.pool_ids[0])); // pool_ab (cetus)
                a.push(json!(opp.pool_ids[1])); // pool_bc (cetus)
                a.extend(self.aftermath_args(&opp.pool_ids[2]));
                a.extend(self.tail_args(&amount, &min_profit));
                a
            }

            // ═══════════════════════════════════════
            //  Tri-hop: Cetus × Turbos × Aftermath
            // ═══════════════════════════════════════
            StrategyType::TriCetusTurbosAftermath => {
                let mut a = self.base_args();
                a.push(json!(self.cetus_global_config));
                a.push(json!(opp.pool_ids[0])); // cetus_pool_ab
                a.push(json!(opp.pool_ids[1])); // turbos_pool_bc
                a.push(json!(self.turbos_versioned));
                a.extend(self.aftermath_args(&opp.pool_ids[2]));
                a.extend(self.tail_args(&amount, &min_profit));
                a
            }

            // ═══════════════════════════════════════
            //  Tri-hop: Cetus × Cetus × FlowX CLMM
            // ═══════════════════════════════════════
            StrategyType::TriCetusCetusFlowxClmm => {
                let mut a = self.base_args();
                a.push(json!(self.cetus_global_config));
                a.push(json!(opp.pool_ids[0])); // pool_ab (cetus)
                a.push(json!(opp.pool_ids[1])); // pool_bc (cetus)
                a.push(json!(opp.pool_ids[2])); // flowx_pool_ca
                a.push(json!(self.flowx_versioned));
                a.extend(self.tail_args(&amount, &min_profit));
                a
            }

            // ═══════════════════════════════════════
            //  Tri-hop: Cetus × FlowX CLMM × Turbos
            // ═══════════════════════════════════════
            StrategyType::TriCetusFlowxClmmTurbos => {
                let mut a = self.base_args();
                a.push(json!(self.cetus_global_config));
                a.push(json!(opp.pool_ids[0])); // cetus_pool_ab
                a.push(json!(opp.pool_ids[1])); // flowx_pool_bc
                a.push(json!(self.flowx_versioned));
                a.push(json!(opp.pool_ids[2])); // turbos_pool_ca
                a.push(json!(self.turbos_versioned));
                a.extend(self.tail_args(&amount, &min_profit));
                a
            }

            // ═══════════════════════════════════════
            //  Tri-hop: FlowX CLMM × Cetus × Turbos
            // ═══════════════════════════════════════
            StrategyType::TriFlowxClmmCetusTurbos => {
                let mut a = self.base_args();
                a.push(json!(self.cetus_global_config));
                a.push(json!(opp.pool_ids[0])); // flowx_pool_ab
                a.push(json!(self.flowx_versioned));
                a.push(json!(opp.pool_ids[1])); // cetus_pool_bc
                a.push(json!(opp.pool_ids[2])); // turbos_pool_ca
                a.push(json!(self.turbos_versioned));
                a.extend(self.tail_args(&amount, &min_profit));
                a
            }
        };

        Ok((args, opp.type_args.clone()))
    }
}
