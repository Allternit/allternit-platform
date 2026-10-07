import { afterEach, expect, test } from "bun:test"
import { Auth } from "../../src/runtime/integrations/auth"
import { Instance } from "../../src/runtime/context/project/instance"
import { Provider } from "../../src/runtime/providers/provider"
import { tmpdir } from "../fixture/fixture"

afterEach(async () => {
  await Auth.remove("openai")
})

test("a key stored with PUT /auth/{provider} is the one requests use", async () => {
  const saved = process.env.OPENAI_API_KEY
  delete process.env.OPENAI_API_KEY
  try {
    await Auth.set("openai", { type: "api", key: "sk-stored-1234567890" })
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const plan = await Provider.prepareAuth({ providerID: "openai", modelID: "gpt-5-mini" })
        expect(plan.source).toBe("auth")
        expect(plan.apiKey).toBe("sk-stored-1234567890")
        // The request goes to OpenAI's own endpoint, not an empty base URL.
        const model = await Provider.getModel("openai", "gpt-5-mini")
        const language: any = await Provider.getLanguage(model, plan)
        expect(String(language.config.url({ path: "/responses", modelId: "gpt-5-mini" }))).toStartWith("https://api.openai.com/")
      },
    })
  } finally {
    if (saved !== undefined) process.env.OPENAI_API_KEY = saved
  }
})
