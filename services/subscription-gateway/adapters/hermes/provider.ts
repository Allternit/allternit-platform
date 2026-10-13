// Hermes AAI provider (lane local, guarantee exact, mode hosted): the user's own Hermes gateway over its
// OpenAI-compatible HTTP API. The client (history, streaming, persistence) is shared with OpenClaw: ../_shared/openai-compat.ts.
import { OpenAiCompatProvider, type OpenAiCompatOptions, type OpenAiCompatSpec } from "../_shared/openai-compat.js";
import { ADAPTER_ID, AGENT_PREFIX, buildCapabilities, DEFAULT_AGENT, DEFAULT_BASE_URL, DEFAULT_MAX_PARALLEL, VENDOR } from "./manifest.js";

/** Options; `defaultAgent` defaults to "hermes-agent". */
export type HermesOptions = OpenAiCompatOptions;

export const HERMES_SPEC: OpenAiCompatSpec = {
  adapterId: ADAPTER_ID, vendor: VENDOR, name: "Hermes", startCommand: "hermes gateway (with API_SERVER_ENABLED=true)", agentPrefix: AGENT_PREFIX,
  defaultBaseUrl: DEFAULT_BASE_URL, defaultAgent: DEFAULT_AGENT, defaultMaxParallel: DEFAULT_MAX_PARALLEL, buildCapabilities,
  summary: (model) => ({ agentId: AGENT_PREFIX + model, displayName: model === "hermes-agent" ? "Hermes" : `Hermes · ${model}`, vendor: VENDOR, state: "READY", kind: "profile", kindLabel: "Profile" }),
};

export class HermesProvider extends OpenAiCompatProvider {
  constructor(opts: HermesOptions = {}) { super(HERMES_SPEC, opts); }
}
