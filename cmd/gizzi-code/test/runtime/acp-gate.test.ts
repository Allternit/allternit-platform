import { afterEach, describe, expect, test } from "bun:test"
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { join } from "node:path"
import { acpGateDecision, acpToolToHookPayload, parseHookOutput } from "@/runtime/drivers/acp-gate"

const BIN = process.env.ALLTERNIT_COMMRAILS_BIN
const fixtures: string[] = []
function fixtureRoot() {
  const root = mkdtempSync(join(import.meta.dir, ".acp-gate-test-"))
  fixtures.push(root)
  return root
}
afterEach(() => {
  for (const root of fixtures.splice(0)) rmSync(root, { recursive: true, force: true })
})

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
  test("no gate binary: catastrophic floor still denies, unbound reads resolve immediately", async () => {
    const deny = await acpGateDecision({ toolCall: { kind: "execute", rawInput: { command: "rm -rf ~/" } }, cwd: "/w", harness: "kimi", bin: "" })
    expect(deny).toMatchObject({ allow: false, fallback: true, reason: expect.stringContaining("hard floor:") })
    const ok = await acpGateDecision({ toolCall: { kind: "read", rawInput: { path: "/w/a.ts" } }, cwd: "/w", harness: "kimi", bin: "" })
    expect(ok).toEqual({ allow: true, fallback: true })
  })
  test("finding 6: missing binary denies a WIH-bound write outside its lease", async () => {
    const result = await acpGateDecision({ bin: "", wihId: "wih1", cwd: "/repo", harness: "kimi", toolCall: { kind: "edit", rawInput: { file_path: "/outside/secret" } }, permission: "edit" })
    expect(result).toMatchObject({ allow: false, fallback: true, reason: expect.stringContaining("WIH wih1") })
  })
  test("finding 6: missing binary denies even a WIH-bound read", async () => {
    const result = await acpGateDecision({ bin: "", wihId: "wih1", cwd: "/repo", harness: "kimi", toolCall: { kind: "read" }, permission: "read" })
    expect(result).toMatchObject({ allow: false, fallback: true, reason: expect.stringContaining("WIH wih1") })
  })
  test.each(["edit", "delete", "move", "execute", "other"])("finding 6: missing binary denies unbound %s calls", async (kind) => {
    const result = await acpGateDecision({ bin: "", cwd: "/repo", harness: "kimi", toolCall: { kind, rawInput: { path: "/repo/a", ...(kind === "execute" ? { command: "ls" } : {}) } } })
    expect(result).toMatchObject({ allow: false, fallback: true, reason: expect.stringContaining("gate unavailable") })
  })
  test("finding 6: read labels cannot hide commands or an effectful permission", async () => {
    for (const toolCall of [{ kind: "read", rawInput: { command: "touch /outside/secret" } }, { kind: "execute", title: "Read a file", rawInput: { command: "touch /outside/secret" } }]) {
      expect((await acpGateDecision({ bin: "", cwd: "/repo", harness: "kimi", toolCall, permission: "read" })).allow).toBe(false)
    }
    expect((await acpGateDecision({ bin: "", cwd: "/repo", harness: "kimi", toolCall: { kind: "read" }, permission: "edit" })).allow).toBe(false)
  })
  test.each([false, true])("finding 6: fallback honors the shared replay marker (explicit root: %s)", async (explicitRoot) => {
    const root = fixtureRoot()
    const markerDir = join(root, ".allternit", "receipts", "_replay")
    mkdirSync(markerDir, { recursive: true })
    const marker = join(markerDir, "run_wih1.json")
    // Presence alone is authoritative, matching Gate.is_replaying; do not trust mutable contents.
    writeFileSync(marker, "{}")
    const result = await acpGateDecision({ bin: "", wihId: "wih1", cwd: explicitRoot ? join(root, "worktree") : root, root: explicitRoot ? root : undefined, harness: "kimi", toolCall: { kind: "read" }, permission: "read" })
    expect(result).toMatchObject({ allow: false, fallback: true, reason: "replay: recorded result served by the gate (run run_wih1 is replaying, effects: recorded_only)" })
    expect(Bun.file(marker).size).toBe(2)
  })
  test("plan mode denies a write even when the gate would allow it", async () => {
    const write = { kind: "edit", locations: [{ path: "/w/a.ts" }] }
    const planned = await acpGateDecision({ toolCall: write, cwd: "/w", harness: "kimi", root: "/w", bin: "", mode: "plan", permission: "edit" })
    expect(planned).toEqual({ allow: false, reason: "plan mode is read-only" })
    const read = await acpGateDecision({ toolCall: { kind: "read" }, cwd: "/w", harness: "kimi", root: "/w", bin: "", mode: "plan", permission: "read" })
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
