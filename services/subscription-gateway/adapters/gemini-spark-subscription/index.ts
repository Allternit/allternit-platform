// Gemini Spark through the user's own Google subscription (Settings → Subscriptions): the shared subscription agent.
import { SubscriptionAgentProvider, type SubscriptionAgentOptions, type SubscriptionAgentSpec } from "../_shared/subscription-agent.js";
import { subscriptionAgentCapabilities, subscriptionAgentManifest } from "../_shared/subscription-agent-manifest.js";

export const ADAPTER_ID = "gemini-spark-subscription";
export const CAPABILITIES = subscriptionAgentCapabilities("gemini-spark", ADAPTER_ID);
export const MANIFEST = subscriptionAgentManifest({ adapterId: ADAPTER_ID, vendor: "gemini-spark", label: "Gemini Spark", origin: "https://gemini.google.com", loginUrl: "https://gemini.google.com/app", poolId: "gemini-spark-web", capabilities: CAPABILITIES });
export const SPEC: SubscriptionAgentSpec = {
  adapterId: ADAPTER_ID, vendor: "gemini-spark", provider: "gemini-spark", agentId: "gemini-spark", displayName: "Gemini Spark", lookPack: "google", site: "gemini.google.com", accountBots: false, capabilities: CAPABILITIES,
};
export const create = (opts: Omit<SubscriptionAgentOptions, "spec"> = {}) => new SubscriptionAgentProvider({ ...opts, spec: SPEC });
