import { describe, expect, test } from "bun:test"
import { appsList, appsRevoke, linkClaudeOrg, NOT_AVAILABLE, type Deps } from "../../src/cli/commands/auth-apps"
import { linkedModelDisplayName } from "../../src/runtime/providers/person-plan/discovery"

const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status })

function deps(handler: (method: string, path: string) => Response, extra: Partial<Deps> = {}) {
  const lines: string[] = []
  const calls: string[] = []
  const d: Deps = {
    origin: "http://fake",
    token: "tok",
    fetch: (async (url: string, init?: RequestInit) => {
      const path = String(url).replace("http://fake", "")
      calls.push(`${init?.method ?? "GET"} ${path}`)
      return handler(init?.method ?? "GET", path)
    }) as unknown as typeof fetch,
    out: (l) => lines.push(l),
    ...extra,
  }
  return { d, lines, calls }
}

const grants = {
  month: "2026-11",
  resets_at: "2026-12-01T00:00:00Z",
  data: [{ id: "g1", app_name: "Notes Sync", app_state: "live", monthly_cap_cents: 1000, spent_cents: 123, last_used_at: null }],
}

describe("gizzi auth apps", () => {
  test("apps_list_prints_spend_and_cap", async () => {
    const { d, lines } = deps(() => json(grants))
    await appsList(d)
    const text = lines.join("\n")
    expect(text).toContain("Notes Sync")
    expect(text).toContain("$1.23 / $10.00")
    expect(text).toContain("never")
    const j = deps(() => json(grants))
    await appsList(j.d, { json: true })
    expect(JSON.parse(j.lines[0]!).data[0].id).toBe("g1")
  })

  test("apps_revoke_requires_confirm", async () => {
    const no = deps((m) => (m === "DELETE" ? new Response(null, { status: 204 }) : json(grants)), { confirm: async () => false })
    expect(await appsRevoke(no.d, "Notes Sync")).toBe(false)
    expect(no.calls.some((c) => c.startsWith("DELETE"))).toBe(false)
    const yes = deps((m) => (m === "DELETE" ? new Response(null, { status: 204 }) : json(grants)))
    expect(await appsRevoke(yes.d, "g1", { yes: true })).toBe(true)
    expect(yes.calls).toContain("DELETE /api/v1/oauth/grants/g1")
    expect(yes.lines).toContain("Revoked Notes Sync.")
  })
})

describe("gizzi auth link", () => {
  test("link_claude_org_polls_until_linked", async () => {
    let polls = 0
    let t = 0
    const opened: string[] = []
    const { d, lines } = deps(
      (m) => {
        if (m === "POST") return json({ login: { kind: "oauth", url: "https://claude.example/auth", expires_at: new Date(1e9).toISOString(), poll: true } })
        polls++
        return json({ state: polls < 3 ? "unlinked" : "linked" })
      },
      { open: async (u) => void opened.push(u), sleep: async () => void (t += 2000), now: () => t },
    )
    expect(await linkClaudeOrg(d)).toBe(true)
    expect(polls).toBe(3)
    expect(opened).toEqual(["https://claude.example/auth"])
    expect(lines[0]).toBe("Needs the Admin or Billing role; your org must pay Anthropic directly.")
    expect(lines.at(-1)).toBe("Linked. Claude models billed to your organization now show as claude-org/<model>.")
  })

  test("link_claude_org_not_available", async () => {
    const { d } = deps(() => json({ error: { code: "claude_link_unavailable" } }, 403))
    await expect(linkClaudeOrg(d)).rejects.toThrow(NOT_AVAILABLE)
    const off = deps(() => json({ available: false }))
    await expect(linkClaudeOrg(off.d)).rejects.toThrow("Linking a Claude organization isn't available yet.")
    const out = deps(() => json({}), { token: undefined })
    await expect(linkClaudeOrg(out.d)).rejects.toThrow("Sign in first: gizzi auth login")
  })
})

describe("claude-org model names", () => {
  test("linked_model_display_name", () => {
    expect(linkedModelDisplayName("claude-sonnet-5-5")).toBe("Claude Sonnet 5.5 (your organization)")
    expect(linkedModelDisplayName("claude-opus-4-1-20250805")).toBe("Claude Opus 4.1 (your organization)")
    expect(linkedModelDisplayName("mystery")).toBe("mystery (your organization)")
  })
})
