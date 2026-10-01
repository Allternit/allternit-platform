// Claude through the user's own claude.ai subscription (Settings → Subscriptions). Lane browser, guarantee best_effort,
// mode linked: turns run as the gateway's own chat tasks on the signed-in claude.ai profile on the Sessions computer,
// so login, thread mapping, usage limits and pacing are the subscription worker's. No API key, no desktop app.
import { adapterManifestSchema, agentCapabilityManifestSchema, type AdapterManifest, type AgentCapabilityManifest } from "@allternit/subscription-fabric-contracts";

export const ADAPTER_ID = "claude-subscription";
export const VENDOR = "anthropic";
export const AGENT_ID = "claude";
export const SUBSCRIPTION_PROVIDER = "claude";

/**
 * Honest capability declaration (runConformance enforces it). One Allternit thread = one claude.ai conversation
 * (gateway thread mapping), so contexts are isolated and resume across turns; replies arrive whole (no token stream).
 */
export const CAPABILITIES: AgentCapabilityManifest = agentCapabilityManifestSchema.parse({
  vendor: VENDOR,
  adapterId: ADAPTER_ID,
  lane: "ui_bridge",
  guarantee: "best_effort",
  context: { supported: true, resume: true, parallel: true, maxParallel: 4, isolation: "isolated" },
  messaging: { send: true, stream: true, steer: false, interrupt: false, cancel: true },
  memory: { read: false, write: false, snapshot: false, opaque: true },
  tools: { tools: false, mcp: false, plugins: false, connectors: false },
  tasks: { list: false, schedule: false, cancel: false, background: false },
  approvals: { read: false, respond: false, exact: false },
  computer: { view: false, control: false, takeover: false },
  artifacts: { read: false, write: false, export: false },
  events: { native: false, polling: true, transcriptDerived: true, replay: true },
  runtime: { alwaysOn: true, localRequired: false, cloud: true },
});

export const CLAUDE_SUBSCRIPTION_MANIFEST: AdapterManifest = adapterManifestSchema.parse({
  adapter_id: ADAPTER_ID,
  adapter_version: "0.1.0",
  provider: VENDOR,
  interface: "ui_bridge_web",
  origins: ["https://claude.ai"],
  auth: { login_url: "https://claude.ai/login", logged_in_probe: "subscription_account_ready" },
  plans: [{ plan_id: "subscription", label: "Your Claude subscription", notes: "Uses the Claude login in Settings → Subscriptions; usage counts against that plan." }],
  capabilities: [
    { id: "chat.create", min_capability_version: 1, plans: ["subscription"], pool_id: "claude-web", detachable: false, export_formats: [], status: "beta" },
    { id: "chat.continue", min_capability_version: 1, plans: ["subscription"], pool_id: "claude-web", detachable: false, export_formats: [], status: "beta" },
  ],
  pacing: { min_action_gap_ms: [0, 0], min_task_gap_s: 0, max_tasks_per_hour: 120, max_tasks_per_day: 1000 },
  selectors_version: "via-claude-web",
  agent: {
    capabilities: CAPABILITIES,
    authDescriptors: [
      {
        id: "anthropic-browser",
        label: "Sign in to Claude",
        authType: "browser_session",
        lane: "ui_bridge",
        guarantee: "best_effort",
        recommended: true,
        loggedInProbe: "subscription_account_ready",
        scopes: ["chat.send", "chat.read"],
        permissionDescription: [
          "Uses the Claude login from Settings → Subscriptions on your Sessions computer.",
          "Each Allternit thread is one Claude conversation; your messages are sent as you.",
        ],
        termsWarning: "This drives claude.ai in a browser with your own subscription. It can break when claude.ai changes, and usage counts against your plan.",
        requiresUserOwnedSubscription: true,
      },
    ],
  },
});
