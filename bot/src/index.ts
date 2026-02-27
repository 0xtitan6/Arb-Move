/**
 * ArbMove Bot — Main Entry Point
 *
 * Poll loop: collect prices → detect opportunities → execute arbs.
 *
 * Usage:
 *   cp .env.example .env   # fill in your values
 *   npm install
 *   npm run dev             # runs with tsx in dev mode
 */
import { SuiClient } from "@mysten/sui/client";
import { config } from "./config.js";
import { PriceCollector, type ArbOpportunity } from "./collector.js";
import {
  scanForOpportunities,
  buildCetusToTurbosTx,
  executeTx,
} from "./strategy.js";

// ── Setup ──

const client = new SuiClient({ url: config.rpcUrl });
const collector = new PriceCollector(config.rpcUrl);

// Configure which pools to monitor
const monitoredPools: { dex: "cetus" | "turbos" | "deepbook"; poolId: string }[] = [];

if (config.pools.cetusPoolSuiUsdc) {
  monitoredPools.push({ dex: "cetus", poolId: config.pools.cetusPoolSuiUsdc });
}
if (config.pools.turbosPoolSuiUsdc) {
  monitoredPools.push({ dex: "turbos", poolId: config.pools.turbosPoolSuiUsdc });
}
if (config.pools.deepbookPoolSuiUsdc) {
  monitoredPools.push({ dex: "deepbook", poolId: config.pools.deepbookPoolSuiUsdc });
}

// ── State ──

let isRunning = true;
let cycleCount = 0;
let totalOpportunities = 0;
let totalExecuted = 0;
let totalProfit = 0n;

// ── Graceful shutdown ──

process.on("SIGINT", () => {
  console.log("\n[Bot] Shutting down...");
  isRunning = false;
});

process.on("SIGTERM", () => {
  console.log("\n[Bot] Terminated.");
  isRunning = false;
});

// ── Main loop ──

async function runCycle(): Promise<void> {
  cycleCount++;
  const start = Date.now();

  try {
    // 1. Collect prices
    const quotes = await collector.collectAll(monitoredPools);

    if (quotes.length < 2) {
      // Need at least 2 quotes to compare
      return;
    }

    // 2. Detect opportunities
    // Test with 1 SUI (1e9 MIST)
    const testAmount = 1_000_000_000n;
    const opportunities = scanForOpportunities(
      quotes,
      testAmount,
      config.minProfit
    );

    if (opportunities.length === 0) return;

    totalOpportunities += opportunities.length;
    const best = opportunities[0];

    console.log(
      `[Bot] Cycle ${cycleCount} | Found ${opportunities.length} opportunities | ` +
      `Best: ${best.strategy} profit=~${best.estimatedProfit} MIST`
    );

    // 3. Execute best opportunity
    // TODO: Map opportunity to the correct tx builder based on strategy name.
    // For now, only Cetus→Turbos is wired up as an example.
    if (
      best.strategy === "cetus_to_turbos" &&
      config.pools.turbosVersioned
    ) {
      // Type args would come from pool metadata — hardcoded for SUI/USDC example
      // You must fill in the actual type arguments for your target pair
      console.log("[Bot] Would execute arb_cetus_to_turbos — type args needed");

      // Uncomment when type args are configured:
      // const tx = buildCetusToTurbosTx(best, [SUI_TYPE, USDC_TYPE, FEE_TYPE]);
      // const result = await executeTx(client, tx);
      // if (result.success) {
      //   totalExecuted++;
      //   totalProfit += best.estimatedProfit;
      // }
    }
  } catch (err) {
    console.error(`[Bot] Cycle ${cycleCount} error:`, err);
  }

  const elapsed = Date.now() - start;
  if (elapsed < config.pollIntervalMs) {
    await sleep(config.pollIntervalMs - elapsed);
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

// ── Entry ──

async function main(): Promise<void> {
  console.log("══════════════════════════════════════════════════");
  console.log("  ArbMove Bot v0.1.0");
  console.log("══════════════════════════════════════════════════");
  console.log(`  RPC:          ${config.rpcUrl}`);
  console.log(`  Package:      ${config.packageId}`);
  console.log(`  Dry run:      ${config.dryRun}`);
  console.log(`  Min profit:   ${config.minProfit} MIST`);
  console.log(`  Poll interval:${config.pollIntervalMs}ms`);
  console.log(`  Pools:        ${monitoredPools.length} configured`);
  console.log("══════════════════════════════════════════════════");

  if (monitoredPools.length === 0) {
    console.error("[Bot] No pools configured. Set pool IDs in .env");
    process.exit(1);
  }

  console.log("[Bot] Starting poll loop...\n");

  while (isRunning) {
    await runCycle();
  }

  // Summary
  console.log("\n══════════════════════════════════════════════════");
  console.log("  Session Summary");
  console.log("══════════════════════════════════════════════════");
  console.log(`  Cycles:        ${cycleCount}`);
  console.log(`  Opportunities: ${totalOpportunities}`);
  console.log(`  Executed:      ${totalExecuted}`);
  console.log(`  Total profit:  ${totalProfit} MIST`);
  console.log("══════════════════════════════════════════════════");
}

main().catch((err) => {
  console.error("[Bot] Fatal error:", err);
  process.exit(1);
});
