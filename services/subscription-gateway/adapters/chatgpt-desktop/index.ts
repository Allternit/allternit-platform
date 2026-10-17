// Public entry: manifest + factory for the ChatGPT desktop app over macOS Accessibility (route desktop-app-ax).
import { AxBridgeDriver, AxChatAppProvider, type AxChatAppProviderOptions, type AxChatAppSpec, type AxDriver } from "../_shared/ax/index.js";
import { ADAPTER_ID, AGENT_ID, APP_NAME, CAPABILITIES, CHATGPT_DESKTOP_MANIFEST, PACING } from "./manifest.js";
import { CHATGPT_BUNDLE_IDS, CHATGPT_DESKTOP_AX_PACK } from "./selectors.js";

export * from "./manifest.js";
export * from "./selectors.js";

export const CHATGPT_DESKTOP_SPEC: AxChatAppSpec = {
  adapterId: ADAPTER_ID, agentId: AGENT_ID, appName: APP_NAME, vendor: "openai", lookPack: "chatgpt-dots",
  bundleIds: CHATGPT_BUNDLE_IDS, pack: CHATGPT_DESKTOP_AX_PACK, capabilities: CAPABILITIES, pacing: PACING,
};
export interface CreateOptions extends Partial<Omit<AxChatAppProviderOptions, "ax">> { ax?: AxDriver; axBinPath?: string }
export function createChatGptDesktopProvider(o: CreateOptions = {}): AxChatAppProvider {
  const { ax, axBinPath, ...rest } = o;
  if (!ax && !axBinPath) throw new Error("chatgpt-desktop needs `ax` or `axBinPath`");
  return new AxChatAppProvider(CHATGPT_DESKTOP_SPEC, { ...rest, ax: ax ?? new AxBridgeDriver({ binPath: axBinPath! }) });
}
export const chatgptDesktop = { adapterId: ADAPTER_ID, manifest: CHATGPT_DESKTOP_MANIFEST, create: createChatGptDesktopProvider } as const;
