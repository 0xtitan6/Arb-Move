import "dotenv/config";

function required(key: string): string {
  const val = process.env[key];
  if (!val) throw new Error(`Missing required env var: ${key}`);
  return val;
}

function optional(key: string, fallback: string): string {
  return process.env[key] ?? fallback;
}

export const config = {
  // Network
  rpcUrl: optional("SUI_RPC_URL", "https://fullnode.mainnet.sui.io:443"),
  wsUrl: optional("SUI_WS_URL", "wss://fullnode.mainnet.sui.io"),

  // Wallet
  privateKey: required("SUI_PRIVATE_KEY"),

  // Package
  packageId: required("PACKAGE_ID"),
  adminCapId: required("ADMIN_CAP_ID"),
  pauseFlagId: required("PAUSE_FLAG_ID"),

  // Well-known objects
  clock: optional(
    "SUI_CLOCK",
    "0x0000000000000000000000000000000000000000000000000000000000000006"
  ),
  cetusGlobalConfig: optional(
    "CETUS_GLOBAL_CONFIG",
    "0xdaa46292632c3c4d8f31f23ea0f9b36a28ff3677e9684980e4438403a67a3d8f"
  ),

  // Pools (optional — populated per pair)
  pools: {
    cetusPoolSuiUsdc: process.env.CETUS_POOL_SUI_USDC ?? "",
    turbosPoolSuiUsdc: process.env.TURBOS_POOL_SUI_USDC ?? "",
    deepbookPoolSuiUsdc: process.env.DEEPBOOK_POOL_SUI_USDC ?? "",
    turbosVersioned: process.env.TURBOS_VERSIONED ?? "",
  },

  // Bot
  minProfit: BigInt(optional("MIN_PROFIT", "20000000")),
  pollIntervalMs: parseInt(optional("POLL_INTERVAL_MS", "1000"), 10),
  maxGasBudget: parseInt(optional("MAX_GAS_BUDGET", "50000000"), 10),
  dryRun: optional("DRY_RUN", "true") === "true",
} as const;
