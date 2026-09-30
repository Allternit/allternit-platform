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
/** Factory: live CDP driver on cdpPort (default 9222) unless a driver is supplied (tests/replay). */
export function createGrokBotProvider(opts: CreateOptions = {}): GrokBotProvider {
  const { cdpPort, driver, ...rest } = opts;
  return new GrokBotProvider({ ...rest, driver: driver ?? new CdpGrokDriver({ port: cdpPort ?? 9222 }) });
}
export const grokBot = { adapterId: ADAPTER_ID, manifest: GROK_BOT_MANIFEST, create: createGrokBotProvider } as const;
