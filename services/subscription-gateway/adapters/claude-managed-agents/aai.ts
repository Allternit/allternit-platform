// AAI registration (loaded at boot by registerVendorAdapters). Reads ONLY non-secret configuration from env.
// The user's Anthropic API key is never read here: it arrives per call as the short-lived `credential` that
// allternit-api attaches to POST /aai/call (see src/aai/call-scope.ts) and is resolved by the injected resolver.
import { claudeManagedAgents } from "./index.js";

const csv = (v: string | undefined) => (v ? v.split(",").map((s) => s.trim()).filter(Boolean) : undefined);

export function createAaiRegistration(env: NodeJS.ProcessEnv) {
  const maxParallel = Number(env.SUBS_GATEWAY_CLAUDE_MA_MAX_PARALLEL);
  return {
    provider: claudeManagedAgents.create({
      environmentId: env.SUBS_GATEWAY_CLAUDE_MA_ENVIRONMENT_ID || undefined,
      baseURL: env.SUBS_GATEWAY_CLAUDE_MA_BASE_URL || undefined,
      maxParallel: Number.isInteger(maxParallel) && maxParallel > 0 ? maxParallel : undefined,
      memoryStoreIds: csv(env.SUBS_GATEWAY_CLAUDE_MA_MEMORY_STORE_IDS),
      vaultIds: csv(env.SUBS_GATEWAY_CLAUDE_MA_VAULT_IDS),
      archiveOnClose: env.SUBS_GATEWAY_CLAUDE_MA_ARCHIVE_ON_CLOSE !== "false",
    }),
    // Anthropic enforces its own limits (429 -> RATE_LIMITED with retryAfterMs); no extra local pacing.
  };
}
