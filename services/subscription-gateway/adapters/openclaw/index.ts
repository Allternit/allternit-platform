// Public entry: manifest + factory for the OpenClaw adapter.
import { ADAPTER_ID, OPENCLAW_MANIFEST } from "./manifest.js";
import { OpenClawProvider, type OpenClawOptions } from "./provider.js";

export * from "./manifest.js";
export { OpenClawProvider, type OpenClawOptions } from "./provider.js";

export const createOpenClawProvider = (opts: OpenClawOptions = {}) => new OpenClawProvider(opts);
export const openclaw = { adapterId: ADAPTER_ID, manifest: OPENCLAW_MANIFEST, create: createOpenClawProvider } as const;
