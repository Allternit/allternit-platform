import { describe, expect, test } from "bun:test"
import { acpGateDecision, acpToolToHookPayload, parseHookOutput } from "@/runtime/drivers/acp-gate"

const BIN = process.env.ALLTERNIT_COMMRAILS_BIN

describe("acp gate mapping", () => {
  test("execute maps to a Bash payload with the command", () => {
    const p = acpToolToHookPayload({ kind: "execute", title: "Run", rawInput: { command: "ls -la" } }, "/w")
    expect(p.tool_name).toBe("Bash")
    expect(p.tool_input).toEqual({ command: "ls -la" })
  })
  test("edit maps to Write with the location path", () => {
    const p = acpToolToHookPayload({ kind: "edit", locations: [{ path: "/w/a.ts" }] }, "/w")
    expect(p.tool_name).toBe("Write")
    expect(p.tool_input).toEqual({ file_path: "/w/a.ts" })
  })
  test("hook output: silence allows, deny denies, garbage fails closed", () => {
    expect(parseHookOutput("", 0)).toEqual({ allow: true })
    expect(parseHookOutput('{"hookSpecificOutput":{"permissionDecision":"deny","permissionDecisionReason":"x"}}', 0)).toEqual({ allow: false, reason: "x" })
    expect(parseHookOutput("nope", 0).allow).toBe(false)
    expect(parseHookOutput("", 2).allow).toBe(false)
  })
  test("no gate binary means no verdict (caller keeps legacy policy)", async () => {
    const saved = { bin: process.env.ALLTERNIT_COMMRAILS_BIN, path: process.env.PATH }
    process.env.ALLTERNIT_COMMRAILS_BIN = ""
    process.env.PATH = "/nonexistent"
    try {
      expect(await acpGateDecision({ toolCall: { kind: "execute" }, cwd: "/w", harness: "kimi", root: "/w" })).toBeUndefined()
    } finally {
      process.env.ALLTERNIT_COMMRAILS_BIN = saved.bin
      process.env.PATH = saved.path
    }
  })
})

// Real gate binary (set ALLTERNIT_COMMRAILS_BIN to run).
describe.skipIf(!BIN)("acp gate against the real commrails binary", () => {
  test("catastrophic command denied, normal allowed", async () => {
    const root = "/tmp"
    const deny = await acpGateDecision({ toolCall: { kind: "execute", rawInput: { command: "rm -rf ~/" } }, cwd: root, root, harness: "kimi", bin: BIN })
    expect(deny?.allow).toBe(false)
    const ok = await acpGateDecision({ toolCall: { kind: "execute", rawInput: { command: "ls" } }, cwd: root, root, harness: "kimi", bin: BIN })
    expect(ok?.allow).toBe(true)
  })
})
