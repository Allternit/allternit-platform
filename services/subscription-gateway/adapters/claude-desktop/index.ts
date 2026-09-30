// Public entry: manifest + factory. Registration (src/aai/registry.ts, owned by another agent) is one line:
//   registry.register({ adapterId: claudeDesktop.adapterId, manifest: claudeDesktop.manifest, create: claudeDesktop.create });
import { AxBridgeDriver, type AxDriver } from "../_shared/ax/index.js";
import { AxClaudeDesktopDriver } from "./ax/driver.js";
import { CdpClaudeDesktopDriver } from "./cdp-driver.js";
import { ADAPTER_ID, CLAUDE_DESKTOP_MANIFEST } from "./manifest.js";
import { ClaudeDesktopProvider, type ClaudeDesktopProviderOptions } from "./provider.js";

export * from "./manifest.js";
export { ClaudeDesktopProvider, type ClaudeDesktopProviderOptions } from "./provider.js";
export { CdpClaudeDesktopDriver, launchWithDebugPort, type LaunchDeps } from "./cdp-driver.js";
export { ReplayClaudeDesktopDriver, type ReplayOptions, type ReplayMode } from "./replay-driver.js";
export { DriverError, type ClaudeDesktopDriver } from "./driver.js";

export { AxClaudeDesktopDriver } from "./ax/driver.js";
export { CLAUDE_AX_PACK, CLAUDE_BUNDLE_ID } from "./ax/selectors.js";

export type ClaudeTransport = "cdp" | "ax";
export interface CreateOptions extends Partial<ClaudeDesktopProviderOptions> {
  cdpPort?: number;
  /** Per binding/lane config. Default "cdp" (unchanged). "ax" drives the macOS Accessibility tree (no debug port). */
  transport?: ClaudeTransport;
  /** ax transport: a ready AxDriver (tests/replay), else `axBinPath` is spawned. */
  ax?: AxDriver;
  axBinPath?: string;
  /** Explicit per-app OK to drive Claude through Accessibility (default false: calls answer LANE_BLOCKED). */
  axConsented?: boolean;
}
/** Default debugging port (not 9222, which Allternit Desktop and Chrome use; grok-bot has 9231). */
export const CLAUDE_DESKTOP_CDP_PORT = 9232;

/** Factory: live CDP driver on cdpPort (default CLAUDE_DESKTOP_CDP_PORT) unless a driver is supplied (tests/replay). */
export function createClaudeDesktopProvider(opts: CreateOptions = {}): ClaudeDesktopProvider {
  const { cdpPort, driver, transport, ax, axBinPath, axConsented, ...rest } = opts;
  if (!driver && transport === "ax") {
    if (!ax && !axBinPath) throw new Error("claude-desktop transport \"ax\" needs `ax` or `axBinPath`");
    return new ClaudeDesktopProvider({ ...rest, driver: new AxClaudeDesktopDriver(ax ?? new AxBridgeDriver({ binPath: axBinPath! }), axConsented === true) });
  }
  return new ClaudeDesktopProvider({ ...rest, driver: driver ?? new CdpClaudeDesktopDriver({ port: cdpPort ?? CLAUDE_DESKTOP_CDP_PORT }) });
}
export const claudeDesktop = { adapterId: ADAPTER_ID, manifest: CLAUDE_DESKTOP_MANIFEST, create: createClaudeDesktopProvider } as const;
