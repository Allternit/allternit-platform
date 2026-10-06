// Allternit Factory engine bridge: lookup order, --json, the API.md §2
// exit-code table, the missing-engine error and pass-through. Uses the fake
// engine script in test/fixture/factory on a private PATH.
import { afterAll, beforeAll, describe, expect, test } from "bun:test"
import fs from "fs"
import os from "os"
import path from "path"
import {
  ENGINE_BIN,
  ENGINE_MISSING_FACT,
  FactoryEngineError,
  engineVersion,
  locateEngine,
  passThrough,
  passThroughSync,
  runEngine,
  type LocateDeps,
} from "../../src/cli/factory/engine"
import { forwardToEngine, forwardedArgs, forwardInteractive, type ForwardIO } from "../../src/cli/factory/forward"
import { approveNode, loadFactoryFloor, openActionFor } from "../../src/cli/ui/tui/screens/factory-floor/data"
import { checkFactoryEngine, checkFactoryHomeMove } from "../../src/cli/commands/doctorChecks"

const FAKE = path.join(import.meta.dir, "..", "fixture", "factory", "allternit-factory")
let root: string
let binDir: string
let emptyDir: string
let locate: LocateDeps
let missing: LocateDeps

beforeAll(() => {
  root = fs.mkdtempSync(path.join(os.tmpdir(), "gizzi-factory-"))
  binDir = path.join(root, "bin")
  emptyDir = path.join(root, "empty")
  fs.mkdirSync(binDir)
  fs.mkdirSync(emptyDir)
  // A shell shim so the fake runs under the same bun as the tests.
  fs.writeFileSync(path.join(binDir, ENGINE_BIN), `#!/bin/sh\nexec "${process.execPath}" "${FAKE}" "$@"\n`, { mode: 0o755 })
  const base = { execPath: path.join(emptyDir, "gizzi"), home: emptyDir }
  locate = { ...base, env: { PATH: binDir } }
  missing = { ...base, env: { PATH: emptyDir } }
})

afterAll(() => fs.rmSync(root, { recursive: true, force: true }))

function capture(): ForwardIO & { out: string[]; err: string[] } {
  const out: string[] = []
  const err: string[] = []
  return { out, err, color: false, stdout: (s) => out.push(s), stderr: (s) => err.push(s) }
}

describe("locateEngine", () => {
  test("order: sibling, ALLTERNIT_FACTORY_BIN, ~/.allternit/bin, PATH", () => {
    const all = new Set(["/app/allternit-factory", "/env/engine", "/home/u/.allternit/bin/allternit-factory", "/p/allternit-factory"])
    const deps = (have: string[]): LocateDeps => ({
      execPath: "/app/gizzi",
      home: "/home/u",
      env: { ALLTERNIT_FACTORY_BIN: "/env/engine", PATH: "/p" },
      isExecutable: (p) => have.includes(p),
    })
    const every = [...all]
    expect(locateEngine(deps(every))?.source).toBe("sibling")
    expect(locateEngine(deps(every.slice(1)))?.source).toBe("env")
    expect(locateEngine(deps(every.slice(2)))?.source).toBe("home")
    expect(locateEngine(deps(every.slice(3)))).toEqual({ path: "/p/allternit-factory", source: "path" })
    expect(locateEngine(deps([]))).toBeNull()
  })
})

describe("runEngine", () => {
  test("adds --json and parses the document", async () => {
    const { data } = await runEngine<{ args: string[] }>(["agents", "echo", "x"], { locate })
    expect(data.args).toEqual(["agents", "echo", "x"])
  })

  test.each([
    ["workspace approve n-refused", 1, "refused"],
    ["workspace board", 2, "not_found"],
    ["workflows crash", 3, "transport"],
    ["workspace approve n-person", 5, "needs_person"],
    ["workflows usage", 64, "usage"],
  ])("%s → exit %d (%s)", async (cmd, exit, code) => {
    const err = (await runEngine(cmd.split(" "), { locate }).catch((e) => e)) as FactoryEngineError
    expect(err).toBeInstanceOf(FactoryEngineError)
    expect(err.exitCode).toBe(exit)
    expect(err.code).toBe(code as never)
  })

  test("not built yet keeps the engine's fact", async () => {
    const err = (await runEngine(["workspace", "board"], { locate }).catch((e) => e)) as FactoryEngineError
    expect(err.fact).toBe("workspace board is not built yet")
    expect(err.notBuiltYet).toBe(true)
  })

  test("stderr is the fact when the engine prints no error document", async () => {
    const err = (await runEngine(["workflows", "crash"], { locate }).catch((e) => e)) as FactoryEngineError
    expect(err.fact).toBe("panic: engine fell over")
  })

  test("non-JSON success is a transport error, never silently accepted", async () => {
    const err = (await runEngine(["workflows", "garbage"], { locate }).catch((e) => e)) as FactoryEngineError
    expect(err.code).toBe("transport")
    expect(err.exitCode).toBe(3)
  })

  test("timeout → exit 4", async () => {
    const err = (await runEngine(["workflows", "sleep"], { locate, timeoutMs: 300 }).catch((e) => e)) as FactoryEngineError
    expect(err.code).toBe("timeout")
    expect(err.exitCode).toBe(4)
  })

  test("missing engine → exit 3 with install action", async () => {
    const err = (await runEngine(["agents", "ps"], { locate: missing }).catch((e) => e)) as FactoryEngineError
    expect(err.exitCode).toBe(3)
    expect(err.fact).toBe(ENGINE_MISSING_FACT)
    expect(err.action).toContain("brew install gizzi-code")
    expect(err.action).toContain("Allternit Desktop")
  })
})

describe("forwardToEngine", () => {
  test("--json prints the engine's document only", async () => {
    const io = capture()
    expect(await forwardToEngine(["agents", "ps"], true, io, { locate })).toBe(0)
    expect(io.out).toHaveLength(1)
    expect(JSON.parse(io.out[0]!).agents).toHaveLength(3)
    expect(io.err).toEqual([])
  })

  test("human output renders badges and proof", async () => {
    const io = capture()
    await forwardToEngine(["agents", "ps"], false, io, { locate })
    const text = io.out.join("\n")
    expect(text).toContain("Hosted · Gizzi")
    expect(text).toContain("Terminal · Claude Code")
    expect(text).toContain("Vendor · ChatGPT · UI bridge")
    expect(text).toContain("1/3")
  })

  test("errors with --json are the API.md error document and keep the exit code", async () => {
    const io = capture()
    expect(await forwardToEngine(["workspace", "approve", "n-refused"], true, io, { locate })).toBe(1)
    expect(JSON.parse(io.out[0]!)).toEqual({
      error: { code: "refused", fact: "Gate refused: node n-refused has no proof", action: "Add proof first" },
    })
  })

  test("not built yet surfaces unchanged on stderr with exit 2", async () => {
    const io = capture()
    expect(await forwardToEngine(["workflows", "drive", "d1"], false, io, { locate })).toBe(2)
    expect(io.err.join("\n")).toContain("workflows drive is not built yet")
  })

  test("missing engine → exit 3, JSON error document", async () => {
    const io = capture()
    expect(await forwardToEngine(["agents", "ps"], true, io, { locate: missing })).toBe(3)
    expect(JSON.parse(io.out[0]!).error).toMatchObject({ code: "transport", fact: ENGINE_MISSING_FACT })
  })

  test("--dry-run reaches the engine", async () => {
    const io = capture()
    await forwardToEngine(["workspace", "approve", "n1", "--dry-run"], true, io, { locate })
    expect(JSON.parse(io.out[0]!)).toEqual({ approved: "n1", dryRun: true })
  })
})

describe("forwardedArgs", () => {
  test("keeps verbs and flags in order, strips --json and Gizzi's root flags", () => {
    expect(
      forwardedArgs("orchestration", ["--print-logs", "orchestration", "send", "a@t", "hi there", "--queue", "--json", "--log-level", "DEBUG"]),
    ).toEqual({ args: ["orchestration", "send", "a@t", "hi there", "--queue"], json: true })
    expect(forwardedArgs("agents", ["agents", "ps"])).toEqual({ args: ["agents", "ps"], json: false })
  })
})

describe("pass-through", () => {
  test("inherits stdio, no --json, returns the engine's exit code", async () => {
    // stdio is inherited, so assert on the exit code the fake chooses.
    expect(await passThrough(["agents", "wall"], { locate, env: { FAKE_PASSTHROUGH_EXIT: "0" } })).toBe(0)
    expect(await passThrough(["agents", "attach", "coder@core"], { locate, env: { FAKE_PASSTHROUGH_EXIT: "7" } })).toBe(7)
    expect(passThroughSync(["agents", "wall"], { locate, env: { FAKE_PASSTHROUGH_EXIT: "5" } })).toBe(5)
  })

  test("missing engine prints the install message and exits 3", async () => {
    const io = capture()
    expect(await forwardInteractive(["agents", "wall"], io, { locate: missing })).toBe(3)
    expect(io.err.join("\n")).toContain(ENGINE_MISSING_FACT)
    expect(() => passThroughSync(["agents", "wall"], { locate: missing })).toThrow(ENGINE_MISSING_FACT)
  })
})

describe("factory floor data", () => {
  const run: typeof runEngine = (args, opts) => runEngine(args, { ...opts, locate })

  test("bots from agents ps; an unbuilt board shows the engine's message", async () => {
    const floor = await loadFactoryFloor(undefined, run)
    expect(floor.agents.ok && floor.agents.data.map((a) => a.id)).toEqual(["a1", "a2", "a3"])
    expect(floor.board).toMatchObject({ ok: false, notBuilt: true, message: "workspace board is not built yet" })
  })

  test("missing engine: both sections say so, no rows", async () => {
    const floor = await loadFactoryFloor(undefined, (args, opts) => runEngine(args, { ...opts, locate: missing }))
    expect(floor.agents).toMatchObject({ ok: false, exitCode: 3, message: ENGINE_MISSING_FACT })
    expect(floor.board).toMatchObject({ ok: false, exitCode: 3 })
  })

  test("approve reports the engine's outcome", async () => {
    expect(await approveNode({ nodeId: "n1", title: "Spec" }, run)).toEqual({ ok: true, message: "Approved n1 · Spec" })
    const refused = await approveNode({ nodeId: "n-refused", title: "x" }, run)
    expect(refused.ok).toBe(false)
    expect(refused.message).toContain("Gate refused")
  })

  test("Enter by binding: terminal attaches, hosted opens the session, vendor shows its thread", () => {
    expect(openActionFor({ id: "1", binding: { type: "terminal", harness: "claude" }, pane: { id: "p", attachable: true } }, "c@t")).toEqual({
      kind: "attach",
      address: "c@t",
    })
    expect(openActionFor({ id: "1", binding: { type: "terminal" }, pane: { id: "p", attachable: false } }, "c@t").kind).toBe("none")
    expect(openActionFor({ id: "1", slug: "al", binding: { type: "hosted" } }, "al")).toEqual({ kind: "session", name: "al" })
    expect(openActionFor({ id: "1", binding: { type: "vendor", vendor: "chatgpt" } }, "g@t")).toEqual({ kind: "vendor", address: "g@t" })
  })
})

describe("gizzi doctor: Factory engine", () => {
  test("found, version, server down is info", async () => {
    const checks = await checkFactoryEngine({
      locate: () => ({ path: "/x/allternit-factory", source: "path" }),
      version: () => "allternit-factory 0.1.0",
      probe: async () => null,
      socketExists: () => false,
      env: {},
      home: emptyDir,
    })
    expect(checks.map((c) => [c.id, c.status])).toEqual([
      ["factory-engine", "pass"],
      ["factory-engine-version", "info"],
      ["factory-serve", "info"],
    ])
  })

  test("missing engine warns with the install action", async () => {
    const checks = await checkFactoryEngine({ locate: () => null, env: {}, home: emptyDir })
    expect(checks).toHaveLength(1)
    expect(checks[0]!.status).toBe("warn")
    expect(checks[0]!.message).toContain(ENGINE_MISSING_FACT)
  })

  test("real lookup + version against the fake engine", () => {
    const found = locateEngine(locate)!
    expect(engineVersion(found.path)).toBe("allternit-factory 0.0.0-test")
  })

  test("server answering is a pass", async () => {
    const checks = await checkFactoryEngine({
      locate: () => ({ path: "/x", source: "env" }),
      version: () => "v",
      probe: async () => 200,
      env: { ALLTERNIT_FACTORY_PORT: "3999" },
      home: emptyDir,
    })
    expect(checks.at(-1)).toMatchObject({ id: "factory-serve", status: "pass" })
    expect(checks.at(-1)!.message).toContain("3999")
  })
})

describe("gizzi doctor: Factory home move", () => {
  const HOME = "/h"
  const OLD = "/h/.agent-orchestrator" // old-names: keep (the folder the engine migrates from)
  const MARKER = "/h/.allternit/factory/migrated-agent-orchestrator.json"
  const run = (files: Record<string, string | true>, env: NodeJS.ProcessEnv = {}) =>
    checkFactoryHomeMove({
      env,
      home: HOME,
      exists: (p) => p in files,
      readFile: (p) => String(files[p]),
    })
  const marker = (extra: object = {}) =>
    JSON.stringify({ from: OLD, to: "/h/.allternit/factory", at: "2026-10-06T09:00:00Z", dryRun: false, moved: [], deduplicated: [], conflicts: [], removedOldHome: true, ...extra })

  test("moved: no old folder + marker passes with the date", () => {
    const c = run({ [MARKER]: marker() })
    expect(c.status).toBe("pass")
    expect(c.message).toContain("on 2026-10-06T09:00:00Z")
  })
  test("nothing to move", () => {
    expect(run({})).toMatchObject({ status: "pass", message: "Nothing to move" })
  })
  test("old folder, no marker: not moved yet", () => {
    const c = run({ [OLD]: true })
    expect(c.status).toBe("warn")
    expect(c.message).toContain("internal migrate-home`")
  })
  test("old folder + conflicts lists the first few", () => {
    const c = run({ [OLD]: true, [MARKER]: marker({ conflicts: ["a.json", "b.json", "c.json", "d.json"], removedOldHome: false }) })
    expect(c.status).toBe("warn")
    expect(c.message).toStartWith("4 item(s) left")
    expect(c.message).toContain("a.json, b.json, c.json, …")
  })
  test("old folder came back after a clean move", () => {
    const c = run({ [OLD]: true, [MARKER]: marker() })
    expect(c.status).toBe("warn")
    expect(c.message).toContain("migrate-home --again")
  })
  test("old folder + kept items (venvs/worktrees) is a pass naming them", () => {
    const c = run({ [OLD]: true, [MARKER]: marker({ kept: ["venv", "wt-a"], removedOldHome: false }) })
    expect(c.status).toBe("pass")
    expect(c.message).toContain("kept in place: venv, wt-a")
  })
  test("conflicts win over kept", () => {
    const c = run({ [OLD]: true, [MARKER]: marker({ kept: ["venv"], conflicts: ["x"], removedOldHome: false }) })
    expect(c.status).toBe("warn")
    expect(c.message).toStartWith("1 item(s) left")
  })
  test("respects ALLTERNIT_FACTORY_HOME", () => {
    const c = run({ "/fh/migrated-agent-orchestrator.json": marker() }, { ALLTERNIT_FACTORY_HOME: "/fh" })
    expect(c.status).toBe("pass")
    expect(c.message).toContain("/fh")
  })
})
