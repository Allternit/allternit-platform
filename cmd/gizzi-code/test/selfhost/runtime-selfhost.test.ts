import { describe, expect, test } from "bun:test"
import path from "path"
import { selfHostedRegister, selfHostedStatus, normalizeServerUrl } from "../../src/cli/commands/runtime/selfhost"
import { createProfileStore } from "../../src/runtime/selfhost/profiles"
import { createCredentialWriter } from "../../src/runtime/context/config/credential-store"
import { tmpdir } from "../fixture/fixture"

// `gizzi runtime register --self-hosted` / `gizzi runtime status` against a
// stand-in Agents Server.

const KEY = "ask_live_0123456789abcdef"

function agentsServer() {
  const srv = Bun.serve({
    port: 0,
    hostname: "127.0.0.1",
    fetch(req) {
      const url = new URL(req.url)
      if (url.pathname === "/v1/server/info") {
        return Response.json({
          version: "1.0.0",
          provider: "bedrock",
          features: { web_tools: false, prometheus: true, oidc: false, vault_secrets: false, postgres: true },
          auth_modes: ["api_key"],
        })
      }
      if (req.headers.get("authorization") !== `Bearer ${KEY}`) {
        return Response.json({ error: { type: "authentication_error", code: "unauthorized", message: "bad key" } }, { status: 401 })
      }
      if (url.pathname === "/v1/environments") {
        return Response.json({
          data: [{ id: "env_1", name: "default", network: "limited", allowed_hosts: ["pypi.org"], queue_depth: 4 }],
          has_more: false,
          next_cursor: null,
        })
      }
      if (url.pathname === "/v1/environments/env_1/workers") {
        return Response.json({
          data: [
            { worker_id: "w-1", last_seen_at: "2026-11-12T10:00:00Z", online: true, current_work_id: "work_9" },
            { worker_id: "w-2", last_seen_at: "2026-11-12T09:55:00Z", online: false },
          ],
        })
      }
      return new Response("not found", { status: 404 })
    },
  })
  return { srv, url: `http://127.0.0.1:${srv.port}` }
}

async function storeIn(dir: string) {
  return createProfileStore({
    file: path.join(dir, "selfhost-servers.json"),
    writer: createCredentialWriter("file", { fileDir: path.join(dir, "creds"), notifier: () => {} }),
  })
}

describe("self-hosted runtime profile", () => {
  test("register checks the key, keeps it out of the profile file, and status shows workers and queue depth", async () => {
    await using tmp = await tmpdir()
    const s = agentsServer()
    const store = await storeIn(tmp.path)
    const out: string[] = []
    const err: string[] = []
    try {
      const code = await selfHostedRegister({ server: s.url, apiKey: KEY, name: "lab" }, { store, out: (l) => out.push(l), err: (l) => err.push(l) })
      expect(err).toEqual([])
      expect(code).toBe(0)
      const profileFile = await Bun.file(store.file).text()
      expect(profileFile).toContain(s.url)
      expect(profileFile).not.toContain(KEY)
      expect(out.join("\n")).not.toContain(KEY)

      const status: string[] = []
      expect(await selfHostedStatus({}, { store, out: (l) => status.push(l) })).toBe(0)
      const text = status.join("\n")
      expect(text).toContain("Provider:  bedrock")
      expect(text).toContain("Workers online: 1   Queue depth: 4")
      expect(text).toContain("w-1")
      expect(text).toContain("working on work_9")
    } finally {
      s.srv.stop(true)
    }
  })

  test("a key the server refuses is not saved", async () => {
    await using tmp = await tmpdir()
    const s = agentsServer()
    const store = await storeIn(tmp.path)
    const err: string[] = []
    try {
      const code = await selfHostedRegister({ server: s.url, apiKey: "ask_wrong_key_000000" }, { store, out: () => {}, err: (l) => err.push(l) })
      expect(code).toBe(1)
      expect(err.join("\n")).toContain("refused the API key")
      expect(await store.get()).toBeUndefined()
    } finally {
      s.srv.stop(true)
    }
  })

  test("http:// is refused for a public host", () => {
    expect(() => normalizeServerUrl("http://agents.example.com")).toThrow(/https/)
    expect(normalizeServerUrl("http://127.0.0.1:8400/")).toBe("http://127.0.0.1:8400")
    expect(normalizeServerUrl("https://agents.example.com")).toBe("https://agents.example.com")
  })
})
