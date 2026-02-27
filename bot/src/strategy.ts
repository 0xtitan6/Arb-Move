/**
 * Strategy Engine
 *
 * Detects arbitrage opportunities by comparing quotes across DEXes.
 * When a profitable opportunity is found, builds and submits
 * the corresponding on-chain transaction.
 */
import { SuiClient } from "@mysten/sui/client";
import { Transaction } from "@mysten/sui/transactions";
import { Ed25519Keypair } from "@mysten/sui/keypairs/ed25519";
import { decodeSuiPrivateKey } from "@mysten/sui/cryptography";
import { config } from "./config.js";
import type { PoolQuote, ArbOpportunity } from "./collector.js";

// ── Strategy names matching on-chain module::function ──

type TwoHopStrategy =
  | "arb_cetus_to_turbos"
  | "arb_cetus_to_turbos_reverse"
  | "arb_turbos_to_cetus"
  | "arb_cetus_to_deepbook"
  | "arb_deepbook_to_cetus"
  | "arb_turbos_to_deepbook"
  | "arb_deepbook_to_turbos";

// ── Helpers ──

function getKeypair(): Ed25519Keypair {
  const { secretKey } = decodeSuiPrivateKey(config.privateKey);
  return Ed25519Keypair.fromSecretKey(secretKey);
}

// ── Opportunity Detection ──

/**
 * Compare two CLMM pool quotes for the same pair.
 * If pool A is cheaper than pool B, buying on A and selling on B is profitable.
 */
export function detectTwoHopOpportunity(
  quoteA: PoolQuote,
  quoteB: PoolQuote,
  testAmount: bigint,
  minProfit: bigint
): ArbOpportunity | null {
  if (quoteA.price <= 0 || quoteB.price <= 0) return null;

  // Price divergence check: if buying on A is cheaper than selling on B
  const priceDiff = quoteB.price - quoteA.price;
  if (priceDiff <= 0) return null;

  // Rough profit estimate (ignoring slippage, fees, tick depth)
  const estimatedProfit = BigInt(
    Math.floor(Number(testAmount) * (priceDiff / quoteA.price))
  );

  if (estimatedProfit < minProfit) return null;

  // Determine strategy name
  const strategyName = `${quoteA.dex}_to_${quoteB.dex}`;

  return {
    strategy: strategyName,
    buyDex: quoteA.dex,
    sellDex: quoteB.dex,
    buyPoolId: quoteA.poolId,
    sellPoolId: quoteB.poolId,
    estimatedProfit,
    optimalAmount: testAmount,
    baseType: quoteA.baseType,
    quoteType: quoteA.quoteType,
  };
}

/**
 * Scan all quote pairs for opportunities.
 */
export function scanForOpportunities(
  quotes: PoolQuote[],
  testAmount: bigint,
  minProfit: bigint
): ArbOpportunity[] {
  const opportunities: ArbOpportunity[] = [];

  // Compare every pair of quotes for the same token pair
  for (let i = 0; i < quotes.length; i++) {
    for (let j = 0; j < quotes.length; j++) {
      if (i === j) continue;

      const opp = detectTwoHopOpportunity(
        quotes[i],
        quotes[j],
        testAmount,
        minProfit
      );
      if (opp) opportunities.push(opp);
    }
  }

  // Sort by estimated profit descending
  opportunities.sort((a, b) =>
    Number(b.estimatedProfit - a.estimatedProfit)
  );

  return opportunities;
}

// ── Transaction Building ──

/**
 * Build a two-hop arb transaction for a Cetus→Turbos opportunity.
 */
export function buildCetusToTurbosTx(
  opp: ArbOpportunity,
  typeArgs: [string, string, string] // [A, B, TurbosFee]
): Transaction {
  const tx = new Transaction();

  tx.moveCall({
    target: `${config.packageId}::two_hop::arb_cetus_to_turbos`,
    typeArguments: typeArgs,
    arguments: [
      tx.object(config.adminCapId),
      tx.object(config.pauseFlagId),
      tx.object(config.cetusGlobalConfig),
      tx.object(opp.buyPoolId),
      tx.object(opp.sellPoolId),
      tx.object(config.pools.turbosVersioned),
      tx.pure.u64(opp.optimalAmount),
      tx.pure.u64(config.minProfit),
      tx.object(config.clock),
    ],
  });

  tx.setGasBudget(config.maxGasBudget);
  return tx;
}

/**
 * Build a two-hop arb transaction for a Cetus→DeepBook opportunity.
 * Requires a DEEP fee coin object.
 */
export function buildCetusToDeepBookTx(
  opp: ArbOpportunity,
  typeArgs: [string, string], // [Base, Quote]
  deepCoinId: string
): Transaction {
  const tx = new Transaction();

  tx.moveCall({
    target: `${config.packageId}::two_hop::arb_cetus_to_deepbook`,
    typeArguments: typeArgs,
    arguments: [
      tx.object(config.adminCapId),
      tx.object(config.pauseFlagId),
      tx.object(config.cetusGlobalConfig),
      tx.object(opp.buyPoolId),
      tx.object(opp.sellPoolId),
      tx.object(deepCoinId),
      tx.pure.u64(opp.optimalAmount),
      tx.pure.u64(config.minProfit),
      tx.object(config.clock),
    ],
  });

  tx.setGasBudget(config.maxGasBudget);
  return tx;
}

// ── Execution ──

/**
 * Execute a transaction (dry-run or submit based on config).
 */
export async function executeTx(
  client: SuiClient,
  tx: Transaction
): Promise<{ success: boolean; digest?: string; gasUsed?: string; error?: string }> {
  const keypair = getKeypair();

  if (config.dryRun) {
    console.log("[Strategy] DRY RUN mode — simulating...");
    try {
      tx.setSender(keypair.toSuiAddress());
      const dryResult = await client.dryRunTransactionBlock({
        transactionBlock: await tx.build({ client }),
      });

      const status = dryResult.effects.status.status;
      const gasUsed =
        BigInt(dryResult.effects.gasUsed.computationCost) +
        BigInt(dryResult.effects.gasUsed.storageCost) -
        BigInt(dryResult.effects.gasUsed.storageRebate);

      console.log(`[Strategy] Dry run status: ${status}`);
      console.log(`[Strategy] Net gas: ${gasUsed} MIST`);

      return {
        success: status === "success",
        gasUsed: gasUsed.toString(),
        error: status !== "success" ? dryResult.effects.status.error : undefined,
      };
    } catch (err) {
      return { success: false, error: String(err) };
    }
  }

  // Live execution
  try {
    console.log("[Strategy] LIVE — submitting transaction...");
    const result = await client.signAndExecuteTransaction({
      transaction: tx,
      signer: keypair,
      options: {
        showEffects: true,
        showEvents: true,
      },
    });

    const status = result.effects?.status.status;
    console.log(`[Strategy] TX digest: ${result.digest}`);
    console.log(`[Strategy] Status: ${status}`);

    if (result.events) {
      for (const event of result.events) {
        if (event.type.includes("ArbExecuted")) {
          console.log("[Strategy] ArbExecuted event:", event.parsedJson);
        }
      }
    }

    return {
      success: status === "success",
      digest: result.digest,
      error: status !== "success" ? result.effects?.status.error : undefined,
    };
  } catch (err) {
    console.error("[Strategy] TX failed:", err);
    return { success: false, error: String(err) };
  }
}
