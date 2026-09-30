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
  test("no gate binary: in-process floor denies rm -rf ~/, allows ls, never prompts", async () => {
    const saved = { bin: process.env.ALLTERNIT_COMMRAILS_BIN, path: process.env.PATH }
    process.env.ALLTERNIT_COMMRAILS_BIN = ""
    process.env.PATH = "/nonexistent"
    try {
      const deny = await acpGateDecision({ toolCall: { kind: "execute", rawInput: { command: "rm -rf ~/" } }, cwd: "/w", harness: "kimi", root: "/w" })
      expect(deny.allow).toBe(false)
      expect(deny.fallback).toBe(true)
      const ok = await acpGateDecision({ toolCall: { kind: "execute", rawInput: { command: "ls" } }, cwd: "/w", harness: "kimi", root: "/w" })
      expect(ok).toEqual({ allow: true, fallback: true })
    } finally {
      process.env.ALLTERNIT_COMMRAILS_BIN = saved.bin
      process.env.PATH = saved.path
    }
  })
  test("plan mode denies a write even when the gate would allow it", async () => {
    const write = { kind: "edit", locations: [{ path: "/w/a.ts" }] }
    // Gate (fallback floor) alone allows this write.
    expect((await acpGateDecision({ toolCall: write, cwd: "/w", harness: "kimi", root: "/w", bin: undefined })).allow).toBe(true)
    const planned = await acpGateDecision({ toolCall: write, cwd: "/w", harness: "kimi", root: "/w", mode: "plan", permission: "edit" })
    expect(planned.allow).toBe(false)
    const read = await acpGateDecision({ toolCall: { kind: "read" }, cwd: "/w", harness: "kimi", root: "/w", mode: "plan", permission: "read" })
    expect(read.allow).toBe(true)
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
