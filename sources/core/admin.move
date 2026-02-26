module arb_move::admin {

    /// Capability granting permission to execute arbitrage strategies.
    /// Only the holder of this object can call strategy entry functions.
    public struct AdminCap has key {
        id: UID,
    }

    /// Shared object that gates all strategy execution.
    /// When `paused == true`, every strategy entry function will abort.
    public struct PauseFlag has key {
        id: UID,
        paused: bool,
    }

    const E_PAUSED: u64 = 2;

    /// Called once on module publish. Creates AdminCap + PauseFlag (unpaused).
    fun init(ctx: &mut TxContext) {
        transfer::transfer(
            AdminCap { id: object::new(ctx) },
            ctx.sender(),
        );
        transfer::share_object(PauseFlag {
            id: object::new(ctx),
            paused: false,
        });
    }

    /// Abort if the system is paused.
    public(package) fun assert_not_paused(flag: &PauseFlag) {
        assert!(!flag.paused, E_PAUSED);
    }

    /// Admin-only: pause all strategy execution.
    entry fun pause(_admin: &AdminCap, flag: &mut PauseFlag) {
        flag.paused = true;
    }

    /// Admin-only: resume strategy execution.
    entry fun unpause(_admin: &AdminCap, flag: &mut PauseFlag) {
        flag.paused = false;
    }

    #[test_only]
    public fun create_admin_cap_for_testing(ctx: &mut TxContext): AdminCap {
        AdminCap { id: object::new(ctx) }
    }

    #[test_only]
    public fun init_for_testing(ctx: &mut TxContext) {
        init(ctx);
    }

    #[test_only]
    /// Transfer AdminCap to an address. Required because AdminCap lacks `store`,
    /// so only this module can call `transfer::transfer<AdminCap>`.
    public fun transfer_for_testing(cap: AdminCap, to: address) {
        transfer::transfer(cap, to);
    }
}
