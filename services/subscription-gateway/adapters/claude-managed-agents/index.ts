// Public entry: manifest + factory for the Claude Managed Agents adapter.
import { ADAPTER_ID, CLAUDE_MANAGED_AGENTS_MANIFEST } from "./manifest.js";
import { ClaudeManagedAgentsProvider, type ClaudeManagedAgentsOptions } from "./provider.js";

export * from "./manifest.js";
export { ClaudeManagedAgentsProvider, type ClaudeManagedAgentsOptions } from "./provider.js";
export { callScopeCredentialResolver, type ResolveCredential, type ApiCredential } from "./credential.js";
export { sdkClientFactory, type MaClient, type ClientFactory } from "./client.js";

export const createClaudeManagedAgentsProvider = (opts: ClaudeManagedAgentsOptions = {}) => new ClaudeManagedAgentsProvider(opts);
export const claudeManagedAgents = { adapterId: ADAPTER_ID, manifest: CLAUDE_MANAGED_AGENTS_MANIFEST, create: createClaudeManagedAgentsProvider } as const;
