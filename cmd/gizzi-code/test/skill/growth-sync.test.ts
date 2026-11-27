// @ts-nocheck
// Skill growth ↔ server knowledge proposals (plan step 13). The network is the
// only fake: an in-memory /api/v1/knowledge server with the real status rules.
import { afterAll, afterEach, beforeEach, describe, expect, test } from "bun:test"
import fs from "fs/promises"
import path from "path"
import { Global } from "../../src/runtime/context/global"
import { Instance } from "../../src/project/instance"
import { SkillGrowth } from "../../src/runtime/skills/growth"
import { SkillActivateTool, SkillEvaluateTool, SkillProposeTool } from "../../src/runtime/tools/builtins/skill-growth"
import { tmpdir } from "../fixture/fixture"

const realFetch = globalThis.fetch
const priorUrl = process.env.ALLTERNIT_API_URL
const priorToken = process.env.ALLTERNIT_API_TOKEN

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } })

const NEXT = { approve: "approved", reject: "rejected", activated: "active", rollback: "rolled_back" }
const FROM = { eval: ["proposed"], approve: ["proposed"], reject: ["proposed"], activated: ["approved"], rollback: ["approved", "active"] }

let server: { up: boolean; rows: Map<string, any> }

function fakeServer() {
  server = { up: true, rows: new Map() }
  globalThis.fetch = (async (input: string, init: RequestInit = {}) => {
    if (!server.up) throw new TypeError("fetch failed")
    const url = new URL(String(input))
    const method = init.method ?? "GET"
    const body = init.body ? JSON.parse(String(init.body)) : undefined
    const rest = url.pathname.replace(/^\/api\/v1\/knowledge\//, "")
    if (method === "POST" && rest === "proposals") {
      const row = { id: `kprop_${server.rows.size + 1}`, status: "proposed", base: null, eval: null, ...body }
      server.rows.set(row.id, row)
      return json({ proposal: row })
    }
    if (method === "GET" && rest === "proposals") {
      const status = url.searchParams.get("status")
      return json({ proposals: [...server.rows.values()].filter((r) => !status || r.status === status) })
    }
    const m = rest.match(/^proposals\/([^/]+)\/(\w+)$/)
    if (method === "POST" && m) {
      const row = server.rows.get(decodeURIComponent(m[1]))
      if (!row) return json({ error: "Not found" }, 404)
      const op = m[2]
      if (!FROM[op]?.includes(row.status)) return json({ error: `proposal is ${row.status}` }, 409)
      if (op === "eval") row.eval = body.eval
      else row.status = NEXT[op]
      return json({ proposal: row })
    }
    return new Response("", { status: 404 })
  }) as typeof fetch
}

function ctx(ask: (input: any) => Promise<void> = async () => {}) {
  const asks: any[] = []
  return {
    asks,
    ctx: {
      sessionID: "ses_growth",
      messageID: "",
      callID: "",
      agent: "build",
      abort: AbortSignal.any([]),
      messages: [],
      metadata: () => {},
      ask: async (input: any) => {
        asks.push(input)
        await ask(input)
      },
    },
  }
}

async function run(tool: any, params: any, c = ctx().ctx) {
  const t = await tool.init()
  return t.execute(params, c)
}

async function inProject<T>(fn: (dir: string) => Promise<T>) {
  await using tmp = await tmpdir({ git: true })
  return await Instance.provide({ directory: tmp.path, fn: () => fn(tmp.path) })
}

async function stored(id: string) {
  return JSON.parse(await fs.readFile(path.join(Global.Path.data, "skill-growth", "proposals", `${id}.json`), "utf8"))
}

const skill = { name: "pr-review", description: "Review a PR", content: "Read the diff first.", scope: "project" }

beforeEach(async () => {
  await fs.rm(path.join(Global.Path.data, "skill-growth"), { recursive: true, force: true })
  await fs.rm(path.join(Global.Path.config, "skills"), { recursive: true, force: true })
  process.env.ALLTERNIT_API_URL = "http://knowledge.test"
  process.env.ALLTERNIT_API_TOKEN = "test-token"
  fakeServer()
})
afterEach(() => {
  globalThis.fetch = realFetch
})
afterAll(() => {
  if (priorUrl === undefined) delete process.env.ALLTERNIT_API_URL
  else process.env.ALLTERNIT_API_URL = priorUrl
  if (priorToken === undefined) delete process.env.ALLTERNIT_API_TOKEN
  else process.env.ALLTERNIT_API_TOKEN = priorToken
})

describe("skill growth server sync", () => {
  test("propose posts a skill proposal and keeps its server id", async () => {
    await inProject(async () => {
      const result = await run(SkillProposeTool, skill)
      const local = await stored(result.metadata.id)
      expect(local.server_id).toBe("kprop_1")
      expect(local.sync).toBe("synced")
      const row = server.rows.get("kprop_1")
      expect([row.target_kind, row.target_ref, row.origin, row.status]).toEqual(["skill", "pr-review", "skill_growth", "proposed"])
      expect(row.body).toContain("name: pr-review")
      expect(row.body).toContain("Read the diff first.")
      expect(row.base ?? null).toBeNull()
    })
  })

  test("offline keeps the proposal as sync pending and the next call sends it with its eval", async () => {
    await inProject(async () => {
      server.up = false
      const proposed = await run(SkillProposeTool, skill)
      const offline = await stored(proposed.metadata.id)
      expect([offline.sync, offline.server_id]).toEqual(["pending", undefined])
      expect(proposed.output).toContain("Not on the server yet")

      server.up = true
      await run(SkillEvaluateTool, { proposal_id: proposed.metadata.id, score: 0.9, report: "9 of 10 cases pass" })
      const synced = await stored(proposed.metadata.id)
      expect([synced.sync, synced.server_id, synced.pending_ops]).toEqual(["synced", "kprop_1", []])
      expect(server.rows.size).toBe(1)
      expect(server.rows.get("kprop_1").eval).toEqual({ score: 0.9, report: "9 of 10 cases pass" })
    })
  })

  test("a proposal approved on the server is never activated without the skill_manage ask", async () => {
    await inProject(async (dir) => {
      const proposed = await run(SkillProposeTool, skill)
      await run(SkillEvaluateTool, { proposal_id: proposed.metadata.id, score: 0.9, report: "ok" })
      server.rows.get("kprop_1").status = "approved" // the person approved it in the review screen
      const target = path.join(dir, ".gizzi", "skills", "pr-review", "SKILL.md")

      const denied = ctx(async () => {
        throw new Error("permission denied")
      })
      await expect(run(SkillActivateTool, { proposal_id: proposed.metadata.id }, denied.ctx)).rejects.toThrow("permission denied")
      expect(denied.asks.map((a) => [a.permission, a.patterns[0]])).toEqual([["skill_manage", `activate:${proposed.metadata.id}`]])
      expect(await fs.stat(target).then(() => true, () => false)).toBe(false)

      // Pulling the approval marks it approved here, and still writes nothing.
      await SkillGrowth.sync()
      const local = await stored(proposed.metadata.id)
      expect([local.status, local.approved_on]).toEqual(["approved", "server"])
      expect(await fs.stat(target).then(() => true, () => false)).toBe(false)
      expect(server.rows.get("kprop_1").status).toBe("approved")

      await run(SkillActivateTool, { proposal_id: proposed.metadata.id })
      expect(await fs.readFile(target, "utf8")).toContain("Read the diff first.")
      expect(server.rows.get("kprop_1").status).toBe("active")
    })
  })

  test("a curator skill proposal approved on the server becomes SKILL.md once the ask is granted", async () => {
    await inProject(async () => {
      server.rows.set("kprop_9", {
        id: "kprop_9",
        target_kind: "skill",
        target_ref: "release-notes",
        title: "Release notes: list breaking changes first",
        body: '---\nname: release-notes\ndescription: "Write release notes"\n---\n\nList breaking changes first.\n',
        base: null,
        origin: "curator",
        eval: null,
        status: "approved",
      })
      const granted = ctx()
      const result = await run(SkillActivateTool, { proposal_id: "kprop_9" }, granted.ctx)
      expect(granted.asks.map((a) => a.patterns[0])).toEqual(["activate:kprop_9"])

      const target = path.join(Global.Path.config, "skills", "release-notes", "SKILL.md")
      expect(result.metadata.target).toBe(target)
      const text = await fs.readFile(target, "utf8")
      expect(text).toContain('description: "Write release notes"')
      expect(text).toContain("List breaking changes first.")
      expect(server.rows.get("kprop_9").status).toBe("active")
      const local = await stored(result.metadata.id)
      expect([local.origin, local.server_id, local.status]).toEqual(["curator", "kprop_9", "active"])
    })
  })
})
