import { afterEach, beforeEach, expect, test } from "bun:test"
import fs from "fs"
import os from "os"
import path from "path"
import { BUNDLED_PROVIDERS } from "../../src/runtime/providers/adapters/bundled"
import { Instance } from "../../src/runtime/context/project/instance"
import { Provider } from "../../src/runtime/providers/provider"
import { SelfhostInference } from "../../src/runtime/providers/selfhost-inference"
import { tmpdir } from "../fixture/fixture"

const savedFetch = globalThis.fetch
let saved: string | undefined

beforeEach(() => {
  saved = process.env[SelfhostInference.ENV]
  SelfhostInference.reset()
})

afterEach(() => {
  globalThis.fetch = savedFetch
  if (saved === undefined) delete process.env[SelfhostInference.ENV]
  else process.env[SelfhostInference.ENV] = saved
  SelfhostInference.reset()
})

function writeConfig(cfg: unknown) {
  const file = path.join(os.tmpdir(), `gizzi-inference-${Math.random().toString(36).slice(2)}.json`)
  fs.writeFileSync(file, JSON.stringify(cfg))
  process.env[SelfhostInference.ENV] = file
  return file
}

const anthropicCfg = {
  provider: "anthropic",
  anthropic: { api_key: "sk-test" },
  model_map: { "claude-sonnet-5": "claude-sonnet-5-20261001" },
}

test("config parse: each provider, defaults and errors", () => {
  expect(SelfhostInference.parse(anthropicCfg).anthropic?.api_key).toBe("sk-test")
  const b = SelfhostInference.parse({
    provider: "bedrock",
    bedrock: { region: "us-east-1", access_key_id: "a", secret_access_key: "b", session_token: "t" },
    model_map: {},
  })
  expect(b.bedrock?.session_token).toBe("t")
  expect(
    SelfhostInference.parse({ provider: "vertex", vertex: { project: "p", location: "l" }, model_map: {} }).vertex
      ?.credentials_json,
  ).toBeUndefined()
  expect(SelfhostInference.parse({ provider: "foundry", foundry: { endpoint: "https://x", api_key: "k" }, model_map: {} }).foundry?.api_version).toBeUndefined()
  expect(() => SelfhostInference.parse({ provider: "nope", model_map: {} })).toThrow(/provider must be/)
  expect(() => SelfhostInference.parse({ provider: "anthropic", model_map: {} })).toThrow(/section is required/)
  expect(() => SelfhostInference.parse({ provider: "gateway", gateway: { format: "x", base_url: "u" }, model_map: {} })).toThrow(
    /gateway.format/,
  )
})

test("load reads the file named by GIZZI_INFERENCE_CONFIG once", () => {
  delete process.env[SelfhostInference.ENV]
  expect(SelfhostInference.load()).toBeUndefined()
  const file = writeConfig(anthropicCfg)
  const first = SelfhostInference.load()
  fs.unlinkSync(file)
  expect(SelfhostInference.load()).toBe(first)
})

test("resolveModel: mapped, refused, and own-id passthrough", () => {
  const cfg = SelfhostInference.parse(anthropicCfg)
  expect(SelfhostInference.resolveModel(cfg, "claude-sonnet-5")).toBe("claude-sonnet-5-20261001")
  expect(() => SelfhostInference.resolveModel(cfg, "gpt-5")).toThrow(/model_not_mapped/)
  const open = SelfhostInference.parse({ ...anthropicCfg, model_map: {} })
  expect(SelfhostInference.resolveModel(open, "claude-opus-5")).toBe("claude-opus-5")
  const gw = SelfhostInference.parse({
    provider: "gateway",
    gateway: { format: "openai-chat", base_url: "https://gw.example/v1" },
    model_map: {},
  })
  expect(() => SelfhostInference.resolveModel(gw, "anything")).toThrow(/model_not_mapped/)
})

test("an unmapped model is refused before any network call, and only the mapped models are listed", async () => {
  writeConfig(anthropicCfg)
  let calls = 0
  globalThis.fetch = (async () => {
    calls++
    return new Response("{}")
  }) as any
  await using tmp = await tmpdir({ git: true })
  await Instance.provide({
    directory: tmp.path,
    fn: async () => {
      const providers = await Provider.list()
      expect(Object.keys(providers)).toEqual(["anthropic"])
      expect(Object.keys(providers.anthropic.models)).toEqual(["claude-sonnet-5"])

      const err: any = await Provider.getModel("anthropic", "gpt-5").catch((e) => e)
      expect(err.code).toBe("model_not_mapped")
      const other: any = await Provider.getModel("openai", "claude-sonnet-5").catch((e) => e)
      expect(other.code).toBe("model_not_mapped")
      const plan: any = await Provider.prepareAuth({ providerID: "anthropic", modelID: "gpt-5" }).catch((e) => e)
      expect(plan.code).toBe("model_not_mapped")

      const model = await Provider.getModel("anthropic", "claude-sonnet-5")
      expect(model.api.id).toBe("claude-sonnet-5-20261001")
      expect(calls).toBe(0)
    },
  })
})

test("non-gateway provider with an empty model_map allows its own ids", async () => {
  writeConfig({ ...anthropicCfg, model_map: {} })
  await using tmp = await tmpdir({ git: true })
  await Instance.provide({
    directory: tmp.path,
    fn: async () => {
      const model = await Provider.getModel("anthropic", "claude-opus-5")
      expect(model.api.id).toBe("claude-opus-5")
    },
  })
})

/** Build the SDK for a spec, call the model once against a mock fetch, return what it would have sent. */
async function capture(rawCfg: unknown, name: string) {
  const cfg = SelfhostInference.parse(rawCfg)
  const spec = SelfhostInference.spec(cfg)
  const apiID = SelfhostInference.resolveModel(cfg, name)
  const seen: { url: string; headers: Headers }[] = []
  const fetchMock = (async (input: any, init?: any) => {
    seen.push({ url: typeof input === "string" ? input : (input.url ?? String(input)), headers: new Headers(init?.headers) })
    return new Response("{}", { status: 500 })
  }) as any
  const sdk = BUNDLED_PROVIDERS[spec.npm(apiID)]({ ...spec.options, fetch: fetchMock })
  await (sdk.languageModel(apiID) as any)
    .doGenerate({ prompt: [{ role: "user", content: [{ type: "text", text: "hi" }] }] })
    .catch(() => {})
  return { spec, seen: seen[0] }
}

const hdr = { "x-gateway-key": "gw-secret", "x-tenant": "acme" }

test("gateway anthropic-messages: baseURL and headers", async () => {
  const { spec, seen } = await capture(
    { provider: "gateway", gateway: { format: "anthropic-messages", base_url: "https://gw.example/anthropic/v1/", headers: hdr }, model_map: { m: "claude-x" } },
    "m",
  )
  expect(spec.npm("claude-x")).toBe("@ai-sdk/anthropic")
  expect(seen.url).toBe("https://gw.example/anthropic/v1/messages")
  expect(seen.headers.get("x-gateway-key")).toBe("gw-secret")
  expect(seen.headers.get("x-tenant")).toBe("acme")
})

test("gateway openai-chat: baseURL and headers", async () => {
  const { spec, seen } = await capture(
    { provider: "gateway", gateway: { format: "openai-chat", base_url: "https://gw.example/v1", headers: hdr }, model_map: { m: "gpt-x" } },
    "m",
  )
  expect(spec.npm("gpt-x")).toBe("@ai-sdk/openai-compatible")
  expect(seen.url).toBe("https://gw.example/v1/chat/completions")
  expect(seen.headers.get("x-gateway-key")).toBe("gw-secret")
})

test("gateway bedrock-invoke: custom endpoint and headers", async () => {
  const { spec, seen } = await capture(
    { provider: "gateway", gateway: { format: "bedrock-invoke", base_url: "https://gw.example/bedrock", headers: hdr }, model_map: { m: "anthropic.claude-x" } },
    "m",
  )
  expect(spec.npm("anthropic.claude-x")).toBe("@ai-sdk/amazon-bedrock")
  expect(seen.url).toStartWith("https://gw.example/bedrock/model/")
  expect(seen.headers.get("x-gateway-key")).toBe("gw-secret")
})

test("gateway vertex-raw-predict: custom baseURL and headers, no Google ADC", async () => {
  const { spec, seen } = await capture(
    { provider: "gateway", gateway: { format: "vertex-raw-predict", base_url: "https://gw.example/vertex", headers: hdr }, model_map: { m: "claude-x@20261001" } },
    "m",
  )
  expect(spec.npm("claude-x@20261001")).toBe("@ai-sdk/google-vertex/anthropic")
  expect(seen.url).toStartWith("https://gw.example/vertex/")
  expect(seen.url).toContain("rawPredict")
  expect(seen.headers.get("x-gateway-key")).toBe("gw-secret")
})

test("foundry: Claude goes to the Anthropic endpoint, others to Azure OpenAI", () => {
  const cfg = SelfhostInference.parse({
    provider: "foundry",
    foundry: { endpoint: "https://res.services.ai.azure.com/", api_key: "k", api_version: "2025-01-01" },
    model_map: { c: "claude-sonnet-5", g: "gpt-5" },
  })
  const spec = SelfhostInference.spec(cfg)
  expect(spec.npm("claude-sonnet-5")).toBe("@allternit/foundry-anthropic")
  expect(spec.npm("gpt-5")).toBe("@allternit/foundry-openai")
  expect(SelfhostInference.foundryAnthropicOptions(spec.options).baseURL).toBe("https://res.services.ai.azure.com/anthropic/v1")
  expect(SelfhostInference.foundryOpenAIOptions(spec.options).baseURL).toBe("https://res.services.ai.azure.com/openai")
  expect(SelfhostInference.foundryAnthropicBase("https://r.services.ai.azure.com/anthropic")).toBe("https://r.services.ai.azure.com/anthropic/v1")
})

test("vertex credentials_json becomes googleAuthOptions; bedrock options map keys", () => {
  const v = SelfhostInference.spec(
    SelfhostInference.parse({ provider: "vertex", vertex: { project: "p", location: "us-east5", credentials_json: '{"client_email":"a@b"}' }, model_map: {} }),
  )
  expect(v.options.googleAuthOptions).toEqual({ credentials: { client_email: "a@b" } })
  const b = SelfhostInference.spec(
    SelfhostInference.parse({ provider: "bedrock", bedrock: { region: "eu-west-1", access_key_id: "AK", secret_access_key: "SK" }, model_map: {} }),
  )
  expect(b.options).toEqual({ region: "eu-west-1", accessKeyId: "AK", secretAccessKey: "SK" })
})
