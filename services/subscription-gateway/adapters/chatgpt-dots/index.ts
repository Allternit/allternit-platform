// Public entry: manifest + factory. Registration is aai.ts (loaded by registerVendorAdapters).
import { BrowserDotsDriver, type BrowserDotsDriverOptions } from "./browser-driver.js";
import { ADAPTER_ID, CHATGPT_DOTS_MANIFEST } from "./manifest.js";
import { ChatGPTDotsProvider, type ChatGPTDotsProviderOptions } from "./provider.js";

export * from "./manifest.js";
export { ChatGPTDotsProvider, type ChatGPTDotsProviderOptions } from "./provider.js";
export { BrowserDotsDriver, type BrowserDotsDriverOptions, type OpenPage, type PageHandle } from "./browser-driver.js";
export { ReplayDotsDriver, type ReplayOptions, type ReplayMode } from "./replay-driver.js";
export { DriverError, type DotsDriver } from "./driver.js";

export interface CreateOptions extends Partial<ChatGPTDotsProviderOptions>, BrowserDotsDriverOptions {}
/** Factory: live browser driver unless a driver is supplied (tests/replay). Nothing launches until first use + consent. */
export function createChatGPTDotsProvider(opts: CreateOptions = {}): ChatGPTDotsProvider {
  const { profileDir, userConsented, openPage, driver, ...rest } = opts;
  return new ChatGPTDotsProvider({ ...rest, driver: driver ?? new BrowserDotsDriver({ profileDir, userConsented, openPage }) });
}
export const chatgptDots = { adapterId: ADAPTER_ID, manifest: CHATGPT_DOTS_MANIFEST, create: createChatGPTDotsProvider } as const;
