// Live eval runner: restore the workspace, run the real agent, check files, grade.
import fs from "fs/promises"
import os from "os"
import path from "path"
import { spawn } from "child_process"
import { runChecks, type CheckResult } from "./checks"
import { gradeAll } from "./graders"
import { restoreTo } from "./workspace"
import type { Case, GradeResult, Trajectory } from "./schema"

export const LIVE_TIMEOUT_MS = 8 * 60_000

/**
 * D8: a live run may only use file and shell tools inside its temp dir.
 * Web tools and every MCP tool are denied (`mcp__*` is how MCP tools are named).
 * MCP servers that a user config names can also be turned off with
 * `mcp.<name>.enabled = false`; runLive adds those at run time when it can see them.
 */
export const BLOCK_EFFECTS_CONFIG = {
  permission: {
    webfetch: "deny",
    websearch: "deny",
    codesearch: "deny",
    "mcp__*": "deny",
  },
} as const

export interface Attempt {
  n: number
  trajectory: Trajectory
  checks: { pass: boolean; results: CheckResult[] }
  grades: GradeResult[]
  pass: boolean
  cost_usd: number
  tokens: number
  ms: number
  events_path: string
}

export interface LiveOptions {
  case: Case
  caseDir: string
  k: number
  model?: string
  timeoutMs?: number
  gizziArgv?: string[]
  signal?: AbortSignal
  /** Where event logs go. Default: a new temp dir. */
  outDir?: string
  /** Test hook. Default reads the session trace. */
  trajectoryFor?: (sessionID: string) => Promise<Trajectory> | Trajectory
  testsTimeoutMs?: number
}

export function passRule(rule: Case["pass_rule"], passes: boolean[]): boolean {
  const n = passes.filter(Boolean).length
  return rule === "all_of_k" ? n === passes.length : n >= rule.n_of_k
}

function childEnv(c: Case): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = { ...process.env }
  if (c.effects !== "allow") env.GIZZI_CONFIG_CONTENT = JSON.stringify(BLOCK_EFFECTS_CONFIG)
  else delete env.GIZZI_CONFIG_CONTENT
  return env
}

interface Spawned {
  stdout: string
  timedOut: boolean
  code: number | null
}

function runGizzi(argv: string[], cwd: string, env: NodeJS.ProcessEnv, timeoutMs: number, eventsFile: string, signal?: AbortSignal): Promise<Spawned> {
  return new Promise((resolve) => {
    const [cmd, ...args] = argv
    const child = spawn(cmd!, args, { cwd, env, stdio: ["ignore", "pipe", "pipe"] })
    let stdout = ""
    let timedOut = false
    child.stdout.on("data", (b: Buffer) => {
      stdout += b.toString()
    })
    child.stderr.on("data", () => {})
    const stop = () => {
      child.kill("SIGTERM")
      setTimeout(() => child.kill("SIGKILL"), 10_000).unref()
    }
    const timer = setTimeout(() => {
      timedOut = true
      stop()
    }, timeoutMs)
    signal?.addEventListener("abort", stop, { once: true })
    const finish = async (code: number | null) => {
      clearTimeout(timer)
      await fs.appendFile(eventsFile, stdout).catch(() => {})
      resolve({ stdout, timedOut, code })
    }
    child.on("error", (e) => {
      stdout += JSON.stringify({ type: "error", error: String(e) }) + "\n"
      void finish(null)
    })
    child.on("close", (code) => void finish(code))
  })
}

function sessionIdOf(stdout: string): string | undefined {
  for (const line of stdout.split("\n")) {
    try {
      const id = JSON.parse(line)?.sessionID
      if (typeof id === "string") return id
    } catch {}
  }
}

async function exportCost(argv: string[], sessionID: string, env: NodeJS.ProcessEnv): Promise<{ cost: number; tokens: number } | undefined> {
  const out = await new Promise<string>((resolve) => {
    const [cmd, ...args] = argv
    const child = spawn(cmd!, [...args, "export", sessionID], { env, stdio: ["ignore", "pipe", "ignore"] })
    let s = ""
    child.stdout.on("data", (b: Buffer) => (s += b.toString()))
    const t = setTimeout(() => child.kill("SIGKILL"), 120_000)
    child.on("error", () => resolve(""))
    child.on("close", () => {
      clearTimeout(t)
      resolve(s)
    })
  })
  try {
    const data = JSON.parse(out)
    let cost = 0
    let tokens = 0
    for (const m of data.messages ?? []) {
      const info = m.info ?? {}
      if (info.role !== "assistant") continue
      cost += Number(info.cost) || 0
      const t = info.tokens ?? {}
      tokens += (t.input ?? 0) + (t.output ?? 0) + (t.reasoning ?? 0) + (t.cache?.read ?? 0) + (t.cache?.write ?? 0)
    }
    return { cost, tokens }
  } catch {
    return undefined
  }
}

const emptyTrajectory = (status: string, ms: number): Trajectory => ({
  status,
  verifier: null,
  steps: [],
  effects: [],
  cost_usd: 0,
  tokens: 0,
  latency_ms: ms,
})

export async function runLive(opts: LiveOptions): Promise<{ attempts: Attempt[]; pass: boolean }> {
  const c = opts.case
  const argv = opts.gizziArgv ?? ["gizzi"]
  const timeoutMs = opts.timeoutMs ?? LIVE_TIMEOUT_MS
  const outDir = opts.outDir ?? (await fs.mkdtemp(path.join(os.tmpdir(), "gizzi-eval-out-")))
  const attempts: Attempt[] = []

  for (let n = 1; n <= opts.k; n++) {
    if (opts.signal?.aborted) break
    const base = await fs.mkdtemp(path.join(os.tmpdir(), "gizzi-eval-ws-"))
    const work = path.join(base, "work")
    const baseline = path.join(base, "baseline")
    const attemptDir = path.join(outDir, `attempt-${n}`)
    await fs.mkdir(attemptDir, { recursive: true })
    const eventsFile = path.join(attemptDir, "events.jsonl")
    try {
      await restoreTo(opts.caseDir, c.workspace, work)
      await restoreTo(opts.caseDir, c.workspace, baseline)
      const env = childEnv(c)
      const t0 = Date.now()
      let sessionID: string | undefined
      let timedOut = false
      for (const [i, turn] of c.turns.entries()) {
        const left = timeoutMs - (Date.now() - t0)
        if (left <= 0) {
          timedOut = true
          break
        }
        const args = [...argv, "run", "--format", "json", "--dir", work]
        if (i > 0 && sessionID) args.push("--session", sessionID)
        if (opts.model) args.push("--model", opts.model)
        args.push(turn.text)
        const r = await runGizzi(args, work, env, left, eventsFile, opts.signal)
        sessionID ??= sessionIdOf(r.stdout)
        if (r.timedOut) timedOut = true
        if (r.timedOut || opts.signal?.aborted || !sessionID) break
      }
      const ms = Date.now() - t0

      const checks = await runChecks(work, c.checks, baseline, { testsTimeoutMs: opts.testsTimeoutMs })
      let trajectory: Trajectory
      try {
        if (!sessionID) throw new Error("no session id in the event stream")
        const get = opts.trajectoryFor ?? (async (id: string) => (await import("./trajectory")).recordedTrajectory(id))
        trajectory = await get(sessionID)
      } catch {
        trajectory = emptyTrajectory("failed", ms)
      }
      if (timedOut) trajectory = { ...trajectory, status: "failed" }
      trajectory = { ...trajectory, verifier: checks.pass }
      if (sessionID) {
        const cost = await exportCost(argv, sessionID, env)
        if (cost) trajectory = { ...trajectory, cost_usd: cost.cost, tokens: cost.tokens }
      }
      const graded = gradeAll(trajectory as any, c.graders as any)
      attempts.push({
        n,
        trajectory,
        checks,
        grades: graded.grades,
        pass: graded.pass && checks.pass && !timedOut,
        cost_usd: trajectory.cost_usd,
        tokens: trajectory.tokens,
        ms,
        events_path: eventsFile,
      })
    } finally {
      await fs.rm(base, { recursive: true, force: true }).catch(() => {})
    }
  }
  const pass = attempts.length === opts.k && passRule(c.pass_rule, attempts.map((a) => a.pass))
  return { attempts, pass }
}
