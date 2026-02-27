# ArbMove

Atomic flash-swap arbitrage on **Sui**. Borrows tokens from one DEX, sells on another, repays the loan, and pockets the difference -- all in a single transaction. If the trade isn't profitable, the transaction reverts and you only lose gas.

Built with **Move** smart contracts + a **Rust** off-chain bot.

## How It Works

```
                        ┌──────────────────────────────────────────────────┐
                        │              Rust Bot (off-chain)                │
                        │                                                  │
                        │   Collector ──► Scanner ──► Optimizer ──► PTB    │
                        │       │            │           │           │     │
                        │   poll/stream   detect     ternary     build &  │
                        │   pool state   spreads     search      sign tx  │
                        └───────┬────────────┬───────────┬──────────┬─────┘
                                │            │           │          │
┌───────────────────────────────▼────────────▼───────────▼──────────▼──────┐
│                         Sui Network (on-chain)                           │
│                                                                          │
│  ┌─────────┐   flash    ┌──────────┐   swap    ┌──────────┐   repay    │
│  │  DEX A  │ ─────────► │ ArbMove  │ ────────► │  DEX B   │ ────────► │
│  │ (Cetus) │  borrow    │ Contract │  sell     │ (Turbos) │  loan +   │
│  └─────────┘            └────┬─────┘           └──────────┘  profit   │
│                              │                                          │
│                     profit ──► sender wallet                            │
└─────────────────────────────────────────────────────────────────────────┘
```

**Key property:** Flash swaps are atomic. If any step fails (bad price, insufficient liquidity, unprofitable), the entire transaction reverts. You never lose principal -- only gas on failed attempts.

## DEX Coverage

| DEX | Type | Role | Capabilities |
|-----|------|------|-------------|
| **Cetus** | CLMM | Flash source + swap | Flash swap, a2b, b2a |
| **Turbos** | CLMM | Flash source + swap | Flash swap, a2b, b2a |
| **DeepBook V3** | CLOB | Flash source + swap | Flash loan, place market orders |
| **Aftermath** | AMM | Swap only | Sell leg (no flash loans) |
| **FlowX** | CLMM | Flash source + swap | Flash swap, a2b, b2a |

## Strategies

**27 on-chain entry functions** across two strategy types:

### Two-Hop (17 functions)
Flash borrow from DEX A, sell on DEX B, repay loan, keep profit.

| Flash Source | Sell DEX | Functions |
|-------------|----------|-----------|
| Cetus | Turbos | `arb_cetus_to_turbos`, `arb_cetus_to_turbos_reverse` |
| Turbos | Cetus | `arb_turbos_to_cetus` |
| Cetus | DeepBook | `arb_cetus_to_deepbook` |
| DeepBook | Cetus | `arb_deepbook_to_cetus` |
| Turbos | DeepBook | `arb_turbos_to_deepbook` |
| DeepBook | Turbos | `arb_deepbook_to_turbos` |
| Cetus | Aftermath | `arb_cetus_to_aftermath`, `arb_cetus_to_aftermath_rev` |
| Turbos | Aftermath | `arb_turbos_to_aftermath` |
| DeepBook | Aftermath | `arb_deepbook_to_aftermath` |
| Cetus | FlowX CLMM | `arb_cetus_to_flowx_clmm` |
| FlowX CLMM | Cetus | `arb_flowx_clmm_to_cetus` |
| Turbos | FlowX CLMM | `arb_turbos_to_flowx_clmm` |
| FlowX CLMM | Turbos | `arb_flowx_clmm_to_turbos` |
| DeepBook | FlowX CLMM | `arb_deepbook_to_flowx_clmm` |
| FlowX CLMM | DeepBook | `arb_flowx_clmm_to_deepbook` |

### Tri-Hop (10 functions)
Triangular arbitrage: A -> B -> C -> A across three pools.

| Leg 1 | Leg 2 | Leg 3 | Function |
|-------|-------|-------|----------|
| Cetus | Cetus | Cetus | `tri_cetus_cetus_cetus` |
| Cetus | Cetus | Turbos | `tri_cetus_cetus_turbos` |
| Cetus | Turbos | DeepBook | `tri_cetus_turbos_deepbook` |
| Cetus | DeepBook | Turbos | `tri_cetus_deepbook_turbos` |
| DeepBook | Cetus | Turbos | `tri_deepbook_cetus_turbos` |
| Cetus | Cetus | Aftermath | `tri_cetus_cetus_aftermath` |
| Cetus | Turbos | Aftermath | `tri_cetus_turbos_aftermath` |
| Cetus | Cetus | FlowX CLMM | `tri_cetus_cetus_flowx_clmm` |
| Cetus | FlowX CLMM | Turbos | `tri_cetus_flowx_clmm_turbos` |
| FlowX CLMM | Cetus | Turbos | `tri_flowx_clmm_cetus_turbos` |

## Architecture

### On-Chain (Move)

```
sources/
  core/
    admin.move           AdminCap + PauseFlag emergency circuit-breaker
    events.move          ArbExecuted event for indexing and P&L tracking
    profit.move          Overflow-safe profit validation + coin utilities
  adapters/
    cetus_adapter.move   Cetus CLMM flash swap (Balance-level API)
    turbos_adapter.move  Turbos CLMM swap (Coin-level API)
    deepbook_adapter.move DeepBook V3 flash loan + market orders
    aftermath_adapter.move Aftermath AMM swap wrapper
    flowx_clmm_adapter.move FlowX CLMM flash swap wrapper
  strategies/
    two_hop.move         17 two-hop arb entry functions
    tri_hop.move         10 tri-hop arb entry functions
```

### Off-Chain (Rust)

```
bot-rs/
  src/main.rs                   Bot orchestrator + strategy loop
  crates/
    types/                      Config, PoolState, ArbOpportunity, StrategyType
    collector/                  Pool data collection (RPC polling + WebSocket streaming)
      parsers/                  DEX-specific JSON parsers (Cetus, Turbos, DeepBook, FlowX, Aftermath)
    strategy/
      scanner.rs                O(n^2) pairwise spread detection across monitored pools
      optimizer.rs              Ternary search for optimal trade size + CLMM/AMM simulation
      dry_runner.rs             RPC dry-run validation before submission
    executor/
      ptb_builder.rs            Programmable Transaction Block construction
      signer.rs                 Ed25519 transaction signing
      submitter.rs              Transaction submission with retry + duplicate detection
```

### Bot Pipeline

Each cycle (default 500ms):

1. **Collect** -- Poll or stream pool state updates from all monitored DEXes
2. **Scan** -- Pairwise comparison of pool prices; detect spreads > 0.1%
3. **Optimize** -- Ternary search finds optimal input amount (maximizes concave profit curve)
4. **Simulate** -- Local CLMM/AMM math estimates profit with price impact
5. **Build** -- Construct Programmable Transaction Block with min_profit guard
6. **Dry-run** -- RPC simulation validates profitability with current on-chain state
7. **Rebuild** -- Re-build PTB with tighter profit bounds from dry-run results
8. **Sign & Submit** -- Ed25519 signature, submit to network

## Safety & Security

### On-Chain Guarantees
- **Atomic execution** -- flash swap + repay in a single transaction. If unprofitable, everything reverts.
- **`profit::assert_profit`** -- overflow-safe check ensures `amount_out > amount_in + min_profit` before completing.
- **`AdminCap` gating** -- only the cap holder can execute strategies. `key`-only (no `store`) prevents wrapping.
- **`PauseFlag`** -- shared object kill switch. Admin can `pause()` / `unpause()` instantly to halt all strategies.
- **Zero-amount guard** -- all entry functions reject `amount == 0`.
- **`public(package)` visibility** -- adapter and utility functions are not callable by third-party packages.

### Off-Chain Safeguards
- **Supervised collectors** -- all collector tasks auto-restart on failure with heartbeat tracking.
- **Staleness guards** -- strategy loop skips cycles when pool data is >10s old or all collectors are dead.
- **Dry-run validation** -- every trade is simulated via RPC before signing (configurable).
- **Duplicate tx detection** -- submitter catches "already executed" errors to avoid wasted retries.
- **90% min_profit guard** -- PTB sets on-chain min_profit to 90% of expected, rejecting excessive slippage.
- **Max trade cap** -- optimizer caps any single trade at 100 SUI.

### Test Coverage
- **65 Move unit tests** -- profit math, admin controls, pause mechanism, coin utilities, adapter edge cases.
- **10 Rust tests** -- scanner pairing, strategy resolution, optimizer simulation, parser robustness.

## Quick Start

### Prerequisites

- [Sui CLI](https://docs.sui.io/build/install) (v1.54+)
- [Rust](https://rustup.rs/) (1.75+)
- Sui wallet with SUI for gas
- DEEP tokens for DeepBook strategies (optional)

### 1. Build & Test

```bash
# Move contracts
sui move build
sui move test

# Rust bot
cd bot-rs
cargo build --release
cargo test --workspace
```

### 2. Deploy Contracts

```bash
# Testnet
sui client switch --env testnet
./scripts/deploy-testnet.sh

# Mainnet (when ready)
sui client switch --env mainnet
sui client publish --gas-budget 500000000
```

Save the output `PACKAGE_ID`, `AdminCap` object ID, and `PauseFlag` object ID.

### 3. Configure the Bot

```bash
cd bot-rs
cp .env.example .env
```

Fill in:
- `SUI_PRIVATE_KEY` -- your Ed25519 private key (hex, 32 bytes)
- `PACKAGE_ID` -- deployed ArbMove package address
- `ADMIN_CAP_ID` -- AdminCap object ID from deployment
- `PAUSE_FLAG_ID` -- PauseFlag object ID from deployment
- `MONITORED_POOLS` -- pool IDs to monitor (see `.env.example` for format)
- `DEEP_FEE_COIN_ID` -- owned `Coin<DEEP>` object (for DeepBook strategies)

### 4. Run

```bash
# Dry-run mode (recommended for initial testing)
DRY_RUN_BEFORE_SUBMIT=true cargo run --release

# With WebSocket streaming (lower latency)
USE_WEBSOCKET=true WS_MODE=event cargo run --release

# Adjust logging
RUST_LOG=info,arb_strategy=debug cargo run --release
```

### 5. Emergency Stop

On-chain kill switch (works even if bot process is dead):
```bash
sui client call \
  --package $PACKAGE_ID \
  --module admin \
  --function pause \
  --args $ADMIN_CAP_ID $PAUSE_FLAG_ID \
  --gas-budget 10000000
```

## Configuration Reference

| Variable | Default | Description |
|----------|---------|-------------|
| `SUI_RPC_URL` | `https://fullnode.mainnet.sui.io:443` | Sui JSON-RPC endpoint |
| `MIN_PROFIT_MIST` | `1000000` (0.001 SUI) | Minimum profit threshold in MIST |
| `POLL_INTERVAL_MS` | `500` | Strategy loop tick interval |
| `MAX_GAS_BUDGET` | `50000000` (0.05 SUI) | Max gas per transaction |
| `DRY_RUN_BEFORE_SUBMIT` | `true` | Simulate before submitting |
| `USE_WEBSOCKET` | `false` | Enable WebSocket streaming |
| `WS_MODE` | `event` | WebSocket mode: `event` or `tx` |

See [`docs/gas-economics.md`](docs/gas-economics.md) for `min_profit` tuning guidance.

## Dependencies

All pinned to specific commit hashes for reproducible builds:

| Dependency | Source | Version/Commit |
|---|---|---|
| Sui Framework | MystenLabs/sui | `mainnet-v1.54.2` |
| Cetus CLMM | CetusProtocol/cetus-clmm-interface | `1f6a1cc` |
| Turbos CLMM | turbos-finance/turbos-sui-move-interface | `cff6932` |
| DeepBook V3 | MystenLabs/deepbookv3 | `26281b9` |
| Aftermath AMM | CetusProtocol/aggregator | `3ecb775` |
| FlowX CLMM | CetusProtocol/aggregator | `3ecb775` |

## Known Limitations

- **Turbos/FlowX flash fee risk** -- `FlashSwapReceipt` has no public `pay_amount` reader. If these DEXes add flash fees, repayment will be short and the tx will abort (safe but blocks that strategy).
- **DeepBook self-swap** -- `arb_deepbook_to_*` strategies borrow and swap against the same pool. Vault reserve reduction may affect pricing.
- **Single-tick CLMM model** -- optimizer assumes trades stay within one tick range. Large trades crossing multiple ticks will have slightly less profit than simulated.
- **No tri-hop scanning** -- tri-hop strategies are implemented on-chain but the scanner currently only detects two-hop opportunities.
- **FlowX AMM** -- referenced in Rust types but no on-chain implementation (FlowX AMM router is not exposed in interface stubs).

## License

MIT
