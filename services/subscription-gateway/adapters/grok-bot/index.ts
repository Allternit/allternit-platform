// Public entry: manifest + factory. Registration (src/aai/registry.ts, owned by another agent) is one line:
//   registry.register({ adapterId: grokBot.adapterId, manifest: grokBot.manifest, create: grokBot.create });
import { CdpGrokDriver } from "./cdp-driver.js";
import { ADAPTER_ID, GROK_BOT_MANIFEST } from "./manifest.js";
import { GrokBotProvider, type GrokBotProviderOptions } from "./provider.js";

export * from "./manifest.js";
export { GrokBotProvider, type GrokBotProviderOptions } from "./provider.js";
export { CdpGrokDriver, launchWithDebugPort, type LaunchDeps } from "./cdp-driver.js";
export { ReplayGrokDriver, type ReplayOptions, type ReplayMode } from "./replay-driver.js";
export { DriverError, type GrokDriver } from "./driver.js";

export interface CreateOptions extends Partial<GrokBotProviderOptions> { cdpPort?: number }
/**
 * Default debugging port. Not 9222: that is Chrome's and Electron's usual port, already taken on a machine running
 * Allternit Desktop, and claude-desktop has its own (9232), so the two vendor apps can run side by side.
 */
export const GROK_BOT_CDP_PORT = 9231;

/** Factory: live CDP driver on cdpPort (default GROK_BOT_CDP_PORT) unless a driver is supplied (tests/replay). */
export function createGrokBotProvider(opts: CreateOptions = {}): GrokBotProvider {
  const { cdpPort, driver, ...rest } = opts;
  return new GrokBotProvider({ ...rest, driver: driver ?? new CdpGrokDriver({ port: cdpPort ?? GROK_BOT_CDP_PORT }) });
}
export const grokBot = { adapterId: ADAPTER_ID, manifest: GROK_BOT_MANIFEST, create: createGrokBotProvider } as const;
