/// Thin wrapper around Aftermath Finance AMM.
/// Aftermath uses weighted/stable pools (Balancer-style CFMM).
/// Pool<LP> has 1 phantom type param (the LP coin type).
/// Swap takes 3 type params: <LP, CoinIn, CoinOut>.
/// NO flash swap support — Aftermath can only be the sell leg.
module arb_move::aftermath_adapter {
    use sui::coin::Coin;

    use aftermath_amm::pool::Pool;
    use aftermath_amm::pool_registry::PoolRegistry;
    use protocol_fee_vault::vault::ProtocolFeeVault;
    use treasury::treasury::Treasury;
    use insurance_fund::insurance_fund::InsuranceFund;
    use referral_vault::referral_vault::ReferralVault;
    use aftermath_amm::swap;

    /// Swap exact CoinIn for CoinOut on Aftermath.
    /// Requires 6 shared objects per swap call.
    /// `expected_out` and `slippage` are passed through to Aftermath —
    /// strategies set these to 0 / u64::MAX and validate profit separately.
    public(package) fun swap_exact_in<LP, CoinIn, CoinOut>(
        pool: &mut Pool<LP>,
        pool_registry: &PoolRegistry,
        fee_vault: &ProtocolFeeVault,
        treasury: &mut Treasury,
        insurance: &mut InsuranceFund,
        referral: &ReferralVault,
        coin_in: Coin<CoinIn>,
        expected_out: u64,
        slippage: u64,
        ctx: &mut TxContext,
    ): Coin<CoinOut> {
        swap::swap_exact_in<LP, CoinIn, CoinOut>(
            pool,
            pool_registry,
            fee_vault,
            treasury,
            insurance,
            referral,
            coin_in,
            expected_out,
            slippage,
            ctx,
        )
    }

    // ── Tests ──

    #[test]
    fun test_module_compiles() {
        // Aftermath adapter compiles — validates interface bindings.
        // Runtime tests require real pool objects (integration test only).
        assert!(true);
    }
}
