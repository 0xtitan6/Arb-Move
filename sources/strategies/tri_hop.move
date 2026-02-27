/// Triangular arbitrage strategies (A → B → C → A).
/// Flash-borrow A, route through three pools across different DEXes, repay, keep profit.
/// Supported DEXes: Cetus CLMM, Turbos CLMM, DeepBook V3, Aftermath AMM, FlowX CLMM v3.
module arb_move::tri_hop {
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
    const MAX_U64: u64 = 18446744073709551615;

    // ════════════════════════════════════════════════════════════
    //  All-Cetus triangle: Cetus(A/B) → Cetus(B/C) → Cetus(C/A)
    // ════════════════════════════════════════════════════════════

    /// A→B on Cetus pool_ab, B→C on Cetus pool_bc, C→A on Cetus pool_ca.
    /// Flash swap on pool_ab, repay with profit in A.
    entry fun tri_cetus_cetus_cetus<A, B, C>(
        _admin: &AdminCap,
        pause: &PauseFlag,
        config: &GlobalConfig,
        pool_ab: &mut CetusPool<A, B>,
        pool_bc: &mut CetusPool<B, C>,
        pool_ca: &mut CetusPool<C, A>,
        amount: u64,
        min_profit: u64,
        clock: &Clock,
        ctx: &mut TxContext,
    ) {
        assert!(amount > 0, E_ZERO_AMOUNT);
        arb_move::admin::assert_not_paused(pause);

        // 1. Flash swap A→B on pool_ab
        let (recv_a, recv_b, receipt) = cetus_adapter::flash_swap_a2b<A, B>(
            config, pool_ab, amount, clock,
        );
        balance::destroy_zero(recv_a);
        let b_amount = balance::value(&recv_b);

        // 2. Swap B→C on pool_bc
        let recv_c = cetus_adapter::swap_a2b<B, C>(
            config, pool_bc, recv_b, b_amount, clock,
        );
        let c_amount = balance::value(&recv_c);

        // 3. Swap C→A on pool_ca
        let recv_a_final = cetus_adapter::swap_a2b<C, A>(
            config, pool_ca, recv_c, c_amount, clock,
        );

        // 4. Validate profit
        let owed = cetus_adapter::swap_pay_amount(&receipt);
        let mut coin_a_out = coin::from_balance(recv_a_final, ctx);
        let received = coin::value(&coin_a_out);
        profit::assert_profit(received, owed, min_profit);

        // 5. Repay
        let repay = coin::split(&mut coin_a_out, owed, ctx);
        cetus_adapter::repay_flash_swap<A, B>(
            config, pool_ab,
            coin::into_balance(repay),
            balance::zero<B>(),
            receipt,
        );

        events::emit_arb_executed(b"tri_ccc", owed, received);
        transfer::public_transfer(coin_a_out, tx_context::sender(ctx));
    }

    // ════════════════════════════════════════════════════════════
    //  Cetus → Cetus → Turbos triangle
    // ════════════════════════════════════════════════════════════

    /// A→B on Cetus, B→C on Cetus, C→A on Turbos.
    /// Pool ordering for Turbos: Pool<C, A, Fee> means swap_a_to_b gives C→A.
    entry fun tri_cetus_cetus_turbos<A, B, C, TurbosFee>(
        _admin: &AdminCap,
        pause: &PauseFlag,
        cetus_config: &GlobalConfig,
        pool_ab: &mut CetusPool<A, B>,
        pool_bc: &mut CetusPool<B, C>,
        turbos_pool_ca: &mut TurbosPool<C, A, TurbosFee>,
        turbos_versioned: &Versioned,
        amount: u64,
        min_profit: u64,
        clock: &Clock,
        ctx: &mut TxContext,
    ) {
        assert!(amount > 0, E_ZERO_AMOUNT);
        arb_move::admin::assert_not_paused(pause);

        // 1. Flash swap A→B
        let (recv_a, recv_b, receipt) = cetus_adapter::flash_swap_a2b<A, B>(
            cetus_config, pool_ab, amount, clock,
        );
        balance::destroy_zero(recv_a);
        let b_amount = balance::value(&recv_b);

        // 2. B→C on Cetus
        let recv_c = cetus_adapter::swap_a2b<B, C>(
            cetus_config, pool_bc, recv_b, b_amount, clock,
        );

        // 3. C→A on Turbos (C is CoinTypeA in Pool<C, A, Fee>)
        let coin_c = coin::from_balance(recv_c, ctx);
        let c_amount = coin::value(&coin_c);
        let mut coin_a_out = turbos_adapter::swap_a_to_b<C, A, TurbosFee>(
            turbos_pool_ca, coin_c, c_amount, clock, turbos_versioned, ctx,
        );

        // 4. Validate
        let owed = cetus_adapter::swap_pay_amount(&receipt);
        let received = coin::value(&coin_a_out);
        profit::assert_profit(received, owed, min_profit);

        // 5. Repay
        let repay = coin::split(&mut coin_a_out, owed, ctx);
        cetus_adapter::repay_flash_swap<A, B>(
            cetus_config, pool_ab,
            coin::into_balance(repay),
            balance::zero<B>(),
            receipt,
        );

        events::emit_arb_executed(b"tri_cct", owed, received);
        transfer::public_transfer(coin_a_out, tx_context::sender(ctx));
    }

    // ════════════════════════════════════════════════════════════
    //  Cetus → Turbos → DeepBook triangle
    // ════════════════════════════════════════════════════════════

    /// A→B on Cetus, B→C on Turbos, C(quote)→A(base) on DeepBook.
    /// Assumes DeepBook pool ordering: Pool<A, C>.
    entry fun tri_cetus_turbos_deepbook<A, B, C, TurbosFee>(
        _admin: &AdminCap,
        pause: &PauseFlag,
        cetus_config: &GlobalConfig,
        cetus_pool_ab: &mut CetusPool<A, B>,
        turbos_pool_bc: &mut TurbosPool<B, C, TurbosFee>,
        turbos_versioned: &Versioned,
        deepbook_pool_ca: &mut DeepBookPool<A, C>,
        deep_fee: Coin<DEEP>,
        amount: u64,
        min_profit: u64,
        clock: &Clock,
        ctx: &mut TxContext,
    ) {
        assert!(amount > 0, E_ZERO_AMOUNT);
        arb_move::admin::assert_not_paused(pause);

        // 1. Flash swap A→B on Cetus
        let (recv_a, recv_b, receipt) = cetus_adapter::flash_swap_a2b<A, B>(
            cetus_config, cetus_pool_ab, amount, clock,
        );
        balance::destroy_zero(recv_a);

        // 2. B→C on Turbos
        let coin_b = coin::from_balance(recv_b, ctx);
        let b_amount = coin::value(&coin_b);
        let coin_c = turbos_adapter::swap_a_to_b<B, C, TurbosFee>(
            turbos_pool_bc, coin_b, b_amount, clock, turbos_versioned, ctx,
        );

        // 3. C→A on DeepBook (C=Quote, A=Base)
        let mut coin_a_out = deepbook_adapter::swap_quote_for_base_cleanup<A, C>(
            deepbook_pool_ca, coin_c, deep_fee, clock, ctx,
        );

        // 4. Validate
        let owed = cetus_adapter::swap_pay_amount(&receipt);
        let received = coin::value(&coin_a_out);
        profit::assert_profit(received, owed, min_profit);

        // 5. Repay
        let repay = coin::split(&mut coin_a_out, owed, ctx);
        cetus_adapter::repay_flash_swap<A, B>(
            cetus_config, cetus_pool_ab,
            coin::into_balance(repay),
            balance::zero<B>(),
            receipt,
        );

        events::emit_arb_executed(b"tri_ctd", owed, received);
        transfer::public_transfer(coin_a_out, tx_context::sender(ctx));
    }

    // ════════════════════════════════════════════════════════════
    //  Cetus → DeepBook → Turbos triangle
    // ════════════════════════════════════════════════════════════

    /// A→B on Cetus, B(base)→C(quote) on DeepBook, C→A on Turbos.
    entry fun tri_cetus_deepbook_turbos<A, B, C, TurbosFee>(
        _admin: &AdminCap,
        pause: &PauseFlag,
        cetus_config: &GlobalConfig,
        cetus_pool_ab: &mut CetusPool<A, B>,
        deepbook_pool_bc: &mut DeepBookPool<B, C>,
        deep_fee: Coin<DEEP>,
        turbos_pool_ca: &mut TurbosPool<C, A, TurbosFee>,
        turbos_versioned: &Versioned,
        amount: u64,
        min_profit: u64,
        clock: &Clock,
        ctx: &mut TxContext,
    ) {
        assert!(amount > 0, E_ZERO_AMOUNT);
        arb_move::admin::assert_not_paused(pause);

        // 1. Flash swap A→B on Cetus
        let (recv_a, recv_b, receipt) = cetus_adapter::flash_swap_a2b<A, B>(
            cetus_config, cetus_pool_ab, amount, clock,
        );
        balance::destroy_zero(recv_a);

        // 2. B→C on DeepBook (B=Base, C=Quote)
        let coin_b = coin::from_balance(recv_b, ctx);
        let coin_c = deepbook_adapter::swap_base_for_quote_cleanup<B, C>(
            deepbook_pool_bc, coin_b, deep_fee, clock, ctx,
        );

        // 3. C→A on Turbos (Pool<C, A, Fee>, swap C→A)
        let c_amount = coin::value(&coin_c);
        let mut coin_a_out = turbos_adapter::swap_a_to_b<C, A, TurbosFee>(
            turbos_pool_ca, coin_c, c_amount, clock, turbos_versioned, ctx,
        );

        // 4. Validate
        let owed = cetus_adapter::swap_pay_amount(&receipt);
        let received = coin::value(&coin_a_out);
        profit::assert_profit(received, owed, min_profit);

        // 5. Repay
        let repay = coin::split(&mut coin_a_out, owed, ctx);
        cetus_adapter::repay_flash_swap<A, B>(
            cetus_config, cetus_pool_ab,
            coin::into_balance(repay),
            balance::zero<B>(),
            receipt,
        );

        events::emit_arb_executed(b"tri_cdt", owed, received);
        transfer::public_transfer(coin_a_out, tx_context::sender(ctx));
    }

    // ════════════════════════════════════════════════════════════
    //  DeepBook-sourced triangle: DeepBook flash → Cetus → Turbos
    // ════════════════════════════════════════════════════════════

    /// Flash borrow A from DeepBook, A→B on Cetus, B→C on Turbos, C→A on DeepBook, repay.
    /// NOTE: Same-pool flash borrow + swap on DeepBook (see M-2 in audit).
    entry fun tri_deepbook_cetus_turbos<A, B, C, TurbosFee>(
        _admin: &AdminCap,
        pause: &PauseFlag,
        cetus_config: &GlobalConfig,
        deepbook_pool_ac: &mut DeepBookPool<A, C>,
        deep_fee: Coin<DEEP>,
        cetus_pool_ab: &mut CetusPool<A, B>,
        turbos_pool_bc: &mut TurbosPool<B, C, TurbosFee>,
        turbos_versioned: &Versioned,
        amount: u64,
        min_profit: u64,
        clock: &Clock,
        ctx: &mut TxContext,
    ) {
        assert!(amount > 0, E_ZERO_AMOUNT);
        arb_move::admin::assert_not_paused(pause);

        // 1. Flash borrow A from DeepBook
        let (borrowed_a, flash_receipt) = deepbook_adapter::flash_borrow_base<A, C>(
            deepbook_pool_ac, amount, ctx,
        );

        // 2. A→B on Cetus
        let bal_b = cetus_adapter::swap_a2b<A, B>(
            cetus_config, cetus_pool_ab,
            coin::into_balance(borrowed_a), amount, clock,
        );

        // 3. B→C on Turbos
        let coin_b = coin::from_balance(bal_b, ctx);
        let b_amount = coin::value(&coin_b);
        let coin_c = turbos_adapter::swap_a_to_b<B, C, TurbosFee>(
            turbos_pool_bc, coin_b, b_amount, clock, turbos_versioned, ctx,
        );

        // 4. C→A on DeepBook (buy A with C)
        let mut coin_a_out = deepbook_adapter::swap_quote_for_base_cleanup<A, C>(
            deepbook_pool_ac, coin_c, deep_fee, clock, ctx,
        );

        // 5. Validate
        let received = coin::value(&coin_a_out);
        profit::assert_profit(received, amount, min_profit);

        // 6. Repay flash loan
        let repay = coin::split(&mut coin_a_out, amount, ctx);
        deepbook_adapter::flash_return_base<A, C>(deepbook_pool_ac, repay, flash_receipt);

        events::emit_arb_executed(b"tri_dct", amount, received);
        transfer::public_transfer(coin_a_out, tx_context::sender(ctx));
    }

    // ════════════════════════════════════════════════════════════
    //  Aftermath triangles (sell leg only — no flash swap)
    // ════════════════════════════════════════════════════════════

    /// Cetus A→B, Cetus B→C, Aftermath C→A. Flash source = Cetus pool_ab.
    entry fun tri_cetus_cetus_aftermath<A, B, C, LP>(
        _admin: &AdminCap,
        pause: &PauseFlag,
        cetus_config: &GlobalConfig,
        pool_ab: &mut CetusPool<A, B>,
        pool_bc: &mut CetusPool<B, C>,
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

        // 1. Flash swap A→B on Cetus pool_ab
        let (recv_a, recv_b, receipt) = cetus_adapter::flash_swap_a2b<A, B>(
            cetus_config, pool_ab, amount, clock,
        );
        balance::destroy_zero(recv_a);
        let b_amount = balance::value(&recv_b);

        // 2. B→C on Cetus pool_bc (Balance level)
        let recv_c = cetus_adapter::swap_a2b<B, C>(
            cetus_config, pool_bc, recv_b, b_amount, clock,
        );

        // 3. C→A on Aftermath (Coin level)
        let coin_c = coin::from_balance(recv_c, ctx);
        let mut coin_a_out = aftermath_adapter::swap_exact_in<LP, C, A>(
            aftermath_pool, aftermath_registry, aftermath_fee_vault,
            aftermath_treasury, aftermath_insurance, aftermath_referral,
            coin_c, 0, MAX_U64, ctx,
        );

        // 4. Validate
        let owed = cetus_adapter::swap_pay_amount(&receipt);
        let received = coin::value(&coin_a_out);
        profit::assert_profit(received, owed, min_profit);

        // 5. Repay
        let repay = coin::split(&mut coin_a_out, owed, ctx);
        cetus_adapter::repay_flash_swap<A, B>(
            cetus_config, pool_ab,
            coin::into_balance(repay),
            balance::zero<B>(),
            receipt,
        );

        events::emit_arb_executed(b"tri_cca", owed, received);
        transfer::public_transfer(coin_a_out, tx_context::sender(ctx));
    }

    /// Cetus A→B, Turbos B→C, Aftermath C→A. Flash source = Cetus pool_ab.
    entry fun tri_cetus_turbos_aftermath<A, B, C, TurbosFee, LP>(
        _admin: &AdminCap,
        pause: &PauseFlag,
        cetus_config: &GlobalConfig,
        cetus_pool_ab: &mut CetusPool<A, B>,
        turbos_pool_bc: &mut TurbosPool<B, C, TurbosFee>,
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

        // 1. Flash swap A→B on Cetus
        let (recv_a, recv_b, receipt) = cetus_adapter::flash_swap_a2b<A, B>(
            cetus_config, cetus_pool_ab, amount, clock,
        );
        balance::destroy_zero(recv_a);

        // 2. B→C on Turbos
        let coin_b = coin::from_balance(recv_b, ctx);
        let b_amount = coin::value(&coin_b);
        let coin_c = turbos_adapter::swap_a_to_b<B, C, TurbosFee>(
            turbos_pool_bc, coin_b, b_amount, clock, turbos_versioned, ctx,
        );

        // 3. C→A on Aftermath
        let mut coin_a_out = aftermath_adapter::swap_exact_in<LP, C, A>(
            aftermath_pool, aftermath_registry, aftermath_fee_vault,
            aftermath_treasury, aftermath_insurance, aftermath_referral,
            coin_c, 0, MAX_U64, ctx,
        );

        // 4. Validate
        let owed = cetus_adapter::swap_pay_amount(&receipt);
        let received = coin::value(&coin_a_out);
        profit::assert_profit(received, owed, min_profit);

        // 5. Repay
        let repay = coin::split(&mut coin_a_out, owed, ctx);
        cetus_adapter::repay_flash_swap<A, B>(
            cetus_config, cetus_pool_ab,
            coin::into_balance(repay),
            balance::zero<B>(),
            receipt,
        );

        events::emit_arb_executed(b"tri_cta", owed, received);
        transfer::public_transfer(coin_a_out, tx_context::sender(ctx));
    }

    // ════════════════════════════════════════════════════════════
    //  FlowX CLMM triangles
    //  FlowX CLMM supports flash swaps — can be source or sell leg.
    //  NOTE: FlowX SwapReceipt has no public pay_amount reader
    //  (same as Turbos H-2). Repayment uses `amount` directly.
    // ════════════════════════════════════════════════════════════

    /// Cetus A→B, Cetus B→C, FlowX CLMM C→A. Flash source = Cetus pool_ab.
    entry fun tri_cetus_cetus_flowx_clmm<A, B, C>(
        _admin: &AdminCap,
        pause: &PauseFlag,
        cetus_config: &GlobalConfig,
        pool_ab: &mut CetusPool<A, B>,
        pool_bc: &mut CetusPool<B, C>,
        flowx_pool_ca: &mut FlowxPool<C, A>,
        flowx_versioned: &FlowxVersioned,
        amount: u64,
        min_profit: u64,
        clock: &Clock,
        ctx: &mut TxContext,
    ) {
        assert!(amount > 0, E_ZERO_AMOUNT);
        arb_move::admin::assert_not_paused(pause);

        // 1. Flash swap A→B on Cetus pool_ab
        let (recv_a, recv_b, receipt) = cetus_adapter::flash_swap_a2b<A, B>(
            cetus_config, pool_ab, amount, clock,
        );
        balance::destroy_zero(recv_a);
        let b_amount = balance::value(&recv_b);

        // 2. B→C on Cetus pool_bc (Balance level)
        let recv_c = cetus_adapter::swap_a2b<B, C>(
            cetus_config, pool_bc, recv_b, b_amount, clock,
        );

        // 3. C→A on FlowX CLMM (Coin level, Pool<C, A>)
        let coin_c = coin::from_balance(recv_c, ctx);
        let mut coin_a_out = flowx_clmm_adapter::swap_coin_a2b<C, A>(
            flowx_pool_ca, coin_c, flowx_versioned, clock, ctx,
        );

        // 4. Validate
        let owed = cetus_adapter::swap_pay_amount(&receipt);
        let received = coin::value(&coin_a_out);
        profit::assert_profit(received, owed, min_profit);

        // 5. Repay
        let repay = coin::split(&mut coin_a_out, owed, ctx);
        cetus_adapter::repay_flash_swap<A, B>(
            cetus_config, pool_ab,
            coin::into_balance(repay),
            balance::zero<B>(),
            receipt,
        );

        events::emit_arb_executed(b"tri_ccf", owed, received);
        transfer::public_transfer(coin_a_out, tx_context::sender(ctx));
    }

    /// Cetus A→B, FlowX CLMM B→C, Turbos C→A. Flash source = Cetus pool_ab.
    entry fun tri_cetus_flowx_clmm_turbos<A, B, C, TurbosFee>(
        _admin: &AdminCap,
        pause: &PauseFlag,
        cetus_config: &GlobalConfig,
        cetus_pool_ab: &mut CetusPool<A, B>,
        flowx_pool_bc: &mut FlowxPool<B, C>,
        flowx_versioned: &FlowxVersioned,
        turbos_pool_ca: &mut TurbosPool<C, A, TurbosFee>,
        turbos_versioned: &Versioned,
        amount: u64,
        min_profit: u64,
        clock: &Clock,
        ctx: &mut TxContext,
    ) {
        assert!(amount > 0, E_ZERO_AMOUNT);
        arb_move::admin::assert_not_paused(pause);

        // 1. Flash swap A→B on Cetus
        let (recv_a, recv_b, receipt) = cetus_adapter::flash_swap_a2b<A, B>(
            cetus_config, cetus_pool_ab, amount, clock,
        );
        balance::destroy_zero(recv_a);

        // 2. B→C on FlowX CLMM (Coin level, Pool<B, C>)
        let coin_b = coin::from_balance(recv_b, ctx);
        let coin_c = flowx_clmm_adapter::swap_coin_a2b<B, C>(
            flowx_pool_bc, coin_b, flowx_versioned, clock, ctx,
        );

        // 3. C→A on Turbos (Pool<C, A, Fee>)
        let c_amount = coin::value(&coin_c);
        let mut coin_a_out = turbos_adapter::swap_a_to_b<C, A, TurbosFee>(
            turbos_pool_ca, coin_c, c_amount, clock, turbos_versioned, ctx,
        );

        // 4. Validate
        let owed = cetus_adapter::swap_pay_amount(&receipt);
        let received = coin::value(&coin_a_out);
        profit::assert_profit(received, owed, min_profit);

        // 5. Repay
        let repay = coin::split(&mut coin_a_out, owed, ctx);
        cetus_adapter::repay_flash_swap<A, B>(
            cetus_config, cetus_pool_ab,
            coin::into_balance(repay),
            balance::zero<B>(),
            receipt,
        );

        events::emit_arb_executed(b"tri_cft", owed, received);
        transfer::public_transfer(coin_a_out, tx_context::sender(ctx));
    }

    /// FlowX CLMM flash A→B, Cetus B→C, Turbos C→A. Flash source = FlowX CLMM.
    /// NOTE: FlowX receipt has no pay_amount reader. Uses `amount` for repayment.
    entry fun tri_flowx_clmm_cetus_turbos<A, B, C, TurbosFee>(
        _admin: &AdminCap,
        pause: &PauseFlag,
        cetus_config: &GlobalConfig,
        flowx_pool_ab: &mut FlowxPool<A, B>,
        flowx_versioned: &FlowxVersioned,
        cetus_pool_bc: &mut CetusPool<B, C>,
        turbos_pool_ca: &mut TurbosPool<C, A, TurbosFee>,
        turbos_versioned: &Versioned,
        amount: u64,
        min_profit: u64,
        clock: &Clock,
        ctx: &mut TxContext,
    ) {
        assert!(amount > 0, E_ZERO_AMOUNT);
        arb_move::admin::assert_not_paused(pause);

        // 1. Flash swap A→B on FlowX CLMM
        let (recv_a, recv_b, receipt) = flowx_clmm_adapter::swap_a2b<A, B>(
            flowx_pool_ab, amount, flowx_versioned, clock, ctx,
        );
        balance::destroy_zero(recv_a);
        let b_amount = balance::value(&recv_b);

        // 2. B→C on Cetus (Balance level)
        let recv_c = cetus_adapter::swap_a2b<B, C>(
            cetus_config, cetus_pool_bc, recv_b, b_amount, clock,
        );

        // 3. C→A on Turbos (Coin level)
        let coin_c = coin::from_balance(recv_c, ctx);
        let c_amount = coin::value(&coin_c);
        let mut coin_a_out = turbos_adapter::swap_a_to_b<C, A, TurbosFee>(
            turbos_pool_ca, coin_c, c_amount, clock, turbos_versioned, ctx,
        );

        // 4. Validate
        let received = coin::value(&coin_a_out);
        profit::assert_profit(received, amount, min_profit);

        // 5. Repay FlowX CLMM with A
        let repay = coin::split(&mut coin_a_out, amount, ctx);
        flowx_clmm_adapter::pay<A, B>(
            flowx_pool_ab, receipt,
            coin::into_balance(repay),
            balance::zero<B>(),
            flowx_versioned, ctx,
        );

        events::emit_arb_executed(b"tri_fct", amount, received);
        transfer::public_transfer(coin_a_out, tx_context::sender(ctx));
    }
}
