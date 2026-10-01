// Public entry: manifest + factory for the Claude subscription adapter.
import { ADAPTER_ID, CLAUDE_SUBSCRIPTION_MANIFEST } from "./manifest.js";
import { ClaudeSubscriptionProvider, type ClaudeSubscriptionOptions } from "./provider.js";

export * from "./manifest.js";
export { ClaudeSubscriptionProvider, type ClaudeSubscriptionOptions, type GatewayTasks } from "./provider.js";

export const createClaudeSubscriptionProvider = (opts: ClaudeSubscriptionOptions) => new ClaudeSubscriptionProvider(opts);
export const claudeSubscription = { adapterId: ADAPTER_ID, manifest: CLAUDE_SUBSCRIPTION_MANIFEST, create: createClaudeSubscriptionProvider } as const;
