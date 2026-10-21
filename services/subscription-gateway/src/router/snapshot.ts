// §A2 — FabricSnapshot assembly: the pure router's only input. Accounts,
// manifests, pools, and per-account session health — with open ui_drift
// circuit breakers (adapter-version scoped, see breakers.ts) surfaced as
// `ui_drift` health on every account of that adapter's provider. That is the
// probe-recovery mapping: a breaker stays open until a probe pass flips the
// account's session_health back to `ready` (the health enum has no explicit
// probe-passed marker; `ready` is what probe success writes).
import type { FabricSnapshot, SessionHealth } from "@allternit/subscription-fabric-contracts";
import type { AdapterRegistry } from "../adapters/registry.js";
import { listOpenBreakers } from "../breakers.js";
import type { Db } from "../store/db.js";
import { listAccounts, listQuotaPools } from "../store/queries.js";

export function buildSnapshot(db: Db, registry: AdapterRegistry, now = new Date()): FabricSnapshot {
  const accounts = listAccounts(db);
  const session_health: Record<string, SessionHealth> = {};
  for (const account of accounts) {
    session_health[account.account_id] = account.session_health;
  }
  for (const breaker of listOpenBreakers(db)) {
    const manifest = registry.byId(breaker.adapter_id)?.manifest;
    if (!manifest) continue;
    for (const account of accounts) {
      if (account.provider === manifest.provider) {
        session_health[account.account_id] = "ui_drift";
      }
    }
  }
  return {
    accounts,
    manifests: registry.adapters.map((a) => a.manifest),
    pools: listQuotaPools(db),
    session_health,
    captured_at: now.toISOString(),
  };
}
