/// Two-hop (DEX-to-DEX) arbitrage strategies.
/// Each entry function flash-borrows from one DEX, swaps on another, repays, and keeps profit.
/// All functions require AdminCap for authorization.
module arb_move::two_hop {
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
        events::emit_arb_executed(b"cetus_to_turbos", amount, received);
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

        events::emit_arb_executed(b"cetus_to_turbos_rev", amount, received);
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

        events::emit_arb_executed(b"cetus_to_deepbook", amount, received);
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
}
