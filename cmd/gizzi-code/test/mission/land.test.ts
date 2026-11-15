import { afterEach, beforeEach, describe, expect, setDefaultTimeout, test } from "bun:test"
import crypto from "node:crypto"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { spawnSync } from "node:child_process"
import type { SwarmRecord } from "../../src/cli/factory/swarm"
import { approveAndSpawn, landAndRecord, newMission, planMission, prepareResume, realDeps, type MissionDeps } from "../../src/cli/mission/actions"
import { setRuntimeRelay, type RuntimeRelay } from "../../src/cli/mission/orchestrator"
import { runMission } from "../../src/cli/mission/runner"
import { advance, type Mission, type PlanInput } from "../../src/cli/mission/state"
import { readMission, writeMission } from "../../src/cli/mission/store"

// Real git, real worktrees, real validation commands and the real record
// store; fakes only for the Factory engine (swarms) and the model turn. A
// fake swarm "lands" by committing a file in the tree it started from, the
// way a merged worker would.

// Each test spawns a few dozen git processes; slow when the machine is busy.
setDefaultTimeout(30_000)

const roots: string[] = []
/** Mission checkouts go here (ALLTERNIT_MISSION_CHECKOUTS), never into the real ~/.allternit. */
let checkouts = ""
const savedCheckouts = process.env.ALLTERNIT_MISSION_CHECKOUTS
beforeEach(() => {
  checkouts = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "gizzi-mco-")))
  roots.push(checkouts)
  process.env.ALLTERNIT_MISSION_CHECKOUTS = checkouts
})
afterEach(() => {
  if (savedCheckouts === undefined) delete process.env.ALLTERNIT_MISSION_CHECKOUTS
  else process.env.ALLTERNIT_MISSION_CHECKOUTS = savedCheckouts
  for (const r of roots.splice(0)) fs.rmSync(r, { recursive: true, force: true })
})

function sh(cwd: string, ...args: string[]): string {
  const r = spawnSync("git", args, { cwd, encoding: "utf8" })
  if (r.status !== 0) throw new Error(`git ${args.join(" ")}: ${r.stderr}`)
  return r.stdout.trim()
}

function repo(): string {
  const r = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "gizzi-mland-")))
  roots.push(r)
  sh(r, "init", "-q", "-b", "main")
  sh(r, "config", "user.email", "t@example.com")
  sh(r, "config", "user.name", "t")
  fs.writeFileSync(path.join(r, "README.md"), "hi\n")
  sh(r, "add", ".")
  sh(r, "commit", "-q", "-m", "init")
  return r
}

const PLAN: PlanInput = {
  milestones: [
    { title: "One", tracks: [{ title: "a", task: "make a.txt" }], validate: "test -f a.txt" },
    { title: "Two", tracks: [{ title: "b", task: "make b.txt" }], validate: "test -f b.txt && test -f a.txt" },
  ],
}

/** Swarms that finish at once; landing commits `<track>.txt` in the swarm's tree. `onLand` runs after each. */
function deps(onLand: (n: number) => void = () => {}): MissionDeps & { cwds: string[] } {
  const real = realDeps()
  const swarms = new Map<string, SwarmRecord>()
  const cwds: string[] = []
  let lands = 0
  return {
    cwds,
    async startSwarm(o) {
      cwds.push(o.cwd)
      const id = `sw-${swarms.size + 1}`
      const name = /: (\w+)$/.exec(o.task)?.[1] ?? id
      const record = {
        id,
        task: o.task,
        brain: o.brain,
        harness: "claude",
        model: null,
        root: o.cwd,
        baseBranch: sh(o.cwd, "rev-parse", "--abbrev-ref", "HEAD"),
        createdAt: 0,
        workers: [{ bot: "w1", address: `w1@${id}`, agentId: "x", slug: name, branch: `swarm/${id}/w1`, workdir: o.cwd, startedAt: 0, taskSentAt: 0, phase: "done" as const }],
      } as SwarmRecord
      swarms.set(id, record)
      return { record, plan: { id, root: o.cwd, baseBranch: record.baseBranch, harness: "claude", model: null, workers: [] } }
    },
    async observeSwarm(rec) {
      return swarms.get(rec.id) ?? rec
    },
    async stopWorker(rec) {
      return rec
    },
    async landSwarm(rec) {
      const file = `${rec.workers[0]!.slug}.txt`
      fs.writeFileSync(path.join(rec.root, file), "x\n")
      sh(rec.root, "add", file)
      sh(rec.root, "commit", "-q", "-m", `land ${file}`)
      onLand(++lands)
      return [{ bot: "w1", branch: rec.workers[0]!.branch, action: "merge", reason: "", outcome: "merged", detail: "abc" }]
    },
    readSwarm: (id) => swarms.get(id) ?? null,
    async orchestratorTurn() {
      throw new Error("no model calls in this test")
    },
    async labelSession() {},
    exec: real.exec,
    git: real.git,
    now: () => Date.now(),
  }
}

/** A mission approved with PLAN and its checkout made, as `gizzi mission start --yes` leaves it (without the spawn). */
function approved(root: string, d: MissionDeps): Mission {
  const made = newMission({ goal: "two files", cwd: root, brain: "claude-cli/claude-sonnet-5", maxWorkers: 1 })
  if ("error" in made) throw new Error(made.error)
  const spawned: string[] = []
  const out = approveAndSpawn(made.mission, { plan: PLAN, deps: d, spawn: (m) => (spawned.push(m.id), undefined) })
  expect(out.error).toBeUndefined()
  expect(spawned).toEqual([made.mission.id])
  return out.mission
}

const run = (m: Mission, d: MissionDeps) => runMission(m.id, m.root, d, { pollMs: 0, handleSignals: false })

describe("mission worktree and landing", () => {
  test("work happens in the mission's own checkout and fast-forwards onto a clean base branch", async () => {
    const root = repo()
    const d = deps()
    const m = approved(root, d)
    // The checkout is outside the project, so the project's test runners and watchers never see it.
    const repoDir = path.join(checkouts, `${path.basename(root)}-${crypto.createHash("sha256").update(root).digest("hex").slice(0, 8)}`)
    expect(m.worktree).toBe(path.join(repoDir, m.id))
    expect(m.worktree.startsWith(root + path.sep)).toBe(false)
    expect(fs.existsSync(path.join(root, ".allternit", "missions", m.id))).toBe(false)
    expect(sh(m.worktree, "rev-parse", "--abbrev-ref", "HEAD")).toBe(`mission/${m.id}`)
    // The mission folder is excluded locally, so the person's status stays clean.
    expect(fs.readFileSync(path.join(root, ".git", "info", "exclude"), "utf8")).toContain(".allternit/missions/")
    expect(sh(root, "status", "--porcelain")).toBe("")

    const end = await run(m, d)
    expect(end.status).toBe("done")
    expect(end.landed).toBe(true)
    expect(end.landNote).toContain("fast-forwarded main")
    expect(d.cwds).toEqual([m.worktree, m.worktree])
    // Swarm workers get `<the tree they start from>-<swarm>-<worker>`: next to the mission's checkout, outside the project too.
    expect(`${d.cwds[0]}-sw-1-w1`.startsWith(repoDir + path.sep)).toBe(true)
    // The record and log stay with the project.
    expect(fs.existsSync(path.join(root, ".allternit", "missions", `${m.id}.json`))).toBe(true)
    // The person's checkout never left main, and main now has both milestones' work.
    expect(sh(root, "rev-parse", "--abbrev-ref", "HEAD")).toBe("main")
    expect(fs.existsSync(path.join(root, "a.txt")) && fs.existsSync(path.join(root, "b.txt"))).toBe(true)
    // The checkout is dropped after landing; the branch stays.
    expect(fs.existsSync(m.worktree)).toBe(false)
    expect(fs.existsSync(repoDir)).toBe(false)
    expect(sh(root, "worktree", "list")).not.toContain(m.worktree)
    expect(sh(root, "rev-parse", "--verify", `refs/heads/mission/${m.id}`)).toMatch(/^[0-9a-f]{40}$/)
    expect(readMission(root, m.id)!.landed).toBe(true)
  })

  test("the engine's state folder doesn't keep a landed mission's checkout around; someone's file there does", async () => {
    // Regression: the Factory engine writes `.allternit/` (ledger, mail, teams) into the checkout the
    // workers run from, and `git worktree remove` refuses untracked files, so every landed mission left its checkout behind.
    const root = repo()
    let m!: Mission
    const d = deps((n) => {
      if (n !== 1) return
      fs.mkdirSync(path.join(m.worktree, ".allternit", "ledger"), { recursive: true })
      fs.writeFileSync(path.join(m.worktree, ".allternit", "ledger", "events.jsonl"), "{}\n")
    })
    m = approved(root, d)
    const end = await run(m, d)
    expect(end.landed).toBe(true)
    expect(fs.existsSync(m.worktree)).toBe(false)
    expect(sh(root, "worktree", "list")).not.toContain(m.worktree)

    const root2 = repo()
    let m2!: Mission
    const d2 = deps((n) => {
      if (n !== 1) return
      fs.mkdirSync(path.join(m2.worktree, ".allternit"), { recursive: true })
      fs.writeFileSync(path.join(m2.worktree, ".allternit", "state"), "x\n")
      fs.writeFileSync(path.join(m2.worktree, "notes.txt"), "the person's notes\n")
    })
    m2 = approved(root2, d2)
    const end2 = await run(m2, d2)
    expect(end2.landed).toBe(true)
    expect(fs.readFileSync(path.join(m2.worktree, "notes.txt"), "utf8")).toBe("the person's notes\n")
  })

  test("a dirty checkout is never landed into; it is done and ready to land, and land works once it's clean", async () => {
    const root = repo()
    const d = deps((n) => n === 2 && fs.writeFileSync(path.join(root, "README.md"), "edited by the person\n"))
    const end = await run(approved(root, d), d)
    expect(end.status).toBe("done")
    expect(end.landed).toBe(false)
    expect(end.landNote).toContain("uncommitted changes")
    expect(end.log.at(-1)!.text).toContain(`ready to land: gizzi mission land ${end.id}`)
    expect(fs.existsSync(path.join(root, "a.txt"))).toBe(false)
    expect(fs.readFileSync(path.join(root, "README.md"), "utf8")).toBe("edited by the person\n")

    sh(root, "checkout", "--", "README.md")
    const landed = landAndRecord(readMission(root, end.id)!, realDeps().git)
    expect(landed.landed).toBe(true)
    expect(fs.existsSync(path.join(root, "b.txt"))).toBe(true)
  })

  test("a base branch that moved isn't fast-forwarded; --merge makes the merge commit", async () => {
    const root = repo()
    const d = deps((n) => {
      if (n !== 2) return
      fs.writeFileSync(path.join(root, "other.txt"), "meanwhile\n")
      sh(root, "add", "other.txt")
      sh(root, "commit", "-q", "-m", "meanwhile on main")
    })
    const end = await run(approved(root, d), d)
    expect(end.landed).toBe(false)
    expect(end.landNote).toContain("--merge")
    expect(sh(root, "log", "-1", "--format=%s")).toBe("meanwhile on main")

    const merged = landAndRecord(end, realDeps().git, { merge: true })
    expect(merged.landed).toBe(true)
    expect(sh(root, "log", "-1", "--format=%s")).toBe(`Land mission ${end.id}: two files`)
    expect(fs.existsSync(path.join(root, "a.txt")) && fs.existsSync(path.join(root, "other.txt"))).toBe(true)
  })

  test("a checkout on another branch is left alone", async () => {
    const root = repo()
    const d = deps((n) => n === 2 && sh(root, "switch", "-q", "-c", "elsewhere"))
    const end = await run(approved(root, d), d)
    expect(end.landed).toBe(false)
    expect(end.landNote).toContain("is on elsewhere, not main")
    expect(sh(root, "rev-parse", "--abbrev-ref", "HEAD")).toBe("elsewhere")
  })
})

describe("mission resume", () => {
  const H = 3600_000
  function stuck(over: Partial<Mission> = {}): Mission {
    const made = newMission({ goal: "g", cwd: repo(), brain: "b", maxHours: 2 })
    if ("error" in made) throw new Error(made.error)
    const running = advance(made.mission, { type: "approved", plan: PLAN }, 1)
    return { ...advance(running, { type: "budget", reason: "over --max-hours 2" }, 1), ...over }
  }

  test("over its hours, it only goes on with a larger --max-hours", () => {
    const m = stuck()
    const now = 3 * H
    const same = prepareResume(m, { now })
    expect(same.kind).toBe("error")
    expect(same.kind === "error" && same.text).toContain(`gizzi mission resume ${m.id} --max-hours`)
    expect(prepareResume(m, { now, maxHours: 2.5 }).kind).toBe("error")
    const more = prepareResume(m, { now, maxHours: 4 })
    expect(more.kind).toBe("run")
    if (more.kind === "run") {
      expect(more.mission.status).toBe("running")
      expect(more.mission.maxHours).toBe(4)
    }
  })

  test("a mission stopped before approval goes back to approval, and a done one is refused", () => {
    const made = newMission({ goal: "g", cwd: repo(), brain: "b" })
    if ("error" in made) throw new Error(made.error)
    const planned = advance(made.mission, { type: "planned", plan: PLAN }, 0)
    const cancelled = advance(planned, { type: "stop", reason: "cancelled" }, 0)
    expect(prepareResume(cancelled).kind).toBe("approve")
    const done = { ...stuck(), status: "done" as const, landed: false, landNote: "dirty" }
    const r = prepareResume(done)
    expect(r.kind === "error" && r.text).toContain("gizzi mission land")
  })

  test("--on a runtime that isn't paired is refused before any model call, and never runs here", async () => {
    const relay: RuntimeRelay = {
      findRuntime: async (key) => ({ error: `No paired runtime named or with id "${key}". Run \`gizzi computers ls\`.` }),
      keyProblem: () => undefined,
      turn: async () => {
        throw new Error("no turn may run")
      },
      patchSession: async () => {},
    }
    const prev = setRuntimeRelay(relay)
    try {
      const made = newMission({ goal: "g", cwd: repo(), brain: "b", on: "studio-mac" })
      if ("error" in made) throw new Error(made.error)
      expect(made.mission.on).toEqual({ runtime: "studio-mac" })
      const out = await planMission(made.mission, { deps: deps() })
      expect(out.error).toContain('--on studio-mac: No paired runtime named or with id "studio-mac"')
      expect(out.error).toContain("without --on")
      expect(readMission(made.mission.root, made.mission.id)).toBeNull()
      // A runner handed one refuses it too, instead of running it here.
      writeMission(advance(made.mission, { type: "approved", plan: PLAN }, 1))
      const d = deps()
      const ran = await runMission(made.mission.id, made.mission.root, d, { handleSignals: false })
      expect(ran.status).toBe("needs_you")
      expect(ran.note).toContain("No paired runtime")
      expect(d.cwds).toEqual([])
    } finally {
      setRuntimeRelay(prev)
    }
  })
})
