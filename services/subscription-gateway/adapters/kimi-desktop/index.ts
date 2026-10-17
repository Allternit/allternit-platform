// Public entry: manifest + factory for the Kimi desktop app over macOS Accessibility (route desktop-app-ax).
import { AxBridgeDriver, AxChatAppProvider, type AxChatAppProviderOptions, type AxChatAppSpec, type AxDriver } from "../_shared/ax/index.js";
import { ADAPTER_ID, AGENT_ID, APP_NAME, CAPABILITIES, KIMI_DESKTOP_MANIFEST, PACING } from "./manifest.js";
import { KIMI_BUNDLE_IDS, KIMI_DESKTOP_AX_PACK, KIMI_PATTERNS } from "./selectors.js";

export * from "./manifest.js";
export * from "./selectors.js";

export const KIMI_DESKTOP_SPEC: AxChatAppSpec = {
  adapterId: ADAPTER_ID, agentId: AGENT_ID, appName: APP_NAME, vendor: "kimi", lookPack: "kimi",
  bundleIds: KIMI_BUNDLE_IDS, pack: KIMI_DESKTOP_AX_PACK, capabilities: CAPABILITIES, pacing: PACING, patterns: KIMI_PATTERNS,
};
export interface CreateOptions extends Partial<Omit<AxChatAppProviderOptions, "ax">> { ax?: AxDriver; axBinPath?: string }
export function createKimiDesktopProvider(o: CreateOptions = {}): AxChatAppProvider {
  const { ax, axBinPath, ...rest } = o;
  if (!ax && !axBinPath) throw new Error("kimi-desktop needs `ax` or `axBinPath`");
  return new AxChatAppProvider(KIMI_DESKTOP_SPEC, { ...rest, ax: ax ?? new AxBridgeDriver({ binPath: axBinPath! }) });
}
export const kimiDesktop = { adapterId: ADAPTER_ID, manifest: KIMI_DESKTOP_MANIFEST, create: createKimiDesktopProvider } as const;
