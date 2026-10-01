// Kimi through the user's own Kimi subscription (Settings → Subscriptions): the shared subscription agent.
import { SubscriptionAgentProvider, type SubscriptionAgentOptions, type SubscriptionAgentSpec } from "../_shared/subscription-agent.js";
import { subscriptionAgentCapabilities, subscriptionAgentManifest } from "../_shared/subscription-agent-manifest.js";

export const ADAPTER_ID = "kimi-subscription";
export const CAPABILITIES = subscriptionAgentCapabilities("kimi", ADAPTER_ID);
export const MANIFEST = subscriptionAgentManifest({ adapterId: ADAPTER_ID, vendor: "kimi", label: "Kimi", origin: "https://www.kimi.ai", loginUrl: "https://www.kimi.ai/", poolId: "kimi-web", capabilities: CAPABILITIES });
export const SPEC: SubscriptionAgentSpec = {
  adapterId: ADAPTER_ID, vendor: "kimi", provider: "kimi", agentId: "kimi", displayName: "Kimi", lookPack: "kimi", site: "kimi.ai", capabilities: CAPABILITIES,
};
export const create = (opts: Omit<SubscriptionAgentOptions, "spec"> = {}) => new SubscriptionAgentProvider({ ...opts, spec: SPEC });
