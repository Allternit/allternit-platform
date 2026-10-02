// Gemini through the user's own Google subscription (Settings → Subscriptions): the shared subscription agent.
import { SubscriptionAgentProvider, type SubscriptionAgentOptions, type SubscriptionAgentSpec } from "../_shared/subscription-agent.js";
import { subscriptionAgentCapabilities, subscriptionAgentManifest } from "../_shared/subscription-agent-manifest.js";

export const ADAPTER_ID = "gemini-subscription";
export const CAPABILITIES = subscriptionAgentCapabilities("google", ADAPTER_ID);
export const MANIFEST = subscriptionAgentManifest({ adapterId: ADAPTER_ID, vendor: "google", label: "Gemini", origin: "https://gemini.google.com", loginUrl: "https://gemini.google.com/app", poolId: "gemini-web", capabilities: CAPABILITIES });
export const SPEC: SubscriptionAgentSpec = {
  adapterId: ADAPTER_ID, vendor: "google", provider: "google", agentId: "gemini", displayName: "Gemini", lookPack: "google", site: "gemini.google.com", capabilities: CAPABILITIES,
};
export const create = (opts: Omit<SubscriptionAgentOptions, "spec"> = {}) => new SubscriptionAgentProvider({ ...opts, spec: SPEC });
