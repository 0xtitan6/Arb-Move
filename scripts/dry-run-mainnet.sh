#!/usr/bin/env bash
# ───────────────────────────────────────────────────────────────
# dry-run-mainnet.sh — Dry-run arb strategies against mainnet state
# ───────────────────────────────────────────────────────────────
# This does NOT submit transactions. It simulates execution against
# real mainnet pool objects to validate gas costs and profitability.
#
# Prerequisites:
#   1. Package published to mainnet (or use --dev-inspect)
#   2. Real pool object IDs set in env or deployments/mainnet-pools.json
# ───────────────────────────────────────────────────────────────
set -euo pipefail

echo "══════════════════════════════════════════════════"
echo "  ArbMove — Mainnet Dry Run"
echo "══════════════════════════════════════════════════"

# Switch to mainnet for inspection
echo "Switching to mainnet..."
sui client switch --env mainnet 2>/dev/null || {
    echo "Adding mainnet env..."
    sui client new-env --alias mainnet --rpc https://fullnode.mainnet.sui.io:443
    sui client switch --env mainnet
}

# ── Well-known mainnet object IDs ──
# Cetus
CETUS_GLOBAL_CONFIG="0xdaa46292632c3c4d8f31f23ea0f9b36a28ff3677e9684980e4438403a67a3d8f"
# Clock (system object)
SUI_CLOCK="0x0000000000000000000000000000000000000000000000000000000000000006"

echo ""
echo "Inspecting Cetus GlobalConfig..."
sui client object "$CETUS_GLOBAL_CONFIG" --json 2>/dev/null | jq '{objectId: .data.objectId, type: .data.type}' || echo "⚠ Could not fetch GlobalConfig"

echo ""
echo "── Finding pools ──"
echo "To dry-run a specific arb, you need pool object IDs."
echo ""
echo "Common Cetus pools (SUI/USDC, SUI/USDT, etc.):"
echo "  Query: sui client dynamic-field <CETUS_POOL_REGISTRY_ID>"
echo ""
echo "Common DeepBook V3 pools:"
echo "  Query via DeepBook SDK or explorer"
echo ""
echo "── Dry-run example ──"
echo ""
echo "Once you have pool IDs, dry-run with:"
echo ""
echo "  sui client call \\"
echo "    --package <PACKAGE_ID> \\"
echo "    --module two_hop \\"
echo "    --function arb_cetus_to_deepbook \\"
echo "    --type-args 0x2::sui::SUI <USDC_TYPE> \\"
echo "    --args <ADMIN_CAP> <PAUSE_FLAG> <CETUS_CONFIG> <CETUS_POOL> <DEEPBOOK_POOL> <DEEP_COIN> 1000000000 100000 $SUI_CLOCK \\"
echo "    --gas-budget 50000000 \\"
echo "    --dry-run"
echo ""
echo "The --dry-run flag simulates against current mainnet state without submitting."
echo "Check the output for:"
echo "  • gasUsed.computationCost  — execution gas"
echo "  • gasUsed.storageCost      — storage gas"
echo "  • status.status            — 'success' or 'failure'"
echo "  • events[]                 — ArbExecuted event with profit"
echo ""
echo "══════════════════════════════════════════════════"

# Switch back to previous env
echo "Switching back to testnet..."
sui client switch --env testnet 2>/dev/null || true
