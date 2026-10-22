// Claude Duet through the user's own Claude subscription (Settings → Connections): the shared subscription agent.
import { SubscriptionAgentProvider, type SubscriptionAgentOptions, type SubscriptionAgentSpec } from "../_shared/subscription-agent.js";
import { subscriptionAgentCapabilities, subscriptionAgentManifest } from "../_shared/subscription-agent-manifest.js";

export const ADAPTER_ID = "claude-duet-subscription";
export const CAPABILITIES = subscriptionAgentCapabilities("claude-duet", ADAPTER_ID);
export const MANIFEST = subscriptionAgentManifest({ adapterId: ADAPTER_ID, vendor: "claude-duet", label: "Claude Duet", origin: "https://claude.ai", loginUrl: "https://claude.ai/", poolId: "claude-duet-web", capabilities: CAPABILITIES });
export const SPEC: SubscriptionAgentSpec = {
  adapterId: ADAPTER_ID, vendor: "claude-duet", provider: "claude-duet", agentId: "claude-duet", displayName: "Claude Duet", lookPack: "claude", site: "claude.ai", accountBots: false, capabilities: CAPABILITIES,
};
export const create = (opts: Omit<SubscriptionAgentOptions, "spec"> = {}) => new SubscriptionAgentProvider({ ...opts, spec: SPEC });
