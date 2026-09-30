// Public entry: manifest + factory. Registration (src/aai/registry.ts, owned by another agent) is one line:
//   registry.register({ adapterId: claudeDesktop.adapterId, manifest: claudeDesktop.manifest, create: claudeDesktop.create });
import { CdpClaudeDesktopDriver } from "./cdp-driver.js";
import { ADAPTER_ID, CLAUDE_DESKTOP_MANIFEST } from "./manifest.js";
import { ClaudeDesktopProvider, type ClaudeDesktopProviderOptions } from "./provider.js";

export * from "./manifest.js";
export { ClaudeDesktopProvider, type ClaudeDesktopProviderOptions } from "./provider.js";
export { CdpClaudeDesktopDriver, launchWithDebugPort, type LaunchDeps } from "./cdp-driver.js";
export { ReplayClaudeDesktopDriver, type ReplayOptions, type ReplayMode } from "./replay-driver.js";
export { DriverError, type ClaudeDesktopDriver } from "./driver.js";

export interface CreateOptions extends Partial<ClaudeDesktopProviderOptions> { cdpPort?: number }
/** Factory: live CDP driver on cdpPort (default 9222) unless a driver is supplied (tests/replay). */
export function createClaudeDesktopProvider(opts: CreateOptions = {}): ClaudeDesktopProvider {
  const { cdpPort, driver, ...rest } = opts;
  return new ClaudeDesktopProvider({ ...rest, driver: driver ?? new CdpClaudeDesktopDriver({ port: cdpPort ?? 9222 }) });
}
export const claudeDesktop = { adapterId: ADAPTER_ID, manifest: CLAUDE_DESKTOP_MANIFEST, create: createClaudeDesktopProvider } as const;
