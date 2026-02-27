/**
 * Price Collector
 *
 * Polls pool state from Cetus, Turbos, and DeepBook to compute
 * real-time price quotes. Feeds data to the strategy engine.
 */
import { SuiClient } from "@mysten/sui/client";
import { config } from "./config.js";

// ── Types ──

export interface PoolQuote {
  dex: "cetus" | "turbos" | "deepbook";
  poolId: string;
  baseType: string;
  quoteType: string;
  /** Price of 1 base unit in quote units (decimal-adjusted) */
  price: number;
  /** sqrt_price from the pool (for CLMM pools) */
  sqrtPrice: bigint;
  /** Available liquidity in the current tick range */
  liquidity: bigint;
  /** Timestamp of the quote */
  timestamp: number;
}

export interface ArbOpportunity {
  strategy: string;
  buyDex: string;
  sellDex: string;
  buyPoolId: string;
  sellPoolId: string;
  estimatedProfit: bigint;
  optimalAmount: bigint;
  baseType: string;
  quoteType: string;
}

// ── Collector ──

export class PriceCollector {
  private client: SuiClient;

  constructor(rpcUrl: string = config.rpcUrl) {
    this.client = new SuiClient({ url: rpcUrl });
  }

  /**
   * Fetch Cetus CLMM pool state and extract sqrt_price / liquidity.
   */
  async getCetusPoolState(poolId: string): Promise<PoolQuote | null> {
    try {
      const obj = await this.client.getObject({
        id: poolId,
        options: { showContent: true },
      });

      if (obj.data?.content?.dataType !== "moveObject") return null;

      const fields = obj.data.content.fields as Record<string, unknown>;
      const sqrtPrice = BigInt((fields as any).current_sqrt_price ?? "0");
      const liquidity = BigInt((fields as any).liquidity ?? "0");

      // price = (sqrtPrice / 2^64)^2, adjusted for decimals
      const price = Number(sqrtPrice * sqrtPrice) / 2 ** 128;

      return {
        dex: "cetus",
        poolId,
        baseType: "", // extracted from pool type params
        quoteType: "",
        price,
        sqrtPrice,
        liquidity,
        timestamp: Date.now(),
      };
    } catch (err) {
      console.error(`[Collector] Failed to fetch Cetus pool ${poolId}:`, err);
      return null;
    }
  }

  /**
   * Fetch Turbos CLMM pool state.
   */
  async getTurbosPoolState(poolId: string): Promise<PoolQuote | null> {
    try {
      const obj = await this.client.getObject({
        id: poolId,
        options: { showContent: true },
      });

      if (obj.data?.content?.dataType !== "moveObject") return null;

      const fields = obj.data.content.fields as Record<string, unknown>;
      const sqrtPrice = BigInt((fields as any).sqrt_price ?? "0");
      const liquidity = BigInt((fields as any).liquidity ?? "0");

      const price = Number(sqrtPrice * sqrtPrice) / 2 ** 128;

      return {
        dex: "turbos",
        poolId,
        baseType: "",
        quoteType: "",
        price,
        sqrtPrice,
        liquidity,
        timestamp: Date.now(),
      };
    } catch (err) {
      console.error(`[Collector] Failed to fetch Turbos pool ${poolId}:`, err);
      return null;
    }
  }

  /**
   * Fetch DeepBook V3 CLOB best bid/ask from the order book.
   */
  async getDeepBookPoolState(poolId: string): Promise<PoolQuote | null> {
    try {
      const obj = await this.client.getObject({
        id: poolId,
        options: { showContent: true },
      });

      if (obj.data?.content?.dataType !== "moveObject") return null;

      const fields = obj.data.content.fields as Record<string, unknown>;

      // DeepBook uses ticks; price is derived from order book state
      // For now, return a placeholder — real implementation queries
      // the bid/ask tree via dynamic fields
      return {
        dex: "deepbook",
        poolId,
        baseType: "",
        quoteType: "",
        price: 0,
        sqrtPrice: 0n,
        liquidity: 0n,
        timestamp: Date.now(),
      };
    } catch (err) {
      console.error(`[Collector] Failed to fetch DeepBook pool ${poolId}:`, err);
      return null;
    }
  }

  /**
   * Collect quotes from all configured pools.
   */
  async collectAll(
    poolIds: { dex: "cetus" | "turbos" | "deepbook"; poolId: string }[]
  ): Promise<PoolQuote[]> {
    const promises = poolIds.map(({ dex, poolId }) => {
      switch (dex) {
        case "cetus":
          return this.getCetusPoolState(poolId);
        case "turbos":
          return this.getTurbosPoolState(poolId);
        case "deepbook":
          return this.getDeepBookPoolState(poolId);
      }
    });

    const results = await Promise.allSettled(promises);
    return results
      .filter(
        (r): r is PromiseFulfilledResult<PoolQuote | null> =>
          r.status === "fulfilled"
      )
      .map((r) => r.value)
      .filter((q): q is PoolQuote => q !== null);
  }
}

// ── Standalone runner ──

if (process.argv[1]?.endsWith("collector.ts") || process.argv[1]?.endsWith("collector.js")) {
  const collector = new PriceCollector();

  const pools: { dex: "cetus" | "turbos" | "deepbook"; poolId: string }[] = [];

  if (config.pools.cetusPoolSuiUsdc)
    pools.push({ dex: "cetus", poolId: config.pools.cetusPoolSuiUsdc });
  if (config.pools.turbosPoolSuiUsdc)
    pools.push({ dex: "turbos", poolId: config.pools.turbosPoolSuiUsdc });
  if (config.pools.deepbookPoolSuiUsdc)
    pools.push({ dex: "deepbook", poolId: config.pools.deepbookPoolSuiUsdc });

  if (pools.length === 0) {
    console.log("No pools configured. Set pool IDs in .env");
    process.exit(1);
  }

  console.log(`Collecting prices from ${pools.length} pools...`);
  const quotes = await collector.collectAll(pools);

  for (const q of quotes) {
    console.log(
      `[${q.dex}] pool=${q.poolId.slice(0, 10)}... price=${q.price.toFixed(8)} sqrtPrice=${q.sqrtPrice} liquidity=${q.liquidity}`
    );
  }
}
