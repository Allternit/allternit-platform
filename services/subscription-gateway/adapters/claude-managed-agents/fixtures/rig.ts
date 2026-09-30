// Offline conformance rig: real provider + fake SDK client + recorded fixtures. Used by the adapter tests.
import type { ConformanceFixtures } from "@allternit/agent-gateway";
import { ClaudeManagedAgentsProvider } from "../provider.js";
import { FakeManagedAgents, type Fault } from "./fake-client.js";
import { RECORDED_APPROVAL_PAUSE } from "./recorded.js";

export const TEST_KEY = "sk-ant-api03-TESTONLYKEY-do-not-use-0000";
export const AGENT_ID = "agent_01";
export const PENDING_SESSION = "sesn_pending";
export const PENDING_APPROVAL = "sevt_a3";

export function makeProvider(fake: FakeManagedAgents, extra: ConstructorParameters<typeof ClaudeManagedAgentsProvider>[0] = {}) {
  return new ClaudeManagedAgentsProvider({
    resolveCredential: () => ({ apiKey: fake.apiKey }), clientFactory: () => fake, environmentId: "env_fake",
    pollMs: 1, replyTimeoutMs: 2000, sleep: async () => undefined, ...extra,
  });
}

export function createOfflineRig() {
  const fake = new FakeManagedAgents({ apiKey: TEST_KEY });
  fake.seedSession(PENDING_SESSION, AGENT_ID, RECORDED_APPROVAL_PAUSE);
  const provider = makeProvider(fake);
  provider.watchSession(PENDING_SESSION, AGENT_ID, "thread-pending");
  const faulty = (fault: Fault) => () => makeProvider(new FakeManagedAgents({ apiKey: TEST_KEY, fault }));
  const fixtures: ConformanceFixtures = {
    agentId: AGENT_ID, approvalId: PENDING_APPROVAL, settleMs: 5,
    faulty: { vendor_down: faulty("unavailable"), rate_limited: faulty("rate_limit"), auth_revoked: faulty("unauthorized") },
  };
  return { fake, provider, fixtures };
}
