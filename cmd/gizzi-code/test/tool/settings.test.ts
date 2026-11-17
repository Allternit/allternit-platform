import { afterAll, beforeEach, describe, expect, test } from "bun:test"

/**
 * settings through the real tool, the real core (with the real
 * contracts/settings/definitions.json) and the real routing client. Only the
 * HTTP edge (allternit-api bot-routing/settings/*) is stubbed: fetch records
 * every call and answers what allternit-api's gate answers for that class.
 * The server's own gate is tested in agent_settings_tools.rs.
 */
import { mkdtempSync } from "node:fs"
import { tmpdir as osTmpdir } from "node:os"
import path from "path"

process.env.ALLTERNIT_INTERNAL_SERVICE_TOKEN = "internal-test"
process.env.GIZZI_CONFIG_DIR = mkdtempSync(path.join(osTmpdir(), "gizzi-settings-"))
const { Settings } = await import("../../src/runtime/primitives/settings")
const { SettingsTool } = await import("../../src/runtime/tools/builtins/settings")

type Call = { path: string; body: any }
let calls: Call[] = []
const realFetch = globalThis.fetch

/** allternit-api's answer per class (agent_settings_tools.rs `decide`). */
function serverAnswer(change: { key: string; value: unknown }) {
  const def = Settings.definition(change.key)
  if (!def) return { key: change.key, outcome: "invalid", reason: "unknown" }
  if (def.class === "confirm") return { key: def.key, outcome: "needs_confirmation", value: change.value, settings_path: def.settings_path }
  if (def.scope === "account") return { key: def.key, outcome: "applied", value: change.value, previous: "balanced" }
  return { key: def.key, outcome: "apply_on_device", value: change.value, scope: def.scope }
}

beforeEach(() => {
  calls = []
  process.env.ALLTERNIT_INTERNAL_SERVICE_TOKEN = "internal-test"
  globalThis.fetch = (async (input: string, init: RequestInit = {}) => {
    const url = new URL(input)
    const body = JSON.parse(String(init.body))
    calls.push({ path: url.pathname, body })
    if (url.pathname.endsWith("/settings/write")) {
      return Response.json({ toolCallId: body.toolCallId, changes: body.changes.map(serverAnswer) })
    }
    if (url.pathname.endsWith("/settings/read")) return Response.json({ settings: [], unknown: [] })
    return new Response(JSON.stringify({ error: "unexpected" }), { status: 500 })
  }) as typeof fetch
})

afterAll(() => {
  globalThis.fetch = realFetch
  delete process.env.ALLTERNIT_INTERNAL_SERVICE_TOKEN
})

/** A tool context whose permission ask always allows (allow-all). */
function ctx(opts: { metadata?: Record<string, unknown>; text?: string } = {}) {
  return {
    sessionID: "ses_settings",
    messageID: "msg_assistant",
    callID: "call_7",
    agent: "build",
    abort: AbortSignal.any([]),
    messages: [
      {
        info: { id: "msg_user", sessionID: "ses_settings", role: "user", time: { created: 0 }, metadata: opts.metadata },
        parts: [{ type: "text", text: opts.text ?? "Make it dark" }],
      },
    ],
    metadata: () => {},
    ask: async () => {},
  } as any
}

async function write(changes: any[], c = ctx()) {
  const tool = await SettingsTool.init()
  const result = await tool.execute({ action: "write", changes }, c)
  return result.metadata.changes
}

const sent = () => calls.filter((c) => c.path.endsWith("/settings/write")).flatMap((c) => c.body.changes)

describe("tool.settings", () => {
  test("locked keys are never applied and their values never leave the runtime, even under allow-all", async () => {
    const changes = await write([
      { key: "account.password", value: "hunter2" },
      { key: "account.plan", value: "max" },
    ])
    expect(changes.map((c) => c.outcome)).toEqual(["locked", "locked"])
    expect(changes[0].settings_path).toBe("settings/signin")
    expect(JSON.stringify(changes)).not.toContain("hunter2")
    expect(calls).toEqual([])
  })

  test("confirm keys come back for the user's tap, never applied", async () => {
    const [global] = await write([{ key: "instructions.global", value: "Always answer in French." }])
    expect(global).toMatchObject({ key: "instructions.global", outcome: "needs_confirmation", settings_path: "settings/cowork" })
    expect(sent().map((c) => c.key)).toEqual(["instructions.global"])
    expect(calls[0].body.automationRun).toBeUndefined()
  })

  test("an automation-started run can't even ask for a confirm key; its safe keys still go", async () => {
    const run = ctx({ metadata: { automation_run: { id: "r-1", runId: "run-1" } } })
    const changes = await write(
      [
        { key: "security.require_trusted_devices", value: false },
        { key: "files.trusted_folders", value: ["/"] },
        { key: "chat.answer_style", value: "concise" },
      ],
      run,
    )
    expect(changes.map((c) => [c.outcome, c.code])).toEqual([
      ["invalid", "automation_run"],
      ["invalid", "automation_run"],
      ["applied", undefined],
    ])
    expect(sent().map((c) => c.key)).toEqual(["chat.answer_style"])
    expect(calls[0].body.automationRun).toBe(true)

    // A bot routine's "[routine: …]" turn counts as an automation run too.
    calls = []
    const [routine] = await write([{ key: "memory.dreaming", value: false }], ctx({ text: "[routine: nightly] tidy up" }))
    expect(routine).toMatchObject({ outcome: "invalid", code: "automation_run" })
    expect(calls).toEqual([])
  })

  test("an unknown key is refused without asking the server", async () => {
    const [unknown] = await write([{ key: "account.balance", value: 1_000_000 }])
    expect(unknown.outcome).toBe("invalid")
    expect(unknown.reason).toContain("account.balance")
    expect(calls).toEqual([])
  })

  test("a safe account key is applied, with the previous value for Undo", async () => {
    const [style] = await write([{ key: "chat.answer_style", value: "concise" }])
    expect(style).toMatchObject({ key: "chat.answer_style", outcome: "applied", value: "concise", previous: "balanced" })
    expect(calls[0].body).toMatchObject({ sessionId: "ses_settings", toolCallId: "call_7" })
  })

  test("a device key comes back apply_on_device keyed by the tool call", async () => {
    const tool = await SettingsTool.init()
    const result = await tool.execute({ action: "write", changes: [{ key: "appearance.theme", value: "dark" }] }, ctx())
    expect(result.metadata.changes).toEqual([{ key: "appearance.theme", outcome: "apply_on_device", value: "dark", scope: "device" }])
    expect(result.metadata.toolCallId).toBe("call_7")
  })

  test("not signed in: device keys still apply on the device, account keys say why", async () => {
    delete process.env.ALLTERNIT_INTERNAL_SERVICE_TOKEN
    delete process.env.ALLTERNIT_API_TOKEN
    const changes = await write([
      { key: "appearance.theme", value: "dark" },
      { key: "appearance.theme", value: "purple" },
      { key: "browser.preferred", value: "chrome" },
    ])
    expect(changes.map((c) => c.outcome)).toEqual(["apply_on_device", "invalid", "invalid"])
    expect(changes[2].reason).toMatch(/^Not changed: /)
    expect(calls).toEqual([])
  })
})
