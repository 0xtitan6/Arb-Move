# ArbMove

On-chain arbitrage framework for **Sui**, inspired by [Paradigm's Artemis](https://github.com/paradigmxyz/artemis). Executes atomic flash-loan arbitrage across **Cetus CLMM**, **Turbos CLMM**, and **DeepBook V3 CLOB**.

## Architecture

```
┌─────────────────────────────────────────────────┐
│                   Off-chain Bot                 │
│  collector.ts ──► strategy.ts ──► executor      │
│       │               │               │         │
│   poll pools    detect arbs     sign & submit   │
└────────┬──────────────┬───────────────┬─────────┘
         │              │               │
    ┌────▼────┐   ┌─────▼──────┐  ┌────▼─────┐
    │ Sui RPC │   │  Strategy  │  │  Events  │
    │  nodes  │   │  Modules   │  │  Indexer │
    └─────────┘   │ (on-chain) │  └──────────┘
                  └─────┬──────┘
         ┌──────────────┼──────────────┐
    ┌────▼────┐   ┌─────▼────┐  ┌─────▼──────┐
    │  Cetus  │   │  Turbos  │  │  DeepBook  │
    │ Adapter │   │ Adapter  │  │  Adapter   │
    └─────────┘   └──────────┘  └────────────┘
```

## On-chain Modules

| Module | Path | Description |
|---|---|---|
| `admin` | `sources/core/admin.move` | `AdminCap` access control + `PauseFlag` emergency circuit-breaker |
| `profit` | `sources/core/profit.move` | Overflow-safe profit validation + coin utilities |
| `events` | `sources/core/events.move` | `ArbExecuted` event emission for indexing |
| `cetus_adapter` | `sources/adapters/cetus_adapter.move` | Cetus CLMM flash swap wrapper (Balance-level) |
| `turbos_adapter` | `sources/adapters/turbos_adapter.move` | Turbos CLMM swap wrapper (Coin-level) |
| `deepbook_adapter` | `sources/adapters/deepbook_adapter.move` | DeepBook V3 swap + flash loan wrapper |
| `two_hop` | `sources/strategies/two_hop.move` | 7 two-hop DEX-to-DEX arb strategies |
| `tri_hop` | `sources/strategies/tri_hop.move` | 5 triangular (A→B→C→A) arb strategies |

### Strategies

**Two-hop** (12 total entry functions):

| Function | Flash Source | Swap DEX |
|---|---|---|
| `arb_cetus_to_turbos` | Cetus (A→B) | Turbos (B→A) |
| `arb_cetus_to_turbos_reverse` | Cetus (B→A) | Turbos (A→B) |
| `arb_turbos_to_cetus` | Turbos (A→B) | Cetus (B→A) |
| `arb_cetus_to_deepbook` | Cetus (A→B) | DeepBook (B→A) |
| `arb_deepbook_to_cetus` | DeepBook flash | Cetus + DeepBook |
| `arb_turbos_to_deepbook` | Turbos (A→B) | DeepBook (B→A) |
| `arb_deepbook_to_turbos` | DeepBook flash | Turbos + DeepBook |

**Tri-hop** (5 triangular):

| Function | Leg 1 | Leg 2 | Leg 3 |
|---|---|---|---|
| `tri_cetus_cetus_cetus` | Cetus A→B | Cetus B→C | Cetus C→A |
| `tri_cetus_cetus_turbos` | Cetus A→B | Cetus B→C | Turbos C→A |
| `tri_cetus_turbos_deepbook` | Cetus A→B | Turbos B→C | DeepBook C→A |
| `tri_cetus_deepbook_turbos` | Cetus A→B | DeepBook B→C | Turbos C→A |
| `tri_deepbook_cetus_turbos` | DeepBook flash A | Cetus A→B | Turbos B→C→A |

## Security

- **AdminCap** — Only the cap holder can execute strategies. `key`-only (no `store`) prevents wrapping into shared objects.
- **PauseFlag** — Shared object that gates all strategy execution. Admin can `pause()` / `unpause()` instantly.
- **Overflow-safe profit check** — Uses checked subtraction to prevent `amount_in + min_profit` overflow.
- **Zero-amount guard** — All entry functions reject `amount == 0`.
- **Package-scoped visibility** — All adapter and utility functions are `public(package)`, preventing third-party misuse.
- **63 unit tests** covering profit math, admin controls, pause mechanism, coin utilities, and edge cases.

### Known Limitations

- **H-2**: Turbos `FlashSwapReceipt` has no public `pay_amount` reader. Repayment uses `amount` directly — if Turbos introduces flash fees, the tx will abort.
- **M-2**: DeepBook `arb_deepbook_to_*` strategies borrow and swap against the same pool. Vault reserve reduction may affect pricing.

## Quick Start

### Prerequisites

- [Sui CLI](https://docs.sui.io/build/install) installed
- Sui wallet with gas: `sui client faucet` (testnet)

### Build & Test

```bash
sui move build
sui move test
```

### Deploy to Testnet

```bash
sui client switch --env testnet
./scripts/deploy-testnet.sh
```

### Run the Bot

```bash
cd bot
cp .env.example .env   # fill in your values
npm install
npm run dev
```

See [`docs/gas-economics.md`](docs/gas-economics.md) for `min_profit` guidance.

## Project Structure

```
.
├── sources/
│   ├── core/
│   │   ├── admin.move          # AdminCap + PauseFlag
│   │   ├── events.move         # ArbExecuted event
│   │   └── profit.move         # Profit validation + coin utils
│   ├── adapters/
│   │   ├── cetus_adapter.move  # Cetus CLMM wrapper
│   │   ├── turbos_adapter.move # Turbos CLMM wrapper
│   │   └── deepbook_adapter.move # DeepBook V3 wrapper
│   └── strategies/
│       ├── two_hop.move        # 7 two-hop arb strategies
│       └── tri_hop.move        # 5 tri-hop arb strategies
├── tests/
│   ├── admin_tests.move        # Admin + pause tests
│   └── profit_tests.move       # Profit + coin utility tests
├── bot/
│   └── src/
│       ├── index.ts            # Bot main loop
│       ├── config.ts           # Environment config
│       ├── collector.ts        # Price collector
│       └── strategy.ts         # Opportunity detection + tx building
├── scripts/
│   ├── deploy-testnet.sh       # Testnet deployment
│   ├── integration-test.sh     # Post-deploy integration tests
│   └── dry-run-mainnet.sh      # Mainnet dry-run guide
├── docs/
│   └── gas-economics.md        # Gas costs + min_profit guidance
├── Move.toml
└── Move.lock
```

## Dependencies

All pinned to specific commit hashes for reproducible builds:

| Dependency | Source | Commit |
|---|---|---|
| Sui Framework | MystenLabs/sui | `mainnet-v1.54.2` |
| Cetus CLMM | CetusProtocol/cetus-clmm-interface | `1f6a1cc` |
| Turbos CLMM | turbos-finance/turbos-sui-move-interface | `cff6932` |
| DeepBook V3 | MystenLabs/deepbookv3 | `26281b9` |

## License

MIT
