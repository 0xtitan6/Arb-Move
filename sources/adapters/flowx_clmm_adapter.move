/// Thin wrapper around FlowX CLMM v3 (concentrated liquidity).
/// Very similar to Cetus — uses Balance<T> at pool level with
/// hot-potato SwapReceipt for flash swaps.
/// Pool has 2 type params: Pool<CoinTypeA, CoinTypeB>.
module arb_move::flowx_clmm_adapter {
    use sui::balance::{Self, Balance};
    use sui::coin::{Self, Coin};
    use sui::clock::Clock;

    use flowx_clmm::pool::{Self, Pool, SwapReceipt};
    use flowx_clmm::versioned::Versioned;

    /// Minimum sqrt price for a2b swaps (price decreases).
    const MIN_SQRT_PRICE: u128 = 4295048016;
    /// Maximum sqrt price for b2a swaps (price increases).
    const MAX_SQRT_PRICE: u128 = 79226673515401279992447579055;

    // ── Flash swap (receipt-based, caller settles) ──

    /// Flash swap A→B. Returns balances and a SwapReceipt that MUST be settled via pay().
    public(package) fun swap_a2b<A, B>(
        pool: &mut Pool<A, B>,
        amount: u64,
        versioned: &Versioned,
        clock: &Clock,
        ctx: &TxContext,
    ): (Balance<A>, Balance<B>, SwapReceipt) {
        pool::swap<A, B>(
            pool,
            true,       // a2b
            true,       // by_amount_in
            amount,
            MIN_SQRT_PRICE,
            versioned,
            clock,
            ctx,
        )
    }

    /// Flash swap B→A.
    public(package) fun swap_b2a<A, B>(
        pool: &mut Pool<A, B>,
        amount: u64,
        versioned: &Versioned,
        clock: &Clock,
        ctx: &TxContext,
    ): (Balance<A>, Balance<B>, SwapReceipt) {
        pool::swap<A, B>(
            pool,
            false,      // b2a
            true,       // by_amount_in
            amount,
            MAX_SQRT_PRICE,
            versioned,
            clock,
            ctx,
        )
    }

    /// Settle a SwapReceipt by providing owed balances.
    public(package) fun pay<A, B>(
        pool: &mut Pool<A, B>,
        receipt: SwapReceipt,
        balance_a: Balance<A>,
        balance_b: Balance<B>,
        versioned: &Versioned,
        ctx: &TxContext,
    ) {
        pool::pay<A, B>(pool, receipt, balance_a, balance_b, versioned, ctx);
    }

    // ── Convenience: swap with immediate settlement ──

    /// Swap A→B with immediate repayment. Consumes input Balance<A>, returns Balance<B>.
    public(package) fun swap_a2b_direct<A, B>(
        pool: &mut Pool<A, B>,
        input: Balance<A>,
        amount: u64,
        versioned: &Versioned,
        clock: &Clock,
        ctx: &TxContext,
    ): Balance<B> {
        let (recv_a, recv_b, receipt) = swap_a2b<A, B>(pool, amount, versioned, clock, ctx);
        balance::destroy_zero(recv_a);
        pay<A, B>(pool, receipt, input, balance::zero<B>(), versioned, ctx);
        recv_b
    }

    /// Swap B→A with immediate repayment.
    public(package) fun swap_b2a_direct<A, B>(
        pool: &mut Pool<A, B>,
        input: Balance<B>,
        amount: u64,
        versioned: &Versioned,
        clock: &Clock,
        ctx: &TxContext,
    ): Balance<A> {
        let (recv_a, recv_b, receipt) = swap_b2a<A, B>(pool, amount, versioned, clock, ctx);
        balance::destroy_zero(recv_b);
        pay<A, B>(pool, receipt, balance::zero<A>(), input, versioned, ctx);
        recv_a
    }

    // ── Coin-level convenience wrappers ──

    /// Swap Coin<A> → Coin<B> via FlowX CLMM.
    public(package) fun swap_coin_a2b<A, B>(
        pool: &mut Pool<A, B>,
        coin_in: Coin<A>,
        versioned: &Versioned,
        clock: &Clock,
        ctx: &mut TxContext,
    ): Coin<B> {
        let amount = coin::value(&coin_in);
        let bal_out = swap_a2b_direct<A, B>(
            pool, coin::into_balance(coin_in), amount, versioned, clock, ctx,
        );
        coin::from_balance(bal_out, ctx)
    }

    /// Swap Coin<B> → Coin<A> via FlowX CLMM.
    public(package) fun swap_coin_b2a<A, B>(
        pool: &mut Pool<A, B>,
        coin_in: Coin<B>,
        versioned: &Versioned,
        clock: &Clock,
        ctx: &mut TxContext,
    ): Coin<A> {
        let amount = coin::value(&coin_in);
        let bal_out = swap_b2a_direct<A, B>(
            pool, coin::into_balance(coin_in), amount, versioned, clock, ctx,
        );
        coin::from_balance(bal_out, ctx)
    }

    // ── Tests ──

    #[test]
    fun test_constants() {
        assert!(MIN_SQRT_PRICE == 4295048016);
        assert!(MAX_SQRT_PRICE == 79226673515401279992447579055);
        assert!(MIN_SQRT_PRICE < MAX_SQRT_PRICE);
    }
}
