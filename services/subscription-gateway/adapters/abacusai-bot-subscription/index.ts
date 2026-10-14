// Abacus.AI bot through the user's own Abacus.AI account (Allternit login browser): the shared subscription agent.
import { SubscriptionAgentProvider, type SubscriptionAgentOptions, type SubscriptionAgentSpec } from "../_shared/subscription-agent.js";
import { subscriptionAgentCapabilities, subscriptionAgentManifest } from "../_shared/subscription-agent-manifest.js";

export const ADAPTER_ID = "abacusai-bot";
export const CAPABILITIES = subscriptionAgentCapabilities("abacusai", ADAPTER_ID);
export const MANIFEST = subscriptionAgentManifest({ adapterId: ADAPTER_ID, vendor: "abacusai", label: "Abacus.AI", origin: "https://apps.abacus.ai", loginUrl: "https://apps.abacus.ai/bot/", poolId: "abacusai-bot-web", capabilities: CAPABILITIES });
export const SPEC: SubscriptionAgentSpec = {
  adapterId: ADAPTER_ID, vendor: "abacusai", provider: "abacusai", agentId: "abacusai-bot", displayName: "Abacus.AI Bot", lookPack: "abacusai", site: "apps.abacus.ai", accountBots: true, capabilities: CAPABILITIES,
};
export const create = (opts: Omit<SubscriptionAgentOptions, "spec"> = {}) => new SubscriptionAgentProvider({ ...opts, spec: SPEC });
