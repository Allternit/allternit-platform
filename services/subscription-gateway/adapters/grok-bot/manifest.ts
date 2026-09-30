// Grok Bot adapter manifest (AdapterManifest + optional `agent` section). Lane ui_bridge, guarantee best_effort,
// mode linked: Allternit drives the user's own logged-in Grok Bot desktop app over local CDP.
import { adapterManifestSchema, agentCapabilityManifestSchema, type AdapterManifest } from "@allternit/subscription-fabric-contracts";
import { SELECTORS_VERSION } from "./selectors.js";

export const ADAPTER_ID = "grok-bot";
export const AGENT_ID = "grok-bot";
export const APP_NAME = "Grok Bot";

export const TERMS_WARNING =
  "Grok Bot is driven through its desktop UI on your machine, using your own signed-in account. This is not an official API: " +
  "it can break when the app updates, is paced to look like normal use, and may be limited or blocked by the vendor's terms or " +
  "usage limits. Allternit never reads your Grok Bot login data, and never approves a Grok Bot action on your behalf.";

/** Honest capability declaration. Everything not true here must answer UNSUPPORTED (enforced by runConformance). */
export const CAPABILITIES = agentCapabilityManifestSchema.parse({
  vendor: "grok",
  adapterId: ADAPTER_ID,
  lane: "ui_bridge",
  guarantee: "best_effort",
  // One main window, one active conversation. Bots keep memory/skills across chats, so isolation is "shared".
  context: { supported: true, resume: false, parallel: false, maxParallel: 1, isolation: "shared" },
  messaging: { send: true, stream: true, steer: false, interrupt: false, cancel: true },
  memory: { read: false, write: false, snapshot: false, opaque: true },
  tools: { tools: false, mcp: false, plugins: false, connectors: false },
  tasks: { list: false, schedule: false, cancel: false, background: false },
  approvals: { read: true, respond: true, exact: false },
  computer: { view: false, control: false, takeover: false },
  artifacts: { read: false, write: false, export: false },
  events: { native: false, polling: true, transcriptDerived: true, replay: true },
  runtime: { alwaysOn: false, localRequired: true, cloud: false },
});

export const PACING = {
  min_action_gap_ms: [900, 2600] as [number, number],
  min_task_gap_s: 6,
  max_tasks_per_hour: 40,
  max_tasks_per_day: 200,
};

export const GROK_BOT_MANIFEST: AdapterManifest = adapterManifestSchema.parse({
  adapter_id: ADAPTER_ID,
  adapter_version: "0.1.0",
  provider: "grok",
  interface: "ui_bridge_desktop",
  // The bundled renderer only allows https://grok.com; the gateway itself never contacts it.
  origins: ["https://grok.com"],
  auth: { login_url: "https://grok.com", logged_in_probe: "process_running+composer_visible" },
  plans: [{ plan_id: "signed_in", label: "Signed-in Grok Bot account", notes: "Whatever plan the user's own account has." }],
  capabilities: [
    { id: "chat.create", min_capability_version: 1, plans: ["signed_in"], pool_id: "chat-msgs", detachable: false, export_formats: [], status: "beta" },
    { id: "chat.continue", min_capability_version: 1, plans: ["signed_in"], pool_id: "chat-msgs", detachable: false, export_formats: [], status: "beta" },
  ],
  pacing: PACING,
  selectors_version: SELECTORS_VERSION,
  agent: {
    capabilities: CAPABILITIES,
    authDescriptors: [
      {
        id: "grok-bot-desktop-session",
        label: "Grok Bot desktop app",
        authType: "desktop_session",
        lane: "ui_bridge",
        guarantee: "best_effort",
        recommended: true,
        loggedInProbe: "process_running+composer_visible",
        scopes: ["chat.send", "chat.read", "approvals.read"],
        permissionDescription: [
          "Grok Bot must be running and signed in. Allternit attaches to it over a local debugging port on this machine only.",
          "Allternit can start a chat, type messages, read replies and see approval prompts. Approvals are only ever answered by you.",
          "Allternit does not read Grok Bot's stored login, cookies or user data.",
        ],
        termsWarning: TERMS_WARNING,
        requiresUserOwnedSubscription: true,
      },
    ],
  },
});
