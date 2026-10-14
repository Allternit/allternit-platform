// Slashy through the user's own Slashy subscription (Settings → Subscriptions): the shared subscription agent.
// v1-UNCONFIRMED product identity and origin: see adapters/slashy-web/manifest.yaml.
import { SubscriptionAgentProvider, type SubscriptionAgentOptions, type SubscriptionAgentSpec } from "../_shared/subscription-agent.js";
import { subscriptionAgentCapabilities, subscriptionAgentManifest } from "../_shared/subscription-agent-manifest.js";

export const ADAPTER_ID = "slashy-subscription";
export const CAPABILITIES = subscriptionAgentCapabilities("slashy", ADAPTER_ID);
export const MANIFEST = subscriptionAgentManifest({ adapterId: ADAPTER_ID, vendor: "slashy", label: "Slashy", origin: "https://www.slashy.com", loginUrl: "https://www.slashy.com/", poolId: "slashy-web", capabilities: CAPABILITIES });
export const SPEC: SubscriptionAgentSpec = {
  adapterId: ADAPTER_ID, vendor: "slashy", provider: "slashy", agentId: "slashy", displayName: "Slashy", lookPack: "slashy", site: "www.slashy.com", accountBots: false, capabilities: CAPABILITIES,
};
export const create = (opts: Omit<SubscriptionAgentOptions, "spec"> = {}) => new SubscriptionAgentProvider({ ...opts, spec: SPEC });
