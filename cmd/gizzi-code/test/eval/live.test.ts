// @ts-nocheck
import { test, expect, beforeAll, afterAll } from "bun:test"
import fs from "fs/promises"
import path from "path"
import os from "os"
import { runChecks } from "../../src/runtime/eval/checks"
import { runLive, BLOCK_EFFECTS_CONFIG, passRule } from "../../src/runtime/eval/live"
import { Case } from "../../src/runtime/eval/schema"

let root: string
let stub: string
let log: string

// A stand-in for `gizzi`: writes out.txt from the prompt ("write:<text>"), logs its
// env and args, prints one JSON event with a fake session id. `export` prints no cost.
const STUB = `
const fs = require("fs"), path = require("path")
const a = process.argv.slice(2)
if (a[0] === "export") { console.log(JSON.stringify({ messages: [{ info: { role: "assistant", cost: 0.25, tokens: { input: 10, output: 5 } } }] })); process.exit(0) }
const dir = a[a.indexOf("--dir") + 1]
const prompt = a[a.length - 1]
fs.appendFileSync(process.env.EVAL_STUB_LOG, JSON.stringify({ args: a, config: process.env.GIZZI_CONFIG_CONTENT ?? null }) + "\\n")
if (prompt.startsWith("write:")) fs.writeFileSync(path.join(dir, "out.txt"), prompt.slice(6))
if (prompt === "hang") setTimeout(() => {}, 60000)
else console.log(JSON.stringify({ type: "text", sessionID: "ses_fake", part: { text: "done" } }))
`

beforeAll(async () => {
  root = await fs.mkdtemp(path.join(os.tmpdir(), "live-test-"))
  stub = path.join(root, "stub.js")
  log = path.join(root, "log.jsonl")
  await fs.writeFile(stub, STUB)
  process.env.EVAL_STUB_LOG = log
})
afterAll(async () => {
  delete process.env.EVAL_STUB_LOG
  await fs.rm(root, { recursive: true, force: true })
})

const traj = async () => ({ status: "completed", steps: [], effects: [], cost_usd: 0, tokens: 0, latency_ms: 5 })

function mkCase(over: Record<string, unknown> = {}) {
  return Case.parse({
    schema: "allternit.gizzi_eval_case.v1",
    id: "case_t",
    suite: "s",
    title: "t",
    label: "good",
    origin: { session_id: "x", through_message_id: null, saved_at: 0, gizzi_version: "0", surface: "cli" },
    turns: [{ role: "user", text: "write:hello world" }],
    workspace: { kind: "none", reason: "no_project" },
    checks: [{ type: "contains", path: "out.txt", text: "hello" }],
    graders: [{ type: "status", equals: "completed" }],
    redaction: { secrets: 0, paths: 0, entries_dropped: 0 },
    ...over,
  })
}

const run = (c: any, extra: Record<string, unknown> = {}) =>
  runLive({ case: c, caseDir: root, k: 1, gizziArgv: [process.execPath, stub], trajectoryFor: traj, outDir: path.join(root, "out"), ...extra })

async function logged() {
  const text = await fs.readFile(log, "utf8").catch(() => "")
  return text.split("\n").filter(Boolean).map((l) => JSON.parse(l))
}

test("checks see the file the agent wrote, and the attempt passes", async () => {
  const r = await run(mkCase())
  expect(r.pass).toBe(true)
  const a = r.attempts[0]
  expect(a.checks.results[0]).toMatchObject({ type: "contains", pass: true })
  expect(a.trajectory.verifier).toBe(true)
  expect(a.cost_usd).toBe(0.25)
  expect(a.tokens).toBe(15)
  expect((await fs.readFile(a.events_path, "utf8")).includes("ses_fake")).toBe(true)
})

test("a failing contains check fails the attempt", async () => {
  const r = await run(mkCase({ checks: [{ type: "contains", path: "out.txt", text: "goodbye" }] }))
  expect(r.pass).toBe(false)
  expect(r.attempts[0].checks.results[0].pass).toBe(false)
  expect(r.attempts[0].trajectory.verifier).toBe(false)
})

test("a failing grader fails the attempt even when files pass", async () => {
  const r = await run(mkCase({ graders: [{ type: "status", equals: "failed" }] }))
  expect(r.attempts[0].checks.pass).toBe(true)
  expect(r.pass).toBe(false)
})

test("second turn continues the same session", async () => {
  await fs.rm(log, { force: true })
  await run(mkCase({ turns: [{ role: "user", text: "write:a" }, { role: "user", text: "write:hello b" }] }))
  const calls = await logged()
  expect(calls).toHaveLength(2)
  expect(calls[0].args).not.toContain("--session")
  expect(calls[1].args).toContain("--session")
  expect(calls[1].args[calls[1].args.indexOf("--session") + 1]).toBe("ses_fake")
})

test("timeout stops the run and the attempt fails", async () => {
  const r = await run(mkCase({ turns: [{ role: "user", text: "hang" }], checks: [] }), { timeoutMs: 700 })
  expect(r.attempts[0].pass).toBe(false)
  expect(r.attempts[0].trajectory.status).toBe("failed")
})

test("n_of_k rule", async () => {
  expect(passRule({ n_of_k: 2 }, [true, false, true])).toBe(true)
  expect(passRule({ n_of_k: 2 }, [true, false, false])).toBe(false)
  expect(passRule("all_of_k", [true, true])).toBe(true)
  expect(passRule("all_of_k", [true, false])).toBe(false)
})

test("n_of_k over real attempts: stub passes only on odd attempts", async () => {
  // The check wants a file that only exists when the stub wrote it; the counter file flips the prompt.
  const counter = path.join(root, "count")
  await fs.writeFile(counter, "0")
  let n = 0
  const c = mkCase({ k: 3, pass_rule: { n_of_k: 2 }, checks: [{ type: "contains", path: "out.txt", text: "hello" }] })
  // Flip the case text per attempt by wrapping the stub: attempts 1 and 3 write "hello", attempt 2 writes "nope".
  const wrapper = path.join(root, "wrap.js")
  await fs.writeFile(
    wrapper,
    `const fs=require("fs");const f=${JSON.stringify(counter)};const n=Number(fs.readFileSync(f,"utf8"))+1;fs.writeFileSync(f,String(n));
const a=process.argv.slice(2);if(a[0]!=="export"&&n===2)a[a.length-1]="write:nope";
process.argv=[process.argv[0],${JSON.stringify(stub)},...a];require(${JSON.stringify(stub)})`,
  )
  const r = await runLive({ case: c, caseDir: root, k: 3, gizziArgv: [process.execPath, wrapper], trajectoryFor: traj, outDir: path.join(root, "out2") })
  void n
  expect(r.attempts.map((a) => a.pass)).toEqual([true, false, true])
  expect(r.pass).toBe(true)
  const strict = await runLive({ case: { ...c, pass_rule: "all_of_k" }, caseDir: root, k: 3, gizziArgv: [process.execPath, wrapper], trajectoryFor: traj, outDir: path.join(root, "out3") })
  expect(strict.pass).toBe(false)
})

test("effects block passes the blocking config; allow passes none", async () => {
  await fs.rm(log, { force: true })
  await run(mkCase({ effects: "block" }))
  await run(mkCase({ effects: "allow" }))
  const [blocked, allowed] = await logged()
  expect(JSON.parse(blocked.config)).toEqual(BLOCK_EFFECTS_CONFIG)
  expect(blocked.config).toContain("mcp__*")
  expect(allowed.config).toBeNull()
})

test("effects default is block", async () => {
  expect(mkCase().effects).toBe("block")
})

// ── runChecks ───────────────────────────────────────────────────────────────
async function dirs(files: Record<string, string>, baseline: Record<string, string> = files) {
  const d = await fs.mkdtemp(path.join(root, "chk-"))
  const b = path.join(d, "b")
  const w = path.join(d, "w")
  for (const [dir, set] of [[w, files], [b, baseline]] as const) {
    for (const [rel, body] of Object.entries(set)) {
      await fs.mkdir(path.dirname(path.join(dir, rel)), { recursive: true })
      await fs.writeFile(path.join(dir, rel), body)
    }
  }
  await fs.mkdir(w, { recursive: true })
  return { w, b }
}

test("contains: any and ignore_case", async () => {
  const { w, b } = await dirs({ "a.txt": "Hello World" })
  const ok = await runChecks(w, [
    { type: "contains", path: "a.txt", text: "", any: ["nope", "WORLD"], ignore_case: true },
    { type: "contains", path: "a.txt", text: "hello", ignore_case: true },
  ], b)
  expect(ok.pass).toBe(true)
  const bad = await runChecks(w, [
    { type: "contains", path: "a.txt", text: "", any: ["nope", "WORLD"] },
    { type: "contains", path: "a.txt", text: "hello" },
  ], b)
  expect(bad.results.map((r) => r.pass)).toEqual([false, false])
})

test("absent: any, ignore_case, and missing file", async () => {
  const { w, b } = await dirs({ "a.txt": "Hello World" })
  const r = await runChecks(w, [
    { type: "absent", path: "a.txt", text: "xyz", any: ["abc"] },
    { type: "absent", path: "a.txt", text: "", any: ["WORLD"], ignore_case: true },
    { type: "absent", path: "a.txt", text: "", any: ["WORLD"] },
    { type: "absent", path: "gone.txt", text: "x" },
  ], b)
  expect(r.results.map((x) => x.pass)).toEqual([true, false, true, true])
})

test("exists and path escape", async () => {
  const { w, b } = await dirs({ "a.txt": "x" })
  const r = await runChecks(w, [
    { type: "exists", path: "a.txt" },
    { type: "exists", path: "b.txt" },
    { type: "exists", path: "../b/a.txt" },
  ], b)
  expect(r.results.map((x) => x.pass)).toEqual([true, false, false])
  expect(r.results[2].detail).toContain("escapes")
})

test("unchanged compares with the baseline copy", async () => {
  const { w, b } = await dirs({ "same.txt": "1", "edit.txt": "2" }, { "same.txt": "1", "edit.txt": "orig" })
  const r = await runChecks(w, [
    { type: "unchanged", path: "same.txt" },
    { type: "unchanged", path: "edit.txt" },
  ], b)
  expect(r.results.map((x) => x.pass)).toEqual([true, false])
})

test("tests: exit 0 passes, exit 1 fails, files are written first", async () => {
  const { w, b } = await dirs({})
  const r = await runChecks(w, [
    { type: "tests", command: "test \"$(cat t/in.txt)\" = ok", files: { "t/in.txt": "ok" } },
    { type: "tests", command: "exit 1" },
  ], b)
  expect(r.results.map((x) => x.pass)).toEqual([true, false])
  expect(r.pass).toBe(false)
  expect(r.results[1].detail).toContain("exit 1")
})

test("tests: timeout fails", async () => {
  const { w, b } = await dirs({})
  const r = await runChecks(w, [{ type: "tests", command: "sleep 5" }], b, { testsTimeoutMs: 200 })
  expect(r.results[0].pass).toBe(false)
  expect(r.results[0].detail).toContain("timed out")
})
