#[test_only]
module arb_move::admin_tests {
    use sui::test_scenario;

    use arb_move::admin::{Self, AdminCap, PauseFlag};

    const DEPLOYER: address = @0xCAFE;
    const OTHER: address = @0xBEEF;

    #[test]
    fun test_init_creates_admin_cap() {
        // Verify that module init creates an AdminCap and transfers it to the deployer
        let mut scenario = test_scenario::begin(DEPLOYER);
        {
            admin::init_for_testing(scenario.ctx());
        };

        // AdminCap should be owned by DEPLOYER
        scenario.next_tx(DEPLOYER);
        {
            let cap = scenario.take_from_sender<AdminCap>();
            scenario.return_to_sender(cap);
        };

        scenario.end();
    }

    #[test]
    fun test_init_creates_pause_flag() {
        // Verify that module init creates a shared PauseFlag
        let mut scenario = test_scenario::begin(DEPLOYER);
        {
            admin::init_for_testing(scenario.ctx());
        };

        scenario.next_tx(DEPLOYER);
        {
            let flag = scenario.take_shared<PauseFlag>();
            // Should not be paused initially
            admin::assert_not_paused(&flag);
            test_scenario::return_shared(flag);
        };

        scenario.end();
    }

    #[test]
    #[expected_failure]
    fun test_init_cap_not_owned_by_other() {
        // Verify that OTHER address does NOT receive the AdminCap
        let mut scenario = test_scenario::begin(DEPLOYER);
        {
            admin::init_for_testing(scenario.ctx());
        };

        // OTHER should NOT have AdminCap — this should fail
        scenario.next_tx(OTHER);
        {
            let cap = scenario.take_from_sender<AdminCap>();
            scenario.return_to_sender(cap);
        };

        scenario.end();
    }

    #[test]
    fun test_create_admin_cap_for_testing() {
        let mut ctx = tx_context::dummy();
        let cap = admin::create_admin_cap_for_testing(&mut ctx);
        // AdminCap has key only (no store) — must use module-scoped transfer
        admin::transfer_for_testing(cap, tx_context::sender(&ctx));
    }

    #[test]
    fun test_admin_cap_transfer() {
        // AdminCap can be transferred between addresses via module helper
        let mut scenario = test_scenario::begin(DEPLOYER);
        {
            admin::init_for_testing(scenario.ctx());
        };

        // DEPLOYER takes cap and transfers to OTHER
        scenario.next_tx(DEPLOYER);
        {
            let cap = scenario.take_from_sender<AdminCap>();
            admin::transfer_for_testing(cap, OTHER);
        };

        // OTHER should now own the AdminCap
        scenario.next_tx(OTHER);
        {
            let cap = scenario.take_from_sender<AdminCap>();
            scenario.return_to_sender(cap);
        };

        scenario.end();
    }

    #[test]
    fun test_multiple_admin_caps_for_testing() {
        // Can create multiple AdminCaps (for testing only)
        let mut ctx = tx_context::dummy();
        let cap1 = admin::create_admin_cap_for_testing(&mut ctx);
        let cap2 = admin::create_admin_cap_for_testing(&mut ctx);
        let sender = tx_context::sender(&ctx);
        admin::transfer_for_testing(cap1, sender);
        admin::transfer_for_testing(cap2, sender);
    }

    #[test]
    fun test_pause_and_unpause() {
        let mut scenario = test_scenario::begin(DEPLOYER);
        {
            admin::init_for_testing(scenario.ctx());
        };

        // Pause
        scenario.next_tx(DEPLOYER);
        {
            let cap = scenario.take_from_sender<AdminCap>();
            let mut flag = scenario.take_shared<PauseFlag>();
            admin::pause(&cap, &mut flag);
            scenario.return_to_sender(cap);
            test_scenario::return_shared(flag);
        };

        // Verify paused — assert_not_paused should abort
        // (tested indirectly via the next test)

        // Unpause
        scenario.next_tx(DEPLOYER);
        {
            let cap = scenario.take_from_sender<AdminCap>();
            let mut flag = scenario.take_shared<PauseFlag>();
            admin::unpause(&cap, &mut flag);
            // Should not abort now
            admin::assert_not_paused(&flag);
            scenario.return_to_sender(cap);
            test_scenario::return_shared(flag);
        };

        scenario.end();
    }

    #[test]
    #[expected_failure(abort_code = arb_move::admin::E_PAUSED)]
    fun test_assert_not_paused_aborts_when_paused() {
        let mut scenario = test_scenario::begin(DEPLOYER);
        {
            admin::init_for_testing(scenario.ctx());
        };

        // Pause the system
        scenario.next_tx(DEPLOYER);
        {
            let cap = scenario.take_from_sender<AdminCap>();
            let mut flag = scenario.take_shared<PauseFlag>();
            admin::pause(&cap, &mut flag);
            scenario.return_to_sender(cap);
            test_scenario::return_shared(flag);
        };

        // This should abort
        scenario.next_tx(DEPLOYER);
        {
            let flag = scenario.take_shared<PauseFlag>();
            admin::assert_not_paused(&flag);
            test_scenario::return_shared(flag);
        };

        scenario.end();
    }
}
