/// Thin wrapper around Turbos CLMM.
/// Turbos uses Coin<T> (not Balance) and requires vector<Coin<T>> for swap_router.
/// Pool has three type params: Pool<CoinTypeA, CoinTypeB, FeeType>.
#[allow(lint(self_transfer))]
module arb_move::turbos_adapter {
    use sui::coin::{Self, Coin};
    use sui::clock::Clock;

    use turbos_clmm::pool::{Self, Pool, Versioned, FlashSwapReceipt};
    use turbos_clmm::swap_router;

    const MIN_SQRT_PRICE: u128 = 4295048016;
    const MAX_SQRT_PRICE: u128 = 79226673515401279992447579055;
    /// u64 max — effectively no deadline.
    const NO_DEADLINE: u64 = 18446744073709551615;

    /// Swap Coin<A> → Coin<B> on Turbos.
    public(package) fun swap_a_to_b<A, B, Fee>(
        pool: &mut Pool<A, B, Fee>,
        coin_in: Coin<A>,
        amount: u64,
        clock: &Clock,
        versioned: &Versioned,
        ctx: &mut TxContext,
    ): Coin<B> {
        let coins_vec = vector::singleton(coin_in);
        let (coin_out, coin_remainder) = swap_router::swap_a_b_with_return_<A, B, Fee>(
            pool,
            coins_vec,
            amount,
            0,                  // amount_threshold (no min output — profit checked later)
            MIN_SQRT_PRICE,
            true,               // is_exact_in
            tx_context::sender(ctx),
            NO_DEADLINE,
            clock,
            versioned,
            ctx,
        );
        destroy_or_return(coin_remainder, ctx);
        coin_out
    }

    /// Swap Coin<B> → Coin<A> on Turbos.
    public(package) fun swap_b_to_a<A, B, Fee>(
        pool: &mut Pool<A, B, Fee>,
        coin_in: Coin<B>,
        amount: u64,
        clock: &Clock,
        versioned: &Versioned,
        ctx: &mut TxContext,
    ): Coin<A> {
        let coins_vec = vector::singleton(coin_in);
        let (coin_out, coin_remainder) = swap_router::swap_b_a_with_return_<A, B, Fee>(
            pool,
            coins_vec,
            amount,
            0,
            MAX_SQRT_PRICE,
            true,
            tx_context::sender(ctx),
            NO_DEADLINE,
            clock,
            versioned,
            ctx,
        );
        destroy_or_return(coin_remainder, ctx);
        coin_out
    }

    // ── Flash swap (for strategies managing receipt lifecycle) ──

    /// Flash swap A→B on Turbos. Returns output coins and a receipt.
    public(package) fun flash_swap_a2b<A, B, Fee>(
        pool: &mut Pool<A, B, Fee>,
        amount: u128,
        clock: &Clock,
        versioned: &Versioned,
        ctx: &mut TxContext,
    ): (Coin<A>, Coin<B>, FlashSwapReceipt<A, B>) {
        pool::flash_swap<A, B, Fee>(
            pool,
            tx_context::sender(ctx),
            true,               // a_to_b
            amount,
            true,               // amount_specified_is_input
            MIN_SQRT_PRICE,
            clock,
            versioned,
            ctx,
        )
    }

    /// Flash swap B→A on Turbos.
    public(package) fun flash_swap_b2a<A, B, Fee>(
        pool: &mut Pool<A, B, Fee>,
        amount: u128,
        clock: &Clock,
        versioned: &Versioned,
        ctx: &mut TxContext,
    ): (Coin<A>, Coin<B>, FlashSwapReceipt<A, B>) {
        pool::flash_swap<A, B, Fee>(
            pool,
            tx_context::sender(ctx),
            false,              // b_to_a
            amount,
            true,
            MAX_SQRT_PRICE,
            clock,
            versioned,
            ctx,
        )
    }

    /// Repay a Turbos flash swap.
    public(package) fun repay_flash_swap<A, B, Fee>(
        pool: &mut Pool<A, B, Fee>,
        coin_a: Coin<A>,
        coin_b: Coin<B>,
        receipt: FlashSwapReceipt<A, B>,
        versioned: &Versioned,
    ) {
        pool::repay_flash_swap<A, B, Fee>(pool, coin_a, coin_b, receipt, versioned);
    }

    // ── Internal helpers ──

    /// Destroy a zero-value remainder coin, or transfer it back to sender.
    fun destroy_or_return<T>(coin: Coin<T>, ctx: &TxContext) {
        if (coin::value(&coin) == 0) {
            coin::destroy_zero(coin);
        } else {
            transfer::public_transfer(coin, tx_context::sender(ctx));
        }
    }

    // ── Tests ──

    #[test_only]
    use sui::test_scenario;

    #[test]
    fun test_destroy_or_return_zero_coin() {
        let mut scenario = test_scenario::begin(@0x1);
        {
            let ctx = scenario.ctx();
            let zero = coin::zero<sui::sui::SUI>(ctx);
            destroy_or_return(zero, ctx);
        };
        scenario.end();
    }

    #[test]
    fun test_destroy_or_return_nonzero_coin() {
        let mut scenario = test_scenario::begin(@0x1);
        {
            let ctx = scenario.ctx();
            let c = coin::mint_for_testing<sui::sui::SUI>(250, ctx);
            destroy_or_return(c, ctx);
        };
        scenario.next_tx(@0x1);
        {
            let c = scenario.take_from_sender<Coin<sui::sui::SUI>>();
            assert!(coin::value(&c) == 250);
            scenario.return_to_sender(c);
        };
        scenario.end();
    }

    #[test]
    fun test_destroy_or_return_one_wei() {
        let mut scenario = test_scenario::begin(@0x1);
        {
            let ctx = scenario.ctx();
            let c = coin::mint_for_testing<sui::sui::SUI>(1, ctx);
            destroy_or_return(c, ctx);
        };
        scenario.next_tx(@0x1);
        {
            let c = scenario.take_from_sender<Coin<sui::sui::SUI>>();
            assert!(coin::value(&c) == 1);
            scenario.return_to_sender(c);
        };
        scenario.end();
    }

    #[test]
    fun test_destroy_or_return_large_value() {
        let mut scenario = test_scenario::begin(@0x1);
        {
            let ctx = scenario.ctx();
            let c = coin::mint_for_testing<sui::sui::SUI>(18446744073709551615, ctx);
            destroy_or_return(c, ctx);
        };
        scenario.next_tx(@0x1);
        {
            let c = scenario.take_from_sender<Coin<sui::sui::SUI>>();
            assert!(coin::value(&c) == 18446744073709551615);
            scenario.return_to_sender(c);
        };
        scenario.end();
    }

    #[test]
    fun test_constants() {
        // Verify the sqrt price limits match known Uniswap V3 / CLMM boundaries
        assert!(MIN_SQRT_PRICE == 4295048016);
        assert!(MAX_SQRT_PRICE == 79226673515401279992447579055);
        assert!(NO_DEADLINE == 18446744073709551615); // u64::MAX
    }
}
