/**
 * Self-hosted inference config (Allternit Self-Hosted Agents).
 *
 * The server writes a JSON file and points GIZZI_INFERENCE_CONFIG at it. When
 * set, gizzi registers ONLY the provider the file names, lists exactly the
 * model_map keys as its models, and refuses any other model name before a
 * network call (error code `model_not_mapped`).
 *
 * This file is pure (no SDK or Provider imports) so provider.ts and
 * adapters/bundled.ts can both use it.
 */
import { readFileSync } from "fs"

export namespace SelfhostInference {
  export const ENV = "GIZZI_INFERENCE_CONFIG"

  export const PROVIDERS = ["anthropic", "bedrock", "vertex", "foundry", "gateway"] as const
  export type ProviderName = (typeof PROVIDERS)[number]
  export const GATEWAY_FORMATS = ["anthropic-messages", "bedrock-invoke", "vertex-raw-predict", "openai-chat"] as const
  export type GatewayFormat = (typeof GATEWAY_FORMATS)[number]

  export interface Config {
    provider: ProviderName
    anthropic?: { api_key: string; base_url?: string }
    bedrock?: { region: string; access_key_id: string; secret_access_key: string; session_token?: string }
    vertex?: { project: string; location: string; credentials_json?: string }
    foundry?: { endpoint: string; api_key: string; api_version?: string }
    gateway?: { format: GatewayFormat; base_url: string; headers?: Record<string, string> }
    model_map: Record<string, string>
  }

  export class ConfigError extends Error {
    readonly code = "invalid_inference_config"
    constructor(message: string) {
      super(message)
      this.name = "InferenceConfigError"
    }
  }

  export class ModelNotMappedError extends Error {
    readonly code = "model_not_mapped"
    constructor(
      readonly modelID: string,
      readonly available: string[],
    ) {
      super(`model_not_mapped: "${modelID}" is not in this server's model_map (available: ${available.join(", ") || "none"})`)
      this.name = "ModelNotMappedError"
    }
  }

  function str(obj: any, key: string, where: string, optional = false): string | undefined {
    const v = obj?.[key]
    if (v === undefined || v === null || v === "") {
      if (optional) return undefined
      throw new ConfigError(`${where}.${key} is required`)
    }
    if (typeof v !== "string") throw new ConfigError(`${where}.${key} must be a string`)
    return v
  }

  export function parse(raw: unknown): Config {
    const r = raw as any
    if (!r || typeof r !== "object") throw new ConfigError("inference config must be a JSON object")
    const provider = r.provider
    if (!PROVIDERS.includes(provider)) throw new ConfigError(`provider must be one of ${PROVIDERS.join(", ")}`)
    const map = r.model_map ?? {}
    if (typeof map !== "object" || Array.isArray(map)) throw new ConfigError("model_map must be an object")
    const model_map: Record<string, string> = {}
    for (const [k, v] of Object.entries(map)) {
      if (typeof v !== "string" || !v) throw new ConfigError(`model_map.${k} must be a non-empty string`)
      model_map[k] = v
    }
    const cfg: Config = { provider, model_map }
    const s = r[provider]
    if (!s || typeof s !== "object") throw new ConfigError(`"${provider}" section is required when provider is "${provider}"`)
    if (provider === "anthropic")
      cfg.anthropic = { api_key: str(s, "api_key", "anthropic")!, base_url: str(s, "base_url", "anthropic", true) }
    if (provider === "bedrock")
      cfg.bedrock = {
        region: str(s, "region", "bedrock")!,
        access_key_id: str(s, "access_key_id", "bedrock")!,
        secret_access_key: str(s, "secret_access_key", "bedrock")!,
        session_token: str(s, "session_token", "bedrock", true),
      }
    if (provider === "vertex")
      cfg.vertex = {
        project: str(s, "project", "vertex")!,
        location: str(s, "location", "vertex")!,
        credentials_json: str(s, "credentials_json", "vertex", true),
      }
    if (provider === "foundry")
      cfg.foundry = {
        endpoint: str(s, "endpoint", "foundry")!,
        api_key: str(s, "api_key", "foundry")!,
        api_version: str(s, "api_version", "foundry", true),
      }
    if (provider === "gateway") {
      const format = str(s, "format", "gateway")!
      if (!GATEWAY_FORMATS.includes(format as GatewayFormat))
        throw new ConfigError(`gateway.format must be one of ${GATEWAY_FORMATS.join(", ")}`)
      const headers = s.headers ?? {}
      if (typeof headers !== "object" || Array.isArray(headers)) throw new ConfigError("gateway.headers must be an object")
      for (const [k, v] of Object.entries(headers))
        if (typeof v !== "string") throw new ConfigError(`gateway.headers.${k} must be a string`)
      cfg.gateway = { format: format as GatewayFormat, base_url: str(s, "base_url", "gateway")!, headers }
    }
    return cfg
  }

  let cached: { path: string; cfg: Config } | undefined

  /** Load once per path. Undefined when GIZZI_INFERENCE_CONFIG is unset. Throws ConfigError when set but bad. */
  export function load(): Config | undefined {
    const path = process.env[ENV]
    if (!path) return undefined
    if (cached?.path === path) return cached.cfg
    let text: string
    try {
      text = readFileSync(path, "utf8")
    } catch (e) {
      throw new ConfigError(`cannot read ${ENV} file ${path}: ${(e as Error).message}`)
    }
    let json: unknown
    try {
      json = JSON.parse(text)
    } catch (e) {
      throw new ConfigError(`${ENV} file ${path} is not valid JSON`)
    }
    const cfg = parse(json)
    cached = { path, cfg }
    return cfg
  }

  export function enabled() {
    return !!process.env[ENV]
  }

  export function reset() {
    cached = undefined
  }

  /** Models the client may name. Gateway or mapped providers: exactly the keys. Non-gateway with an empty map: any id (undefined). */
  export function listed(cfg: Config): string[] {
    return Object.keys(cfg.model_map)
  }

  export function passthrough(cfg: Config) {
    return cfg.provider !== "gateway" && Object.keys(cfg.model_map).length === 0
  }

  /** The provider's own model id for a name a client used. Throws ModelNotMappedError before any network call. */
  export function resolveModel(cfg: Config, name: string): string {
    if (Object.prototype.hasOwnProperty.call(cfg.model_map, name)) return cfg.model_map[name]
    if (passthrough(cfg) && name) return name
    throw new ModelNotMappedError(name, listed(cfg))
  }

  const trim = (u: string) => u.replace(/\/+$/, "")

  /** Foundry's Anthropic endpoint: <resource>.services.ai.azure.com/anthropic/v1 */
  export function foundryAnthropicBase(endpoint: string) {
    const e = trim(endpoint)
    if (/\/anthropic\/v1$/.test(e)) return e
    if (/\/anthropic$/.test(e)) return `${e}/v1`
    return `${e.replace(/\/openai(\/v1)?$/, "")}/anthropic/v1`
  }

  /** Foundry / Azure OpenAI endpoint: <resource>.openai.azure.com/openai */
  export function foundryOpenAIBase(endpoint: string) {
    const e = trim(endpoint).replace(/\/anthropic(\/v1)?$/, "")
    return /\/openai$/.test(e) ? e : `${e}/openai`
  }

  /** Drop empty/undefined values so an SDK default applies (an empty baseURL breaks every request). */
  export function clean<T extends Record<string, any>>(o: T): T {
    return Object.fromEntries(Object.entries(o).filter(([, v]) => v !== undefined && v !== "" && v !== null)) as T
  }

  /** Option normalizers used by the bundled factories (adapters/bundled.ts). */
  export function foundryAnthropicOptions(o: Record<string, any>) {
    const { endpoint, baseURL, ...rest } = o
    return clean({ ...rest, baseURL: endpoint ? foundryAnthropicBase(endpoint) : baseURL })
  }

  export function foundryOpenAIOptions(o: Record<string, any>) {
    const { endpoint, baseURL, ...rest } = o
    return clean({ ...rest, baseURL: endpoint ? foundryOpenAIBase(endpoint) : baseURL, useDeploymentBasedUrls: true })
  }

  export interface Spec {
    id: ProviderName
    name: string
    /** SDK package key (a BUNDLED_PROVIDERS key) for this provider-side model id. */
    npm: (apiID: string) => string
    options: Record<string, any>
    url: string
  }

  const isClaude = (id: string) => /(^|[./:])claude/i.test(id)

  export function spec(cfg: Config): Spec {
    switch (cfg.provider) {
      case "anthropic": {
        const a = cfg.anthropic!
        return {
          id: "anthropic",
          name: "Anthropic",
          npm: () => "@ai-sdk/anthropic",
          options: clean({ apiKey: a.api_key, baseURL: a.base_url }),
          url: a.base_url ?? "https://api.anthropic.com/v1",
        }
      }
      case "bedrock": {
        const b = cfg.bedrock!
        return {
          id: "bedrock",
          name: "Amazon Bedrock",
          npm: () => "@ai-sdk/amazon-bedrock",
          options: clean({
            region: b.region,
            accessKeyId: b.access_key_id,
            secretAccessKey: b.secret_access_key,
            sessionToken: b.session_token,
          }),
          url: "",
        }
      }
      case "vertex": {
        const v = cfg.vertex!
        let googleAuthOptions: Record<string, any> | undefined
        if (v.credentials_json) {
          try {
            googleAuthOptions = { credentials: JSON.parse(v.credentials_json) }
          } catch {
            throw new ConfigError("vertex.credentials_json is not valid JSON")
          }
        }
        return {
          id: "vertex",
          name: "Google Vertex AI",
          // Claude on Vertex uses the SDK's Anthropic subpath; Gemini and others the main package.
          npm: (apiID) => (isClaude(apiID) ? "@ai-sdk/google-vertex/anthropic" : "@ai-sdk/google-vertex"),
          options: clean({ project: v.project, location: v.location, googleAuthOptions }),
          url: "",
        }
      }
      case "foundry": {
        const f = cfg.foundry!
        return {
          id: "foundry",
          name: "Azure AI Foundry",
          // Claude on Foundry: Anthropic Messages API; everything else (GPT, etc.): Azure OpenAI.
          npm: (apiID) => (isClaude(apiID) ? "@allternit/foundry-anthropic" : "@allternit/foundry-openai"),
          options: clean({ endpoint: f.endpoint, apiKey: f.api_key, apiVersion: f.api_version }),
          url: "",
        }
      }
      case "gateway": {
        const g = cfg.gateway!
        const base = trim(g.base_url)
        const headers = g.headers ?? {}
        switch (g.format) {
          case "anthropic-messages":
            return {
              id: "gateway",
              name: "Gateway",
              npm: () => "@ai-sdk/anthropic",
              // apiKey placeholder: the SDK refuses to start without one; headers (applied after) carry the real auth.
              options: { apiKey: "gateway", baseURL: base, headers },
              url: base,
            }
          case "bedrock-invoke":
            return {
              id: "gateway",
              name: "Gateway",
              npm: () => "@ai-sdk/amazon-bedrock",
              // Bearer placeholder so no SigV4 credentials are needed; the gateway authenticates via headers.
              options: { apiKey: "gateway", region: "us-east-1", baseURL: base, headers },
              url: base,
            }
          case "vertex-raw-predict":
            return {
              id: "gateway",
              name: "Gateway",
              npm: (apiID) => (isClaude(apiID) ? "@ai-sdk/google-vertex/anthropic" : "@ai-sdk/google-vertex"),
              // Custom headers replace Google ADC token generation.
              options: { project: "gateway", location: "global", baseURL: base, headers },
              url: base,
            }
          case "openai-chat":
            return {
              id: "gateway",
              name: "Gateway",
              npm: () => "@ai-sdk/openai-compatible",
              options: { name: "gateway", baseURL: base, headers },
              url: base,
            }
        }
      }
    }
  }
}
