import { describe, test, expect, beforeAll, afterAll } from "bun:test"
import { mkdtempSync, writeFileSync, rmSync } from "fs"
import { tmpdir } from "os"
import { join } from "path"
import { describeProviderError, formatProviderErrorHelp } from "../../src/cli/ui/ink-app/services/api/providerErrors"
import { cloudRefusalHelp } from "../../src/cli/ui/ink-app/services/api/errors"
import { cliBrainErrorMessage } from "../../src/cli/ui/ink-app/services/api/cliBrain"
import { queryLocalModelWithStreaming } from "../../src/cli/ui/ink-app/services/api/localModel"

const NOV_9 = new Date("2026-11-09T12:00:00Z")
const ALT = { model: "claude-cli/claude-sonnet-5", label: "your Claude Code plan" }

// Response bodies as allternit-cloud-api sends them (inference_settlement.rs, user_trust.rs).
const CLOUD = {
  allowance: "Free inference allowance of $2.00/month is used up — buy credits or subscribe to keep going.",
  credits: "Inference credits exhausted — add credits to keep using hosted models.",
  verify: "Free inference requires a verified email address — verify your email, then retry.",
}

describe("Allternit Cloud refusals", () => {
  test("a used-up free allowance names the allowance and the reset, not sign-in", () => {
    const help = describeProviderError({ provider: "allternit", status: 403, text: CLOUD.allowance, alternative: ALT, now: NOV_9 })!
    expect(help.message).toBe("Allternit Cloud free allowance ($2.00/month) is used up · resets Dec 1")
    expect(help.commands.map((c) => c.command)).toEqual(["/model claude-cli/claude-sonnet-5", "/usage"])
    expect(formatProviderErrorHelp(help)).not.toContain("Not logged in")
  })

  test("the errors.ts path maps a 403 allowance refusal before the sign-in handler", () => {
    const text = cloudRefusalHelp(403, `403 ${JSON.stringify({ error: "FORBIDDEN", message: CLOUD.allowance })}`, NOV_9)!
    expect(text).toContain("free allowance ($2.00/month) is used up · resets Dec 1")
    expect(text).toContain("`/usage`")
    expect(text).not.toMatch(/login/i)
  })

  test("credits and the plan's monthly limit", () => {
    expect(describeProviderError({ provider: "allternit", status: 403, text: CLOUD.credits })!.message).toBe(
      "Allternit Cloud credits are used up",
    )
    expect(
      describeProviderError({ provider: "allternit", status: 402, text: "Free plan monthly limit reached", now: NOV_9 })!.message,
    ).toBe("Allternit Cloud plan limit reached for this month · resets Dec 1")
  })

  test("an unverified email asks for verification, a plain 401 asks for sign-in", () => {
    expect(describeProviderError({ provider: "allternit", status: 403, text: CLOUD.verify })!.message).toContain("verified email")
    expect(describeProviderError({ provider: "allternit", status: 401, text: "Unauthorized" })!.commands[0]!.command).toBe("/login")
  })

  test("a real sign-in failure is left to the sign-in handler", () => {
    expect(cloudRefusalHelp(403, "403 OAuth token has been revoked")).toBeUndefined()
    expect(cloudRefusalHelp(401, "401 invalid x-api-key")).toBeUndefined()
  })

  test("the reset date rolls over the year", () => {
    const help = describeProviderError({ provider: "allternit", status: 403, text: CLOUD.allowance, now: new Date("2026-12-20T00:00:00Z") })!
    expect(help.message).toEndWith("resets Jan 1")
  })
})

describe("CLI brains", () => {
  test("a signed-out Claude Code says how to sign in to it", () => {
    const text = cliBrainErrorMessage({ provider: "claude-cli" }, "Not logged in · Please run /login")
    expect(text).toStartWith("Claude Code isn't signed in on this computer")
    expect(text).toContain("`claude, then /login`")
  })

  test("a missing CLI says so", () => {
    const text = cliBrainErrorMessage({ provider: "codex-cli" }, "No runtime found for CLI codex-cli. Install and authenticate the CLI")
    expect(text).toStartWith("Codex isn't installed on this computer")
  })

  test("a usage-limit reply keeps the CLI's words so the limit pause can read them", () => {
    const text = cliBrainErrorMessage({ provider: "claude-cli" }, "You've hit your session limit · resets 7:40pm")
    expect(text).toContain("resets 7:40pm")
  })
})

describe("API-key providers over the OpenAI-compatible path", () => {
  let dir: string
  const prevDir = process.env.GIZZI_CONFIG_DIR
  const prevFetch = globalThis.fetch

  beforeAll(() => {
    dir = mkdtempSync(join(tmpdir(), "gizzi-provider-errors-"))
    writeFileSync(
      join(dir, "gizzi.json"),
      JSON.stringify({ provider: { openrouter: { name: "OpenRouter", options: { baseURL: "https://openrouter.test/api/v1", apiKey: "sk-or-expired" } } } }),
    )
    process.env.GIZZI_CONFIG_DIR = dir
    globalThis.fetch = (async (input: RequestInfo | URL) => {
      const url = String(input instanceof Request ? input.url : input)
      if (url.endsWith("/models")) return Response.json({ data: [{ id: "z-ai/glm-4.7" }] })
      return Response.json({ error: { message: "User not found.", code: 401 } }, { status: 401 })
    }) as typeof fetch
  })

  afterAll(() => {
    globalThis.fetch = prevFetch
    if (prevDir === undefined) delete process.env.GIZZI_CONFIG_DIR
    else process.env.GIZZI_CONFIG_DIR = prevDir
    rmSync(dir, { recursive: true, force: true })
  })

  test("an expired OpenRouter key reads as a rejected key, not 'Local model error'", async () => {
    const out: any[] = []
    for await (const ev of queryLocalModelWithStreaming({
      messages: [],
      systemPrompt: ["sys"] as any,
      tools: [],
      signal: new AbortController().signal,
      options: { model: "openrouter/z-ai/glm-4.7" } as any,
    })) out.push(ev)
    const text = out.find((e) => e.type === "assistant").message.content[0].text as string
    expect(text).toStartWith("OpenRouter rejected the API key (401)")
    expect(text).toContain("`gizzi connect login`")
    expect(text).not.toContain("Local model error")
  })
})
