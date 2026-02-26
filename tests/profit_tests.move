#[test_only]
module arb_move::profit_tests {
    use sui::coin;
    use sui::balance;

    use arb_move::profit;

    // ════════════════════════════════════════════════════════════
    //  assert_profit — basic cases
    // ════════════════════════════════════════════════════════════

    #[test]
    fun test_assert_profit_exact_min() {
        // amount_out == amount_in + min_profit → should pass
        profit::assert_profit(110, 100, 10);
    }

    #[test]
    fun test_assert_profit_above_min() {
        // amount_out > amount_in + min_profit → should pass
        profit::assert_profit(200, 100, 10);
    }

    #[test]
    fun test_assert_profit_zero_min() {
        // min_profit = 0, amount_out == amount_in → should pass
        profit::assert_profit(100, 100, 0);
    }

    #[test]
    fun test_assert_profit_zero_values() {
        // All zeros → should pass (0 >= 0 + 0)
        profit::assert_profit(0, 0, 0);
    }

    #[test]
    #[expected_failure(abort_code = arb_move::profit::E_NOT_PROFITABLE)]
    fun test_assert_profit_below_min_aborts() {
        // amount_out < amount_in + min_profit → should abort
        profit::assert_profit(109, 100, 10);
    }

    #[test]
    #[expected_failure(abort_code = arb_move::profit::E_NOT_PROFITABLE)]
    fun test_assert_profit_no_profit_aborts() {
        // amount_out == amount_in, min_profit > 0 → should abort
        profit::assert_profit(100, 100, 1);
    }

    #[test]
    #[expected_failure(abort_code = arb_move::profit::E_NOT_PROFITABLE)]
    fun test_assert_profit_loss_aborts() {
        // amount_out < amount_in → should abort
        profit::assert_profit(90, 100, 0);
    }

    // ════════════════════════════════════════════════════════════
    //  assert_profit — edge cases and boundary values
    // ════════════════════════════════════════════════════════════

    #[test]
    fun test_assert_profit_one_wei_profit() {
        // Smallest possible profit
        profit::assert_profit(101, 100, 1);
    }

    #[test]
    #[expected_failure(abort_code = arb_move::profit::E_NOT_PROFITABLE)]
    fun test_assert_profit_one_wei_short() {
        // Off by one — should fail
        profit::assert_profit(100, 100, 1);
    }

    #[test]
    fun test_assert_profit_large_values() {
        // Realistic DEX amounts (e.g. 1B tokens with 9 decimals)
        let billion_tokens: u64 = 1_000_000_000_000_000_000;
        let profit_amount: u64 = 1_000_000; // tiny profit
        profit::assert_profit(billion_tokens + profit_amount, billion_tokens, profit_amount);
    }

    #[test]
    fun test_assert_profit_max_u64_out() {
        // Maximum u64 as output
        let max: u64 = 18446744073709551615;
        profit::assert_profit(max, 0, 0);
    }

    #[test]
    fun test_assert_profit_max_u64_exact() {
        // amount_in + min_profit = max u64 exactly
        let max: u64 = 18446744073709551615;
        profit::assert_profit(max, max - 1, 1);
    }

    #[test]
    #[expected_failure(abort_code = arb_move::profit::E_NOT_PROFITABLE)]
    fun test_assert_profit_overflow_handled() {
        // amount_in + min_profit would overflow u64, but checked subtraction
        // catches this: amount_out (max) >= amount_in (max) is true, but
        // amount_out - amount_in (0) >= min_profit (1) is false → E_NOT_PROFITABLE
        let max: u64 = 18446744073709551615;
        profit::assert_profit(max, max, 1);
    }

    #[test]
    fun test_assert_profit_zero_in_nonzero_out() {
        // Zero input, any output is profit
        profit::assert_profit(1, 0, 1);
    }

    #[test]
    fun test_assert_profit_zero_in_zero_min() {
        profit::assert_profit(0, 0, 0);
    }

    #[test]
    #[expected_failure(abort_code = arb_move::profit::E_NOT_PROFITABLE)]
    fun test_assert_profit_zero_out_nonzero_in() {
        // Got nothing back
        profit::assert_profit(0, 100, 0);
    }

    #[test]
    fun test_assert_profit_huge_surplus() {
        // 10x return
        profit::assert_profit(1_000_000, 100_000, 1);
    }

    #[test]
    #[expected_failure(abort_code = arb_move::profit::E_NOT_PROFITABLE)]
    fun test_assert_profit_high_min_profit() {
        // min_profit is unrealistically high
        profit::assert_profit(200, 100, 200);
    }

    // ════════════════════════════════════════════════════════════
    //  balance_to_coin / coin_to_balance — round-trip
    // ════════════════════════════════════════════════════════════

    #[test]
    fun test_balance_to_coin() {
        let mut ctx = tx_context::dummy();
        let bal = balance::create_for_testing<sui::sui::SUI>(500);
        let c = profit::balance_to_coin(bal, &mut ctx);
        assert!(coin::value(&c) == 500);
        coin::burn_for_testing(c);
    }

    #[test]
    fun test_coin_to_balance() {
        let mut ctx = tx_context::dummy();
        let c = coin::mint_for_testing<sui::sui::SUI>(300, &mut ctx);
        let bal = profit::coin_to_balance(c);
        assert!(balance::value(&bal) == 300);
        balance::destroy_for_testing(bal);
    }

    #[test]
    fun test_balance_coin_round_trip() {
        let mut ctx = tx_context::dummy();
        // Balance → Coin → Balance preserves value
        let bal = balance::create_for_testing<sui::sui::SUI>(12345);
        let c = profit::balance_to_coin(bal, &mut ctx);
        let bal2 = profit::coin_to_balance(c);
        assert!(balance::value(&bal2) == 12345);
        balance::destroy_for_testing(bal2);
    }

    #[test]
    fun test_coin_balance_round_trip() {
        let mut ctx = tx_context::dummy();
        // Coin → Balance → Coin preserves value
        let c = coin::mint_for_testing<sui::sui::SUI>(67890, &mut ctx);
        let bal = profit::coin_to_balance(c);
        let c2 = profit::balance_to_coin(bal, &mut ctx);
        assert!(coin::value(&c2) == 67890);
        coin::burn_for_testing(c2);
    }

    #[test]
    fun test_balance_to_coin_zero() {
        let mut ctx = tx_context::dummy();
        let bal = balance::create_for_testing<sui::sui::SUI>(0);
        let c = profit::balance_to_coin(bal, &mut ctx);
        assert!(coin::value(&c) == 0);
        coin::destroy_zero(c);
    }

    #[test]
    fun test_coin_to_balance_zero() {
        let mut ctx = tx_context::dummy();
        let c = coin::zero<sui::sui::SUI>(&mut ctx);
        let bal = profit::coin_to_balance(c);
        assert!(balance::value(&bal) == 0);
        balance::destroy_for_testing(bal);
    }

    #[test]
    fun test_balance_to_coin_large() {
        let mut ctx = tx_context::dummy();
        let large: u64 = 18446744073709551615; // u64::MAX
        let bal = balance::create_for_testing<sui::sui::SUI>(large);
        let c = profit::balance_to_coin(bal, &mut ctx);
        assert!(coin::value(&c) == large);
        coin::burn_for_testing(c);
    }

    // ════════════════════════════════════════════════════════════
    //  coin_value
    // ════════════════════════════════════════════════════════════

    #[test]
    fun test_coin_value() {
        let mut ctx = tx_context::dummy();
        let c = coin::mint_for_testing<sui::sui::SUI>(42, &mut ctx);
        assert!(profit::coin_value(&c) == 42);
        coin::burn_for_testing(c);
    }

    #[test]
    fun test_coin_value_zero() {
        let mut ctx = tx_context::dummy();
        let c = coin::zero<sui::sui::SUI>(&mut ctx);
        assert!(profit::coin_value(&c) == 0);
        coin::destroy_zero(c);
    }

    #[test]
    fun test_coin_value_large() {
        let mut ctx = tx_context::dummy();
        let large: u64 = 18446744073709551615;
        let c = coin::mint_for_testing<sui::sui::SUI>(large, &mut ctx);
        assert!(profit::coin_value(&c) == large);
        coin::burn_for_testing(c);
    }

    #[test]
    fun test_coin_value_does_not_consume() {
        // Verify coin_value is non-destructive (takes &Coin)
        let mut ctx = tx_context::dummy();
        let c = coin::mint_for_testing<sui::sui::SUI>(999, &mut ctx);
        let v1 = profit::coin_value(&c);
        let v2 = profit::coin_value(&c);
        assert!(v1 == v2);
        assert!(v1 == 999);
        coin::burn_for_testing(c);
    }

    // ════════════════════════════════════════════════════════════
    //  split_coin
    // ════════════════════════════════════════════════════════════

    #[test]
    fun test_split_coin() {
        let mut ctx = tx_context::dummy();
        let mut c = coin::mint_for_testing<sui::sui::SUI>(1000, &mut ctx);
        let split = profit::split_coin(&mut c, 400, &mut ctx);
        assert!(coin::value(&c) == 600);
        assert!(coin::value(&split) == 400);
        coin::burn_for_testing(c);
        coin::burn_for_testing(split);
    }

    #[test]
    fun test_split_coin_zero_amount() {
        let mut ctx = tx_context::dummy();
        let mut c = coin::mint_for_testing<sui::sui::SUI>(500, &mut ctx);
        let split = profit::split_coin(&mut c, 0, &mut ctx);
        assert!(coin::value(&c) == 500);
        assert!(coin::value(&split) == 0);
        coin::burn_for_testing(c);
        coin::destroy_zero(split);
    }

    #[test]
    fun test_split_coin_entire_amount() {
        let mut ctx = tx_context::dummy();
        let mut c = coin::mint_for_testing<sui::sui::SUI>(500, &mut ctx);
        let split = profit::split_coin(&mut c, 500, &mut ctx);
        assert!(coin::value(&c) == 0);
        assert!(coin::value(&split) == 500);
        coin::destroy_zero(c);
        coin::burn_for_testing(split);
    }

    #[test]
    fun test_split_coin_multiple_times() {
        let mut ctx = tx_context::dummy();
        let mut c = coin::mint_for_testing<sui::sui::SUI>(1000, &mut ctx);
        let s1 = profit::split_coin(&mut c, 300, &mut ctx);
        let s2 = profit::split_coin(&mut c, 200, &mut ctx);
        let s3 = profit::split_coin(&mut c, 100, &mut ctx);
        assert!(coin::value(&c) == 400);
        assert!(coin::value(&s1) == 300);
        assert!(coin::value(&s2) == 200);
        assert!(coin::value(&s3) == 100);
        coin::burn_for_testing(c);
        coin::burn_for_testing(s1);
        coin::burn_for_testing(s2);
        coin::burn_for_testing(s3);
    }

    #[test]
    #[expected_failure] // insufficient balance
    fun test_split_coin_exceeds_balance() {
        let mut ctx = tx_context::dummy();
        let mut c = coin::mint_for_testing<sui::sui::SUI>(100, &mut ctx);
        let _split = profit::split_coin(&mut c, 101, &mut ctx);
        coin::burn_for_testing(c);
        coin::burn_for_testing(_split);
    }

    // ════════════════════════════════════════════════════════════
    //  merge_coins
    // ════════════════════════════════════════════════════════════

    #[test]
    fun test_merge_coins() {
        let mut ctx = tx_context::dummy();
        let mut base = coin::mint_for_testing<sui::sui::SUI>(100, &mut ctx);
        let other = coin::mint_for_testing<sui::sui::SUI>(50, &mut ctx);
        profit::merge_coins(&mut base, other);
        assert!(coin::value(&base) == 150);
        coin::burn_for_testing(base);
    }

    #[test]
    fun test_merge_coins_zero_into_nonzero() {
        let mut ctx = tx_context::dummy();
        let mut base = coin::mint_for_testing<sui::sui::SUI>(100, &mut ctx);
        let zero = coin::zero<sui::sui::SUI>(&mut ctx);
        profit::merge_coins(&mut base, zero);
        assert!(coin::value(&base) == 100);
        coin::burn_for_testing(base);
    }

    #[test]
    fun test_merge_coins_nonzero_into_zero() {
        let mut ctx = tx_context::dummy();
        let mut base = coin::zero<sui::sui::SUI>(&mut ctx);
        let other = coin::mint_for_testing<sui::sui::SUI>(100, &mut ctx);
        profit::merge_coins(&mut base, other);
        assert!(coin::value(&base) == 100);
        coin::burn_for_testing(base);
    }

    #[test]
    fun test_merge_coins_both_zero() {
        let mut ctx = tx_context::dummy();
        let mut base = coin::zero<sui::sui::SUI>(&mut ctx);
        let other = coin::zero<sui::sui::SUI>(&mut ctx);
        profit::merge_coins(&mut base, other);
        assert!(coin::value(&base) == 0);
        coin::destroy_zero(base);
    }

    #[test]
    fun test_merge_coins_multiple() {
        let mut ctx = tx_context::dummy();
        let mut base = coin::mint_for_testing<sui::sui::SUI>(10, &mut ctx);
        let c1 = coin::mint_for_testing<sui::sui::SUI>(20, &mut ctx);
        let c2 = coin::mint_for_testing<sui::sui::SUI>(30, &mut ctx);
        let c3 = coin::mint_for_testing<sui::sui::SUI>(40, &mut ctx);
        profit::merge_coins(&mut base, c1);
        profit::merge_coins(&mut base, c2);
        profit::merge_coins(&mut base, c3);
        assert!(coin::value(&base) == 100);
        coin::burn_for_testing(base);
    }

    // ════════════════════════════════════════════════════════════
    //  zero_coin
    // ════════════════════════════════════════════════════════════

    #[test]
    fun test_zero_coin() {
        let mut ctx = tx_context::dummy();
        let c = profit::zero_coin<sui::sui::SUI>(&mut ctx);
        assert!(coin::value(&c) == 0);
        coin::destroy_zero(c);
    }

    #[test]
    fun test_zero_coin_merge_then_check() {
        let mut ctx = tx_context::dummy();
        let mut z = profit::zero_coin<sui::sui::SUI>(&mut ctx);
        let other = coin::mint_for_testing<sui::sui::SUI>(42, &mut ctx);
        profit::merge_coins(&mut z, other);
        assert!(coin::value(&z) == 42);
        coin::burn_for_testing(z);
    }

    // ════════════════════════════════════════════════════════════
    //  Combined workflows — simulating arb profit extraction
    // ════════════════════════════════════════════════════════════

    #[test]
    fun test_arb_profit_extraction_flow() {
        // Simulates: receive output coin, validate profit, split repayment, keep remainder
        let mut ctx = tx_context::dummy();
        let amount_borrowed: u64 = 1_000_000;
        let amount_received: u64 = 1_050_000; // 5% profit
        let min_profit: u64 = 10_000;

        // Simulate receiving output from a swap
        let mut output = coin::mint_for_testing<sui::sui::SUI>(amount_received, &mut ctx);

        // Validate profitability
        profit::assert_profit(
            profit::coin_value(&output),
            amount_borrowed,
            min_profit,
        );

        // Split repayment
        let repay = profit::split_coin(&mut output, amount_borrowed, &mut ctx);
        assert!(coin::value(&repay) == amount_borrowed);
        assert!(coin::value(&output) == amount_received - amount_borrowed);

        // The remainder is profit
        assert!(coin::value(&output) == 50_000);

        coin::burn_for_testing(output);
        coin::burn_for_testing(repay);
    }

    #[test]
    #[expected_failure(abort_code = arb_move::profit::E_NOT_PROFITABLE)]
    fun test_arb_unprofitable_flow_aborts() {
        // Simulates: output is less than borrowed + min_profit
        let amount_borrowed: u64 = 1_000_000;
        let amount_received: u64 = 1_005_000; // only 0.5% return
        let min_profit: u64 = 10_000;         // need 1% min

        profit::assert_profit(amount_received, amount_borrowed, min_profit);
    }

    #[test]
    fun test_split_merge_round_trip() {
        // Split a coin, then merge it back — value should be preserved
        let mut ctx = tx_context::dummy();
        let mut c = coin::mint_for_testing<sui::sui::SUI>(1000, &mut ctx);
        let split = profit::split_coin(&mut c, 600, &mut ctx);
        assert!(coin::value(&c) == 400);
        assert!(coin::value(&split) == 600);
        profit::merge_coins(&mut c, split);
        assert!(coin::value(&c) == 1000);
        coin::burn_for_testing(c);
    }

    #[test]
    fun test_multi_hop_coin_tracking() {
        // Simulates coin value tracking through multiple conversions
        let mut ctx = tx_context::dummy();

        // Start with a coin
        let c = coin::mint_for_testing<sui::sui::SUI>(5000, &mut ctx);
        assert!(profit::coin_value(&c) == 5000);

        // Convert to balance and back
        let bal = profit::coin_to_balance(c);
        assert!(balance::value(&bal) == 5000);
        let mut c2 = profit::balance_to_coin(bal, &mut ctx);
        assert!(profit::coin_value(&c2) == 5000);

        // Split some off
        let portion = profit::split_coin(&mut c2, 3000, &mut ctx);
        assert!(profit::coin_value(&c2) == 2000);
        assert!(profit::coin_value(&portion) == 3000);

        // Merge back
        profit::merge_coins(&mut c2, portion);
        assert!(profit::coin_value(&c2) == 5000);

        coin::burn_for_testing(c2);
    }
}
