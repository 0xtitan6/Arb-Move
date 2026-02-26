/// Triangular arbitrage strategies (A → B → C → A).
/// Flash-borrow A, route through three pools across different DEXes, repay, keep profit.
module arb_move::tri_hop {
    use sui::coin::{Self, Coin};
    use sui::balance;
    use sui::clock::Clock;

    use cetus_clmm::pool::{Pool as CetusPool};
    use cetus_clmm::config::GlobalConfig;
    use turbos_clmm::pool::{Pool as TurbosPool, Versioned};
    use deepbook::pool::{Pool as DeepBookPool};
    use token::deep::DEEP;

    use arb_move::admin::{AdminCap, PauseFlag};
    use arb_move::profit;
    use arb_move::events;
    use arb_move::cetus_adapter;
    use arb_move::turbos_adapter;
    use arb_move::deepbook_adapter;

    const E_ZERO_AMOUNT: u64 = 1;

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

        events::emit_arb_executed(b"tri_ccc", amount, received);
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

        events::emit_arb_executed(b"tri_cct", amount, received);
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

        events::emit_arb_executed(b"tri_ctd", amount, received);
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

        events::emit_arb_executed(b"tri_cdt", amount, received);
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
}
