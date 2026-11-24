/**
 * `gizzi auth apps|link|unlink` logic: connected apps (OAuth grants) and the
 * Claude organization link. Every function takes its dependencies so tests run
 * against a fake server. Calls use the person's own Allternit sign-in.
 */
export const SIGN_IN_FIRST = "Sign in first: gizzi auth login"
export const NOT_AVAILABLE = "Linking a Claude organization isn't available yet."
export const ELIGIBILITY = "Needs the Admin or Billing role; your org must pay Anthropic directly."

export type Grant = {
  id: string
  app_name: string
  app_state?: string
  monthly_cap_cents: number
  spent_cents: number
  last_used_at: string | null
}

export type Deps = {
  origin: string
  token: string | undefined
  fetch: typeof fetch
  out: (line: string) => void
  confirm?: (message: string) => Promise<boolean>
  open?: (url: string) => Promise<void>
  sleep?: (ms: number) => Promise<void>
  now?: () => number
}

export class CliError extends Error {}

const dollars = (cents: number) => `$${(cents / 100).toFixed(2)}`

async function call(d: Deps, method: string, path: string, body?: unknown): Promise<Response> {
  if (!d.token) throw new CliError(SIGN_IN_FIRST)
  const res = await d.fetch(`${d.origin}${path}`, {
    method,
    headers: {
      authorization: `Bearer ${d.token}`,
      accept: "application/json",
      ...(body === undefined ? {} : { "content-type": "application/json" }),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  if (res.status === 401) throw new CliError(SIGN_IN_FIRST)
  return res
}

async function serverError(res: Response): Promise<string> {
  const text = await res.text().catch(() => "")
  try {
    const j = JSON.parse(text) as { error?: string | { message?: string }; message?: string }
    const m = typeof j.error === "string" ? j.error : j.error?.message ?? j.message
    if (m) return m
  } catch {}
  return text.trim() || `request failed (${res.status})`
}

async function listGrants(d: Deps): Promise<{ month?: string; resets_at?: string; data: Grant[] }> {
  const res = await call(d, "GET", "/api/v1/oauth/grants")
  if (!res.ok) throw new CliError(await serverError(res))
  const body = (await res.json()) as { month?: string; resets_at?: string; data?: Grant[] }
  return { ...body, data: body.data ?? [] }
}

/** Find by id or by name (case-insensitive); more than one name match is an error. */
function pick(grants: Grant[], key: string): Grant {
  const byId = grants.find((g) => g.id === key)
  if (byId) return byId
  const named = grants.filter((g) => g.app_name.toLowerCase() === key.toLowerCase())
  if (named.length === 1) return named[0]!
  if (named.length > 1) throw new CliError(`More than one app is named "${key}". Use its id.`)
  throw new CliError(`No connected app "${key}". Run: gizzi auth apps list`)
}

export async function appsList(d: Deps, opts: { json?: boolean } = {}): Promise<void> {
  const body = await listGrants(d)
  if (opts.json) {
    d.out(JSON.stringify(body, null, 2))
    return
  }
  if (body.data.length === 0) {
    d.out("No connected apps.")
    return
  }
  const rows = body.data.map((g) => [
    g.app_name,
    g.app_state ?? "",
    `${dollars(g.spent_cents)} / ${dollars(g.monthly_cap_cents)}`,
    g.last_used_at ?? "never",
    g.id,
  ])
  const head = ["NAME", "STATE", "SPENT / CAP", "LAST USED", "ID"]
  const widths = head.map((h, i) => Math.max(h.length, ...rows.map((r) => r[i]!.length)))
  const fmt = (r: string[]) => r.map((c, i) => c.padEnd(widths[i]!)).join("  ").trimEnd()
  d.out(fmt(head))
  for (const r of rows) d.out(fmt(r))
  if (body.month) d.out(`Spend is for ${body.month}${body.resets_at ? `; resets ${body.resets_at}` : ""}.`)
}

export async function appsCap(d: Deps, key: string, amount: string): Promise<void> {
  const value = Number(amount.replace(/^\$/, ""))
  if (!Number.isFinite(value) || value < 0) throw new CliError(`"${amount}" is not a dollar amount.`)
  const cents = Math.round(value * 100)
  const grant = pick((await listGrants(d)).data, key)
  const res = await call(d, "PATCH", `/api/v1/oauth/grants/${grant.id}`, { monthly_cap_cents: cents })
  if (!res.ok) throw new CliError(await serverError(res))
  d.out(`${grant.app_name}: monthly cap is now ${dollars(cents)}.`)
}

export async function appsRevoke(d: Deps, key: string, opts: { yes?: boolean } = {}): Promise<boolean> {
  const grant = pick((await listGrants(d)).data, key)
  if (!opts.yes) {
    const ok = d.confirm ? await d.confirm(`Revoke ${grant.app_name}? It loses access to your account right away.`) : false
    if (!ok) {
      d.out("Not revoked. Pass --yes to skip the question.")
      return false
    }
  }
  const res = await call(d, "DELETE", `/api/v1/oauth/grants/${grant.id}`)
  if (!res.ok && res.status !== 204) throw new CliError(await serverError(res))
  d.out(`Revoked ${grant.app_name}.`)
  return true
}

type LinkStatus = { state?: string; linked_at?: string | null; relink_by?: string | null; spend_limit?: { reached?: boolean } | null }

async function claudeStatus(d: Deps): Promise<LinkStatus> {
  const res = await call(d, "GET", "/api/v1/claude-link")
  if (res.status === 403 || res.status === 404) throw new CliError(NOT_AVAILABLE)
  if (!res.ok) throw new CliError(await serverError(res))
  return (await res.json()) as LinkStatus
}

export async function linkClaudeOrg(d: Deps): Promise<boolean> {
  d.out(ELIGIBILITY)
  const res = await call(d, "POST", "/api/v1/claude-link/start", {})
  if (res.status === 403 || res.status === 404 || res.status === 503) throw new CliError(NOT_AVAILABLE)
  if (!res.ok) {
    const text = await serverError(res)
    throw new CliError(/not_available|not available/i.test(text) ? NOT_AVAILABLE : text)
  }
  const body = (await res.json()) as { available?: boolean; login?: { url?: string; expires_at?: string } }
  if (body.available === false || !body.login?.url) throw new CliError(NOT_AVAILABLE)
  const url = body.login.url
  d.out(`Open this link and approve access: ${url}`)
  if (d.open) await d.open(url).catch(() => undefined)
  const now = d.now ?? Date.now
  const sleep = d.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)))
  const expires = body.login.expires_at ? Date.parse(body.login.expires_at) : now() + 10 * 60_000
  while (now() < expires) {
    const st = await claudeStatus(d)
    if (st.state === "linked") {
      d.out("Linked. Claude models billed to your organization now show as claude-org/<model>.")
      return true
    }
    await sleep(2_000)
  }
  d.out("The link expired before it was approved. Run: gizzi auth link claude-org")
  return false
}

export async function unlinkClaudeOrg(d: Deps, opts: { yes?: boolean } = {}): Promise<boolean> {
  if (!opts.yes) {
    const ok = d.confirm ? await d.confirm("Unlink your Claude organization? claude-org models stop working.") : false
    if (!ok) {
      d.out("Not unlinked. Pass --yes to skip the question.")
      return false
    }
  }
  const res = await call(d, "DELETE", "/api/v1/claude-link")
  if (res.status === 403 || res.status === 404) throw new CliError(NOT_AVAILABLE)
  if (!res.ok && res.status !== 204) throw new CliError(await serverError(res))
  d.out("Unlinked your Claude organization.")
  return true
}

export function claudeLinkLine(st: LinkStatus | undefined): string {
  if (!st || st.state === undefined || st.state === "unlinked") return "Claude organization: not linked"
  if (st.state === "linked") return st.spend_limit?.reached ? "Claude organization: needs attention (spend limit reached)" : "Claude organization: linked"
  if (st.state === "relink_needed") return `Claude organization: needs attention (link again${st.relink_by ? ` by ${st.relink_by}` : ""}: gizzi auth link claude-org)`
  return "Claude organization: needs attention (disconnected: gizzi auth link claude-org)"
}

export async function claudeStatusLine(d: Deps): Promise<string | undefined> {
  if (!d.token) return undefined
  try {
    return claudeLinkLine(await claudeStatus(d))
  } catch {
    return undefined
  }
}
