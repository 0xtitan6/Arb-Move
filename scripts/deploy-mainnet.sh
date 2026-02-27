#!/usr/bin/env bash
# ───────────────────────────────────────────────────────────────
# deploy-mainnet.sh — Publish ArbMove to Sui mainnet
#
# ⚠️  MAINNET DEPLOYMENT — This costs real SUI.
#     Run deploy-testnet.sh first to verify everything works.
# ───────────────────────────────────────────────────────────────
set -euo pipefail

echo "══════════════════════════════════════════════════"
echo "  ArbMove — MAINNET Deployment"
echo "══════════════════════════════════════════════════"
echo ""
echo "⚠️  WARNING: This deploys to MAINNET using real SUI."
echo "   Make sure you have tested on testnet first!"
echo ""

# ── Prerequisites check ──
command -v sui >/dev/null 2>&1 || { echo "❌ 'sui' CLI not found. Install: https://docs.sui.io/build/install"; exit 1; }
command -v jq >/dev/null 2>&1 || { echo "❌ 'jq' not found. Install: brew install jq / apt install jq"; exit 1; }

# Verify environment
ENV=$(sui client active-env 2>/dev/null || echo "unknown")
echo "Active environment: $ENV"

if [[ "$ENV" != "mainnet" ]]; then
    echo ""
    echo "❌ Active environment is '$ENV', not 'mainnet'."
    echo ""
    echo "To add mainnet environment:"
    echo "  sui client new-env --alias mainnet --rpc https://fullnode.mainnet.sui.io:443"
    echo ""
    echo "To switch:"
    echo "  sui client switch --env mainnet"
    exit 1
fi

ADDR=$(sui client active-address)
echo "Deployer address:  $ADDR"

# Check balance (need at least ~0.5 SUI for publish)
echo ""
echo "Checking SUI balance..."
BALANCE_OUTPUT=$(sui client gas --json 2>/dev/null || echo "[]")
TOTAL_BALANCE=$(echo "$BALANCE_OUTPUT" | jq '[.[].mistBalance] | add // 0' 2>/dev/null || echo "0")
TOTAL_SUI=$(echo "scale=4; $TOTAL_BALANCE / 1000000000" | bc 2>/dev/null || echo "unknown")

echo "Total balance: $TOTAL_SUI SUI ($TOTAL_BALANCE MIST)"

if [[ "$TOTAL_BALANCE" -lt 500000000 ]] 2>/dev/null; then
    echo "❌ Insufficient balance. Need at least 0.5 SUI for deployment."
    echo "   Current: $TOTAL_SUI SUI"
    exit 1
fi

# Double confirmation
echo ""
echo "═════════════════════════════════════════════"
echo "  CONFIRM MAINNET DEPLOYMENT"
echo "  Deployer:  $ADDR"
echo "  Balance:   $TOTAL_SUI SUI"
echo "  Network:   mainnet"
echo "═════════════════════════════════════════════"
echo ""
read -rp "Type 'deploy-mainnet' to confirm: " confirm
if [[ "$confirm" != "deploy-mainnet" ]]; then
    echo "Aborted."
    exit 1
fi

# Build first to catch errors early
echo ""
echo "Building package..."
sui move build

# Run tests before deploying
echo ""
echo "Running tests..."
sui move test
echo "✅ All tests passed"

# Publish with --skip-dependency-verification for mainnet
# (dependency framework objects are on-chain and verified)
echo ""
echo "Publishing to mainnet..."
RESULT=$(sui client publish --gas-budget 500000000 --skip-dependency-verification --json 2>&1)

# Extract package ID
PACKAGE_ID=$(echo "$RESULT" | jq -r '.objectChanges[] | select(.type == "published") | .packageId' 2>/dev/null)

if [[ -z "$PACKAGE_ID" || "$PACKAGE_ID" == "null" ]]; then
    echo "❌ Publish failed. Raw output:"
    echo "$RESULT" | head -50
    exit 1
fi

echo ""
echo "✅ Published successfully!"
echo "   Package ID: $PACKAGE_ID"

# Extract AdminCap, PauseFlag, and UpgradeCap object IDs
ADMIN_CAP=$(echo "$RESULT" | jq -r '.objectChanges[] | select(.objectType | contains("AdminCap")) | .objectId' 2>/dev/null)
PAUSE_FLAG=$(echo "$RESULT" | jq -r '.objectChanges[] | select(.objectType | contains("PauseFlag")) | .objectId' 2>/dev/null)
UPGRADE_CAP=$(echo "$RESULT" | jq -r '.objectChanges[] | select(.objectType | contains("UpgradeCap")) | .objectId' 2>/dev/null)

echo "   AdminCap:   $ADMIN_CAP"
echo "   PauseFlag:  $PAUSE_FLAG"
echo "   UpgradeCap: $UPGRADE_CAP"

# Extract transaction digest
TX_DIGEST=$(echo "$RESULT" | jq -r '.digest' 2>/dev/null)
echo "   Tx Digest:  $TX_DIGEST"

# Extract gas cost
GAS_COST=$(echo "$RESULT" | jq -r '
  .effects.gasUsed |
  ((.computationCost | tonumber) + (.storageCost | tonumber) - (.storageRebate | tonumber))
' 2>/dev/null || echo "unknown")
GAS_SUI=$(echo "scale=4; $GAS_COST / 1000000000" | bc 2>/dev/null || echo "unknown")
echo "   Gas Cost:   $GAS_SUI SUI ($GAS_COST MIST)"

# Save deployment info
DEPLOY_FILE="deployments/mainnet.json"
mkdir -p deployments
cat > "$DEPLOY_FILE" <<DEPLOY
{
  "network": "mainnet",
  "packageId": "$PACKAGE_ID",
  "adminCap": "$ADMIN_CAP",
  "pauseFlag": "$PAUSE_FLAG",
  "upgradeCap": "$UPGRADE_CAP",
  "deployer": "$ADDR",
  "txDigest": "$TX_DIGEST",
  "gasCost": "$GAS_COST",
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
DEPLOY

echo ""
echo "Deployment info saved to $DEPLOY_FILE"

# Show .env values to copy
echo ""
echo "══════════════════════════════════════════════════"
echo "  Copy these to your bot-rs/.env file:"
echo "══════════════════════════════════════════════════"
echo ""
echo "PACKAGE_ID=$PACKAGE_ID"
echo "ADMIN_CAP_ID=$ADMIN_CAP"
echo "PAUSE_FLAG_ID=$PAUSE_FLAG"
echo ""
echo "══════════════════════════════════════════════════"
echo "  ⚠️  UpgradeCap Decision (IMPORTANT):"
echo "══════════════════════════════════════════════════"
echo ""
echo "  UpgradeCap ID: $UPGRADE_CAP"
echo ""
echo "  The UpgradeCap controls package upgrades. Choose ONE:"
echo ""
echo "  Option A — Make IMMUTABLE (recommended for trust):"
echo "    sui client call --package 0x2 --module package \\"
echo "      --function make_immutable \\"
echo "      --args $UPGRADE_CAP \\"
echo "      --gas-budget 10000000"
echo ""
echo "  Option B — Keep for future upgrades:"
echo "    Store in a multisig wallet (e.g., MSafe) or"
echo "    transfer to cold storage. If your hot wallet is"
echo "    compromised, an attacker could upgrade the package."
echo ""
echo "══════════════════════════════════════════════════"
echo "  Next steps:"
echo "  1. Handle UpgradeCap (Option A or B above)"
echo "  2. Copy the .env values above into bot-rs/.env"
echo "  3. Set MONITORED_POOLS with pool IDs"
echo "  4. Set DEEP_FEE_COIN_ID if using DeepBook"
echo "  5. Run dry-run test: ./scripts/dry-run-mainnet.sh"
echo "  6. Start the bot: cd bot-rs && cargo run --release"
echo "  7. Verify on explorer:"
echo "     https://suiscan.xyz/mainnet/object/$PACKAGE_ID"
echo "══════════════════════════════════════════════════"
