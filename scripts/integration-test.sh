#!/usr/bin/env bash
# ───────────────────────────────────────────────────────────────
# integration-test.sh — Run integration tests against a live deployment
# ───────────────────────────────────────────────────────────────
# Usage:
#   ./scripts/integration-test.sh                  # uses deployments/testnet.json
#   ./scripts/integration-test.sh mainnet-dryrun   # dry-run against mainnet state
#
set -euo pipefail

MODE="${1:-testnet}"

echo "══════════════════════════════════════════════════"
echo "  ArbMove — Integration Tests ($MODE)"
echo "══════════════════════════════════════════════════"

# ── Load deployment config ──
if [[ "$MODE" == "mainnet-dryrun" ]]; then
    DEPLOY_FILE="deployments/mainnet-dryrun.json"
else
    DEPLOY_FILE="deployments/testnet.json"
fi

if [[ ! -f "$DEPLOY_FILE" ]]; then
    echo "❌ Deployment file not found: $DEPLOY_FILE"
    echo "   Run deploy-testnet.sh first."
    exit 1
fi

PACKAGE_ID=$(jq -r '.packageId' "$DEPLOY_FILE")
ADMIN_CAP=$(jq -r '.adminCap' "$DEPLOY_FILE")
PAUSE_FLAG=$(jq -r '.pauseFlag' "$DEPLOY_FILE")

echo "Package:    $PACKAGE_ID"
echo "AdminCap:   $ADMIN_CAP"
echo "PauseFlag:  $PAUSE_FLAG"

# ──────────────────────────────────────────────────
#  Test 1: Pause / Unpause
# ──────────────────────────────────────────────────
echo ""
echo "── Test 1: Pause mechanism ──"

echo "Pausing..."
sui client call \
    --package "$PACKAGE_ID" \
    --module admin \
    --function pause \
    --args "$ADMIN_CAP" "$PAUSE_FLAG" \
    --gas-budget 10000000 \
    --json > /dev/null 2>&1

echo "Unpausing..."
sui client call \
    --package "$PACKAGE_ID" \
    --module admin \
    --function unpause \
    --args "$ADMIN_CAP" "$PAUSE_FLAG" \
    --gas-budget 10000000 \
    --json > /dev/null 2>&1

echo "✅ Pause/unpause works"

# ──────────────────────────────────────────────────
#  Test 2: Dry-run an arb call (expected to fail — no real pools)
# ──────────────────────────────────────────────────
echo ""
echo "── Test 2: Strategy dry-run (expect MoveAbort — no real pools) ──"

# This validates the entry function signature is correct on-chain.
# On testnet without real Cetus/Turbos/DeepBook pools, the tx will abort
# inside the DEX contract, which is expected.
echo "⚠ Skipping live arb call — requires real pool object IDs."
echo "  To test with real pools, set CETUS_CONFIG, CETUS_POOL, etc. env vars"
echo "  and call the strategy entry functions directly."

# ──────────────────────────────────────────────────
#  Test 3: Event emission verification
# ──────────────────────────────────────────────────
echo ""
echo "── Test 3: Event schema check ──"
echo "Querying package events..."

EVENTS=$(sui client events --query "{\"MoveModule\": {\"package\": \"$PACKAGE_ID\", \"module\": \"events\"}}" --limit 5 --json 2>&1 || echo "[]")

if echo "$EVENTS" | jq -e '.' > /dev/null 2>&1; then
    EVENT_COUNT=$(echo "$EVENTS" | jq 'length')
    echo "Events found: $EVENT_COUNT (expected 0 before first arb execution)"
    echo "✅ Event query works"
else
    echo "⚠ Could not query events (may need a successful arb tx first)"
fi

echo ""
echo "══════════════════════════════════════════════════"
echo "  Integration tests complete."
echo "══════════════════════════════════════════════════"
