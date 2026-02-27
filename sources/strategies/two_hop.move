/// Two-hop (DEX-to-DEX) arbitrage strategies.
/// Each entry function flash-borrows from one DEX, swaps on another, repays, and keeps profit.
/// All functions require AdminCap for authorization.
/// Supported DEXes: Cetus CLMM, Turbos CLMM, DeepBook V3, Aftermath AMM, FlowX CLMM v3.
module arb_move::two_hop {
    use sui::coin::{Self, Coin};
    use sui::balance;
    use sui::clock::Clock;

    // ── DEX pool types ──
    use cetus_clmm::pool::{Pool as CetusPool};
    use cetus_clmm::config::GlobalConfig;
    use turbos_clmm::pool::{Pool as TurbosPool, Versioned};
    use deepbook::pool::{Pool as DeepBookPool};
    use token::deep::DEEP;
    // Aftermath (6 shared objects per swap)
    use aftermath_amm::pool::{Pool as AftermathPool};
    use aftermath_amm::pool_registry::PoolRegistry;
    use protocol_fee_vault::vault::ProtocolFeeVault;
    use treasury::treasury::Treasury;
    use insurance_fund::insurance_fund::InsuranceFund;
    use referral_vault::referral_vault::ReferralVault;
    // FlowX CLMM v3
    use flowx_clmm::pool::{Pool as FlowxPool};
    use flowx_clmm::versioned::{Versioned as FlowxVersioned};

    // ── Internal modules ──
    use arb_move::admin::{AdminCap, PauseFlag};
    use arb_move::profit;
    use arb_move::events;
    use arb_move::cetus_adapter;
    use arb_move::turbos_adapter;
    use arb_move::deepbook_adapter;
    use arb_move::aftermath_adapter;
    use arb_move::flowx_clmm_adapter;

    const E_ZERO_AMOUNT: u64 = 1;
    /// Maximum u64 — used as Aftermath slippage to disable their internal check.
    /// We rely on profit::assert_profit() for the real profitability guard.
    const MAX_U64: u64 = 18446744073709551615;
    /// Minimum expected output for Aftermath swaps (defense-in-depth).
    /// Catches zero-output edge cases (empty pool, overflow) before assert_profit.
    const AFTERMATH_MIN_OUT: u64 = 1;

    // ════════════════════════════════════════════════════════════
    //  Cetus ↔ Turbos
    // ════════════════════════════════════════════════════════════

    /// Flash swap A→B on Cetus, sell B→A on Turbos, repay Cetus, keep A profit.
    /// Exploits: Cetus price(A/B) < Turbos price(A/B).
    entry fun arb_cetus_to_turbos<A, B, TurbosFee>(
        _admin: &AdminCap,
        pause: &PauseFlag,
        cetus_config: &GlobalConfig,
        cetus_pool: &mut CetusPool<A, B>,
        turbos_pool: &mut TurbosPool<A, B, TurbosFee>,
        turbos_versioned: &Versioned,
        amount: u64,
        min_profit: u64,
        clock: &Clock,
        ctx: &mut TxContext,
    ) {
        assert!(amount > 0, E_ZERO_AMOUNT);
        arb_move::admin::assert_not_paused(pause);

        // 1. Flash swap A→B on Cetus: receive Balance<B>, owe Balance<A>
        let (recv_a, recv_b, receipt) = cetus_adapter::flash_swap_a2b<A, B>(
            cetus_config, cetus_pool, amount, clock,
        );
        balance::destroy_zero(recv_a);

        // 2. Sell B→A on Turbos
        let coin_b = coin::from_balance(recv_b, ctx);
        let b_amount = coin::value(&coin_b);
        let mut coin_a_out = turbos_adapter::swap_b_to_a<A, B, TurbosFee>(
            turbos_pool, coin_b, b_amount, clock, turbos_versioned, ctx,
        );

        // 3. Validate profit
        let owed = cetus_adapter::swap_pay_amount(&receipt);
        let received = coin::value(&coin_a_out);
        profit::assert_profit(received, owed, min_profit);

        // 4. Repay Cetus
        let repay_coin = coin::split(&mut coin_a_out, owed, ctx);
        cetus_adapter::repay_flash_swap<A, B>(
            cetus_config, cetus_pool,
            coin::into_balance(repay_coin),
            balance::zero<B>(),
            receipt,
        );

        // 5. Emit event + transfer profit
        events::emit_arb_executed(b"cetus_to_turbos", owed, received);
        transfer::public_transfer(coin_a_out, tx_context::sender(ctx));
    }

    /// Flash swap B→A on Cetus, sell A→B on Turbos, repay Cetus, keep B profit.
    /// Exploits: Cetus price(B/A) < Turbos price(B/A).
    entry fun arb_cetus_to_turbos_reverse<A, B, TurbosFee>(
        _admin: &AdminCap,
        pause: &PauseFlag,
        cetus_config: &GlobalConfig,
        cetus_pool: &mut CetusPool<A, B>,
        turbos_pool: &mut TurbosPool<A, B, TurbosFee>,
        turbos_versioned: &Versioned,
        amount: u64,
        min_profit: u64,
        clock: &Clock,
        ctx: &mut TxContext,
    ) {
        assert!(amount > 0, E_ZERO_AMOUNT);
        arb_move::admin::assert_not_paused(pause);

        let (recv_a, recv_b, receipt) = cetus_adapter::flash_swap_b2a<A, B>(
            cetus_config, cetus_pool, amount, clock,
        );
        balance::destroy_zero(recv_b);

        let coin_a = coin::from_balance(recv_a, ctx);
        let a_amount = coin::value(&coin_a);
        let mut coin_b_out = turbos_adapter::swap_a_to_b<A, B, TurbosFee>(
            turbos_pool, coin_a, a_amount, clock, turbos_versioned, ctx,
        );

        let owed = cetus_adapter::swap_pay_amount(&receipt);
        let received = coin::value(&coin_b_out);
        profit::assert_profit(received, owed, min_profit);

        let repay_coin = coin::split(&mut coin_b_out, owed, ctx);
        cetus_adapter::repay_flash_swap<A, B>(
            cetus_config, cetus_pool,
            balance::zero<A>(),
            coin::into_balance(repay_coin),
            receipt,
        );

        events::emit_arb_executed(b"cetus_to_turbos_rev", owed, received);
        transfer::public_transfer(coin_b_out, tx_context::sender(ctx));
    }

    /// Flash swap A→B on Turbos, sell B→A on Cetus, repay Turbos, keep A profit.
    /// NOTE: Turbos FlashSwapReceipt does not expose a public pay_amount reader.
    /// Repayment uses `amount` directly. If Turbos adds flash fees in a future
    /// upgrade, repay_flash_swap will abort and this function must be updated.
    entry fun arb_turbos_to_cetus<A, B, TurbosFee>(
        _admin: &AdminCap,
        pause: &PauseFlag,
        cetus_config: &GlobalConfig,
        cetus_pool: &mut CetusPool<A, B>,
        turbos_pool: &mut TurbosPool<A, B, TurbosFee>,
        turbos_versioned: &Versioned,
        amount: u64,
        min_profit: u64,
        clock: &Clock,
        ctx: &mut TxContext,
    ) {
        assert!(amount > 0, E_ZERO_AMOUNT);
        arb_move::admin::assert_not_paused(pause);

        // 1. Flash swap A→B on Turbos
        let (recv_a, recv_b, receipt) = turbos_adapter::flash_swap_a2b<A, B, TurbosFee>(
            turbos_pool, (amount as u128), clock, turbos_versioned, ctx,
        );
        coin::destroy_zero(recv_a);

        // 2. Sell B→A on Cetus
        let mut coin_a_out = cetus_adapter::swap_coin_b2a<A, B>(
            cetus_config, cetus_pool, recv_b, clock, ctx,
        );

        // 3. Validate profit — Turbos receipt: the owed amount is the A we didn't provide
        let received = coin::value(&coin_a_out);
        profit::assert_profit(received, amount, min_profit);

        // 4. Repay Turbos: provide the owed A, and zero B
        let repay_a = coin::split(&mut coin_a_out, amount, ctx);
        turbos_adapter::repay_flash_swap<A, B, TurbosFee>(
            turbos_pool, repay_a, coin::zero<B>(ctx), receipt, turbos_versioned,
        );

        // 5. Profit
        events::emit_arb_executed(b"turbos_to_cetus", amount, received);
        transfer::public_transfer(coin_a_out, tx_context::sender(ctx));
    }

    // ════════════════════════════════════════════════════════════
    //  Cetus ↔ DeepBook
    // ════════════════════════════════════════════════════════════

    /// Flash swap A→B on Cetus, sell B(quote)→A(base) on DeepBook, repay, keep A profit.
    /// Assumes: A=Base, B=Quote in DeepBook pool ordering.
    entry fun arb_cetus_to_deepbook<Base, Quote>(
        _admin: &AdminCap,
        pause: &PauseFlag,
        cetus_config: &GlobalConfig,
        cetus_pool: &mut CetusPool<Base, Quote>,
        deepbook_pool: &mut DeepBookPool<Base, Quote>,
        deep_fee: Coin<DEEP>,
        amount: u64,
        min_profit: u64,
        clock: &Clock,
        ctx: &mut TxContext,
    ) {
        assert!(amount > 0, E_ZERO_AMOUNT);
        arb_move::admin::assert_not_paused(pause);

        // 1. Flash swap Base→Quote on Cetus
        let (recv_base, recv_quote, receipt) = cetus_adapter::flash_swap_a2b<Base, Quote>(
            cetus_config, cetus_pool, amount, clock,
        );
        balance::destroy_zero(recv_base);

        // 2. Sell Quote→Base on DeepBook
        let quote_coin = coin::from_balance(recv_quote, ctx);
        let mut base_out = deepbook_adapter::swap_quote_for_base_cleanup<Base, Quote>(
            deepbook_pool, quote_coin, deep_fee, clock, ctx,
        );

        // 3. Validate
        let owed = cetus_adapter::swap_pay_amount(&receipt);
        let received = coin::value(&base_out);
        profit::assert_profit(received, owed, min_profit);

        // 4. Repay Cetus
        let repay = coin::split(&mut base_out, owed, ctx);
        cetus_adapter::repay_flash_swap<Base, Quote>(
            cetus_config, cetus_pool,
            coin::into_balance(repay),
            balance::zero<Quote>(),
            receipt,
        );

        events::emit_arb_executed(b"cetus_to_deepbook", owed, received);
        transfer::public_transfer(base_out, tx_context::sender(ctx));
    }

    /// Flash borrow Base from DeepBook, sell Base→Quote on Cetus, buy Base with Quote on DeepBook, repay.
    /// NOTE: This borrows and swaps against the SAME DeepBook pool. DeepBook V3 allows
    /// swaps while a flash loan is outstanding, but vault reserve reduction may affect pricing.
    entry fun arb_deepbook_to_cetus<Base, Quote>(
        _admin: &AdminCap,
        pause: &PauseFlag,
        cetus_config: &GlobalConfig,
        cetus_pool: &mut CetusPool<Base, Quote>,
        deepbook_pool: &mut DeepBookPool<Base, Quote>,
        deep_fee: Coin<DEEP>,
        amount: u64,
        min_profit: u64,
        clock: &Clock,
        ctx: &mut TxContext,
    ) {
        assert!(amount > 0, E_ZERO_AMOUNT);
        arb_move::admin::assert_not_paused(pause);

        // 1. Flash borrow Base from DeepBook
        let (borrowed_base, flash_receipt) = deepbook_adapter::flash_borrow_base<Base, Quote>(
            deepbook_pool, amount, ctx,
        );

        // 2. Sell Base→Quote on Cetus
        let quote_out = cetus_adapter::swap_coin_a2b<Base, Quote>(
            cetus_config, cetus_pool, borrowed_base, clock, ctx,
        );

        // 3. Buy Base with Quote on DeepBook (to get repayment + profit)
        let mut base_out = deepbook_adapter::swap_quote_for_base_cleanup<Base, Quote>(
            deepbook_pool, quote_out, deep_fee, clock, ctx,
        );

        // 4. Validate
        let received = coin::value(&base_out);
        profit::assert_profit(received, amount, min_profit);

        // 5. Repay flash loan with `amount` of Base
        let repay = coin::split(&mut base_out, amount, ctx);
        deepbook_adapter::flash_return_base<Base, Quote>(deepbook_pool, repay, flash_receipt);

        events::emit_arb_executed(b"deepbook_to_cetus", amount, received);
        transfer::public_transfer(base_out, tx_context::sender(ctx));
    }

    // ════════════════════════════════════════════════════════════
    //  Turbos ↔ DeepBook
    // ════════════════════════════════════════════════════════════

    /// Flash swap A→B on Turbos, sell B(quote)→A(base) on DeepBook, repay, keep profit.
    /// NOTE: Turbos repayment uses `amount` directly (see H-2 in audit).
    entry fun arb_turbos_to_deepbook<A, B, TurbosFee>(
        _admin: &AdminCap,
        pause: &PauseFlag,
        turbos_pool: &mut TurbosPool<A, B, TurbosFee>,
        turbos_versioned: &Versioned,
        deepbook_pool: &mut DeepBookPool<A, B>,
        deep_fee: Coin<DEEP>,
        amount: u64,
        min_profit: u64,
        clock: &Clock,
        ctx: &mut TxContext,
    ) {
        assert!(amount > 0, E_ZERO_AMOUNT);
        arb_move::admin::assert_not_paused(pause);

        // 1. Flash swap A→B on Turbos
        let (recv_a, recv_b, receipt) = turbos_adapter::flash_swap_a2b<A, B, TurbosFee>(
            turbos_pool, (amount as u128), clock, turbos_versioned, ctx,
        );
        coin::destroy_zero(recv_a);

        // 2. Sell B→A on DeepBook (B=Quote, A=Base)
        let mut base_out = deepbook_adapter::swap_quote_for_base_cleanup<A, B>(
            deepbook_pool, recv_b, deep_fee, clock, ctx,
        );

        // 3. Validate
        let received = coin::value(&base_out);
        profit::assert_profit(received, amount, min_profit);

        // 4. Repay Turbos
        let repay_a = coin::split(&mut base_out, amount, ctx);
        turbos_adapter::repay_flash_swap<A, B, TurbosFee>(
            turbos_pool, repay_a, coin::zero<B>(ctx), receipt, turbos_versioned,
        );

        events::emit_arb_executed(b"turbos_to_deepbook", amount, received);
        transfer::public_transfer(base_out, tx_context::sender(ctx));
    }

    /// Flash borrow Base from DeepBook, sell Base→Quote on Turbos, buy Base with Quote, repay.
    /// NOTE: Same-pool flash borrow + swap on DeepBook (see M-2 in audit).
    entry fun arb_deepbook_to_turbos<A, B, TurbosFee>(
        _admin: &AdminCap,
        pause: &PauseFlag,
        turbos_pool: &mut TurbosPool<A, B, TurbosFee>,
        turbos_versioned: &Versioned,
        deepbook_pool: &mut DeepBookPool<A, B>,
        deep_fee: Coin<DEEP>,
        amount: u64,
        min_profit: u64,
        clock: &Clock,
        ctx: &mut TxContext,
    ) {
        assert!(amount > 0, E_ZERO_AMOUNT);
        arb_move::admin::assert_not_paused(pause);

        // 1. Flash borrow Base (A) from DeepBook
        let (borrowed, flash_receipt) = deepbook_adapter::flash_borrow_base<A, B>(
            deepbook_pool, amount, ctx,
        );

        // 2. Sell A→B on Turbos
        let coin_b = turbos_adapter::swap_a_to_b<A, B, TurbosFee>(
            turbos_pool, borrowed, amount, clock, turbos_versioned, ctx,
        );

        // 3. Buy A with B on DeepBook
        let mut base_out = deepbook_adapter::swap_quote_for_base_cleanup<A, B>(
            deepbook_pool, coin_b, deep_fee, clock, ctx,
        );

        // 4. Validate
        let received = coin::value(&base_out);
        profit::assert_profit(received, amount, min_profit);

        // 5. Repay
        let repay = coin::split(&mut base_out, amount, ctx);
        deepbook_adapter::flash_return_base<A, B>(deepbook_pool, repay, flash_receipt);

        events::emit_arb_executed(b"deepbook_to_turbos", amount, received);
        transfer::public_transfer(base_out, tx_context::sender(ctx));
    }

    // ════════════════════════════════════════════════════════════
    //  Cetus / Turbos / DeepBook → Aftermath (sell leg only)
    // ════════════════════════════════════════════════════════════

    /// Flash swap A→B on Cetus, sell B→A on Aftermath, repay Cetus, keep A profit.
    /// LP = Aftermath pool LP coin type. Aftermath requires 6 shared objects.
    entry fun arb_cetus_to_aftermath<A, B, LP>(
        _admin: &AdminCap,
        pause: &PauseFlag,
        cetus_config: &GlobalConfig,
        cetus_pool: &mut CetusPool<A, B>,
        aftermath_pool: &mut AftermathPool<LP>,
        aftermath_registry: &PoolRegistry,
        aftermath_fee_vault: &ProtocolFeeVault,
        aftermath_treasury: &mut Treasury,
        aftermath_insurance: &mut InsuranceFund,
        aftermath_referral: &ReferralVault,
        amount: u64,
        min_profit: u64,
        clock: &Clock,
        ctx: &mut TxContext,
    ) {
        assert!(amount > 0, E_ZERO_AMOUNT);
        arb_move::admin::assert_not_paused(pause);

        // 1. Flash swap A→B on Cetus
        let (recv_a, recv_b, receipt) = cetus_adapter::flash_swap_a2b<A, B>(
            cetus_config, cetus_pool, amount, clock,
        );
        balance::destroy_zero(recv_a);

        // 2. Sell B→A on Aftermath
        let coin_b = coin::from_balance(recv_b, ctx);
        let mut coin_a_out = aftermath_adapter::swap_exact_in<LP, B, A>(
            aftermath_pool, aftermath_registry, aftermath_fee_vault,
            aftermath_treasury, aftermath_insurance, aftermath_referral,
            coin_b, AFTERMATH_MIN_OUT, MAX_U64, ctx,
        );

        // 3. Validate profit
        let owed = cetus_adapter::swap_pay_amount(&receipt);
        let received = coin::value(&coin_a_out);
        profit::assert_profit(received, owed, min_profit);

        // 4. Repay Cetus
        let repay = coin::split(&mut coin_a_out, owed, ctx);
        cetus_adapter::repay_flash_swap<A, B>(
            cetus_config, cetus_pool,
            coin::into_balance(repay),
            balance::zero<B>(),
            receipt,
        );

        events::emit_arb_executed(b"cetus_to_aftermath", owed, received);
        transfer::public_transfer(coin_a_out, tx_context::sender(ctx));
    }

    /// Flash swap B→A on Cetus, sell A→B on Aftermath, repay Cetus, keep B profit.
    entry fun arb_cetus_to_aftermath_rev<A, B, LP>(
        _admin: &AdminCap,
        pause: &PauseFlag,
        cetus_config: &GlobalConfig,
        cetus_pool: &mut CetusPool<A, B>,
        aftermath_pool: &mut AftermathPool<LP>,
        aftermath_registry: &PoolRegistry,
        aftermath_fee_vault: &ProtocolFeeVault,
        aftermath_treasury: &mut Treasury,
        aftermath_insurance: &mut InsuranceFund,
        aftermath_referral: &ReferralVault,
        amount: u64,
        min_profit: u64,
        clock: &Clock,
        ctx: &mut TxContext,
    ) {
        assert!(amount > 0, E_ZERO_AMOUNT);
        arb_move::admin::assert_not_paused(pause);

        // 1. Flash swap B→A on Cetus
        let (recv_a, recv_b, receipt) = cetus_adapter::flash_swap_b2a<A, B>(
            cetus_config, cetus_pool, amount, clock,
        );
        balance::destroy_zero(recv_b);

        // 2. Sell A→B on Aftermath
        let coin_a = coin::from_balance(recv_a, ctx);
        let mut coin_b_out = aftermath_adapter::swap_exact_in<LP, A, B>(
            aftermath_pool, aftermath_registry, aftermath_fee_vault,
            aftermath_treasury, aftermath_insurance, aftermath_referral,
            coin_a, AFTERMATH_MIN_OUT, MAX_U64, ctx,
        );

        // 3. Validate profit
        let owed = cetus_adapter::swap_pay_amount(&receipt);
        let received = coin::value(&coin_b_out);
        profit::assert_profit(received, owed, min_profit);

        // 4. Repay Cetus with B
        let repay = coin::split(&mut coin_b_out, owed, ctx);
        cetus_adapter::repay_flash_swap<A, B>(
            cetus_config, cetus_pool,
            balance::zero<A>(),
            coin::into_balance(repay),
            receipt,
        );

        events::emit_arb_executed(b"cetus_to_aftermath_rev", owed, received);
        transfer::public_transfer(coin_b_out, tx_context::sender(ctx));
    }

    /// Flash swap A→B on Turbos, sell B→A on Aftermath, repay Turbos, keep A profit.
    /// NOTE: Turbos repayment uses `amount` directly (see H-2).
    entry fun arb_turbos_to_aftermath<A, B, TurbosFee, LP>(
        _admin: &AdminCap,
        pause: &PauseFlag,
        turbos_pool: &mut TurbosPool<A, B, TurbosFee>,
        turbos_versioned: &Versioned,
        aftermath_pool: &mut AftermathPool<LP>,
        aftermath_registry: &PoolRegistry,
        aftermath_fee_vault: &ProtocolFeeVault,
        aftermath_treasury: &mut Treasury,
        aftermath_insurance: &mut InsuranceFund,
        aftermath_referral: &ReferralVault,
        amount: u64,
        min_profit: u64,
        clock: &Clock,
        ctx: &mut TxContext,
    ) {
        assert!(amount > 0, E_ZERO_AMOUNT);
        arb_move::admin::assert_not_paused(pause);

        // 1. Flash swap A→B on Turbos
        let (recv_a, recv_b, receipt) = turbos_adapter::flash_swap_a2b<A, B, TurbosFee>(
            turbos_pool, (amount as u128), clock, turbos_versioned, ctx,
        );
        coin::destroy_zero(recv_a);

        // 2. Sell B→A on Aftermath
        let mut coin_a_out = aftermath_adapter::swap_exact_in<LP, B, A>(
            aftermath_pool, aftermath_registry, aftermath_fee_vault,
            aftermath_treasury, aftermath_insurance, aftermath_referral,
            recv_b, AFTERMATH_MIN_OUT, MAX_U64, ctx,
        );

        // 3. Validate
        let received = coin::value(&coin_a_out);
        profit::assert_profit(received, amount, min_profit);

        // 4. Repay Turbos
        let repay_a = coin::split(&mut coin_a_out, amount, ctx);
        turbos_adapter::repay_flash_swap<A, B, TurbosFee>(
            turbos_pool, repay_a, coin::zero<B>(ctx), receipt, turbos_versioned,
        );

        events::emit_arb_executed(b"turbos_to_aftermath", amount, received);
        transfer::public_transfer(coin_a_out, tx_context::sender(ctx));
    }

    /// Flash borrow Base from DeepBook, sell Base→Quote on Aftermath, buy Base on DeepBook, repay.
    entry fun arb_deepbook_to_aftermath<Base, Quote, LP>(
        _admin: &AdminCap,
        pause: &PauseFlag,
        deepbook_pool: &mut DeepBookPool<Base, Quote>,
        deep_fee: Coin<DEEP>,
        aftermath_pool: &mut AftermathPool<LP>,
        aftermath_registry: &PoolRegistry,
        aftermath_fee_vault: &ProtocolFeeVault,
        aftermath_treasury: &mut Treasury,
        aftermath_insurance: &mut InsuranceFund,
        aftermath_referral: &ReferralVault,
        amount: u64,
        min_profit: u64,
        clock: &Clock,
        ctx: &mut TxContext,
    ) {
        assert!(amount > 0, E_ZERO_AMOUNT);
        arb_move::admin::assert_not_paused(pause);

        // 1. Flash borrow Base from DeepBook
        let (borrowed, flash_receipt) = deepbook_adapter::flash_borrow_base<Base, Quote>(
            deepbook_pool, amount, ctx,
        );

        // 2. Sell Base→Quote on Aftermath
        let coin_quote = aftermath_adapter::swap_exact_in<LP, Base, Quote>(
            aftermath_pool, aftermath_registry, aftermath_fee_vault,
            aftermath_treasury, aftermath_insurance, aftermath_referral,
            borrowed, AFTERMATH_MIN_OUT, MAX_U64, ctx,
        );

        // 3. Buy Base with Quote on DeepBook
        let mut base_out = deepbook_adapter::swap_quote_for_base_cleanup<Base, Quote>(
            deepbook_pool, coin_quote, deep_fee, clock, ctx,
        );

        // 4. Validate
        let received = coin::value(&base_out);
        profit::assert_profit(received, amount, min_profit);

        // 5. Repay
        let repay = coin::split(&mut base_out, amount, ctx);
        deepbook_adapter::flash_return_base<Base, Quote>(deepbook_pool, repay, flash_receipt);

        events::emit_arb_executed(b"deepbook_to_aftermath", amount, received);
        transfer::public_transfer(base_out, tx_context::sender(ctx));
    }

    // ════════════════════════════════════════════════════════════
    //  Cetus / Turbos / DeepBook ↔ FlowX CLMM
    //  FlowX CLMM supports flash swaps (hot-potato SwapReceipt).
    //  NOTE: FlowX SwapReceipt has no public pay_amount reader
    //  (same as Turbos H-2). Repayment uses `amount` directly.
    // ════════════════════════════════════════════════════════════

    /// Flash swap A→B on Cetus, sell B→A on FlowX CLMM, repay Cetus, keep A profit.
    entry fun arb_cetus_to_flowx_clmm<A, B>(
        _admin: &AdminCap,
        pause: &PauseFlag,
        cetus_config: &GlobalConfig,
        cetus_pool: &mut CetusPool<A, B>,
        flowx_pool: &mut FlowxPool<A, B>,
        flowx_versioned: &FlowxVersioned,
        amount: u64,
        min_profit: u64,
        clock: &Clock,
        ctx: &mut TxContext,
    ) {
        assert!(amount > 0, E_ZERO_AMOUNT);
        arb_move::admin::assert_not_paused(pause);

        // 1. Flash swap A→B on Cetus
        let (recv_a, recv_b, receipt) = cetus_adapter::flash_swap_a2b<A, B>(
            cetus_config, cetus_pool, amount, clock,
        );
        balance::destroy_zero(recv_a);

        // 2. Sell B→A on FlowX CLMM
        let coin_b = coin::from_balance(recv_b, ctx);
        let mut coin_a_out = flowx_clmm_adapter::swap_coin_b2a<A, B>(
            flowx_pool, coin_b, flowx_versioned, clock, ctx,
        );

        // 3. Validate
        let owed = cetus_adapter::swap_pay_amount(&receipt);
        let received = coin::value(&coin_a_out);
        profit::assert_profit(received, owed, min_profit);

        // 4. Repay Cetus
        let repay = coin::split(&mut coin_a_out, owed, ctx);
        cetus_adapter::repay_flash_swap<A, B>(
            cetus_config, cetus_pool,
            coin::into_balance(repay),
            balance::zero<B>(),
            receipt,
        );

        events::emit_arb_executed(b"cetus_to_flowx_clmm", owed, received);
        transfer::public_transfer(coin_a_out, tx_context::sender(ctx));
    }

    /// Flash swap A→B on FlowX CLMM, sell B→A on Cetus, repay FlowX, keep A profit.
    entry fun arb_flowx_clmm_to_cetus<A, B>(
        _admin: &AdminCap,
        pause: &PauseFlag,
        cetus_config: &GlobalConfig,
        cetus_pool: &mut CetusPool<A, B>,
        flowx_pool: &mut FlowxPool<A, B>,
        flowx_versioned: &FlowxVersioned,
        amount: u64,
        min_profit: u64,
        clock: &Clock,
        ctx: &mut TxContext,
    ) {
        assert!(amount > 0, E_ZERO_AMOUNT);
        arb_move::admin::assert_not_paused(pause);

        // 1. Flash swap A→B on FlowX CLMM
        let (recv_a, recv_b, receipt) = flowx_clmm_adapter::swap_a2b<A, B>(
            flowx_pool, amount, flowx_versioned, clock, ctx,
        );
        balance::destroy_zero(recv_a);

        // 2. Sell B→A on Cetus (Balance level)
        let b_amount = balance::value(&recv_b);
        let recv_a_final = cetus_adapter::swap_b2a<A, B>(
            cetus_config, cetus_pool, recv_b, b_amount, clock,
        );

        // 3. Convert to Coin, validate profit
        let mut coin_a_out = coin::from_balance(recv_a_final, ctx);
        let received = coin::value(&coin_a_out);
        profit::assert_profit(received, amount, min_profit);

        // 4. Repay FlowX CLMM with A
        let repay = coin::split(&mut coin_a_out, amount, ctx);
        flowx_clmm_adapter::pay<A, B>(
            flowx_pool, receipt,
            coin::into_balance(repay),
            balance::zero<B>(),
            flowx_versioned, ctx,
        );

        events::emit_arb_executed(b"flowx_clmm_to_cetus", amount, received);
        transfer::public_transfer(coin_a_out, tx_context::sender(ctx));
    }

    /// Flash swap A→B on Turbos, sell B→A on FlowX CLMM, repay Turbos, keep A profit.
    entry fun arb_turbos_to_flowx_clmm<A, B, TurbosFee>(
        _admin: &AdminCap,
        pause: &PauseFlag,
        turbos_pool: &mut TurbosPool<A, B, TurbosFee>,
        turbos_versioned: &Versioned,
        flowx_pool: &mut FlowxPool<A, B>,
        flowx_versioned: &FlowxVersioned,
        amount: u64,
        min_profit: u64,
        clock: &Clock,
        ctx: &mut TxContext,
    ) {
        assert!(amount > 0, E_ZERO_AMOUNT);
        arb_move::admin::assert_not_paused(pause);

        // 1. Flash swap A→B on Turbos
        let (recv_a, recv_b, receipt) = turbos_adapter::flash_swap_a2b<A, B, TurbosFee>(
            turbos_pool, (amount as u128), clock, turbos_versioned, ctx,
        );
        coin::destroy_zero(recv_a);

        // 2. Sell B→A on FlowX CLMM
        let mut coin_a_out = flowx_clmm_adapter::swap_coin_b2a<A, B>(
            flowx_pool, recv_b, flowx_versioned, clock, ctx,
        );

        // 3. Validate
        let received = coin::value(&coin_a_out);
        profit::assert_profit(received, amount, min_profit);

        // 4. Repay Turbos
        let repay_a = coin::split(&mut coin_a_out, amount, ctx);
        turbos_adapter::repay_flash_swap<A, B, TurbosFee>(
            turbos_pool, repay_a, coin::zero<B>(ctx), receipt, turbos_versioned,
        );

        events::emit_arb_executed(b"turbos_to_flowx_clmm", amount, received);
        transfer::public_transfer(coin_a_out, tx_context::sender(ctx));
    }

    /// Flash swap A→B on FlowX CLMM, sell B→A on Turbos, repay FlowX, keep A profit.
    entry fun arb_flowx_clmm_to_turbos<A, B, TurbosFee>(
        _admin: &AdminCap,
        pause: &PauseFlag,
        turbos_pool: &mut TurbosPool<A, B, TurbosFee>,
        turbos_versioned: &Versioned,
        flowx_pool: &mut FlowxPool<A, B>,
        flowx_versioned: &FlowxVersioned,
        amount: u64,
        min_profit: u64,
        clock: &Clock,
        ctx: &mut TxContext,
    ) {
        assert!(amount > 0, E_ZERO_AMOUNT);
        arb_move::admin::assert_not_paused(pause);

        // 1. Flash swap A→B on FlowX CLMM
        let (recv_a, recv_b, receipt) = flowx_clmm_adapter::swap_a2b<A, B>(
            flowx_pool, amount, flowx_versioned, clock, ctx,
        );
        balance::destroy_zero(recv_a);

        // 2. Sell B→A on Turbos
        let coin_b = coin::from_balance(recv_b, ctx);
        let b_amount = coin::value(&coin_b);
        let mut coin_a_out = turbos_adapter::swap_b_to_a<A, B, TurbosFee>(
            turbos_pool, coin_b, b_amount, clock, turbos_versioned, ctx,
        );

        // 3. Validate
        let received = coin::value(&coin_a_out);
        profit::assert_profit(received, amount, min_profit);

        // 4. Repay FlowX CLMM
        let repay = coin::split(&mut coin_a_out, amount, ctx);
        flowx_clmm_adapter::pay<A, B>(
            flowx_pool, receipt,
            coin::into_balance(repay),
            balance::zero<B>(),
            flowx_versioned, ctx,
        );

        events::emit_arb_executed(b"flowx_clmm_to_turbos", amount, received);
        transfer::public_transfer(coin_a_out, tx_context::sender(ctx));
    }

    /// Flash borrow Base from DeepBook, sell Base→Quote on FlowX CLMM, buy Base on DeepBook, repay.
    entry fun arb_deepbook_to_flowx_clmm<Base, Quote>(
        _admin: &AdminCap,
        pause: &PauseFlag,
        deepbook_pool: &mut DeepBookPool<Base, Quote>,
        deep_fee: Coin<DEEP>,
        flowx_pool: &mut FlowxPool<Base, Quote>,
        flowx_versioned: &FlowxVersioned,
        amount: u64,
        min_profit: u64,
        clock: &Clock,
        ctx: &mut TxContext,
    ) {
        assert!(amount > 0, E_ZERO_AMOUNT);
        arb_move::admin::assert_not_paused(pause);

        // 1. Flash borrow Base from DeepBook
        let (borrowed, flash_receipt) = deepbook_adapter::flash_borrow_base<Base, Quote>(
            deepbook_pool, amount, ctx,
        );

        // 2. Sell Base→Quote on FlowX CLMM
        let coin_quote = flowx_clmm_adapter::swap_coin_a2b<Base, Quote>(
            flowx_pool, borrowed, flowx_versioned, clock, ctx,
        );

        // 3. Buy Base with Quote on DeepBook
        let mut base_out = deepbook_adapter::swap_quote_for_base_cleanup<Base, Quote>(
            deepbook_pool, coin_quote, deep_fee, clock, ctx,
        );

        // 4. Validate
        let received = coin::value(&base_out);
        profit::assert_profit(received, amount, min_profit);

        // 5. Repay
        let repay = coin::split(&mut base_out, amount, ctx);
        deepbook_adapter::flash_return_base<Base, Quote>(deepbook_pool, repay, flash_receipt);

        events::emit_arb_executed(b"deepbook_to_flowx_clmm", amount, received);
        transfer::public_transfer(base_out, tx_context::sender(ctx));
    }

    /// Flash swap Base→Quote on FlowX CLMM, sell Quote→Base on DeepBook, repay FlowX, keep profit.
    entry fun arb_flowx_clmm_to_deepbook<Base, Quote>(
        _admin: &AdminCap,
        pause: &PauseFlag,
        deepbook_pool: &mut DeepBookPool<Base, Quote>,
        deep_fee: Coin<DEEP>,
        flowx_pool: &mut FlowxPool<Base, Quote>,
        flowx_versioned: &FlowxVersioned,
        amount: u64,
        min_profit: u64,
        clock: &Clock,
        ctx: &mut TxContext,
    ) {
        assert!(amount > 0, E_ZERO_AMOUNT);
        arb_move::admin::assert_not_paused(pause);

        // 1. Flash swap Base→Quote on FlowX CLMM
        let (recv_base, recv_quote, receipt) = flowx_clmm_adapter::swap_a2b<Base, Quote>(
            flowx_pool, amount, flowx_versioned, clock, ctx,
        );
        balance::destroy_zero(recv_base);

        // 2. Sell Quote→Base on DeepBook
        let coin_quote = coin::from_balance(recv_quote, ctx);
        let mut base_out = deepbook_adapter::swap_quote_for_base_cleanup<Base, Quote>(
            deepbook_pool, coin_quote, deep_fee, clock, ctx,
        );

        // 3. Validate
        let received = coin::value(&base_out);
        profit::assert_profit(received, amount, min_profit);

        // 4. Repay FlowX CLMM with Base
        let repay = coin::split(&mut base_out, amount, ctx);
        flowx_clmm_adapter::pay<Base, Quote>(
            flowx_pool, receipt,
            coin::into_balance(repay),
            balance::zero<Quote>(),
            flowx_versioned, ctx,
        );

        events::emit_arb_executed(b"flowx_clmm_to_deepbook", amount, received);
        transfer::public_transfer(base_out, tx_context::sender(ctx));
    }
}
