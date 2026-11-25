/**
 * Cloud client + pure helpers for `gizzi app` and `gizzi share`
 * (Apps + MCP 2026, plan v3 lane 43). Talks to cloud-api:
 *   GET  /api/v1/apps                       shelf {yours, servers}
 *   POST /api/v1/apps/:kind/:ref/install    {granted_tier, consent?}
 *   GET|PUT /api/v2/artifacts/:id/app
 *   GET|POST /api/v1/share-links, DELETE /:id, POST /:id/pause
 */
import { cloudInferenceToken, cloudOrigin } from "@/runtime/providers/discovery/allternit-cloud"

export const APP_DOMAIN = (process.env.ALLTERNIT_APP_DOMAIN || "allternitusercontent.com").trim()
export const LEVELS = ["browser", "app", "full"] as const
export type Level = (typeof LEVELS)[number]

export type ShelfApp = {
  kind: "artifact" | "vendor"
  ref: string
  name: string
  icon?: string | null
  provenance: string
  tier: string
  installed: boolean
  pinned?: boolean
  link?: string
}

export type ShareLink = {
  id: string
  kind: string
  subject: string
  label?: string | null
  subdomain: string
  target_port?: number | null
  auth_mode?: string
  status: string
  url?: string | null
}

export class AppsError extends Error {
  constructor(
    readonly code: string,
    readonly status: number,
    message: string,
  ) {
    super(message)
  }
}

export async function cloud<T>(method: string, path: string, body?: unknown): Promise<T> {
  const token = await cloudInferenceToken().catch(() => undefined)
  if (!token) throw new AppsError("unauthorized", 401, "Not signed in. Run `gizzi login`, or set ALLTERNIT_API_TOKEN.")
  let res: Response
  try {
    res = await fetch(`${cloudOrigin()}${path}`, {
      method,
      headers: {
        Accept: "application/json",
        Authorization: `Bearer ${token}`,
        ...(body === undefined ? {} : { "content-type": "application/json" }),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(20_000),
    })
  } catch (e) {
    throw new AppsError("offline", 0, `Couldn't reach Allternit Cloud: ${(e as Error).message}`)
  }
  const text = await res.text()
  if (res.status === 401) throw new AppsError("unauthorized", 401, "Not signed in. Run `gizzi login`, or set ALLTERNIT_API_TOKEN.")
  if (!res.ok) {
    let code = `http_${res.status}`
    let message = `Allternit Cloud answered ${res.status}.`
    try {
      const j = JSON.parse(text) as { error?: string; message?: string }
      if (j.error) code = j.error
      if (j.message) message = j.message
    } catch {}
    throw new AppsError(code, res.status, message)
  }
  return (text ? JSON.parse(text) : {}) as T
}

/** The slug of an app from its shelf link (`/app/<slug>`). Vendor apps (`/app/vendor/<id>`) have none. */
export function slugOf(app: Pick<ShelfApp, "link" | "kind">): string | undefined {
  if (app.kind !== "artifact") return undefined
  const m = /^\/app\/([a-z][a-z0-9]{0,15})$/.exec(app.link ?? "")
  return m?.[1]
}

/** The app's own address: https://<slug>.<APP_DOMAIN>. */
export function appLink(slug: string, domain: string = APP_DOMAIN): string {
  return `https://${slug}.${domain}`
}

export const validSlug = (s: string) => /^[a-z][a-z0-9]{0,15}$/.test(s)

export async function shelf(): Promise<ShelfApp[]> {
  const r = await cloud<{ yours?: ShelfApp[]; servers?: ShelfApp[] }>("GET", "/api/v1/apps")
  return [...(r.yours ?? []), ...(r.servers ?? [])]
}

export async function findApp(slug: string): Promise<ShelfApp> {
  const all = await shelf()
  const hit = all.find(a => slugOf(a) === slug)
  if (!hit) throw new AppsError("not_found", 404, `No app "${slug}" on your shelf. Run \`gizzi app list\`.`)
  return hit
}

export function pad(rows: string[][]): string {
  const widths = rows[0]!.map((_, i) => Math.max(...rows.map(r => (r[i] ?? "").length)))
  return rows.map(r => r.map((c, i) => (i === r.length - 1 ? c : c.padEnd(widths[i]!))).join("  ").trimEnd()).join("\n")
}

export function formatApps(apps: ShelfApp[]): string {
  if (apps.length === 0) return "No apps yet. Make an artifact an app in Allternit, then run `gizzi app list`."
  return pad([
    ["NAME", "SLUG", "PROVENANCE", "LEVEL", "INSTALLED"],
    ...apps.map(a => [a.name, slugOf(a) ?? "-", a.provenance, a.tier, a.installed ? "yes" : "no"]),
  ])
}

export function formatLinks(links: ShareLink[]): string {
  if (links.length === 0) return "No share links."
  return pad([
    ["ID", "KIND", "SUBJECT", "STATUS", "URL"],
    ...links.map(l => [
      l.id,
      l.kind,
      l.subject === "port" && l.target_port ? `port ${l.target_port}` : l.label || l.subdomain,
      l.status,
      l.url ?? "-",
    ]),
  ])
}

export function consentLines(r: { requested_tier?: string; added_capabilities?: string[] }): string[] {
  const lines = [`This app asks for the "${r.requested_tier ?? "browser"}" level.`]
  const caps = r.added_capabilities ?? []
  if (caps.length) lines.push("It can use: " + caps.join(", "))
  return lines
}

export const listLinks = (includeRevoked = false) =>
  cloud<{ links: ShareLink[] }>("GET", `/api/v1/share-links${includeRevoked ? "?include_revoked=true" : ""}`).then(r => r.links ?? [])
export const revokeLink = (id: string) => cloud<{ revoked: boolean; link: ShareLink }>("DELETE", `/api/v1/share-links/${encodeURIComponent(id)}`)
export const pauseLink = (id: string, paused = true) =>
  cloud<{ link: ShareLink }>("POST", `/api/v1/share-links/${encodeURIComponent(id)}/pause`, { paused })

export function fail(e: unknown): never {
  process.stderr.write(`${e instanceof Error ? e.message : String(e)}\n`)
  process.exit(1)
}
