import { afterEach, expect, test } from "bun:test"
import { PermissionNext } from "../../src/permission/next"
import { PermissionS1 } from "../../src/runtime/tools/guard/permission/s1-shadow"
import { Instance } from "../../src/project/instance"
import { tmpdir } from "../fixture/fixture"

type Call = { url: string; body: any }
const fake = (pTrue: number, calls: Call[]) => async (url: string, init?: RequestInit) => {
  const body = JSON.parse(String(init?.body))
  calls.push({ url, body })
  if (url.endsWith("/outcome")) return new Response("{}")
  return new Response(
    JSON.stringify({ answer: pTrue >= 0.5, probabilities: { true: pTrue, false: 1 - pTrue }, confidence: 1, threshold_action: "REVIEW", extensions: { "x-decision_id": "d1" } }),
  )
}
const use = (pTrue: number, calls: Call[]) =>
  PermissionS1._setClient({ url: "http://s1", fetchImpl: fake(pTrue, calls) as any, enabled: true, backend: "auto" })
const settle = () => new Promise((r) => setTimeout(r, 50))
afterEach(() => PermissionS1._setClient(undefined))

test("tighten-only: an S1 allow can never turn ask/deny into allow", () => {
  expect(PermissionS1.combine("ask", "allow")).toBe("ask")
  expect(PermissionS1.combine("deny", "allow")).toBe("deny")
  expect(PermissionS1.combine("allow", "deny")).toBe("deny")
  expect(PermissionS1.combine("deny", null)).toBe("deny")
})

test("allow path: shadow GATE logged, incumbent allow unchanged even when S1 says deny", async () => {
  await using tmp = await tmpdir({ git: true })
  await Instance.provide({
    directory: tmp.path,
    fn: async () => {
      const calls: Call[] = []
      use(0.01, calls)
      const r = await PermissionNext.ask({
        sessionID: "ses_allow", permission: "bash", patterns: ["ls"], metadata: {}, always: [],
        tool: { messageID: "msg_1", callID: "c1" },
        ruleset: [{ permission: "bash", pattern: "*", action: "allow" }],
      })
      expect(r).toBeUndefined()
      await settle()
      expect(calls.length).toBe(1)
      const req = calls[0].body.request
      expect(calls[0].url).toBe("http://s1/v1/decision")
      expect(calls[0].body.backend).toBe("auto")
      expect(req.operation).toBe("GATE")
      expect(req.decision_bank_id).toBe(PermissionS1.BANK)
      expect(req.extensions["x-primitive_id"]).toBe(PermissionS1.PRIMITIVE)
      expect(req.extensions["x-incumbent_action"]).toBe("allow")
      expect(req.extensions["x-subject_ref"]).toBe("gizzi-call:c1:bash:ls")
    },
  })
})

test("ask path: S1 allow does not skip the card; the person's reply is the outcome label", async () => {
  await using tmp = await tmpdir({ git: true })
  await Instance.provide({
    directory: tmp.path,
    fn: async () => {
      for (const [reply, truth] of [["once", "true"], ["reject", "false"]] as const) {
        const calls: Call[] = []
        use(0.99, calls)
        const id = `permission_s1_${reply}`
        let settled = false
        const p = PermissionNext.ask({
          id, sessionID: `ses_${reply}`, permission: "bash", patterns: ["make deploy"], metadata: {}, always: [],
          ruleset: [{ permission: "bash", pattern: "*", action: "ask" }],
        }).then(() => (settled = true), (e) => e)
        for (let i = 0; i < 100 && !(await PermissionNext.list()).some((r: any) => r.id === id); i++) await settle()
        expect(settled).toBe(false) // still waiting on the person despite S1's confident allow
        expect(calls[0].body.request.extensions["x-subject_ref"]).toBe(`gizzi-permission:${id}`)
        await PermissionNext.reply({ requestID: id, reply })
        await p
        await settle()
        const out = calls.find((c) => c.url.endsWith("/v1/decision/outcome"))!
        expect(out.body).toEqual({ subject_ref: `gizzi-permission:${id}`, question_id: PermissionS1.QUESTION, truth, source: "gizzi.permission_reply" })
      }
    },
  })
})

test("deny path: the floor/policy denial stands and S1 is only logged", async () => {
  await using tmp = await tmpdir({ git: true })
  await Instance.provide({
    directory: tmp.path,
    fn: async () => {
      const calls: Call[] = []
      use(0.99, calls)
      const err = await PermissionNext.ask({
        sessionID: "ses_deny", permission: "bash", patterns: ["rm"], metadata: {}, always: [],
        ruleset: [{ permission: "bash", pattern: "*", action: "deny" }],
      }).catch((e) => e)
      expect(err).toBeInstanceOf(PermissionNext.DeniedError)
      await settle()
      expect(calls[0].body.request.extensions["x-incumbent_action"]).toBe("deny")
    },
  })
})

test("disabled under bun test unless opted in: no runtime call", () => {
  expect(PermissionS1.shadow({ permission: "bash", pattern: "ls", mode: undefined, incumbent: "allow", sessionID: "s" })).toBeUndefined()
})
