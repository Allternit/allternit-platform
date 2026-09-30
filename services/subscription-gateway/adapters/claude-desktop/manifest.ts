// Claude adapter manifest (AdapterManifest + optional `agent` section). Lane ui_bridge, guarantee best_effort,
// mode linked: Allternit drives the user's own logged-in Claude desktop app over local CDP.
import { adapterManifestSchema, agentCapabilityManifestSchema, type AdapterManifest } from "@allternit/subscription-fabric-contracts";
import { SELECTORS_VERSION } from "./selectors.js";

export const ADAPTER_ID = "claude-desktop";
export const AGENT_ID = "claude-desktop";
export const APP_NAME = "Claude"
export const COWORK_AGENT_ID = "claude-desktop:cowork";
/** Process name of /Applications/Claude.app (CFBundleExecutable). */
export const PROCESS_NAME = "Claude";;

export const TERMS_WARNING =
  "Claude Desktop is driven through its screen on your machine, using your own Claude subscription. This is not an official API: " +
  "it can break when the app updates, is paced to look like normal use, and Anthropic's usage policy or plan limits may restrict or " +
  "block automated use of a consumer subscription. Claude Desktop only accepts a debugging connection when it was started with a " +
  "signed developer token, so this lane may be unavailable. Allternit never reads your Claude login, cookies or tokens, and never " +
  "approves a Claude permission prompt on your behalf. The official, supported lane is the Claude API (Managed Agents).";

/** Honest capability declaration. Everything not true here must answer UNSUPPORTED (enforced by runConformance). */
export const CAPABILITIES = agentCapabilityManifestSchema.parse({
  vendor: "claude",
  adapterId: ADAPTER_ID,
  lane: "ui_bridge",
  guarantee: "best_effort",
  // One main window, one active conversation. Claude keeps account-level memory/projects, so isolation is "shared".
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

export const CLAUDE_DESKTOP_MANIFEST: AdapterManifest = adapterManifestSchema.parse({
  adapter_id: ADAPTER_ID,
  adapter_version: "0.1.0",
  provider: "claude",
  interface: "ui_bridge_desktop",
  // The desktop shell loads claude.ai remotely; the gateway itself never contacts it.
  origins: ["https://claude.ai"],
  auth: { login_url: "https://claude.ai", logged_in_probe: "process_running+composer_visible" },
  plans: [{ plan_id: "signed_in", label: "Your Claude subscription", notes: "Whatever plan (Free/Pro/Max/Team) the user's own account has; no API key." }],
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
        id: "claude-desktop-session",
        label: "Claude desktop app (Chat + Cowork)",
        authType: "desktop_session",
        lane: "ui_bridge",
        guarantee: "best_effort",
        recommended: true,
        loggedInProbe: "process_running+composer_visible",
        scopes: ["chat.send", "chat.read", "approvals.read"],
        permissionDescription: [
          "Claude must be running and signed in. Allternit attaches to it over a local debugging port on this machine only, and only if the app accepts one (it refuses the flag unless started with a signed developer token).",
          "Allternit can start a chat or a Cowork task, type messages, read replies and see permission prompts. Permission prompts are only ever answered by you.",
          "Allternit does not read Claude's stored login, cookies or user data.",
        ],
        termsWarning: TERMS_WARNING,
        requiresUserOwnedSubscription: true,
      },
    ],
  },
});
