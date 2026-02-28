/// Thin wrapper around Cetus CLMM.
/// Cetus works with Balance<T> internally; this adapter provides both
/// Balance-level and Coin-level convenience functions.
module arb_move::cetus_adapter {
    use sui::balance::{Self, Balance};
    use sui::coin::{Self, Coin};
    use sui::clock::Clock;

    use cetusclmm::pool::{Self, Pool, FlashSwapReceipt};
    use cetusclmm::config::GlobalConfig;

    /// Minimum sqrt price for a2b swaps (price decreases).
    const MIN_SQRT_PRICE: u128 = 4295048016;
    /// Maximum sqrt price for b2a swaps (price increases).
    const MAX_SQRT_PRICE: u128 = 79226673515401279992447579055;

    // ── Balance-level swaps (used by strategies that already hold Balance<T>) ──

    /// Swap A→B on Cetus using flash_swap. Consumes `input` Balance<A>, returns Balance<B>.
    public(package) fun swap_a2b<A, B>(
        config: &GlobalConfig,
        pool: &mut Pool<A, B>,
        input: Balance<A>,
        amount: u64,
        clock: &Clock,
    ): Balance<B> {
        let (recv_a, recv_b, receipt) = pool::flash_swap<A, B>(
            config,
            pool,
            true,   // a2b
            true,   // by_amount_in
            amount,
            MIN_SQRT_PRICE,
            clock,
        );
        // For a2b: recv_a is zero, recv_b is the output.
        balance::destroy_zero(recv_a);

        pool::repay_flash_swap<A, B>(
            config,
            pool,
            input,
            balance::zero<B>(),
            receipt,
        );

        recv_b
    }

    /// Swap B→A on Cetus using flash_swap. Consumes `input` Balance<B>, returns Balance<A>.
    public(package) fun swap_b2a<A, B>(
        config: &GlobalConfig,
        pool: &mut Pool<A, B>,
        input: Balance<B>,
        amount: u64,
        clock: &Clock,
    ): Balance<A> {
        let (recv_a, recv_b, receipt) = pool::flash_swap<A, B>(
            config,
            pool,
            false,  // b2a
            true,   // by_amount_in
            amount,
            MAX_SQRT_PRICE,
            clock,
        );
        balance::destroy_zero(recv_b);

        pool::repay_flash_swap<A, B>(
            config,
            pool,
            balance::zero<A>(),
            input,
            receipt,
        );

        recv_a
    }

    // ── Coin-level convenience wrappers ──

    /// Swap Coin<A> → Coin<B> via Cetus.
    public(package) fun swap_coin_a2b<A, B>(
        config: &GlobalConfig,
        pool: &mut Pool<A, B>,
        coin_in: Coin<A>,
        clock: &Clock,
        ctx: &mut TxContext,
    ): Coin<B> {
        let amount = coin::value(&coin_in);
        let bal_out = swap_a2b<A, B>(config, pool, coin::into_balance(coin_in), amount, clock);
        coin::from_balance(bal_out, ctx)
    }

    /// Swap Coin<B> → Coin<A> via Cetus.
    public(package) fun swap_coin_b2a<A, B>(
        config: &GlobalConfig,
        pool: &mut Pool<A, B>,
        coin_in: Coin<B>,
        clock: &Clock,
        ctx: &mut TxContext,
    ): Coin<A> {
        let amount = coin::value(&coin_in);
        let bal_out = swap_b2a<A, B>(config, pool, coin::into_balance(coin_in), amount, clock);
        coin::from_balance(bal_out, ctx)
    }

    // ── Flash swap helpers (for strategies that manage receipt lifecycle) ──

    /// Execute a Cetus flash swap without immediate repayment.
    /// Caller is responsible for repaying via `repay_flash_swap`.
    public(package) fun flash_swap_a2b<A, B>(
        config: &GlobalConfig,
        pool: &mut Pool<A, B>,
        amount: u64,
        clock: &Clock,
    ): (Balance<A>, Balance<B>, FlashSwapReceipt<A, B>) {
        pool::flash_swap<A, B>(config, pool, true, true, amount, MIN_SQRT_PRICE, clock)
    }

    public(package) fun flash_swap_b2a<A, B>(
        config: &GlobalConfig,
        pool: &mut Pool<A, B>,
        amount: u64,
        clock: &Clock,
    ): (Balance<A>, Balance<B>, FlashSwapReceipt<A, B>) {
        pool::flash_swap<A, B>(config, pool, false, true, amount, MAX_SQRT_PRICE, clock)
    }

    /// Repay a flash swap receipt.
    public(package) fun repay_flash_swap<A, B>(
        config: &GlobalConfig,
        pool: &mut Pool<A, B>,
        balance_a: Balance<A>,
        balance_b: Balance<B>,
        receipt: FlashSwapReceipt<A, B>,
    ) {
        pool::repay_flash_swap<A, B>(config, pool, balance_a, balance_b, receipt);
    }

    /// Read the amount owed from a flash swap receipt.
    public(package) fun swap_pay_amount<A, B>(receipt: &FlashSwapReceipt<A, B>): u64 {
        pool::swap_pay_amount(receipt)
    }

    // ── Tests ──

    #[test]
    fun test_constants() {
        // Verify sqrt price limits match known CLMM tick boundaries
        assert!(MIN_SQRT_PRICE == 4295048016);
        assert!(MAX_SQRT_PRICE == 79226673515401279992447579055);
        // MIN < MAX
        assert!(MIN_SQRT_PRICE < MAX_SQRT_PRICE);
    }
}
