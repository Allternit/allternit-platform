import { afterEach, describe, expect, test } from "bun:test"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import type { LandStep, StartOptions, SwarmRecord } from "../../src/cli/factory/swarm"
import type { CommandResult, MissionDeps } from "../../src/cli/mission/actions"
import { runMission } from "../../src/cli/mission/runner"
import { advance, newMilestones, type Mission, type PlanInput } from "../../src/cli/mission/state"
import { logFile, readMission, writeMission } from "../../src/cli/mission/store"

// Fakes stand in for the edges only: the Factory engine (swarms), the
// orchestrator's model turn, git and the shell. The mission record, its
// store and the state machine are the real ones.

const roots: string[] = []
afterEach(() => {
  for (const r of roots.splice(0)) fs.rmSync(r, { recursive: true, force: true })
})

function tmpRoot(): string {
  const r = fs.mkdtempSync(path.join(os.tmpdir(), "gizzi-mission-"))
  roots.push(r)
  return r
}

const PLAN: PlanInput = {
  milestones: [
    { title: "Core", tracks: [{ title: "a", task: "do a" }, { title: "b", task: "do b" }], validate: "check-core" },
    { title: "Docs", tracks: [{ title: "docs", task: "write docs" }], validate: "check-docs" },
  ],
}

function seed(root: string, plan = PLAN, over: Partial<Mission> = {}): Mission {
  const m: Mission = {
    id: "mi-run1",
    goal: "ship it",
    root,
    baseBranch: "main",
    branch: "mission/mi-run1",
    worktree: root,
    brain: "claude-cli/claude-sonnet-5",
    maxWorkers: 3,
    maxAttempts: 2,
    status: "awaiting_approval",
    milestones: newMilestones(plan),
    current: 0,
    createdAt: 1,
    log: [],
    ...over,
  }
  const running = advance(m, { type: "approved" }, 2)
  writeMission(running)
  return running
}

interface Fake {
  deps: MissionDeps
  swarms: Map<string, SwarmRecord>
  started: StartOptions[]
  validations: string[]
  prompts: string[]
  stopped: string[]
  branch: () => string
  branches: Set<string>
}

/**
 * `finishAfter`: polls before a swarm's workers finish (Infinity = never).
 * `validate`: exit code for each validation command run.
 * `land`: the outcome landing gives each worker (default merged).
 */
function fake(opts: { finishAfter?: number; validate?: (cmd: string, n: number) => number; land?: (rec: SwarmRecord, n: number) => LandStep["outcome"]; onObserve?: (rec: SwarmRecord) => void } = {}): Fake {
  const swarms = new Map<string, SwarmRecord>()
  const polls = new Map<string, number>()
  const started: StartOptions[] = []
  const validations: string[] = []
  const prompts: string[] = []
  const stopped: string[] = []
  const branches = new Set(["main"])
  let head = "main"
  let lands = 0
  const deps: MissionDeps = {
    async startSwarm(o) {
      started.push(o)
      const id = `sw-${swarms.size + 1}`
      const n = o.tasks?.length || o.workers
      const record: SwarmRecord = {
        id,
        task: o.task,
        brain: o.brain,
        harness: "claude",
        model: null,
        root: o.cwd,
        baseBranch: head,
        createdAt: 0,
        workers: Array.from({ length: n }, (_, i) => ({
          bot: `w${i + 1}`,
          address: `w${i + 1}@${id}`,
          agentId: `local:w${i + 1}-${id}`,
          slug: `w${i + 1}-${id}`,
          branch: `swarm/${id}/w${i + 1}`,
          workdir: `${o.cwd}-${id}-w${i + 1}`,
          task: o.tasks?.[i],
          startedAt: 0,
          taskSentAt: 0,
          phase: "working" as const,
        })),
      }
      swarms.set(id, record)
      return { record, plan: { id, root: o.cwd, baseBranch: head, harness: "claude", model: null, workers: [] } }
    },
    async observeSwarm(rec) {
      const cur = swarms.get(rec.id)!
      const n = (polls.get(rec.id) ?? 0) + 1
      polls.set(rec.id, n)
      opts.onObserve?.(cur)
      if (n > (opts.finishAfter ?? 1)) {
        const done = { ...cur, workers: cur.workers.map((w) => (w.phase === "working" ? { ...w, phase: "done" as const } : w)) }
        swarms.set(rec.id, done)
        return done
      }
      return cur
    },
    async stopWorker(rec, bot) {
      stopped.push(`${bot}@${rec.id}`)
      const cur = swarms.get(rec.id)!
      const next = { ...cur, workers: cur.workers.map((w) => (w.bot === bot ? { ...w, phase: "stopped" as const } : w)) }
      swarms.set(rec.id, next)
      return next
    },
    async landSwarm(rec) {
      lands++
      expect(rec.baseBranch).toBe(head) // landing merges into the branch the swarm started from
      const outcome = opts.land?.(rec, lands) ?? "merged"
      const steps = rec.workers.map((w) => ({ bot: w.bot, branch: w.branch, action: "merge" as const, reason: "", outcome, detail: outcome === "conflict" ? "CONFLICT (content)" : "abc123" }))
      swarms.set(rec.id, { ...rec, workers: rec.workers.map((w) => (outcome === "merged" ? { ...w, phase: "landed" as const } : w)) })
      return steps
    },
    readSwarm: (id) => swarms.get(id) ?? null,
    async orchestratorTurn(_m, prompt) {
      prompts.push(prompt)
      return { sessionID: "ses_orch", text: '```json\n{"fix":{"title":"repair","task":"make the check pass"}}\n```' }
    },
    async labelSession() {},
    async exec(cmd): Promise<CommandResult> {
      validations.push(cmd)
      const code = opts.validate?.(cmd, validations.length) ?? 0
      return { code, output: code ? `${"x".repeat(5000)}FAILED ${cmd}` : "ok" }
    },
    git(args) {
      if (args[0] === "rev-parse" && args[1] === "--abbrev-ref") return { code: 0, output: head }
      if (args[0] === "rev-parse" && args[1] === "--verify") return { code: branches.has(args[3]!.replace("refs/heads/", "")) ? 0 : 1, output: "" }
      if (args[0] === "branch") {
        branches.add(args[1]!)
        return { code: 0, output: "" }
      }
      if (args[0] === "switch") {
        if (!branches.has(args[1]!)) return { code: 128, output: `fatal: invalid reference: ${args[1]}` }
        head = args[1]!
        return { code: 0, output: "" }
      }
      return { code: 1, output: `unexpected git ${args.join(" ")}` }
    },
    now: () => 3,
  }
  return { deps, swarms, started, validations, prompts, stopped, branch: () => head, branches }
}

const run = (root: string, f: Fake, extra: Parameters<typeof runMission>[3] = {}) => runMission("mi-run1", root, f.deps, { pollMs: 0, handleSignals: false, ...extra })

describe("mission runner", () => {
  test("a two-milestone mission runs on its own branch to done", async () => {
    const root = tmpRoot()
    seed(root)
    const f = fake()
    const end = await run(root, f)

    const disk = readMission(root, "mi-run1")!
    expect(end.status).toBe("done")
    expect(disk.status).toBe("done")
    expect(disk.runnerPid).toBeUndefined()
    expect(disk.milestones.map((x) => x.status)).toEqual(["passed", "passed"])
    expect(disk.milestones.map((x) => x.swarms)).toEqual([["sw-1"], ["sw-2"]])
    expect(disk.milestones[0]!.workers).toEqual(["w1@sw-1", "w2@sw-1"])
    // The mission branch was made from main and checked out before any swarm started.
    expect(f.branch()).toBe("mission/mi-run1")
    expect(f.started.map((s) => [s.workers, s.cwd, s.brain])).toEqual([
      [2, root, "claude-cli/claude-sonnet-5"],
      [1, root, "claude-cli/claude-sonnet-5"],
    ])
    expect(f.started[0]!.tasks![1]).toContain("b: do b")
    expect(f.started[0]!.tasks![1]).toContain("check-core")
    expect(f.validations).toEqual(["check-core", "check-docs"])
    const log = fs.readFileSync(logFile(disk), "utf8")
    expect(log).toContain("created mission/mi-run1 from main")
    expect(log).toContain("M1: validating: check-core")
    expect(log).toContain("runner exits: done")
  })

  test("maxWorkers caps a swarm; the rest of the tracks follow in the next batch", async () => {
    const root = tmpRoot()
    seed(root, PLAN, { maxWorkers: 1 })
    const f = fake()
    expect((await run(root, f)).status).toBe("done")
    expect(f.started.map((s) => s.workers)).toEqual([1, 1, 1])
    expect(f.validations).toEqual(["check-core", "check-docs"])
  })

  test("a validation that always fails gets maxAttempts fix rounds, then stops to ask", async () => {
    const root = tmpRoot()
    seed(root, PLAN, { maxAttempts: 2 })
    const f = fake({ validate: () => 1 })
    const end = await run(root, f)

    const ms = readMission(root, "mi-run1")!.milestones[0]!
    expect(end.status).toBe("needs_you")
    expect(end.note).toContain("after 3 tries")
    expect(ms.status).toBe("failed")
    expect(ms.attempts).toBe(3)
    expect(ms.fixes.map((x) => x.title)).toEqual(["repair", "repair"])
    expect(f.prompts).toHaveLength(2)
    expect(f.started.map((s) => s.workers)).toEqual([2, 1, 1])
    expect(f.validations).toEqual(["check-core", "check-core", "check-core"])
    // The failure handed to the orchestrator is the tail of the output.
    expect(ms.lastFailure!.length).toBe(3000)
    expect(ms.lastFailure!.endsWith("FAILED check-core")).toBe(true)
    expect(end.orchestratorSession).toBe("ses_orch")
    expect(end.runnerPid).toBeUndefined()
  })

  test("a merge conflict counts as a failed attempt and goes to the orchestrator, not to validation", async () => {
    const root = tmpRoot()
    seed(root)
    const f = fake({ land: (_rec, n) => (n === 1 ? "conflict" : "merged") })
    const end = await run(root, f)
    expect(end.status).toBe("done")
    expect(f.prompts[0]).toContain("swarm/sw-1/w1: CONFLICT (content)")
    // Validation first ran after the fix landed.
    expect(f.validations).toEqual(["check-core", "check-docs"])
    expect(end.milestones[0]!.attempts).toBe(1)
  })

  test("a stop mid-swarm stops the workers and keeps the state; resume watches the same swarm", async () => {
    const root = tmpRoot()
    seed(root)
    const ac = new AbortController()
    let looks = 0
    const f = fake({ finishAfter: Infinity, onObserve: () => (++looks === 2 ? ac.abort() : undefined) })
    const end = await run(root, f, { signal: ac.signal })

    const disk = readMission(root, "mi-run1")!
    expect(end.status).toBe("stopped")
    expect(disk.status).toBe("stopped")
    expect(disk.runnerPid).toBeUndefined()
    expect(disk.milestones[0]!.status).toBe("working")
    expect(disk.milestones[0]!.swarms).toEqual(["sw-1"])
    expect(f.stopped).toEqual(["w1@sw-1", "w2@sw-1"])
    expect(f.swarms.get("sw-1")!.workers.map((w) => w.phase)).toEqual(["stopped", "stopped"])

    // Resume: the in-flight swarm is observed and landed again, not restarted.
    writeMission(advance(disk, { type: "resume" }))
    const f2 = fake()
    for (const [k, v] of f.swarms) f2.swarms.set(k, v)
    f2.branches.add("mission/mi-run1")
    const again = await run(root, f2)
    expect(again.status).toBe("done")
    expect(f2.started).toHaveLength(1) // only M2's swarm
    expect(again.milestones[0]!.swarms).toEqual(["sw-1"])
  })

  test("a branch it can't switch to stops to ask instead of starting workers", async () => {
    const root = tmpRoot()
    seed(root)
    const f = fake()
    const git = f.deps.git
    f.deps.git = (args, cwd) => (args[0] === "switch" ? { code: 1, output: "error: Your local changes would be overwritten" } : git(args, cwd))
    const end = await run(root, f)
    expect(end.status).toBe("needs_you")
    expect(end.note).toContain("local changes would be overwritten")
    expect(f.started).toHaveLength(0)
  })

  test("a mission that isn't running is left alone", async () => {
    const root = tmpRoot()
    const m = seed(root)
    writeMission(advance(m, { type: "stop" }))
    const f = fake()
    expect((await run(root, f)).status).toBe("stopped")
    expect(f.started).toHaveLength(0)
  })
})
