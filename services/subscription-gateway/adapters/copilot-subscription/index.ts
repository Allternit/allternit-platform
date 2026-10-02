// Copilot through the user's own Microsoft subscription (Settings → Subscriptions): the shared subscription agent.
import { SubscriptionAgentProvider, type SubscriptionAgentOptions, type SubscriptionAgentSpec } from "../_shared/subscription-agent.js";
import { subscriptionAgentCapabilities, subscriptionAgentManifest } from "../_shared/subscription-agent-manifest.js";

export const ADAPTER_ID = "copilot-subscription";
export const CAPABILITIES = subscriptionAgentCapabilities("microsoft", ADAPTER_ID);
export const MANIFEST = subscriptionAgentManifest({ adapterId: ADAPTER_ID, vendor: "microsoft", label: "Copilot", origin: "https://copilot.microsoft.com", loginUrl: "https://copilot.microsoft.com/", poolId: "copilot-web", capabilities: CAPABILITIES });
export const SPEC: SubscriptionAgentSpec = {
  adapterId: ADAPTER_ID, vendor: "microsoft", provider: "microsoft", agentId: "copilot", displayName: "Copilot", lookPack: "microsoft", site: "copilot.microsoft.com", capabilities: CAPABILITIES,
};
export const create = (opts: Omit<SubscriptionAgentOptions, "spec"> = {}) => new SubscriptionAgentProvider({ ...opts, spec: SPEC });
