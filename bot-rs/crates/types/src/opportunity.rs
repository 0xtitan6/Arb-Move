use crate::pool::Dex;
use serde::{Deserialize, Serialize};

/// Describes which on-chain strategy entry function to call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StrategyType {
    // ── Two-hop ──
    CetusToTurbos,
    CetusToTurbosRev,
    TurbosToCetus,
    CetusToDeepBook,
    DeepBookToCetus,
    TurbosToDeepBook,
    DeepBookToTurbos,
    // Aftermath (sell leg only)
    CetusToAftermath,
    CetusToAftermathRev,
    TurbosToAftermath,
    DeepBookToAftermath,
    // FlowX CLMM
    CetusToFlowxClmm,
    FlowxClmmToCetus,
    TurbosToFlowxClmm,
    FlowxClmmToTurbos,
    DeepBookToFlowxClmm,
    FlowxClmmToDeepBook,
    // FlowX AMM (sell leg only)
    CetusToFlowxAmm,
    TurbosToFlowxAmm,
    DeepBookToFlowxAmm,

    // ── Tri-hop ──
    TriCetusCetusCetus,
    TriCetusCetusTurbos,
    TriCetusTurbosDeepBook,
    TriCetusDeepBookTurbos,
    TriDeepBookCetusTurbos,
    TriCetusCetusAftermath,
    TriCetusTurbosAftermath,
    TriCetusCetusFlowxClmm,
    TriCetusFlowxClmmTurbos,
    TriFlowxClmmCetusTurbos,
}

impl StrategyType {
    /// The Move entry function name for this strategy.
    pub fn move_function_name(&self) -> &'static str {
        match self {
            Self::CetusToTurbos => "arb_cetus_to_turbos",
            Self::CetusToTurbosRev => "arb_cetus_to_turbos_reverse",
            Self::TurbosToCetus => "arb_turbos_to_cetus",
            Self::CetusToDeepBook => "arb_cetus_to_deepbook",
            Self::DeepBookToCetus => "arb_deepbook_to_cetus",
            Self::TurbosToDeepBook => "arb_turbos_to_deepbook",
            Self::DeepBookToTurbos => "arb_deepbook_to_turbos",
            Self::CetusToAftermath => "arb_cetus_to_aftermath",
            Self::CetusToAftermathRev => "arb_cetus_to_aftermath_rev",
            Self::TurbosToAftermath => "arb_turbos_to_aftermath",
            Self::DeepBookToAftermath => "arb_deepbook_to_aftermath",
            Self::CetusToFlowxClmm => "arb_cetus_to_flowx_clmm",
            Self::FlowxClmmToCetus => "arb_flowx_clmm_to_cetus",
            Self::TurbosToFlowxClmm => "arb_turbos_to_flowx_clmm",
            Self::FlowxClmmToTurbos => "arb_flowx_clmm_to_turbos",
            Self::DeepBookToFlowxClmm => "arb_deepbook_to_flowx_clmm",
            Self::FlowxClmmToDeepBook => "arb_flowx_clmm_to_deepbook",
            Self::CetusToFlowxAmm => "arb_cetus_to_flowx_amm",
            Self::TurbosToFlowxAmm => "arb_turbos_to_flowx_amm",
            Self::DeepBookToFlowxAmm => "arb_deepbook_to_flowx_amm",
            Self::TriCetusCetusCetus => "tri_cetus_cetus_cetus",
            Self::TriCetusCetusTurbos => "tri_cetus_cetus_turbos",
            Self::TriCetusTurbosDeepBook => "tri_cetus_turbos_deepbook",
            Self::TriCetusDeepBookTurbos => "tri_cetus_deepbook_turbos",
            Self::TriDeepBookCetusTurbos => "tri_deepbook_cetus_turbos",
            Self::TriCetusCetusAftermath => "tri_cetus_cetus_aftermath",
            Self::TriCetusTurbosAftermath => "tri_cetus_turbos_aftermath",
            Self::TriCetusCetusFlowxClmm => "tri_cetus_cetus_flowx_clmm",
            Self::TriCetusFlowxClmmTurbos => "tri_cetus_flowx_clmm_turbos",
            Self::TriFlowxClmmCetusTurbos => "tri_flowx_clmm_cetus_turbos",
        }
    }

    /// The Move module containing this strategy ("two_hop" or "tri_hop").
    pub fn move_module(&self) -> &'static str {
        match self {
            Self::TriCetusCetusCetus
            | Self::TriCetusCetusTurbos
            | Self::TriCetusTurbosDeepBook
            | Self::TriCetusDeepBookTurbos
            | Self::TriDeepBookCetusTurbos
            | Self::TriCetusCetusAftermath
            | Self::TriCetusTurbosAftermath
            | Self::TriCetusCetusFlowxClmm
            | Self::TriCetusFlowxClmmTurbos
            | Self::TriFlowxClmmCetusTurbos => "tri_hop",
            _ => "two_hop",
        }
    }

    /// Which DEX provides the flash loan / flash swap for this strategy.
    pub fn flash_source(&self) -> Dex {
        match self {
            Self::CetusToTurbos
            | Self::CetusToTurbosRev
            | Self::CetusToDeepBook
            | Self::CetusToAftermath
            | Self::CetusToAftermathRev
            | Self::CetusToFlowxClmm
            | Self::CetusToFlowxAmm => Dex::Cetus,

            Self::TurbosToCetus
            | Self::TurbosToDeepBook
            | Self::TurbosToAftermath
            | Self::TurbosToFlowxClmm
            | Self::TurbosToFlowxAmm => Dex::Turbos,

            Self::DeepBookToCetus
            | Self::DeepBookToTurbos
            | Self::DeepBookToAftermath
            | Self::DeepBookToFlowxClmm
            | Self::DeepBookToFlowxAmm => Dex::DeepBook,

            Self::FlowxClmmToCetus
            | Self::FlowxClmmToTurbos
            | Self::FlowxClmmToDeepBook => Dex::FlowxClmm,

            Self::TriCetusCetusCetus
            | Self::TriCetusCetusTurbos
            | Self::TriCetusTurbosDeepBook
            | Self::TriCetusDeepBookTurbos
            | Self::TriCetusCetusAftermath
            | Self::TriCetusTurbosAftermath
            | Self::TriCetusCetusFlowxClmm
            | Self::TriCetusFlowxClmmTurbos => Dex::Cetus,

            Self::TriDeepBookCetusTurbos => Dex::DeepBook,
            Self::TriFlowxClmmCetusTurbos => Dex::FlowxClmm,
        }
    }
}

/// A detected arbitrage opportunity, ready for execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArbOpportunity {
    /// Which strategy to execute.
    pub strategy: StrategyType,
    /// Optimal input amount in MIST (after ternary search).
    pub amount_in: u64,
    /// Expected profit in MIST (before gas).
    pub expected_profit: u64,
    /// Estimated gas cost in MIST.
    pub estimated_gas: u64,
    /// Net profit after gas.
    pub net_profit: i64,
    /// Pool object IDs involved (ordered per strategy params).
    pub pool_ids: Vec<String>,
    /// Coin type arguments for the Move call.
    pub type_args: Vec<String>,
    /// When this opportunity was detected (ms since epoch).
    pub detected_at_ms: u64,
}

impl ArbOpportunity {
    /// Returns true if the opportunity is profitable after gas.
    pub fn is_profitable(&self) -> bool {
        self.net_profit > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_opp(strategy: StrategyType, pool_count: usize, expected_profit: u64) -> ArbOpportunity {
        ArbOpportunity {
            strategy,
            amount_in: 1_000_000_000,
            expected_profit,
            estimated_gas: 5_000_000,
            net_profit: expected_profit as i64 - 5_000_000,
            pool_ids: (0..pool_count).map(|i| format!("0xpool{i}")).collect(),
            type_args: vec!["SUI".to_string(), "USDC".to_string()],
            detected_at_ms: 0,
        }
    }

    #[test]
    fn test_is_profitable_positive() {
        let opp = make_opp(StrategyType::CetusToTurbos, 2, 10_000_000);
        assert!(opp.is_profitable());
    }

    #[test]
    fn test_is_profitable_zero() {
        let mut opp = make_opp(StrategyType::CetusToTurbos, 2, 10_000_000);
        opp.net_profit = 0;
        assert!(!opp.is_profitable());
    }

    #[test]
    fn test_is_profitable_negative() {
        let mut opp = make_opp(StrategyType::CetusToTurbos, 2, 1_000_000);
        opp.net_profit = -4_000_000;
        assert!(!opp.is_profitable());
    }

    // ── StrategyType tests ──

    #[test]
    fn test_move_module_two_hop() {
        assert_eq!(StrategyType::CetusToTurbos.move_module(), "two_hop");
        assert_eq!(StrategyType::DeepBookToAftermath.move_module(), "two_hop");
        assert_eq!(StrategyType::FlowxClmmToCetus.move_module(), "two_hop");
        assert_eq!(StrategyType::CetusToFlowxAmm.move_module(), "two_hop");
    }

    #[test]
    fn test_move_module_tri_hop() {
        assert_eq!(StrategyType::TriCetusCetusCetus.move_module(), "tri_hop");
        assert_eq!(StrategyType::TriCetusTurbosDeepBook.move_module(), "tri_hop");
        assert_eq!(StrategyType::TriFlowxClmmCetusTurbos.move_module(), "tri_hop");
    }

    #[test]
    fn test_move_function_names() {
        assert_eq!(StrategyType::CetusToTurbos.move_function_name(), "arb_cetus_to_turbos");
        assert_eq!(StrategyType::DeepBookToCetus.move_function_name(), "arb_deepbook_to_cetus");
        assert_eq!(StrategyType::TriCetusCetusCetus.move_function_name(), "tri_cetus_cetus_cetus");
    }

    #[test]
    fn test_flash_source_dex() {
        assert_eq!(StrategyType::CetusToTurbos.flash_source(), Dex::Cetus);
        assert_eq!(StrategyType::TurbosToCetus.flash_source(), Dex::Turbos);
        assert_eq!(StrategyType::DeepBookToCetus.flash_source(), Dex::DeepBook);
        assert_eq!(StrategyType::FlowxClmmToCetus.flash_source(), Dex::FlowxClmm);
        assert_eq!(StrategyType::CetusToFlowxAmm.flash_source(), Dex::Cetus);
    }

    #[test]
    fn test_tri_hop_flash_source() {
        assert_eq!(StrategyType::TriCetusCetusCetus.flash_source(), Dex::Cetus);
        assert_eq!(StrategyType::TriDeepBookCetusTurbos.flash_source(), Dex::DeepBook);
        assert_eq!(StrategyType::TriFlowxClmmCetusTurbos.flash_source(), Dex::FlowxClmm);
    }

    #[test]
    fn test_min_profit_calculation() {
        // 90% of expected profit
        let opp = make_opp(StrategyType::CetusToTurbos, 2, 100_000);
        let min_profit = opp.expected_profit * 9 / 10;
        assert_eq!(min_profit, 90_000);
    }

    #[test]
    fn test_min_profit_zero() {
        let opp = make_opp(StrategyType::CetusToTurbos, 2, 0);
        let min_profit = opp.expected_profit * 9 / 10;
        assert_eq!(min_profit, 0);
    }

    #[test]
    fn test_pool_ids_two_hop_needs_two() {
        let opp = make_opp(StrategyType::CetusToTurbos, 2, 100);
        let expected_pools = if opp.strategy.move_module() == "tri_hop" { 3 } else { 2 };
        assert!(opp.pool_ids.len() >= expected_pools);
    }

    #[test]
    fn test_pool_ids_tri_hop_needs_three() {
        let opp = make_opp(StrategyType::TriCetusCetusCetus, 3, 100);
        let expected_pools = if opp.strategy.move_module() == "tri_hop" { 3 } else { 2 };
        assert!(opp.pool_ids.len() >= expected_pools);
    }

    #[test]
    fn test_pool_ids_tri_hop_too_few_detected() {
        let opp = make_opp(StrategyType::TriCetusCetusCetus, 2, 100);
        let expected_pools = if opp.strategy.move_module() == "tri_hop" { 3 } else { 2 };
        assert!(opp.pool_ids.len() < expected_pools, "Should detect insufficient pool IDs");
    }
}
