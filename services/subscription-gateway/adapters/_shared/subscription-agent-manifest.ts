// Manifest builder for subscription agents (Claude, ChatGPT, Kimi): turns run as the gateway's own chat tasks on the
// provider's signed-in subscription login (Settings → Subscriptions) on the Sessions computer.
import { adapterManifestSchema, agentCapabilityManifestSchema, type AdapterManifest, type AgentCapabilityManifest } from "@allternit/subscription-fabric-contracts";

export function subscriptionAgentCapabilities(vendor: string, adapterId: string): AgentCapabilityManifest {
  return agentCapabilityManifestSchema.parse({
    vendor, adapterId, lane: "ui_bridge", guarantee: "best_effort",
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
}

export function subscriptionAgentManifest(o: {
  adapterId: string; vendor: string; label: string; origin: string; loginUrl: string; poolId: string; capabilities: AgentCapabilityManifest;
}): AdapterManifest {
  return adapterManifestSchema.parse({
    adapter_id: o.adapterId, adapter_version: "0.1.0", provider: o.vendor, interface: "ui_bridge_web",
    origins: [o.origin], auth: { login_url: o.loginUrl, logged_in_probe: "subscription_account_ready" },
    plans: [{ plan_id: "subscription", label: `Your ${o.label} subscription`, notes: `Uses the ${o.label} login in Settings → Subscriptions; usage counts against that plan.` }],
    capabilities: [
      { id: "chat.create", min_capability_version: 1, plans: ["subscription"], pool_id: o.poolId, detachable: false, export_formats: [], status: "beta" },
      { id: "chat.continue", min_capability_version: 1, plans: ["subscription"], pool_id: o.poolId, detachable: false, export_formats: [], status: "beta" },
    ],
    pacing: { min_action_gap_ms: [0, 0], min_task_gap_s: 0, max_tasks_per_hour: 120, max_tasks_per_day: 1000 },
    selectors_version: `via-${o.poolId}`,
    agent: {
      capabilities: o.capabilities,
      authDescriptors: [{
        id: `${o.vendor}-browser-subscription`, label: `Sign in to ${o.label}`, authType: "browser_session", lane: "ui_bridge", guarantee: "best_effort",
        recommended: true, loggedInProbe: "subscription_account_ready", scopes: ["chat.send", "chat.read"],
        permissionDescription: [`Uses the ${o.label} login from Settings → Subscriptions on your Sessions computer.`, `Each Allternit thread is one ${o.label} conversation; your messages are sent as you.`],
        termsWarning: `This drives ${o.label} in a browser with your own subscription. It can break when the site changes, and usage counts against your plan.`,
        requiresUserOwnedSubscription: true,
      }],
    },
  });
}
