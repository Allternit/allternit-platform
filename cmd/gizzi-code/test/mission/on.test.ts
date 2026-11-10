import { afterEach, describe, expect, test } from "bun:test"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { spawnSync } from "node:child_process"
import { newMission, planMission, realDeps, type MissionDeps } from "../../src/cli/mission/actions"
import { fixPrompt, labelSession, onUnsupported, orchestratorTurn, setRuntimeRelay, type RuntimeRelay } from "../../src/cli/mission/orchestrator"
import { advance, newMilestones, type Mission } from "../../src/cli/mission/state"
import { readMission } from "../../src/cli/mission/store"
import { RelayError, turnOnRuntime, type RelayCall, type Runtime } from "../../src/cli/work/api"

// `--on <runtime>`: the orchestrator's turns go through a fake relay; no
// network, no model, no child process.

const roots: string[] = []
afterEach(() => {
  setRuntimeRelay()
  for (const r of roots.splice(0)) fs.rmSync(r, { recursive: true, force: true })
})

function repo(): string {
  const r = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "gizzi-mon-")))
  roots.push(r)
  const sh = (...args: string[]) => spawnSync("git", args, { cwd: r, encoding: "utf8" })
  sh("init", "-q", "-b", "main")
  sh("config", "user.email", "t@example.com")
  sh("config", "user.name", "t")
  fs.writeFileSync(path.join(r, "README.md"), "hi\n")
  fs.mkdirSync(path.join(r, "src"))
  fs.writeFileSync(path.join(r, "src", "parser.ts"), "export {}\n")
  sh("add", ".")
  sh("commit", "-q", "-m", "init")
  return r
}

const STUDIO: Runtime = { id: "rt_studio", name: "Studio Mac", runtimeType: "gizzi", status: "online" }

interface Fake extends RuntimeRelay {
  turns: { runtimeId: string; prompt: string; opts: { sessionID?: string; model?: string; title?: string } }[]
  patches: { runtimeId: string; sessionID: string; body: any }[]
}

function fakeRelay(over: Partial<RuntimeRelay> & { runtimes?: Runtime[]; reply?: string } = {}): Fake {
  const turns: Fake["turns"] = []
  const patches: Fake["patches"] = []
  const runtimes = over.runtimes ?? [STUDIO]
  const fake: Fake = {
    turns,
    patches,
    async findRuntime(key) {
      const rt = runtimes.find((r) => r.id === key) ?? runtimes.find((r) => r.name.toLowerCase() === key.toLowerCase())
      return rt ? { runtime: rt } : { error: `No paired runtime named or with id "${key}". Run \`gizzi computers ls\`.` }
    },
    keyProblem: () => undefined,
    async turn(runtimeId, prompt, opts) {
      turns.push({ runtimeId, prompt, opts })
      return { sessionID: opts.sessionID ?? "ses_remote", web: `https://ai.allternit.com/?runtime=${runtimeId}&session=ses_remote`, text: over.reply ?? "{}" }
    },
    async patchSession(runtimeId, sessionID, body) {
      patches.push({ runtimeId, sessionID, body })
    },
  }
  return Object.assign(fake, over)
}

const PLAN_REPLY = '{"milestones":[{"title":"Parser","tracks":[{"title":"p","task":"port src/parser.ts"}],"validate":"true"}]}'

/** Real record store and git; the model turn is the real orchestratorTurn, which routes --on to the relay. */
function deps(): MissionDeps {
  return { ...realDeps(), orchestratorTurn: (m, prompt, opts) => orchestratorTurn(m, prompt, opts), labelSession: async () => {} }
}

function onMission(on = "studio mac"): Mission {
  const made = newMission({ goal: "port the parser", cwd: repo(), brain: "claude-cli/claude-sonnet-5", on })
  if ("error" in made) throw new Error(made.error)
  return made.mission
}

describe("mission --on", () => {
  test("the plan turn runs on the paired runtime with the mission's brain, and its session is recorded", async () => {
    const relay = fakeRelay({ reply: PLAN_REPLY })
    setRuntimeRelay(relay)
    const m = onMission()
    const out = await planMission(m, { engine: "http://127.0.0.1:4096", deps: deps() })
    expect(out.error).toBeUndefined()
    expect(out.problems).toEqual([])
    expect(relay.turns).toHaveLength(1)
    const t = relay.turns[0]!
    expect(t.runtimeId).toBe("rt_studio")
    expect(t.opts).toEqual({ sessionID: undefined, model: "claude-cli/claude-sonnet-5", title: "Mission · port the parser" })
    // It can't open the repo from there, so it gets the tracked paths.
    expect(t.prompt).toContain("You are on a different computer from the repository")
    expect(t.prompt).toContain("  src/parser.ts")
    const rec = readMission(m.root, m.id)!
    expect(rec.status).toBe("awaiting_approval")
    expect(rec.on).toEqual({ runtime: "studio mac", id: "rt_studio", name: "Studio Mac", sessionID: "ses_remote", web: "https://ai.allternit.com/?runtime=rt_studio&session=ses_remote" })
    // Nothing local: no engine session, no engine URL to label.
    expect(rec.orchestratorSession).toBeUndefined()
    expect(rec.engineURL).toBeUndefined()
  })

  test("a later turn continues the same session there", async () => {
    const relay = fakeRelay({ reply: '{"fix":{"title":"f","task":"t"}}' })
    setRuntimeRelay(relay)
    const m: Mission = { ...onMission(), on: { runtime: "studio mac", id: "rt_studio", name: "Studio Mac", sessionID: "ses_remote" } }
    const turn = await orchestratorTurn(m, "fix it")
    expect(turn.error).toBeUndefined()
    expect(turn.sessionID).toBeUndefined()
    expect(turn.on?.sessionID).toBe("ses_remote")
    expect(relay.turns[0]!.opts.sessionID).toBe("ses_remote")
  })

  test("not paired, offline or no relay key: a clear error, and no turn runs anywhere", async () => {
    const asleep = fakeRelay({ runtimes: [{ ...STUDIO, status: "offline" }] })
    setRuntimeRelay(asleep)
    expect(await onUnsupported(onMission("nope"))).toContain('--on nope: No paired runtime named or with id "nope"')
    expect(await onUnsupported(onMission())).toContain("Studio Mac is offline")
    const turn = await orchestratorTurn(onMission(), "plan")
    expect(turn.error).toContain("Studio Mac is offline")
    expect(asleep.turns).toEqual([])

    // A cloud computer the relay wakes is fine.
    setRuntimeRelay(fakeRelay({ runtimes: [{ ...STUDIO, status: "offline", wakeable: true }] }))
    expect(await onUnsupported(onMission())).toBeUndefined()

    setRuntimeRelay(fakeRelay({ keyProblem: () => "Set ALLTERNIT_API_TOKEN" }))
    expect(await onUnsupported(onMission())).toBe("--on studio mac: Set ALLTERNIT_API_TOKEN")
    // Without --on nothing is checked.
    expect(await onUnsupported({})).toBeUndefined()
  })

  test("a failed remote turn is an error, not a local run", async () => {
    setRuntimeRelay(
      fakeRelay({
        turn: async () => {
          throw new Error("the computer has no way to say when the turn is done")
        },
      }),
    )
    const out = await planMission(onMission(), { deps: deps() })
    expect(out.error).toBe("the plan turn failed: on Studio Mac: the computer has no way to say when the turn is done")
  })

  test("labelSession PATCHes the session on the runtime through the relay; a failure is logged", async () => {
    const relay = fakeRelay()
    setRuntimeRelay(relay)
    const base: Mission = { ...onMission(), milestones: newMilestones(JSON.parse(PLAN_REPLY)), status: "running", on: { runtime: "studio mac", id: "rt_studio", sessionID: "ses_remote" } }
    await labelSession(base)
    expect(relay.patches).toHaveLength(1)
    expect(relay.patches[0]).toMatchObject({ runtimeId: "rt_studio", sessionID: "ses_remote", body: { mission: { id: base.id, label: "Mission · M1/1", milestone: 1, total: 1 }, glance: "Mission · M1/1 · port the parser" } })

    setRuntimeRelay(fakeRelay({ patchSession: async () => Promise.reject(new Error("relay /v1/session/ses_remote: 404")) }))
    const logged: string[] = []
    await labelSession(base, (l) => logged.push(l))
    expect(logged).toEqual(["couldn't label the orchestrator session on studio mac: relay /v1/session/ses_remote: 404"])
  })

  test("the fix prompt doesn't point the remote orchestrator at a local path", () => {
    const m = advance({ ...onMission(), milestones: newMilestones(JSON.parse(PLAN_REPLY)), status: "awaiting_approval" }, { type: "approved" }, 1)
    const p = fixPrompt({ ...m, on: { runtime: "studio mac" } }, 1)
    expect(p).toContain("on the computer that started it")
    expect(p).not.toContain(m.worktree)
  })
})

describe("turnOnRuntime", () => {
  type Call = { method: string; path: string; body?: any }
  function relayOf(handler: (c: Call, n: number) => unknown): { relay: RelayCall; calls: Call[] } {
    const calls: Call[] = []
    const relay: RelayCall = async (method, p, body) => {
      const c = { method, path: p, body }
      calls.push(c)
      const out = handler(c, calls.length)
      if (out instanceof Error) throw out
      return out
    }
    return { relay, calls }
  }
  const nap = async () => {}
  const MSGS = "/api/v1/agent-sessions/ses_1/messages"

  test("a new session: create, send with the model, and the blocking reply is the answer", async () => {
    const { relay, calls } = relayOf((c) =>
      c.path === "/api/v1/agent-sessions" ? { id: "ses_1" } : { id: "msg_a", role: "assistant", content: "the plan", metadata: {} },
    )
    const r = await turnOnRuntime("rt_9", "plan it", { relay, model: "claude-cli/claude-sonnet-5", title: "Mission · x", sleep: nap })
    expect(r).toEqual({ sessionID: "ses_1", web: "https://ai.allternit.com/?runtime=rt_9&session=ses_1", phone: "https://m.allternit.com/?runtime=rt_9&session=ses_1", text: "the plan" })
    expect(calls.map((c) => `${c.method} ${c.path}`)).toEqual(["POST /api/v1/agent-sessions", `POST ${MSGS}`])
    expect(calls[0]!.body).toMatchObject({ name: "Mission · x", model: { providerID: "claude-cli", modelID: "claude-sonnet-5" } })
    expect(calls[1]!.body).toEqual({ text: "plan it", metadata: { model: { providerID: "claude-cli", modelID: "claude-sonnet-5" } } })
  })

  test("a turn longer than the relay waits: poll status until idle, then read the new assistant message", async () => {
    let posted = false
    let polls = 0
    const old = [{ id: "m1", role: "user", content: "earlier" }, { id: "m2", role: "assistant", content: "earlier reply" }]
    const { relay, calls } = relayOf((c) => {
      if (c.method === "GET" && c.path === MSGS) return posted ? [...old, { id: "m3", role: "user", content: "fix" }, { id: "m4", role: "assistant", content: "step" }, { id: "m5", role: "assistant", content: '{"fix":{}}' }] : old
      if (c.method === "POST" && c.path === MSGS) {
        posted = true
        return new RelayError('relay x: 504 {"error":"runtime_timeout"}', 504, c.path)
      }
      // allternit-api has no status route on this runtime (a BYO gizzi box): fall back to gizzi's map.
      if (c.path.endsWith("/status") && c.path.startsWith("/api/")) return new RelayError("relay: 404", 404, c.path)
      if (c.path === "/v1/session/status") return ++polls < 2 ? { ses_1: { type: "busy" } } : {}
      throw new Error(`unexpected ${c.method} ${c.path}`)
    })
    const r = await turnOnRuntime("rt_9", "fix", { relay, sessionID: "ses_1", sleep: nap })
    expect(r.text).toBe('{"fix":{}}')
    expect(polls).toBe(2)
    expect(calls[0]).toMatchObject({ method: "GET", path: MSGS }) // what was there before the send
  })

  test("a runtime that can't say when the turn is done is a clear error", async () => {
    const { relay } = relayOf((c) => {
      if (c.method === "GET" && c.path === MSGS) return []
      if (c.method === "POST") return new RelayError("timeout", 0, c.path)
      return new RelayError("relay: 404", 404, c.path)
    })
    await expect(turnOnRuntime("rt_9", "x", { relay, sessionID: "ses_1", sleep: nap })).rejects.toThrow(/no way to say when the turn is done/)
  })

  test("a failed turn and an empty reply are errors", async () => {
    const failed = relayOf((c) => (c.method === "GET" ? [] : { id: "a", role: "assistant", content: "rate limited", metadata: { error: { message: "rate limited" } } }))
    await expect(turnOnRuntime("rt_9", "x", { relay: failed.relay, sessionID: "ses_1", sleep: nap })).rejects.toThrow("rate limited")
    const empty = relayOf((c) => (c.method === "GET" ? [] : { id: "a", role: "assistant", content: "[No text content]", metadata: {} }))
    await expect(turnOnRuntime("rt_9", "x", { relay: empty.relay, sessionID: "ses_1", sleep: nap })).rejects.toThrow(/replied with nothing/)
    const other = relayOf((c) => (c.method === "GET" ? [] : new RelayError("relay: 403 Runtime pairing does not grant", 403, c.path)))
    await expect(turnOnRuntime("rt_9", "x", { relay: other.relay, sessionID: "ses_1", sleep: nap })).rejects.toThrow(/403/)
  })
})
