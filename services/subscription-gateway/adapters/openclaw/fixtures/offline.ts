// Offline AAI registration (local fake OpenClaw gateway) for POST /aai/conformance/openclaw.
import type { AaiRegistration } from "../../../src/aai/registry.js";
import { OpenClawProvider } from "../index.js";
import { startFakeOpenClaw } from "./fake-server.js";

export async function createOfflineAaiRegistration(): Promise<{ registration: AaiRegistration; close: () => Promise<void> }> {
  const [good, limited, locked] = await Promise.all([startFakeOpenClaw(), startFakeOpenClaw({ fault: "rate_limit" }), startFakeOpenClaw({ fault: "unauthorized" })]);
  const tmp = await startFakeOpenClaw(); const dead = tmp.url; await tmp.close();
  return {
    registration: { provider: new OpenClawProvider({ baseUrl: good.url, replyTimeoutMs: 5000 }), fixtures: {
      agentId: "openclaw:openclaw", settleMs: 10,
      faulty: { vendor_down: () => new OpenClawProvider({ baseUrl: dead }), rate_limited: () => new OpenClawProvider({ baseUrl: limited.url }), auth_revoked: () => new OpenClawProvider({ baseUrl: locked.url, token: "stale" }) },
    } },
    close: async () => { await Promise.all([good, limited, locked].map((s) => s.close())); },
  };
}
