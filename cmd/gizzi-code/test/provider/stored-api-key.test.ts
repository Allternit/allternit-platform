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
      },
    })
  } finally {
    if (saved !== undefined) process.env.OPENAI_API_KEY = saved
  }
})
