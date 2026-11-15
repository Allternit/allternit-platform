/**
 * The mission's orchestrator: the brain that plans milestones and decides on
 * fix tracks, talked to through an ordinary engine session.
 *
 * Every orchestrator turn is a `gizzi run --format json` against the engine
 * (attached to the one serving on this computer when there is one: Desktop's,
 * or `gizzi serve`), continuing the same session each time. So the session
 * holds the whole mission conversation and Remote Control (Desktop, web,
 * phone) shows it like any session; whatever the person writes there is in
 * the orchestrator's context on its next turn. The runner labels the session
 * with `mission` ("Mission · M2/5") and the glance line.
 *
 * With `--on <runtime>` the plan and fix turns run on that paired computer
 * instead, through the cloud relay (`turnOnRuntime`), continuing one session
 * there (`m.on.sessionID`); workers, validation and landing stay on this
 * computer. When the runtime isn't paired, is offline or the relay can't be
 * used, the turn fails with that reason: it never quietly runs here instead.
 */
import { spawn, type ChildProcess } from "node:child_process"
import { isInBundledMode } from "@/shared/utils/bundledMode"
import type { Mission } from "@/cli/mission/state"
import { label } from "@/cli/mission/state"
import { findRuntime, patchRuntimeSession, relayKeyProblem, turnOnRuntime, type Runtime } from "@/cli/work/api"

export function engineURL(): string {
  return (process.env.GIZZI_ENGINE_URL || "http://127.0.0.1:4096").replace(/\/+$/, "")
}

/** The engine serving on this computer, or undefined (the turn then runs in-process). */
export async function servingEngine(): Promise<string | undefined> {
  if (process.env.GIZZI_MISSION_IN_PROCESS === "1") return undefined
  const url = engineURL()
  try {
    const res = await fetch(`${url}/health`, { signal: AbortSignal.timeout(1500) })
    return res.ok ? url : undefined
  } catch {
    return undefined
  }
}

/** How to run this same gizzi again (compiled binary, or bun + the entry file with its flags). */
export function selfArgv(): string[] {
  const override = process.env.GIZZI_SELF?.trim()
  if (override) return override.split(/\s+/)
  if (isInBundledMode()) return [process.execPath]
  return [process.execPath, ...process.execArgv, process.argv[1]!]
}

/**
 * Child processes the mission still has running (orchestrator turns and
 * validation commands); `killChildren()` ends them when the runner stops.
 */
const children = new Set<ChildProcess>()

export function trackChild(child: ChildProcess): void {
  children.add(child)
  child.once("close", () => children.delete(child))
}

export function killChildren(): void {
  for (const c of children) c.kill("SIGTERM")
  children.clear()
}

// ---------------------------------------------------------------- --on

/** The paired-runtime calls `--on` makes; tests swap in a fake with `setRuntimeRelay`. */
export interface RuntimeRelay {
  findRuntime(key: string): Promise<{ runtime: Runtime } | { error: string }>
  /** Why the relay can't be used from here (no API key), or undefined. */
  keyProblem(): string | undefined
  /** One turn that waits for the reply (continuing `sessionID` when given). */
  turn(runtimeId: string, prompt: string, opts: { sessionID?: string; model?: string; title?: string }): Promise<{ sessionID: string; web: string; text: string }>
  /** PATCH the runtime engine's session record (`mission`, `glance`). */
  patchSession(runtimeId: string, sessionID: string, body: unknown): Promise<void>
}

const realRelay: RuntimeRelay = {
  findRuntime: (key) => findRuntime(key),
  keyProblem: relayKeyProblem,
  turn: (id, prompt, opts) => turnOnRuntime(id, prompt, opts),
  patchSession: (id, sessionID, body) => patchRuntimeSession(id, sessionID, body),
}
let relay: RuntimeRelay = realRelay

/** Use `r` for every `--on` call (undefined: the real relay again); returns the one it replaces. */
export function setRuntimeRelay(r?: RuntimeRelay): RuntimeRelay {
  const prev = relay
  relay = r ?? realRelay
  return prev
}

/**
 * The runtime `m.on` names, checked: paired to this account, online (or a
 * cloud computer the relay wakes), and a relay key to reach it. Returns
 * `m.on` with its `id` and `name` filled in, or the error to show. A mission
 * without --on resolves to `{}`.
 */
export async function resolveOn(m: Pick<Mission, "on">): Promise<{ on?: NonNullable<Mission["on"]> } | { error: string }> {
  if (!m.on) return {}
  const key = m.on.runtime
  const found = await relay.findRuntime(m.on.id ?? key)
  const here = "Nothing ran on this computer; start the mission without --on to run it here."
  if ("error" in found) return { error: `--on ${key}: ${found.error} ${here}` }
  const rt = found.runtime
  if (rt.status !== "online" && !rt.wakeable) return { error: `--on ${key}: ${rt.name} is ${rt.status}, so the orchestrator can't run there. Bring it online and resume, or ${here.charAt(0).toLowerCase()}${here.slice(1)}` }
  const key2 = relay.keyProblem()
  if (key2) return { error: `--on ${key}: ${key2}` }
  return { on: { ...m.on, id: rt.id, name: rt.name } }
}

/**
 * Why `m` can't run as asked (its `--on` runtime isn't paired, is offline, or
 * the relay can't be reached from here), or undefined when it can. Checked up
 * front so a mission never falls back to running on this computer.
 */
export async function onUnsupported(m: Pick<Mission, "on">): Promise<string | undefined> {
  const r = await resolveOn(m)
  return "error" in r ? r.error : undefined
}

export interface TurnResult {
  /** The orchestrator's session on this computer's engine (not set for --on). */
  sessionID?: string
  text: string
  error?: string
  /** With --on: `m.on` with the session on that computer and where to follow it; the caller records it. */
  on?: NonNullable<Mission["on"]>
}

/** The --on turn: on the paired runtime, continuing its session there. */
async function runtimeTurn(m: Pick<Mission, "brain" | "goal" | "on">, prompt: string): Promise<TurnResult> {
  const r = await resolveOn(m)
  if ("error" in r) return { text: "", error: r.error }
  const on = r.on!
  try {
    const t = await relay.turn(on.id!, prompt, { sessionID: on.sessionID, model: m.brain, title: `Mission · ${m.goal.slice(0, 60)}` })
    return { text: t.text, on: { ...on, sessionID: t.sessionID, web: t.web } }
  } catch (err) {
    return { text: "", error: `on ${on.name ?? on.runtime}: ${(err as Error)?.message ?? String(err)}`, on }
  }
}

/** One orchestrator turn; resolves with the session id and the reply text. */
export function orchestratorTurn(
  m: Pick<Mission, "brain" | "goal" | "orchestratorSession" | "root" | "on">,
  prompt: string,
  opts: { engine?: string; cwd?: string; timeoutMs?: number } = {},
): Promise<TurnResult> {
  if (m.on) return runtimeTurn(m, prompt)
  const [cmd, ...pre] = selfArgv()
  const args = [...pre, "run", "--format", "json", "-m", m.brain]
  if (opts.engine) args.push("--attach", opts.engine, "--dir", opts.cwd ?? m.root)
  if (m.orchestratorSession) args.push("--session", m.orchestratorSession)
  else args.push("--title", `Mission · ${m.goal.slice(0, 60)}`)
  args.push(prompt)
  return new Promise((resolve) => {
    const child = spawn(cmd!, args, { cwd: opts.cwd ?? m.root, stdio: ["ignore", "pipe", "pipe"], env: process.env })
    trackChild(child)
    let out = ""
    let err = ""
    const timer = setTimeout(() => child.kill("SIGTERM"), opts.timeoutMs ?? 15 * 60_000)
    child.stdout.on("data", (d) => (out += d))
    child.stderr.on("data", (d) => (err += d))
    child.on("error", (e) => (err += `\n${e.message}`))
    child.on("close", (code) => {
      clearTimeout(timer)
      const texts = new Map<string, string>()
      let sessionID: string | undefined
      let error: string | undefined
      for (const line of out.split("\n")) {
        if (!line.startsWith("{")) continue
        try {
          const ev = JSON.parse(line)
          sessionID ??= ev.sessionID
          if (ev.type === "text" && ev.part?.id) texts.set(ev.part.id, ev.part.text ?? "")
          if (ev.type === "error") error = ev.error?.data?.message ?? ev.error?.name ?? "the orchestrator turn failed"
        } catch {
          // not an event line
        }
      }
      const text = [...texts.values()].join("\n").trim()
      if (!text && !error) error = code === 0 ? "the orchestrator replied with nothing" : (err.trim().split("\n").slice(-3).join(" ") || `gizzi run exited ${code}`)
      resolve({ sessionID, text, error })
    })
  })
}

/**
 * The plan turn's prompt. `files` (the repo's tracked files) is given when
 * the orchestrator runs on another computer (--on) and can't open the repo.
 */
export function planPrompt(goal: string, root: string, maxWorkers: number, files?: string[]): string {
  const remote = files
    ? [
        `You are on a different computer from the repository (it is at ${root} on the computer that started the mission), so you can't open its files. Its tracked files${files.length >= REMOTE_FILES_MAX ? ` (the first ${REMOTE_FILES_MAX})` : ""}:`,
        ...files.map((f) => `  ${f}`),
        "",
        "Write a scoped plan from the goal and these paths. Workers run on that computer, in the repo, and can read everything.",
      ]
    : ["Write a scoped plan. Look at the repo as much as you need, but do NOT change any files: workers do the work."]
  return [
    `You are the orchestrator of a mission in the repository at ${root}.`,
    `Goal: ${goal}`,
    "",
    ...remote,
    "Split the goal into 1-5 milestones, in order. Each milestone has:",
    `- tracks: 1-${maxWorkers} work items that can run at the same time, each in its own git worktree, by a worker with fresh context that sees only its task. Tracks in one milestone must not edit the same files. Each task is self-contained: name the files, say what to build, and how the worker checks it.`,
    "- validate: one shell command, run from the repo root after the tracks are merged, that exits 0 only when the milestone is done (the repo's tests, a typecheck, or a check you name). It must work in this repo as it is.",
    "",
    'Reply with ONLY this JSON, no prose: {"milestones":[{"title":"...","tracks":[{"title":"...","task":"..."}],"validate":"..."}]}',
  ].join("\n")
}

/** Tracked files listed in a --on plan prompt, at most. */
export const REMOTE_FILES_MAX = 400

/**
 * `changes` is a `git diff --stat` of the mission branch against where it
 * started, so the orchestrator sees what the workers changed even when the
 * mission checkout is outside the folder its session can read.
 */
export function fixPrompt(m: Mission, attempt: number, changes?: string): string {
  const ms = m.milestones[m.current]!
  return [
    `Mission milestone ${ms.id} ("${ms.title}") failed validation (attempt ${attempt} of ${m.maxAttempts + 1}).`,
    `Command: ${ms.validate}`,
    "Output (tail):",
    "```",
    (ms.lastFailure ?? "").slice(-3000),
    "```",
    ...(m.on
      ? [`The mission's merged work is on branch ${m.branch} on the computer that started it, not on yours, so the output above is what you have to go on.`]
      : m.worktree && m.worktree !== m.root
        ? [`The mission's merged work is checked out at ${m.worktree} (branch ${m.branch}); look there, not at ${m.root}.`]
        : []),
    ...(changes?.trim() ? ["Changes on the mission branch so far (git diff --stat):", "```", changes.trim().slice(-2000), "```"] : []),
    "Do NOT change files yourself. Either give one fix track for a worker with fresh context (it starts from the merged work on the mission branch), or stop if a person has to decide.",
    'Reply with ONLY JSON: {"fix":{"title":"...","task":"..."}} or {"stop":"<why a person is needed>"}',
  ].join("\n")
}

/**
 * Label the orchestrator session for Remote Control: `mission` + the glance
 * line. Best effort. With --on it is the same engine PATCH, sent through the
 * relay to the session on that computer; a failure there is written to the
 * runner's log (stderr), not thrown.
 */
export async function labelSession(m: Mission, log: (line: string) => void = (l) => process.stderr.write(`${l}\n`)): Promise<void> {
  const total = m.milestones.length
  const body = {
    mission: { id: m.id, goal: m.goal, milestone: Math.min(m.current + 1, total), total, status: m.status, label: label(m), at: Date.now() },
    glance: `${label(m)} · ${m.goal.slice(0, 80)}`,
  }
  if (m.on) {
    if (!m.on.id || !m.on.sessionID) return
    try {
      await relay.patchSession(m.on.id, m.on.sessionID, body)
    } catch (err) {
      log(`couldn't label the orchestrator session on ${m.on.name ?? m.on.runtime}: ${(err as Error)?.message ?? String(err)}`)
    }
    return
  }
  if (!m.orchestratorSession || !m.engineURL) return
  await fetch(`${m.engineURL}/session/${encodeURIComponent(m.orchestratorSession)}`, {
    method: "PATCH",
    headers: { "content-type": "application/json", "x-gizzi-directory": m.root },
    body: JSON.stringify(body),
    signal: AbortSignal.timeout(5000),
  }).catch(() => {})
}
