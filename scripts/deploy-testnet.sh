#!/usr/bin/env bash
# ───────────────────────────────────────────────────────────────
# deploy-testnet.sh — Publish ArbMove to Sui testnet
# ───────────────────────────────────────────────────────────────
set -euo pipefail

# ── Prerequisites ──
# 1. `sui` CLI installed (https://docs.sui.io/build/install)
# 2. Active Sui wallet: `sui client active-address`
# 3. Switch to testnet:  `sui client switch --env testnet`
# 4. Fund your address:  `sui client faucet`
#
# To add testnet env if missing:
#   sui client new-env --alias testnet --rpc https://fullnode.testnet.sui.io:443

echo "══════════════════════════════════════════════════"
echo "  ArbMove — Testnet Deployment"
echo "══════════════════════════════════════════════════"

# Verify environment
ENV=$(sui client active-env 2>/dev/null || echo "unknown")
echo "Active environment: $ENV"
if [[ "$ENV" != "testnet" ]]; then
    echo "⚠ WARNING: Active environment is '$ENV', not 'testnet'."
    echo "  Switch with: sui client switch --env testnet"
    read -rp "Continue anyway? [y/N] " confirm
    [[ "$confirm" =~ ^[Yy]$ ]] || exit 1
fi

ADDR=$(sui client active-address)
echo "Deployer address:  $ADDR"

# Check balance
echo ""
echo "Checking balance..."
sui client gas

# Build first to catch errors early
echo ""
echo "Building package..."
sui move build

# Publish
echo ""
echo "Publishing to $ENV..."
RESULT=$(sui client publish --gas-budget 500000000 --json 2>&1)

# Extract package ID
PACKAGE_ID=$(echo "$RESULT" | jq -r '.objectChanges[] | select(.type == "published") | .packageId' 2>/dev/null)

if [[ -z "$PACKAGE_ID" || "$PACKAGE_ID" == "null" ]]; then
    echo "❌ Publish failed. Raw output:"
    echo "$RESULT"
    exit 1
fi

echo ""
echo "✅ Published successfully!"
echo "   Package ID: $PACKAGE_ID"

# Extract AdminCap and PauseFlag object IDs
ADMIN_CAP=$(echo "$RESULT" | jq -r '.objectChanges[] | select(.objectType | contains("AdminCap")) | .objectId' 2>/dev/null)
PAUSE_FLAG=$(echo "$RESULT" | jq -r '.objectChanges[] | select(.objectType | contains("PauseFlag")) | .objectId' 2>/dev/null)

echo "   AdminCap:   $ADMIN_CAP"
echo "   PauseFlag:  $PAUSE_FLAG"

# Save deployment info
DEPLOY_FILE="deployments/testnet.json"
mkdir -p deployments
cat > "$DEPLOY_FILE" <<DEPLOY
{
  "network": "$ENV",
  "packageId": "$PACKAGE_ID",
  "adminCap": "$ADMIN_CAP",
  "pauseFlag": "$PAUSE_FLAG",
  "deployer": "$ADDR",
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
DEPLOY

echo ""
echo "Deployment info saved to $DEPLOY_FILE"
echo ""
echo "══════════════════════════════════════════════════"
echo "  Next steps:"
echo "  1. Note the object IDs above"
echo "  2. Run integration tests: ./scripts/integration-test.sh"
echo "  3. Verify on explorer: https://suiscan.xyz/testnet/object/$PACKAGE_ID"
echo "══════════════════════════════════════════════════"
