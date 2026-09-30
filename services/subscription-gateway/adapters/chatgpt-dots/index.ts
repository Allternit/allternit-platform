// Public entry: manifest + factory. Registration is aai.ts (loaded by registerVendorAdapters).
import { AxBridgeDriver, type AxDriver } from "../_shared/ax/index.js";
import { AxChatGptAppDriver } from "./ax/driver.js";
import { BrowserDotsDriver, type BrowserDotsDriverOptions } from "./browser-driver.js";
import { ADAPTER_ID, CHATGPT_DOTS_MANIFEST } from "./manifest.js";
import { ChatGPTDotsProvider, type ChatGPTDotsProviderOptions } from "./provider.js";

export * from "./manifest.js";
export { ChatGPTDotsProvider, type ChatGPTDotsProviderOptions } from "./provider.js";
export { BrowserDotsDriver, type BrowserDotsDriverOptions, type OpenPage, type PageHandle } from "./browser-driver.js";
export { ReplayDotsDriver, type ReplayOptions, type ReplayMode } from "./replay-driver.js";
export { DriverError, type DotsDriver } from "./driver.js";

export { AxChatGptAppDriver, dotSlug } from "./ax/driver.js";
export { CHATGPT_APP_AX_PACK, CHATGPT_APP_BUNDLE_ID } from "./ax/selectors.js";

export type DotsTransport = "browser" | "chatgpt-app";
export interface CreateOptions extends Partial<ChatGPTDotsProviderOptions>, BrowserDotsDriverOptions {
  /** Per binding/lane config. Default "browser" (unchanged). "chatgpt-app" drives native ChatGPT.app (com.openai.chat) through macOS Accessibility. */
  transport?: DotsTransport;
  ax?: AxDriver;
  axBinPath?: string;
  /** Explicit per-app OK to drive the app through Accessibility (default false: calls answer LANE_BLOCKED). */
  axConsented?: boolean;
}
/** Factory: live browser driver unless a driver is supplied (tests/replay). Nothing launches until first use + consent. */
export function createChatGPTDotsProvider(opts: CreateOptions = {}): ChatGPTDotsProvider {
  const { profileDir, resolveProfileDir, userConsented, openPage, driver, transport, ax, axBinPath, axConsented, ...rest } = opts;
  if (!driver && transport === "chatgpt-app") {
    if (!ax && !axBinPath) throw new Error("chatgpt-dots transport \"chatgpt-app\" needs `ax` or `axBinPath`");
    return new ChatGPTDotsProvider({ ...rest, driver: new AxChatGptAppDriver(ax ?? new AxBridgeDriver({ binPath: axBinPath! }), axConsented === true) });
  }
  return new ChatGPTDotsProvider({ ...rest, driver: driver ?? new BrowserDotsDriver({ profileDir, resolveProfileDir, userConsented, openPage }) });
}
export const chatgptDots = { adapterId: ADAPTER_ID, manifest: CHATGPT_DOTS_MANIFEST, create: createChatGPTDotsProvider } as const;
