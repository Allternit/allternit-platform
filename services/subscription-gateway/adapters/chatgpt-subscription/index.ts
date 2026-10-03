// ChatGPT through the user's own ChatGPT subscription (Settings → Subscriptions): the shared subscription agent.
import { SubscriptionAgentProvider, type SubscriptionAgentOptions, type SubscriptionAgentSpec } from "../_shared/subscription-agent.js";
import { subscriptionAgentCapabilities, subscriptionAgentManifest } from "../_shared/subscription-agent-manifest.js";

export const ADAPTER_ID = "chatgpt-subscription";
export const CAPABILITIES = subscriptionAgentCapabilities("openai", ADAPTER_ID);
export const MANIFEST = subscriptionAgentManifest({ adapterId: ADAPTER_ID, vendor: "openai", label: "ChatGPT", origin: "https://chatgpt.com", loginUrl: "https://chatgpt.com/auth/login", poolId: "chatgpt-web", capabilities: CAPABILITIES });
export const SPEC: SubscriptionAgentSpec = {
  adapterId: ADAPTER_ID, vendor: "openai", provider: "chatgpt", agentId: "chatgpt", displayName: "ChatGPT", lookPack: "chatgpt-dots", site: "chatgpt.com", accountBots: true, capabilities: CAPABILITIES,
};
export const create = (opts: Omit<SubscriptionAgentOptions, "spec"> = {}) => new SubscriptionAgentProvider({ ...opts, spec: SPEC });
