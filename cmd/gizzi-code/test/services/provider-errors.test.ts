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
const CREDITS_REQUIRED = {
  error: {
    type: "billing_error",
    code: "credits_required",
    message: "Allternit Cloud models need credits. Buy credits or choose a plan.",
    balance_usd: 0,
    packs: [
      { id: "credits_10", label: "$10", credits_usd: 10, price_usd: 10 },
      { id: "credits_25", label: "$25", credits_usd: 25, price_usd: 25 },
      { id: "credits_50", label: "$50", credits_usd: 50, price_usd: 50 },
      { id: "credits_100", label: "$100", credits_usd: 100, price_usd: 100 },
    ],
    billing_url: "https://ai.allternit.com/settings/billing",
  },
}
const DISCLOSURE =
  "Allternit Cloud models need credits. Packs: $10, $25, $50, $100. Buy: https://ai.allternit.com/settings/billing · or choose a plan."
const CLOUD = {
  verify: "Free inference requires a verified email address — verify your email, then retry.",
}

describe("Allternit Cloud refusals", () => {
  test("a 402 credits_required names the packs and where to buy, not sign-in", () => {
    const help = describeProviderError({ provider: "allternit", status: 402, text: JSON.stringify(CREDITS_REQUIRED), alternative: ALT, now: NOV_9 })!
    expect(help.message).toBe(DISCLOSURE)
    expect(help.commands.map((c) => c.command)).toEqual(["/model claude-cli/claude-sonnet-5", "/usage"])
    expect(formatProviderErrorHelp(help)).not.toContain("Not logged in")
  })

  test("matched by code, whatever the message says; exhausted has its own lead", () => {
    const exhausted = { error: { ...CREDITS_REQUIRED.error, code: "credits_exhausted", message: "anything" } }
    expect(describeProviderError({ provider: "allternit", status: 402, text: JSON.stringify(exhausted) })!.message).toStartWith(
      "Your Allternit Cloud credits are used up. Packs: $10, $25, $50, $100.",
    )
    // A code-less 402 from an older Cloud is still a credits refusal, never "plan limit · resets".
    expect(describeProviderError({ provider: "allternit", status: 402, text: "Payment Required" })!.message).toBe(DISCLOSURE)
  })

  test("the errors.ts path maps an SDK 402 (body embedded in the message) before the sign-in handler", () => {
    const text = cloudRefusalHelp(402, `402 ${JSON.stringify(CREDITS_REQUIRED)}`, NOV_9)!
    expect(text).toContain(DISCLOSURE)
    expect(text).not.toMatch(/login/i)
  })

  test("a paid plan's monthly limit (403) still names the reset", () => {
    expect(
      describeProviderError({ provider: "allternit", status: 403, text: "Plus plan monthly limit reached", now: NOV_9 })!.message,
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
    const help = describeProviderError({ provider: "allternit", status: 403, text: "Plus plan monthly limit reached", now: new Date("2026-12-20T00:00:00Z") })!
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

  test("an unknown model gives the CLI's own words as the cause, on one line, with /model", () => {
    // Claude Code's is_error result for --model claude-nosuch-model-9 (stream-json, 2026-11-09).
    const said =
      "There's an issue with the selected model (claude-nosuch-model-9). It may not exist or you may not have access to it. Run --model to pick a different model."
    const text = cliBrainErrorMessage({ provider: "claude-cli" }, `${said}\n\nstack…`)
    const [cause, hint] = text.split("\n")
    expect(cause).toBe(`Claude Code: ${said}`)
    expect(hint).toContain("`/model`")
    expect(text).not.toContain("the CLI ended the turn with an error")
    expect(text).not.toContain("stack")
  })

  test("an unrecognised CLI error still leads with the CLI's words and a /model hint", () => {
    const text = cliBrainErrorMessage({ provider: "claude-cli" }, "Something odd happened\nmore")
    expect(text.split("\n")[0]).toBe("Claude Code: Something odd happened")
    expect(text).toContain("`/model`")
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

describe("the session error keeps Cloud's error.code (relay path)", () => {
  test("a 402 from Cloud reaches the session error with code, server message and no retry", async () => {
    const { APICallError } = await import("ai")
    const { MessageV2 } = await import("../../src/runtime/session/message-v2")
    const err = new APICallError({
      message: "Payment Required",
      url: "https://api.allternit.com/v1/chat/completions",
      requestBodyValues: {},
      statusCode: 402,
      responseBody: JSON.stringify(CREDITS_REQUIRED),
      isRetryable: false,
    })
    const out = MessageV2.fromError(err, { providerID: "allternit" }) as { name: string; data: Record<string, any> }
    expect(out.name).toBe("APIError")
    expect(out.data.metadata).toEqual({ url: "https://api.allternit.com/v1/chat/completions", code: "credits_required" })
    expect(out.data.message).toBe("Allternit Cloud models need credits. Buy credits or choose a plan.")
    expect(out.data.isRetryable).toBe(false)
    expect(out.data.statusCode).toBe(402)
  })
})
