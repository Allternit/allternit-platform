// Offline AAI registration (local fake Hermes gateway) for POST /aai/conformance/hermes.
import type { AaiRegistration } from "../../../src/aai/registry.js";
import { HermesProvider } from "../index.js";
import { startFakeHermes } from "./fake-server.js";

export async function createOfflineAaiRegistration(): Promise<{ registration: AaiRegistration; close: () => Promise<void> }> {
  const [good, limited, locked] = await Promise.all([startFakeHermes(), startFakeHermes({ fault: "rate_limit" }), startFakeHermes({ fault: "unauthorized" })]);
  const tmp = await startFakeHermes(); const dead = tmp.url; await tmp.close();
  return {
    registration: { provider: new HermesProvider({ baseUrl: good.url, replyTimeoutMs: 5000 }), fixtures: {
      agentId: "hermes:hermes-agent", settleMs: 10,
      faulty: { vendor_down: () => new HermesProvider({ baseUrl: dead }), rate_limited: () => new HermesProvider({ baseUrl: limited.url }), auth_revoked: () => new HermesProvider({ baseUrl: locked.url, token: "stale" }) },
    } },
    close: async () => { await Promise.all([good, limited, locked].map((s) => s.close())); },
  };
}
