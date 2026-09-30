// OpenClaw adapter manifest. Lane local, guarantee exact, mode hosted: Allternit talks to the user's OWN OpenClaw
// gateway (default http://127.0.0.1:18789) over its local OpenAI-compatible HTTP API. No vendor account, no reselling.
import { adapterManifestSchema, agentCapabilityManifestSchema, type AdapterManifest, type AgentCapabilityManifest } from "@allternit/subscription-fabric-contracts";

export const ADAPTER_ID = "openclaw";
export const VENDOR = "openclaw";
export const AGENT_PREFIX = "openclaw:";
export const DEFAULT_BASE_URL = "http://127.0.0.1:18789";
export const DEFAULT_AGENT = "openclaw";
export const DEFAULT_MAX_PARALLEL = 4;

export const TERMS_WARNING =
  "OpenClaw runs on your own machine. Allternit talks to its local HTTP gateway and sends your messages to whatever " +
  "model and channels YOUR OpenClaw is configured for; those providers bill you directly. Allternit never sees or stores " +
  "OpenClaw's own provider keys. If you set a gateway token for Allternit, it is kept as a secret reference.";

/**
 * Honest capability declaration. Everything not true here answers UNSUPPORTED (enforced by runConformance).
 * - context: the OpenAI-compatible endpoint is stateless per request, so a context is Allternit-side history (see README);
 *   contexts are isolated by construction and resumable only while this process holds them.
 * - tools/plugins/tasks/approvals: OpenClaw runs skills and channels itself; none of that is visible through chat
 *   completions, so none of it is claimed.
 */
export function buildCapabilities(maxParallel: number = DEFAULT_MAX_PARALLEL): AgentCapabilityManifest {
  return agentCapabilityManifestSchema.parse({
    vendor: VENDOR,
    adapterId: ADAPTER_ID,
    lane: "local",
    guarantee: "exact",
    context: { supported: true, resume: true, parallel: true, maxParallel, isolation: "isolated" },
    messaging: { send: true, stream: true, steer: false, interrupt: false, cancel: true },
    memory: { read: false, write: false, snapshot: false, opaque: true },
    tools: { tools: false, mcp: false, plugins: false, connectors: false },
    tasks: { list: false, schedule: false, cancel: false, background: false },
    approvals: { read: false, respond: false, exact: false },
    computer: { view: false, control: false, takeover: false },
    artifacts: { read: false, write: false, export: false },
    events: { native: true, polling: true, transcriptDerived: false, replay: true },
    runtime: { alwaysOn: true, localRequired: true, cloud: false },
  });
}
export const CAPABILITIES = buildCapabilities();

export const OPENCLAW_MANIFEST: AdapterManifest = adapterManifestSchema.parse({
  adapter_id: ADAPTER_ID,
  adapter_version: "0.1.0",
  provider: VENDOR,
  interface: "official",
  origins: [DEFAULT_BASE_URL],
  auth: { login_url: DEFAULT_BASE_URL, logged_in_probe: "local_endpoint_reachable" },
  plans: [{ plan_id: "self_hosted", label: "Your own OpenClaw gateway", notes: "Runs on your machine; model costs are whatever your OpenClaw is configured with." }],
  capabilities: [
    { id: "chat.create", min_capability_version: 1, plans: ["self_hosted"], pool_id: "openclaw-local", detachable: false, export_formats: [], status: "beta" },
    { id: "chat.continue", min_capability_version: 1, plans: ["self_hosted"], pool_id: "openclaw-local", detachable: false, export_formats: [], status: "beta" },
  ],
  pacing: { min_action_gap_ms: [0, 0], min_task_gap_s: 0, max_tasks_per_hour: 3600, max_tasks_per_day: 50000 },
  selectors_version: "n/a-local-http",
  agent: {
    capabilities: CAPABILITIES,
    authDescriptors: [
      {
        id: "openclaw-local-endpoint",
        label: "OpenClaw gateway on this machine",
        authType: "local_endpoint",
        lane: "local",
        guarantee: "exact",
        recommended: true,
        loggedInProbe: "local_endpoint_reachable",
        scopes: ["chat.send", "chat.read", "agents.read"],
        permissionDescription: [
          "OpenClaw must be running with its OpenAI-compatible HTTP endpoint enabled (default http://127.0.0.1:18789).",
          "Allternit can list your OpenClaw agents and send messages to them, one conversation per Allternit thread.",
          "Allternit does not change OpenClaw's configuration, channels or skills.",
        ],
        termsWarning: TERMS_WARNING,
        requiresUserOwnedSubscription: false,
      },
    ],
  },
});
