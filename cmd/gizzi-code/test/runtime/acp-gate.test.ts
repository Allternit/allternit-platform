import { afterEach, describe, expect, test } from "bun:test"
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { join } from "node:path"
import { acpGateDecision, acpToolToHookPayload, parseHookOutput } from "@/runtime/drivers/acp-gate"

const BIN = process.env.ALLTERNIT_COMMRAILS_BIN
const fixtures: string[] = []
function fixtureRoot() {
  const root = mkdtempSync(join(import.meta.dir, ".acp-gate-test-"))
  fixtures.push(root)
  return root
}
function pathCheckingGate() {
  const root = fixtureRoot()
  const bin = join(root, "hook.js")
  const log = join(root, "calls.jsonl")
  writeFileSync(bin, `#!${process.execPath}
import { appendFileSync } from "node:fs"
const input = JSON.parse(await Bun.stdin.text())
appendFileSync(${JSON.stringify(log)}, JSON.stringify(input) + "\\n")
// Test seam for the real hook's single-path lease protocol: source leased, destination unleased.
if (input.tool_input.file_path?.startsWith("/outside/")) {
  console.log(JSON.stringify({ hookSpecificOutput: { permissionDecision: "deny", permissionDecisionReason: "destination outside WIH lease" } }))
} else {
  console.log("{}")
}
`, { mode: 0o755 })
  return { root, bin, calls: () => readFileSync(log, "utf8").trim().split("\n").map((line) => JSON.parse(line)) }
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
  test("finding 7: move denies an unleased second location after a leased source", async () => {
    const gate = pathCheckingGate()
    const result = await acpGateDecision({ bin: gate.bin, root: gate.root, cwd: "/repo", wihId: "wih1", sessionId: "session1", harness: "kimi", toolCall: { kind: "move", locations: [{ path: "/repo/leased/source" }, { path: "/outside/destination" }] } })
    expect(result).toEqual({ allow: false, reason: "destination outside WIH lease" })
    expect(gate.calls().map((call) => call.tool_input.file_path)).toEqual(["/repo/leased/source", "/outside/destination"])
    expect(gate.calls().every((call) => call.tool_name === "Write" && call.session_id === "session1" && call.cwd === "/repo")).toBe(true)
  })
  test.each(["destination", "destination_path", "new_path", "to"])("finding 7: move gates a raw %s outside the lease", async (field) => {
    const gate = pathCheckingGate()
    const result = await acpGateDecision({ bin: gate.bin, root: gate.root, cwd: "/repo", wihId: "wih1", harness: "kimi", toolCall: { kind: "move", rawInput: { path: "/repo/leased/source", [field]: "/outside/destination" }, locations: [{ path: "/repo/leased/source" }] } })
    expect(result.allow).toBe(false)
    expect(gate.calls().map((call) => call.tool_input.file_path)).toEqual(["/repo/leased/source", "/outside/destination"])
  })
  test.each(["edit", "delete"])("finding 7: %s gates all locations, including paths beyond the second", async (kind) => {
    const gate = pathCheckingGate()
    const paths = ["/repo/leased/a", "/repo/leased/b", "/outside/c"]
    const result = await acpGateDecision({ bin: gate.bin, root: gate.root, cwd: "/repo", wihId: "wih1", harness: "kimi", toolCall: { kind, locations: paths.map((path) => ({ path })) } })
    expect(result.allow).toBe(false)
    expect(gate.calls().map((call) => call.tool_input.file_path)).toEqual(paths)
  })
  test("finding 7: all leased mutation paths allow, duplicates checked once", async () => {
    const gate = pathCheckingGate()
    const result = await acpGateDecision({ bin: gate.bin, root: gate.root, cwd: "/repo", harness: "kimi", toolCall: { kind: "move", rawInput: { source: "/repo/leased/a", destination: "/repo/leased/b" }, locations: [{ path: "/repo/leased/a" }, { path: "/repo/leased/b" }] } })
    expect(result).toEqual({ allow: true })
    expect(gate.calls().map((call) => call.tool_input.file_path)).toEqual(["/repo/leased/a", "/repo/leased/b"])
  })
  test.each(["edit", "delete", "move"])("finding 7: %s with no recoverable write paths denies", async (kind) => {
    const gate = pathCheckingGate()
    const result = await acpGateDecision({ bin: gate.bin, cwd: "/repo", harness: "kimi", toolCall: { kind } })
    expect(result).toMatchObject({ allow: false, reason: expect.stringContaining("write paths") })
  })
  test("hook output: silence allows, deny denies, garbage fails closed", () => {
    expect(parseHookOutput("", 0)).toEqual({ allow: true })
    expect(parseHookOutput('{"hookSpecificOutput":{"permissionDecision":"deny","permissionDecisionReason":"x"}}', 0)).toEqual({ allow: false, reason: "x" })
    expect(parseHookOutput("nope", 0).allow).toBe(false)
    expect(parseHookOutput("", 2).allow).toBe(false)
  })
  test.each([1, 2, 3, 127, null])("finding 16: unsuccessful exit %s denies even with success JSON", (code) => {
    for (const stdout of ["{}", '{"hookSpecificOutput":{"permissionDecision":"allow"}}', ""]) {
      expect(parseHookOutput(stdout, code)).toMatchObject({ allow: false, reason: expect.stringContaining("fail closed") })
    }
  })
  test("finding 16: successful JSON still allows and successful explicit denial is preserved", () => {
    expect(parseHookOutput("{}", 0)).toEqual({ allow: true })
    expect(parseHookOutput('{"hookSpecificOutput":{"permissionDecision":"allow"}}', 0)).toEqual({ allow: true })
    expect(parseHookOutput('{"hookSpecificOutput":{"permissionDecision":"deny","permissionDecisionReason":"lease missing"}}', 0)).toEqual({ allow: false, reason: "lease missing" })
  })
  test.each(["exit", "signal"])("finding 16: a real child with JSON stdout followed by %s denies", async (failure) => {
    const root = fixtureRoot()
    const bin = join(root, "failed-hook.js")
    writeFileSync(bin, `#!${process.execPath}\nawait Bun.stdin.text()\nprocess.stdout.write("{}")\n${failure === "exit" ? "process.exit(1)" : 'process.kill(process.pid, "SIGTERM")'}\n`, { mode: 0o755 })
    const result = await acpGateDecision({ bin, cwd: root, harness: "kimi", toolCall: { kind: "read" } })
    expect(result).toMatchObject({ allow: false, reason: expect.stringContaining("fail closed") })
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
