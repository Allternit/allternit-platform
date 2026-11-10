import { describe, expect, test } from "bun:test"
import {
  advance,
  badge,
  currentMilestone,
  label,
  markdownToPlan,
  nextAction,
  overHours,
  parseFixReply,
  parsePlanReply,
  planProblems,
  planToMarkdown,
  type Mission,
  type PlanInput,
} from "../../src/cli/mission/state"

const T0 = 1_000_000

const PLAN: PlanInput = {
  milestones: [
    {
      title: "Parser",
      tracks: [
        { title: "lexer", task: "Write src/lex.ts and test it." },
        { title: "grammar", task: "Write src/grammar.ts." },
      ],
      validate: "bun test test/parse",
    },
    { title: "Docs", tracks: [{ title: "guide", task: "Write docs/parse.md." }], validate: "test -f docs/parse.md" },
  ],
}

function mission(over: Partial<Mission> = {}): Mission {
  return {
    id: "mi-test1",
    goal: "build a parser",
    root: "/repo",
    baseBranch: "main",
    branch: "mission/mi-test1",
    worktree: "/repo",
    brain: "claude-cli/claude-sonnet-5",
    maxWorkers: 3,
    maxAttempts: 2,
    status: "planning",
    milestones: [],
    current: 0,
    createdAt: T0,
    log: [],
    ...over,
  }
}

const approved = (over: Partial<Mission> = {}) => advance(advance(mission(over), { type: "planned", plan: PLAN }, T0), { type: "approved" }, T0 + 1)

describe("mission happy path", () => {
  test("planned → approved → tracks → land → validate → merge → next milestone → done", () => {
    let m = advance(mission(), { type: "planned", plan: PLAN, session: "ses_1" }, T0)
    expect(m.status).toBe("awaiting_approval")
    expect(m.orchestratorSession).toBe("ses_1")
    expect(m.milestones.map((x) => x.id)).toEqual(["M1", "M2"])
    expect(nextAction(m).kind).toBe("none")

    m = advance(m, { type: "approved" }, T0 + 1)
    expect(m.status).toBe("running")
    expect(m.startedAt).toBe(T0 + 1)

    const a1 = nextAction(m)
    expect(a1).toEqual({ kind: "start_tracks", tracks: PLAN.milestones[0]!.tracks })
    m = advance(m, { type: "tracks_started", swarm: "sw-1", workers: ["w1@sw-1", "w2@sw-1"], count: 2 })
    expect(currentMilestone(m)!.status).toBe("working")
    expect(nextAction(m)).toEqual({ kind: "wait", swarm: "sw-1" })

    m = advance(m, { type: "landed" })
    expect(nextAction(m)).toEqual({ kind: "validate", command: "bun test test/parse" })

    m = advance(m, { type: "validated", ok: true, output: "3 pass" })
    expect(currentMilestone(m)!.status).toBe("passed")
    expect(nextAction(m).kind).toBe("merge")

    m = advance(m, { type: "merged" })
    expect(m.current).toBe(1)
    expect(m.milestones[0]!.status).toBe("passed")
    expect(nextAction(m)).toEqual({ kind: "start_tracks", tracks: PLAN.milestones[1]!.tracks })

    m = advance(m, { type: "tracks_started", swarm: "sw-2", workers: ["w1@sw-2"], count: 1 })
    m = advance(m, { type: "landed" })
    m = advance(m, { type: "validated", ok: true })
    m = advance(m, { type: "merged" }, T0 + 99)
    expect(m.status).toBe("done")
    expect(m.endedAt).toBe(T0 + 99)
    expect(m.current).toBe(1)
    expect(nextAction(m).kind).toBe("none")
    expect(m.log.at(-1)!.text).toContain("mission done")
  })

  test("more tracks than maxWorkers start in batches before validating", () => {
    let m = approved({ maxWorkers: 1 })
    expect(nextAction(m)).toEqual({ kind: "start_tracks", tracks: [PLAN.milestones[0]!.tracks[0]!] })
    m = advance(m, { type: "tracks_started", swarm: "sw-1", workers: ["w1@sw-1"], count: 1 })
    m = advance(m, { type: "landed" })
    expect(nextAction(m)).toEqual({ kind: "start_tracks", tracks: [PLAN.milestones[0]!.tracks[1]!] })
    m = advance(m, { type: "tracks_started", swarm: "sw-2", workers: ["w1@sw-2"], count: 1 })
    expect(nextAction(m)).toEqual({ kind: "wait", swarm: "sw-2" })
    m = advance(m, { type: "landed" })
    expect(nextAction(m).kind).toBe("validate")
    expect(currentMilestone(m)!.swarms).toEqual(["sw-1", "sw-2"])
  })

  test("a fix for a batch whose landing conflicted still starts the remaining tracks before validating", () => {
    let m = approved({ maxWorkers: 1 })
    m = advance(m, { type: "tracks_started", swarm: "sw-1", workers: ["w1@sw-1"], count: 1 })
    m = advance(m, { type: "landed" })
    // The runner reports a conflict as a failed validation.
    m = advance(m, { type: "validated", ok: false, output: "conflict" })
    m = advance(m, { type: "fix_planned", fix: { title: "merge", task: "resolve it" } })
    m = advance(m, { type: "fix_started", swarm: "sw-f", workers: ["w1@sw-f"] })
    m = advance(m, { type: "landed" })
    expect(nextAction(m)).toEqual({ kind: "start_tracks", tracks: [PLAN.milestones[0]!.tracks[1]!] })
  })

  test("approval with an edited plan replaces the milestones", () => {
    let m = advance(mission(), { type: "planned", plan: PLAN }, T0)
    m = advance(m, { type: "approved", plan: { milestones: [PLAN.milestones[1]!] } })
    expect(m.milestones.map((x) => x.title)).toEqual(["Docs"])
  })
})

describe("validation failures and fixes", () => {
  const toValidating = (m: Mission) => advance(advance(m, { type: "tracks_started", swarm: "sw-1", workers: ["w1@sw-1"], count: 2 }), { type: "landed" })

  test("fail → plan_fix → start_fix, maxAttempts rounds, then the milestone fails and the mission asks", () => {
    let m = toValidating(approved({ maxAttempts: 2 }))
    for (let round = 1; round <= 2; round++) {
      m = advance(m, { type: "validated", ok: false, output: `boom ${round}` })
      expect(currentMilestone(m)!.status).toBe("fixing")
      expect(currentMilestone(m)!.attempts).toBe(round)
      expect(currentMilestone(m)!.lastFailure).toBe(`boom ${round}`)
      expect(nextAction(m).kind).toBe("plan_fix")
      const fix = { title: `fix ${round}`, task: `fix boom ${round}` }
      m = advance(m, { type: "fix_planned", fix })
      expect(nextAction(m)).toEqual({ kind: "start_fix", fix })
      m = advance(m, { type: "fix_started", swarm: `sw-f${round}`, workers: [`w1@sw-f${round}`] })
      expect(nextAction(m)).toEqual({ kind: "wait", swarm: `sw-f${round}` })
      m = advance(m, { type: "landed" })
      expect(nextAction(m).kind).toBe("validate")
    }
    m = advance(m, { type: "validated", ok: false, output: "boom 3" })
    expect(currentMilestone(m)!.status).toBe("failed")
    expect(currentMilestone(m)!.attempts).toBe(3)
    expect(currentMilestone(m)!.fixesStarted).toBe(2)
    expect(m.status).toBe("needs_you")
    expect(m.note).toContain("after 3 tries")
    expect(nextAction(m).kind).toBe("none")
  })

  test("a passing fix moves on and clears the failure", () => {
    let m = toValidating(approved())
    m = advance(m, { type: "validated", ok: false, output: "nope" })
    m = advance(m, { type: "fix_planned", fix: { title: "f", task: "t" } })
    m = advance(m, { type: "fix_started", swarm: "sw-f", workers: ["w1@sw-f"] })
    m = advance(m, { type: "landed" })
    m = advance(m, { type: "validated", ok: true })
    expect(currentMilestone(m)!.status).toBe("passed")
    expect(currentMilestone(m)!.lastFailure).toBeUndefined()
  })

  test("fix_declined stops to ask; resume validates again with a fresh budget", () => {
    let m = toValidating(approved())
    m = advance(m, { type: "validated", ok: false, output: "nope" })
    m = advance(m, { type: "fix_declined", reason: "the API key is missing" })
    expect(m.status).toBe("needs_you")
    expect(currentMilestone(m)!.status).toBe("failed")
    expect(m.note).toContain("the API key is missing")
    expect(nextAction(m).kind).toBe("none")
    m = advance(m, { type: "resume" })
    expect(m.status).toBe("running")
    expect(currentMilestone(m)!.attempts).toBe(0)
    expect(nextAction(m).kind).toBe("validate")
  })
})

describe("stop, budget, resume", () => {
  test("stop ends a running mission and later events are ignored", () => {
    let m = approved()
    m = advance(m, { type: "stop" }, T0 + 5)
    expect(m.status).toBe("stopped")
    expect(m.endedAt).toBe(T0 + 5)
    expect(m.note).toBe("stopped by you")
    expect(nextAction(m).kind).toBe("none")
    expect(advance(m, { type: "stop", reason: "again" })).toBe(m)
    expect(advance(m, { type: "error", text: "x" })).toBe(m)
  })

  test("resume of an in-flight milestone keeps waiting on the same swarm", () => {
    let m = advance(approved(), { type: "tracks_started", swarm: "sw-1", workers: ["w1@sw-1"], count: 2 })
    m = advance(m, { type: "stop", reason: "SIGTERM" })
    m = advance(m, { type: "resume" })
    expect(m.status).toBe("running")
    expect(m.endedAt).toBeUndefined()
    expect(nextAction(m)).toEqual({ kind: "wait", swarm: "sw-1" })
  })

  test("a mission stopped before approval resumes to awaiting approval, not running", () => {
    let m = advance(mission(), { type: "planned", plan: PLAN }, T0)
    m = advance(m, { type: "stop" })
    m = advance(m, { type: "resume" })
    expect(m.status).toBe("awaiting_approval")
    expect(nextAction(m).kind).toBe("none")
  })

  test("over --max-hours asks a person", () => {
    const m = approved({ maxHours: 1 })
    expect(overHours(m, T0 + 1 + 3600_000)).toBe(false)
    expect(overHours(m, T0 + 2 + 3600_000)).toBe(true)
    expect(overHours(approved(), T0 + 1e12)).toBe(false)
    const b = advance(m, { type: "budget", reason: "over 1h" })
    expect(b.status).toBe("needs_you")
    expect(b.note).toBe("over 1h")
  })

  test("an error asks a person", () => {
    const m = advance(approved(), { type: "error", text: "git switch failed" })
    expect(m.status).toBe("needs_you")
    expect(m.note).toBe("git switch failed")
  })
})

describe("plan format", () => {
  test("markdownToPlan(planToMarkdown(p)) round-trips", () => {
    const md = planToMarkdown("build a parser", PLAN)
    expect(md).toContain("# Mission: build a parser")
    expect(md).toContain("## M2: Docs")
    expect(md).toContain("validate: `test -f docs/parse.md`")
    expect(markdownToPlan(md)).toEqual(PLAN)
  })

  test("multi-line tasks are folded to one line", () => {
    const plan = { milestones: [{ title: "A", tracks: [{ title: "t", task: "line one\n  line two" }], validate: "true" }] }
    expect(markdownToPlan(planToMarkdown("g", plan)).milestones[0]!.tracks[0]!.task).toBe("line one line two")
  })

  test("hand-edited markdown: untitled tracks and 'validation:'", () => {
    const plan = markdownToPlan("## Setup\n* just do the thing\nvalidation: make check\n")
    expect(plan.milestones).toEqual([{ title: "Setup", tracks: [{ title: "just do the thing", task: "just do the thing" }], validate: "make check" }])
  })

  test("planProblems names each gap; a good plan has none", () => {
    expect(planProblems(PLAN)).toEqual([])
    expect(planProblems({ milestones: [] })).toEqual(["the plan has no milestones"])
    expect(planProblems({ milestones: [{ title: " ", tracks: [{ title: "t", task: "" }], validate: "" }, { title: "B", tracks: [], validate: "true" }] })).toEqual([
      "M1 has no title",
      "M1 has no validation command",
      "M1 track 1 has no task",
      "M2 has no tracks",
    ])
  })

  test("parsePlanReply and parseFixReply read fenced JSON", () => {
    const reply = 'Here:\n```json\n{"milestones":[{"title":"A","tracks":["do x"],"validation":"true"}]}\n```'
    expect(parsePlanReply(reply)).toEqual({ milestones: [{ title: "A", tracks: [{ title: "do x", task: "do x" }], validate: "true" }] })
    expect(parsePlanReply("no json")).toBeUndefined()
    expect(parseFixReply('{"fix":{"title":"f","task":"t"}}')).toEqual({ fix: { title: "f", task: "t" } })
    expect(parseFixReply('{"stop":"needs a key"}')).toEqual({ stop: "needs a key" })
    expect(parseFixReply('{"fix":{"task":""}}')).toBeUndefined()
  })
})

describe("badge and label", () => {
  test("show the current milestone and the status", () => {
    let m = approved()
    expect(badge(m)).toBe("◆ M1/2")
    expect(label(m)).toBe("Mission · M1/2")
    m = { ...m, current: 1 }
    expect(badge(m)).toBe("◆ M2/2")
    expect(label({ ...m, status: "done" })).toBe("Mission · M2/2 · done")
    expect(label({ ...m, status: "needs_you" })).toBe("Mission · M2/2 · needs you")
    expect(label({ ...m, status: "stopped" })).toBe("Mission · M2/2 · stopped")
    expect(label({ ...m, status: "failed" })).toBe("Mission · M2/2 · failed")
    expect(label(mission())).toBe("Mission · M0/?")
  })
})
