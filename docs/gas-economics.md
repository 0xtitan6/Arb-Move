# Gas Economics & min_profit Guidance

## Sui Gas Model

Sui charges gas in **MIST** (1 SUI = 1,000,000,000 MIST / 1e9 MIST).

| Parameter | Value |
|---|---|
| Minimum gas budget | 2,000 MIST |
| Maximum gas budget | 50,000,000,000 MIST (50 SUI) |
| Reference gas price | ~750–1,000 MIST/unit (set per epoch by validators) |
| Average tx fee (30d) | ~2,800,000 MIST (~0.003 SUI) |

Transaction cost = `computation_cost + storage_cost - storage_rebate`

## Estimated Costs by Strategy

These estimates assume typical pool state. Actual costs depend on tick traversal depth.

| Strategy | Estimated Gas (MIST) | Notes |
|---|---|---|
| Two-hop (Cetus→Turbos) | 3,000,000 – 8,000,000 | 2 flash swaps, 1 repay |
| Two-hop (Cetus→DeepBook) | 4,000,000 – 10,000,000 | Flash swap + CLOB matching |
| Two-hop (DeepBook flash→Cetus) | 5,000,000 – 12,000,000 | Flash loan + 2 swaps |
| Tri-hop (Cetus×3) | 5,000,000 – 15,000,000 | 3 CLMM pool interactions |
| Tri-hop (Cetus→Turbos→DeepBook) | 6,000,000 – 18,000,000 | Mixed DEX, highest variance |

## Setting `min_profit`

`min_profit` should cover gas cost + desired margin. Calculate in the **arb token denomination**.

```
min_profit >= gas_cost_in_token + margin
```

### Example: SUI-denominated arb

- Gas budget: 10,000,000 MIST = 0.01 SUI
- Safety margin (2x gas): 0.02 SUI = 20,000,000 MIST
- **Recommended `min_profit`: 20,000,000** (for SUI-denominated arbs)

### Example: USDC-denominated arb (6 decimals)

- Gas cost: ~0.01 SUI ≈ $0.03 at $3/SUI
- In USDC units: 30,000 (0.03 USDC)
- Safety margin (3x): 90,000
- **Recommended `min_profit`: 100,000** (0.10 USDC)

## Profitability Thresholds

For the bot to be profitable after gas:

| Token | Decimals | Min Profitable Amount In | Min Profit Setting |
|---|---|---|---|
| SUI | 9 | 1,000,000,000 (1 SUI) | 20,000,000 (0.02 SUI) |
| USDC | 6 | 1,000,000 (1 USDC) | 100,000 (0.10 USDC) |
| USDT | 6 | 1,000,000 (1 USDT) | 100,000 (0.10 USDT) |
| WETH | 8 | 10,000,000 (0.1 WETH) | 50,000 (0.0005 WETH) |

## Validation Procedure

1. **Publish to testnet** and run real swaps against testnet pools
2. **Dry-run against mainnet** with `sui client call --dry-run`:
   - Record `gasUsed.computationCost` and `gasUsed.storageCost`
   - Record `gasUsed.storageRebate`
   - Net gas = computation + storage - rebate
3. **Set min_profit = 2× net gas** (in arb token denomination) as starting floor
4. **Monitor and adjust** — gas fluctuates with network load and validator epoch pricing

## Important Notes

- Gas is always paid in SUI regardless of the arb token
- Cross-token gas conversion requires a price oracle (the bot handles this)
- DEEP token fees for DeepBook are separate from SUI gas
- Flash loan fees (if any DEX adds them) are on top of gas
- Storage rebates can reduce effective cost by 50-99% for transactions that delete objects
