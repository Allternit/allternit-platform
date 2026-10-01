// Claude through the user's claude.ai subscription: the shared subscription agent, plus Claude Projects as agents.
import { SubscriptionAgentProvider, type SubscriptionAgentOptions, type SubscriptionAgentSpec } from "../_shared/subscription-agent.js";
import { ADAPTER_ID, AGENT_ID, CAPABILITIES, SUBSCRIPTION_PROVIDER, VENDOR } from "./manifest.js";

export type { GatewayTasks } from "../_shared/subscription-agent.js";
export const PROJECT_PREFIX = `${AGENT_ID}:project:`;

export const CLAUDE_SPEC: SubscriptionAgentSpec = {
  adapterId: ADAPTER_ID, vendor: VENDOR, provider: SUBSCRIPTION_PROVIDER, agentId: AGENT_ID, displayName: "Claude",
  lookPack: "claude", site: "claude.ai", projects: true, capabilities: CAPABILITIES,
};

export type ClaudeSubscriptionOptions = Omit<SubscriptionAgentOptions, "spec">;
export class ClaudeSubscriptionProvider extends SubscriptionAgentProvider {
  constructor(opts: ClaudeSubscriptionOptions = {}) { super({ ...opts, spec: CLAUDE_SPEC }); }
}
