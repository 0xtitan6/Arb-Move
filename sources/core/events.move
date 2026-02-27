/// Emits structured events for off-chain indexing of arb executions.
module arb_move::events {
    use sui::event;

    /// Emitted after every successful arbitrage execution.
    public struct ArbExecuted has copy, drop {
        /// Short identifier for the strategy (e.g. b"cetus_to_turbos").
        strategy: vector<u8>,
        /// Amount fed into the flash loan / first swap leg.
        amount_in: u64,
        /// Net profit transferred to the caller (amount_out - amount_in).
        profit: u64,
    }

    /// Emit an ArbExecuted event. Called by strategy modules after profit validation.
    public(package) fun emit_arb_executed(
        strategy: vector<u8>,
        amount_in: u64,
        amount_out: u64,
    ) {
        // Guard against underflow — caller should have validated profit already,
        // but saturate to 0 rather than abort if something went wrong.
        let profit = if (amount_out >= amount_in) {
            amount_out - amount_in
        } else {
            0
        };
        event::emit(ArbExecuted {
            strategy,
            amount_in,
            profit,
        });
    }
}
