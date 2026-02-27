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
