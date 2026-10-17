// Kimi desktop app adapter manifest. Lane ui_bridge, guarantee best_effort, mode linked: Allternit drives the user's
// own signed-in Kimi app through the macOS Accessibility tree (route desktop-app-ax). No API key, no cookies/tokens read.
import { adapterManifestSchema, agentCapabilityManifestSchema, type AdapterManifest } from "@allternit/subscription-fabric-contracts";
import { AX_CHAT_CONTRACT, DESKTOP_APP_AX_ROUTE } from "../_shared/ax/index.js";
import { PACK_VERSION } from "./selectors.js";

export const ADAPTER_ID = "kimi-desktop";
export const AGENT_ID = "kimi-desktop";
export const APP_NAME = "Kimi";
/**
 * Route ids this adapter serves (the route registry keys on these exact ids). Only `desktop-app-ax` for now:
 * Kimi.app is Electron and its bundle carries no deny-list for --remote-debugging-port (unlike Claude), so CDP is NOT
 * vendor-blocked — but it needs the app started with that flag, which this adapter never does to an app the user has
 * open, and it is unprobed on a live launch. See README "desktop-app-cdp".
 */
export const ROUTE_IDS = [DESKTOP_APP_AX_ROUTE] as const;
export const CONTRACT = AX_CHAT_CONTRACT;

export const TERMS_WARNING =
  "The Kimi desktop app is driven through macOS Accessibility on your machine, using your own Kimi account. This is not an " +
  "official API: it can break when the app updates, is paced to look like normal use, and Moonshot's terms or plan limits may " +
  "restrict automated use of a consumer account. Allternit never reads your Kimi login, cookies or tokens.";

export const CAPABILITIES = agentCapabilityManifestSchema.parse({
  vendor: "kimi",
  adapterId: ADAPTER_ID,
  lane: "ui_bridge",
  guarantee: "best_effort",
  // One window, one active conversation; Kimi keeps account-level history, so isolation is "shared".
  // resume=true: an open context can be re-adopted, and past threads reopen via adoptContextId "thread:<id>".
  context: { supported: true, resume: true, parallel: false, maxParallel: 1, isolation: "shared" },
  messaging: { send: true, stream: true, steer: false, interrupt: false, cancel: true },
  memory: { read: false, write: false, snapshot: false, opaque: true },
  tools: { tools: false, mcp: false, plugins: false, connectors: false },
  tasks: { list: false, schedule: false, cancel: false, background: false },
  approvals: { read: false, respond: false, exact: false },
  computer: { view: false, control: false, takeover: false },
  artifacts: { read: false, write: false, export: false },
  events: { native: false, polling: true, transcriptDerived: true, replay: true },
  runtime: { alwaysOn: false, localRequired: true, cloud: false },
});

export const PACING = { min_action_gap_ms: [900, 2600] as [number, number], min_task_gap_s: 6, max_tasks_per_hour: 40, max_tasks_per_day: 200 };

export const KIMI_DESKTOP_MANIFEST: AdapterManifest = Object.assign(adapterManifestSchema.parse({
  adapter_id: ADAPTER_ID,
  adapter_version: "0.1.0",
  provider: "kimi",
  interface: "ui_bridge_desktop",
  origins: ["https://www.kimi.com"],
  auth: { login_url: "https://www.kimi.com", logged_in_probe: "process_running+composer_visible" },
  plans: [{ plan_id: "signed_in", label: "Your Kimi account", notes: "Whatever plan the user's own account has; no API key." }],
  capabilities: [
    { id: "chat.create", min_capability_version: 1, plans: ["signed_in"], pool_id: "chat-msgs", detachable: false, export_formats: [], status: "beta" },
    { id: "chat.continue", min_capability_version: 1, plans: ["signed_in"], pool_id: "chat-msgs", detachable: false, export_formats: [], status: "beta" },
  ],
  pacing: PACING,
  selectors_version: PACK_VERSION,
  agent: {
    capabilities: CAPABILITIES,
    authDescriptors: [{
      id: "kimi-desktop-session", label: "Kimi desktop app", authType: "desktop_session", lane: "ui_bridge", guarantee: "best_effort",
      recommended: false, loggedInProbe: "process_running+composer_visible", scopes: ["chat.send", "chat.read"],
      permissionDescription: [
        "Kimi must be running and signed in. Allternit reads and operates it through macOS Accessibility (you grant the Allternit helper that permission in System Settings).",
        "Allternit can start or reopen a chat, pick the model, type messages, stop a reply and read replies. It cannot attach files or save generated images.",
        "Allternit does not read Kimi's stored login, cookies or user data.",
      ],
      termsWarning: TERMS_WARNING, requiresUserOwnedSubscription: true,
    }],
  },
}), { routes: ROUTE_IDS });
