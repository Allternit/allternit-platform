import { afterEach, expect, test } from "bun:test"
import { Instance } from "../../src/runtime/context/project/instance"
import { Provider } from "../../src/runtime/providers/provider"
import { tmpdir } from "../fixture/fixture"

const originalFetch = globalThis.fetch
const saved = { key: process.env.ALLTERNIT_API_KEY, token: process.env.ALLTERNIT_API_TOKEN, cloud: process.env.ALLTERNIT_CLOUD_URL }

afterEach(() => {
  globalThis.fetch = originalFetch
  for (const [name, value] of [["ALLTERNIT_API_KEY", saved.key], ["ALLTERNIT_API_TOKEN", saved.token], ["ALLTERNIT_CLOUD_URL", saved.cloud]] as const) {
    if (value === undefined) delete process.env[name]
    else process.env[name] = value
  }
})

// Regression (2026-11-13): a Platform runtime ran its agent on allternit/<model>,
// but every turn failed "Unauthorized". The computer has only the device token
// (ALLTERNIT_API_TOKEN); the provider was loaded with it, yet a turn always
// carries an auth plan and the SDK takes only the plan's key, so the Cloud chat
// request went out with no Authorization header.
test("an Allternit Cloud turn is signed with the device token when there is no ALLTERNIT_API_KEY", async () => {
  delete process.env.ALLTERNIT_API_KEY
  process.env.ALLTERNIT_API_TOKEN = "allternit_runtime_platform_device"
  process.env.ALLTERNIT_CLOUD_URL = "https://cloud.test"
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input)
    if (url === "https://cloud.test/v1/models") return Response.json({ data: [{ id: "kimi-k3", default: true }] })
    if (url.startsWith("https://cloud.test/")) return new Response("{}", { status: 401 })
    return originalFetch(input as any, init)
  }) as typeof fetch

  await using tmp = await tmpdir({ git: true })
  await Instance.provide({
    directory: tmp.path,
    fn: async () => {
      const plan = await Provider.prepareAuth({ providerID: "allternit", modelID: "kimi-k3" })
      expect(plan.apiKey).toBe("allternit_runtime_platform_device")
      const model = await Provider.getModel("allternit", "kimi-k3")
      const language: any = await Provider.getLanguage(model, plan)
      expect(String(language.config.url({ path: "/chat/completions", modelId: "kimi-k3" }))).toBe("https://cloud.test/v1/chat/completions")
      expect(language.config.headers().Authorization).toBe("Bearer allternit_runtime_platform_device")
    },
  })
})
