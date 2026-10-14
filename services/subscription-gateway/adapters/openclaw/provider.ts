// OpenClaw AAI provider (lane local, guarantee exact, mode hosted): the user's own OpenClaw gateway over its
// OpenAI-compatible HTTP API. The client (history, streaming, persistence) is shared with Hermes: ../_shared/openai-compat.ts.
import { OpenAiCompatProvider, type OpenAiCompatOptions, type OpenAiCompatSpec } from "../_shared/openai-compat.js";
import { ADAPTER_ID, AGENT_PREFIX, buildCapabilities, DEFAULT_AGENT, DEFAULT_BASE_URL, DEFAULT_MAX_PARALLEL, VENDOR } from "./manifest.js";

/** Options; `defaultAgent` defaults to "openclaw". */
export type OpenClawOptions = OpenAiCompatOptions;

export const OPENCLAW_SPEC: OpenAiCompatSpec = {
  adapterId: ADAPTER_ID, vendor: VENDOR, name: "OpenClaw", startCommand: "openclaw gateway", agentPrefix: AGENT_PREFIX,
  defaultBaseUrl: DEFAULT_BASE_URL, defaultAgent: DEFAULT_AGENT, defaultMaxParallel: DEFAULT_MAX_PARALLEL, buildCapabilities,
  summary: (model) => ({ agentId: AGENT_PREFIX + model, displayName: model === "openclaw" ? "OpenClaw" : `OpenClaw (${model.replace(/^openclaw\//, "")})`, vendor: VENDOR, state: "READY" }),
};

export class OpenClawProvider extends OpenAiCompatProvider {
  constructor(opts: OpenClawOptions = {}) { super(OPENCLAW_SPEC, opts); }
}
