/// Thin wrapper around DeepBook V3 CLOB.
/// DeepBook uses Coin<T> and requires a Coin<DEEP> for protocol fees.
/// Flash loans use the hot-potato FlashLoan struct (no abilities).
#[allow(lint(self_transfer))]
module arb_move::deepbook_adapter {
    use sui::coin::{Self, Coin};
    use sui::clock::Clock;

    use deepbook::pool::{Self, Pool};
    use deepbook::vault::FlashLoan;
    use token::deep::DEEP;

    // ── Swap functions ──

    /// Swap exact base for quote on DeepBook.
    /// Returns (remaining_base, received_quote, remaining_deep).
    public(package) fun swap_base_for_quote<Base, Quote>(
        pool: &mut Pool<Base, Quote>,
        base_in: Coin<Base>,
        deep_in: Coin<DEEP>,
        min_quote_out: u64,
        clock: &Clock,
        ctx: &mut TxContext,
    ): (Coin<Base>, Coin<Quote>, Coin<DEEP>) {
        pool::swap_exact_base_for_quote<Base, Quote>(
            pool, base_in, deep_in, min_quote_out, clock, ctx,
        )
    }

    /// Swap exact quote for base on DeepBook.
    /// Returns (received_base, remaining_quote, remaining_deep).
    public(package) fun swap_quote_for_base<Base, Quote>(
        pool: &mut Pool<Base, Quote>,
        quote_in: Coin<Quote>,
        deep_in: Coin<DEEP>,
        min_base_out: u64,
        clock: &Clock,
        ctx: &mut TxContext,
    ): (Coin<Base>, Coin<Quote>, Coin<DEEP>) {
        pool::swap_exact_quote_for_base<Base, Quote>(
            pool, quote_in, deep_in, min_base_out, clock, ctx,
        )
    }

    // ── Convenience: swap with automatic remainder cleanup ──

    /// Swap base→quote and transfer remainders back to sender. Returns only the quote output.
    public(package) fun swap_base_for_quote_cleanup<Base, Quote>(
        pool: &mut Pool<Base, Quote>,
        base_in: Coin<Base>,
        deep_in: Coin<DEEP>,
        clock: &Clock,
        ctx: &mut TxContext,
    ): Coin<Quote> {
        let (base_rem, quote_out, deep_rem) = pool::swap_exact_base_for_quote<Base, Quote>(
            pool, base_in, deep_in, 0, clock, ctx,
        );
        return_or_destroy(base_rem, ctx);
        return_or_destroy(deep_rem, ctx);
        quote_out
    }

    /// Swap quote→base and transfer remainders back to sender. Returns only the base output.
    public(package) fun swap_quote_for_base_cleanup<Base, Quote>(
        pool: &mut Pool<Base, Quote>,
        quote_in: Coin<Quote>,
        deep_in: Coin<DEEP>,
        clock: &Clock,
        ctx: &mut TxContext,
    ): Coin<Base> {
        let (base_out, quote_rem, deep_rem) = pool::swap_exact_quote_for_base<Base, Quote>(
            pool, quote_in, deep_in, 0, clock, ctx,
        );
        return_or_destroy(quote_rem, ctx);
        return_or_destroy(deep_rem, ctx);
        base_out
    }

    // ── Flash loan functions (hot potato pattern) ──

    /// Borrow base asset via flash loan. Returns (borrowed_coin, receipt).
    /// The FlashLoan receipt MUST be consumed by `flash_return_base` in the same tx.
    public(package) fun flash_borrow_base<Base, Quote>(
        pool: &mut Pool<Base, Quote>,
        amount: u64,
        ctx: &mut TxContext,
    ): (Coin<Base>, FlashLoan) {
        pool::borrow_flashloan_base<Base, Quote>(pool, amount, ctx)
    }

    /// Return borrowed base asset, consuming the flash loan receipt.
    public(package) fun flash_return_base<Base, Quote>(
        pool: &mut Pool<Base, Quote>,
        coin: Coin<Base>,
        receipt: FlashLoan,
    ) {
        pool::return_flashloan_base<Base, Quote>(pool, coin, receipt);
    }

    /// Borrow quote asset via flash loan.
    public(package) fun flash_borrow_quote<Base, Quote>(
        pool: &mut Pool<Base, Quote>,
        amount: u64,
        ctx: &mut TxContext,
    ): (Coin<Quote>, FlashLoan) {
        pool::borrow_flashloan_quote<Base, Quote>(pool, amount, ctx)
    }

    /// Return borrowed quote asset.
    public(package) fun flash_return_quote<Base, Quote>(
        pool: &mut Pool<Base, Quote>,
        coin: Coin<Quote>,
        receipt: FlashLoan,
    ) {
        pool::return_flashloan_quote<Base, Quote>(pool, coin, receipt);
    }

    // ── Internal helpers ──

    fun return_or_destroy<T>(coin: Coin<T>, ctx: &TxContext) {
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
    fun test_return_or_destroy_zero_coin() {
        // Zero-value coin should be destroyed (no transfer)
        let mut scenario = test_scenario::begin(@0x1);
        {
            let ctx = scenario.ctx();
            let zero = coin::zero<sui::sui::SUI>(ctx);
            return_or_destroy(zero, ctx);
        };
        scenario.end();
    }

    #[test]
    fun test_return_or_destroy_nonzero_coin() {
        // Non-zero coin should be transferred to sender
        let mut scenario = test_scenario::begin(@0x1);
        {
            let ctx = scenario.ctx();
            let c = coin::mint_for_testing<sui::sui::SUI>(100, ctx);
            return_or_destroy(c, ctx);
        };
        // Verify the coin was transferred to sender
        scenario.next_tx(@0x1);
        {
            let c = scenario.take_from_sender<Coin<sui::sui::SUI>>();
            assert!(coin::value(&c) == 100);
            scenario.return_to_sender(c);
        };
        scenario.end();
    }

    #[test]
    fun test_return_or_destroy_one_wei() {
        // Smallest non-zero value — should transfer, not destroy
        let mut scenario = test_scenario::begin(@0x1);
        {
            let ctx = scenario.ctx();
            let c = coin::mint_for_testing<sui::sui::SUI>(1, ctx);
            return_or_destroy(c, ctx);
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
    fun test_return_or_destroy_large_value() {
        let mut scenario = test_scenario::begin(@0x1);
        {
            let ctx = scenario.ctx();
            let c = coin::mint_for_testing<sui::sui::SUI>(18446744073709551615, ctx);
            return_or_destroy(c, ctx);
        };
        scenario.next_tx(@0x1);
        {
            let c = scenario.take_from_sender<Coin<sui::sui::SUI>>();
            assert!(coin::value(&c) == 18446744073709551615);
            scenario.return_to_sender(c);
        };
        scenario.end();
    }
}
