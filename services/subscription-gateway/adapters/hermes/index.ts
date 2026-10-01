// Public entry: manifest + factory for the Hermes adapter.
import { ADAPTER_ID, HERMES_MANIFEST } from "./manifest.js";
import { HermesProvider, type HermesOptions } from "./provider.js";

export * from "./manifest.js";
export { HermesProvider, type HermesOptions } from "./provider.js";

export const createHermesProvider = (opts: HermesOptions = {}) => new HermesProvider(opts);
export const hermes = { adapterId: ADAPTER_ID, manifest: HERMES_MANIFEST, create: createHermesProvider } as const;
