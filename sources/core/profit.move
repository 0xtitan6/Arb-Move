module arb_move::profit {
    use sui::coin::{Self, Coin};
    use sui::balance::Balance;

    const E_NOT_PROFITABLE: u64 = 1;

    /// Abort if the arbitrage is not profitable.
    /// `amount_out` must exceed `amount_in + min_profit`.
    /// Uses checked subtraction to avoid u64 overflow when amount_in + min_profit > u64::MAX.
    public(package) fun assert_profit(amount_out: u64, amount_in: u64, min_profit: u64) {
        assert!(amount_out >= amount_in && amount_out - amount_in >= min_profit, E_NOT_PROFITABLE);
    }

    /// Convert a Balance<T> into a Coin<T>.
    public(package) fun balance_to_coin<T>(balance: Balance<T>, ctx: &mut TxContext): Coin<T> {
        coin::from_balance(balance, ctx)
    }

    /// Convert a Coin<T> into a Balance<T>.
    public(package) fun coin_to_balance<T>(c: Coin<T>): Balance<T> {
        coin::into_balance(c)
    }

    /// Read the value of a coin without consuming it.
    public(package) fun coin_value<T>(c: &Coin<T>): u64 {
        coin::value(c)
    }

    /// Split `amount` from a coin, returning the split portion.
    public(package) fun split_coin<T>(c: &mut Coin<T>, amount: u64, ctx: &mut TxContext): Coin<T> {
        coin::split(c, amount, ctx)
    }

    /// Merge `other` into `base`.
    public(package) fun merge_coins<T>(base: &mut Coin<T>, other: Coin<T>) {
        coin::join(base, other);
    }

    /// Create a zero-value coin.
    public(package) fun zero_coin<T>(ctx: &mut TxContext): Coin<T> {
        coin::zero(ctx)
    }
}
