// chatgpt-dots adapter manifest (AdapterManifest + `agent` section). Lane ui_bridge, guarantee best_effort, mode linked:
// Allternit drives the user's own logged-in ChatGPT web session to talk to their OpenAI dot(s). Origins, login URL,
// login probe and pacing are INHERITED from chatgpt-web so the two adapters can never disagree about the login flow.
import { adapterManifestSchema, agentCapabilityManifestSchema, type AdapterManifest } from "@allternit/subscription-fabric-contracts";
import { loadManifest } from "../chatgpt-web/adapter.js";
import { SELECTORS_VERSION } from "./selectors.js";

export const ADAPTER_ID = "chatgpt-dots";
export const AGENT_ID = "chatgpt-dots";
export const APP_NAME = "ChatGPT dots";

const web = loadManifest();

export const TERMS_WARNING =
  "Dots are driven through the ChatGPT website in a browser on your machine, using your own signed-in ChatGPT account. This is " +
  "not an official API (OpenAI publishes no dots API): it can break when ChatGPT changes, is paced to look like normal use, and may " +
  "be limited or blocked under OpenAI's terms or your plan's usage limits. Allternit never sees your password or cookies, never " +
  "solves a verification challenge for you, and never approves an \"Ask first\" or \"Hand off\" item on your behalf.";

/** Honest capability declaration. Everything not true here must answer UNSUPPORTED (enforced by runConformance). */
export const CAPABILITIES = agentCapabilityManifestSchema.parse({
  vendor: "openai",
  adapterId: ADAPTER_ID,
  lane: "ui_bridge",
  guarantee: "best_effort",
  // One browser page, one dot conversation at a time. A dot keeps one continuous memory across everything: "shared".
  context: { supported: true, resume: false, parallel: false, maxParallel: 1, isolation: "shared" },
  messaging: { send: true, stream: true, steer: false, interrupt: false, cancel: true },
  memory: { read: false, write: false, snapshot: false, opaque: true },
  tools: { tools: false, mcp: false, plugins: false, connectors: false },
  // Tasks are read from the dot's profile panel only; nothing can be scheduled or cancelled from here.
  tasks: { list: true, schedule: false, cancel: false, background: true },
  approvals: { read: true, respond: true, exact: false },
  computer: { view: false, control: false, takeover: false },
  artifacts: { read: false, write: false, export: false },
  events: { native: false, polling: true, transcriptDerived: true, replay: true },
  // The dot itself is an always-on cloud agent; our bridge needs a local browser session.
  runtime: { alwaysOn: true, localRequired: true, cloud: true },
});

export const PACING = web.pacing;

export const CHATGPT_DOTS_MANIFEST: AdapterManifest = adapterManifestSchema.parse({
  adapter_id: ADAPTER_ID,
  adapter_version: "0.1.0",
  provider: "chatgpt",
  interface: "ui_bridge_web",
  origins: web.origins,
  auth: { login_url: web.auth.login_url, logged_in_probe: web.auth.logged_in_probe, session_cookies: web.auth.session_cookies },
  // Dots come with Pro / Business Premium (memo section 9); Free/Go/Plus have none.
  plans: [
    { plan_id: "pro", label: "Pro", notes: "First dot included. Pro users in EEA, Switzerland and UK are excluded." },
    { plan_id: "business_premium", label: "Business Premium" },
  ],
  capabilities: [
    { id: "chat.create", min_capability_version: 1, plans: ["pro", "business_premium"], pool_id: "dot-msgs", detachable: false, export_formats: [], status: "beta" },
    { id: "chat.continue", min_capability_version: 1, plans: ["pro", "business_premium"], pool_id: "dot-msgs", detachable: false, export_formats: [], status: "beta" },
  ],
  pacing: PACING,
  selectors_version: SELECTORS_VERSION,
  agent: {
    capabilities: CAPABILITIES,
    authDescriptors: [
      {
        id: "chatgpt-dots-browser-session",
        label: "Your ChatGPT account (browser session)",
        authType: "browser_session",
        lane: "ui_bridge",
        guarantee: "best_effort",
        recommended: true,
        loginUrl: web.auth.login_url,
        loggedInProbe: web.auth.logged_in_probe,
        sessionCookieHints: web.auth.session_cookies,
        scopes: ["dots.list", "chat.send", "chat.read", "approvals.read", "tasks.read"],
        permissionDescription: [
          "You sign in to ChatGPT yourself in a dedicated Chrome profile on this machine. Allternit never asks for or stores your password or cookies.",
          "Allternit can list your dots, open a dot's conversation, type messages, read replies, tasks, and see \"Ask first\" and \"Hand off\" items.",
          "Approvals are only ever answered by you. \"Hand off\" items (passwords, money) are always yours to do in ChatGPT.",
          "Requires a plan that includes a dot (Pro or Business Premium).",
        ],
        termsWarning: TERMS_WARNING,
        requiresUserOwnedSubscription: true,
      },
    ],
  },
});
