// Claude Managed Agents adapter manifest (AdapterManifest + agent section).
// Lane official, guarantee exact, mode hosted: Allternit drives the user's OWN Managed Agents (agents, environments and
// sessions in their Anthropic workspace) through the official API with their OWN API key. No reselling: the key is a
// user-owned secret reference, never an Allternit key.
import { adapterManifestSchema, agentCapabilityManifestSchema, type AdapterManifest, type AgentCapabilityManifest } from "@allternit/subscription-fabric-contracts";

export const ADAPTER_ID = "claude-managed-agents";
export const VENDOR = "claude";
export const DEFAULT_MODEL = "claude-opus-5-5";
export const DEFAULT_MAX_PARALLEL = 4;

export const TERMS_WARNING =
  "Claude Managed Agents runs on your own Anthropic account and is billed to it by Anthropic. Allternit uses the API key " +
  "you link (stored as a secret reference, never shown or reused for anyone else) and never resells Claude access. " +
  "Agents with tools set to ask first will wait for you: Allternit never answers a tool approval on your behalf.";

/**
 * Honest capability declaration. Everything not true here must answer UNSUPPORTED (enforced by runConformance).
 * - context: one Managed Agents session per AAI context; sessions are isolated, resumable by session id.
 * - steer: documented as interrupt + follow-up user.message (managed-agents-events.md, Interrupt).
 * - memory: memory stores are mounted into the session filesystem and used by the agent's own file tools; there is no
 *   key/value read or write API, so memory is opaque (memory stores can be attached at session create via config).
 * - computer: not a documented Managed Agents surface -> false.
 */
export function buildCapabilities(maxParallel: number = DEFAULT_MAX_PARALLEL): AgentCapabilityManifest {
  return agentCapabilityManifestSchema.parse({
    vendor: VENDOR,
    adapterId: ADAPTER_ID,
    lane: "official",
    guarantee: "exact",
    context: { supported: true, resume: true, parallel: true, maxParallel, isolation: "isolated" },
    messaging: { send: true, stream: true, steer: true, interrupt: true, cancel: true },
    memory: { read: false, write: false, snapshot: false, opaque: true },
    tools: { tools: true, mcp: true, plugins: false, connectors: false },
    tasks: { list: false, schedule: false, cancel: false, background: false },
    approvals: { read: true, respond: true, exact: true },
    computer: { view: false, control: false, takeover: false },
    artifacts: { read: true, write: false, export: false },
    events: { native: true, polling: true, transcriptDerived: false, replay: true },
    runtime: { alwaysOn: false, localRequired: false, cloud: true },
  });
}

export const CAPABILITIES = buildCapabilities();

export const CLAUDE_MANAGED_AGENTS_MANIFEST: AdapterManifest = adapterManifestSchema.parse({
  adapter_id: ADAPTER_ID,
  adapter_version: "0.1.0",
  provider: VENDOR,
  interface: "official",
  origins: ["https://api.anthropic.com"],
  auth: { login_url: "https://platform.claude.com/settings/keys", logged_in_probe: "api_key_ref_resolves" },
  plans: [{ plan_id: "user_api_key", label: "Your Anthropic API account", notes: "Billed by Anthropic to the key's owner." }],
  capabilities: [
    { id: "chat.create", min_capability_version: 1, plans: ["user_api_key"], pool_id: "managed-agent-sessions", detachable: false, export_formats: [], status: "beta" },
    { id: "chat.continue", min_capability_version: 1, plans: ["user_api_key"], pool_id: "managed-agent-sessions", detachable: false, export_formats: [], status: "beta" },
  ],
  pacing: { min_action_gap_ms: [0, 0], min_task_gap_s: 0, max_tasks_per_hour: 600, max_tasks_per_day: 5000 },
  selectors_version: "n/a-official-api",
  agent: {
    capabilities: CAPABILITIES,
    authDescriptors: [
      {
        id: "claude-api-key",
        label: "Anthropic API key (your own)",
        authType: "api_key",
        lane: "official",
        guarantee: "exact",
        recommended: true,
        loggedInProbe: "api_key_ref_resolves",
        scopes: ["agents.read", "sessions.write", "sessions.read", "approvals.read"],
        permissionDescription: [
          "Allternit lists your Managed Agents and starts one session per conversation, using an API key you provide.",
          "Allternit can send messages, read the session's events and files, interrupt a running turn and archive a session it opened.",
          "Tool calls your agent marks as 'ask first' wait for you. Allternit only relays a decision you make; it never approves for you.",
          "Allternit never creates, edits or deletes your agents or environments.",
        ],
        termsWarning: TERMS_WARNING,
        requiresUserOwnedSubscription: true,
      },
    ],
  },
});
