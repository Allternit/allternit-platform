import path from "path"
import fs from "fs/promises"
import { createHash } from "crypto"
import z from "zod/v4"
import { Global } from "@/runtime/context/global"
import { Instance } from "@/runtime/context/project/instance"
import { Filesystem } from "@/shared/util/filesystem"
import { Wildcard } from "@/shared/util/wildcard"
import { Log } from "@/shared/util/log"

/**
 * Project panels: JSON files in `<project>/.allternit/panels/*.json`.
 *
 * Each file is an ops sheet (`"kind": "ops"`) or an MCP App
 * (`"kind": "app"`) that the app shows as a pane beside the Code chat or a
 * strip above its composer. A file may also carry rules for the agent
 * (`agent.instructions`, `agent.guards`). Those rules only apply after the
 * user approves the file in the panel, and an approval is tied to the file's
 * sha256: an edited file needs approving again. Approvals live in the user's
 * data folder, never in the project, so a cloned repo can't approve itself.
 */
export namespace ProjectPanels {
  const log = Log.create({ service: "project-panels" })

  export const DIR = path.join(".allternit", "panels")
  export const MAX_FILES = 50
  export const MAX_BYTES = 256 * 1024
  const MAX_SOURCE_BYTES = 2 * 1024 * 1024
  const SOURCE_TIMEOUT_MS = 10_000

  export const Guard = z.object({
    /** Tool (permission) name, e.g. "bash", "edit", "webfetch". `*` wildcards allowed. */
    tool: z.string().min(1).max(64),
    /** Text the call must contain (a command, a path). `*` wildcards allowed. */
    match: z.string().min(1).max(500),
    /** Ask before the call runs. Only `true` is meaningful today. */
    ask: z.boolean().default(true),
  })
  export type Guard = z.infer<typeof Guard>

  export const Agent = z.object({
    instructions: z.array(z.string().min(1).max(2000)).max(20).default([]),
    guards: z.array(Guard).max(20).default([]),
  })
  export type Agent = z.infer<typeof Agent>

  const UrlSource = z.object({
    url: z.string().url().refine((u) => /^https?:\/\//i.test(u), "Use an http or https URL."),
    /** `connection:<name>`: sign the request with the user's own connection. Never a token. */
    auth: z
      .string()
      .regex(/^connection:[a-z0-9][a-z0-9._-]{0,63}$/i, 'Use "connection:<name>"; never put a token in the file.')
      .optional(),
    label: z.string().max(40).optional(),
    rowsPath: z.string().max(200).optional(),
  })
  const BuiltinSource = z.object({
    allternit: z.string().min(1).max(64),
    args: z.record(z.string(), z.unknown()).optional(),
    label: z.string().max(40).optional(),
    rowsPath: z.string().max(200).optional(),
  })
  const ToolSource = z.object({
    tool: z.string().regex(/^[a-z0-9][a-z0-9._-]*__[a-z0-9._-]+$/i, 'A connector tool is "<connector>__<tool>".'),
    args: z.record(z.string(), z.unknown()).optional(),
    label: z.string().max(40).optional(),
    rowsPath: z.string().max(200).optional(),
  })
  export const Source = z.union([UrlSource, BuiltinSource, ToolSource])
  export type Source = z.infer<typeof Source>

  const Action = z.object({
    label: z.string().min(1).max(40),
    prompt: z.string().min(1).max(4000).optional(),
    tool: z.string().optional(),
  })

  const Common = {
    title: z.string().min(1).max(80),
    place: z.enum(["pane", "strip"]).default("pane"),
    agent: Agent.optional(),
  }

  export const OpsPanel = z
    .object({
      kind: z.literal("ops"),
      ...Common,
      refresh: z.number().int().min(0).max(86_400).optional(),
      sources: z
        .record(
          z.string().regex(/^[a-z][a-z0-9_]{0,31}$/i, "Source names are letters, digits and _.").refine((k) => k !== "header", '"header" is reserved.'),
          Source,
        )
        .default({}),
      actions: z.array(Action).max(4).default([]),
    })
    .passthrough()

  export const AppPanel = z
    .object({
      kind: z.literal("app"),
      ...Common,
      app: z.object({
        /** The user's connector, by id or name. */
        connector: z.string().min(1).max(80),
        /** The `ui://` resource the connector serves the app from. */
        resource: z.string().regex(/^ui:\/\//, 'An MCP App resource starts with "ui://".'),
        tool: z.string().max(120).optional(),
        input: z.record(z.string(), z.unknown()).optional(),
      }),
      actions: z.array(Action).max(4).default([]),
    })
    .passthrough()

  export const Panel = z.discriminatedUnion("kind", [OpsPanel, AppPanel])
  export type Panel = z.infer<typeof Panel>

  export const Info = z.object({
    /** File name inside `.allternit/panels`. */
    file: z.string(),
    /** Path relative to the project root, for display. */
    path: z.string(),
    sha256: z.string(),
    /** Raw file text (the app parses the panel body itself). Empty when too large. */
    content: z.string(),
    /** Why the file can't be used (bad JSON, schema). The app shows it in the panel. */
    error: z.string().nullable(),
    title: z.string(),
    kind: z.enum(["ops", "app"]).nullable(),
    place: z.enum(["pane", "strip"]),
    agent: Agent.nullable(),
    /** Whether this exact content's agent rules (and signed feeds) are approved. */
    approved: z.boolean(),
  })
  export type Info = z.infer<typeof Info>

  export const Listing = z.object({
    root: z.string(),
    dir: z.string(),
    panels: z.array(Info),
  })
  export type Listing = z.infer<typeof Listing>

  // ─── Pure helpers (unit-tested) ────────────────────────────────────────────

  export function sha256(text: string): string {
    return createHash("sha256").update(text, "utf8").digest("hex")
  }

  /** A name the route accepts: a plain `*.json` file name, no folders. */
  export function validFileName(file: string): boolean {
    return /^[A-Za-z0-9][A-Za-z0-9._-]{0,99}\.json$/.test(file) && !file.includes("..")
  }

  export type Parsed = { panel: Panel; error: null } | { panel: null; error: string; title?: string; kind?: "ops" | "app" }

  /** Parse and validate one panel file. Never throws. */
  export function parse(text: string): Parsed {
    let json: unknown
    try {
      json = JSON.parse(text)
    } catch (e) {
      return { panel: null, error: `Not valid JSON: ${e instanceof Error ? e.message : String(e)}` }
    }
    const result = Panel.safeParse(json)
    if (result.success) return { panel: result.data, error: null }
    const raw = json && typeof json === "object" ? (json as Record<string, unknown>) : {}
    const issue = result.error.issues[0]
    const where = issue?.path?.length ? `${issue.path.join(".")}: ` : ""
    return {
      panel: null,
      error: `${where}${issue?.message ?? "Not a panel file."}${result.error.issues.length > 1 ? ` (+${result.error.issues.length - 1} more)` : ""}`,
      title: typeof raw.title === "string" ? raw.title : undefined,
      kind: raw.kind === "ops" || raw.kind === "app" ? raw.kind : undefined,
    }
  }

  export type Approvals = Record<string, Record<string, string>>

  export function isApproved(approvals: Approvals, root: string, file: string, hash: string): boolean {
    return approvals[root]?.[file] === hash
  }

  /** Does the call (permission + pattern) hit one of the guards? */
  export function guardMatches(guards: Guard[], permission: string, pattern: string): Guard | undefined {
    const perm = permission.toLowerCase()
    const text = pattern.toLowerCase()
    return guards.find((g) => {
      if (!g.ask) return false
      const tool = g.tool.toLowerCase()
      if (!(tool === perm || (tool.includes("*") && Wildcard.match(perm, tool)))) return false
      const match = g.match.toLowerCase()
      return match.includes("*") ? Wildcard.match(text, match) : text.includes(match)
    })
  }

  /** The approved instructions as one system block per file. */
  export function instructionBlocks(panels: Array<Pick<Info, "path" | "title" | "agent" | "approved" | "error">>): string[] {
    return panels
      .filter((p) => p.approved && !p.error && p.agent && p.agent.instructions.length)
      .map(
        (p) =>
          `Instructions from project panel ${p.path} ("${p.title}"), approved by the user:\n` +
          p.agent!.instructions.map((line) => `- ${line}`).join("\n"),
      )
  }

  // ─── Files ─────────────────────────────────────────────────────────────────

  const approvalsFile = () => path.join(Global.Path.data, "project-panels-approvals.json")

  export async function readApprovals(): Promise<Approvals> {
    const raw = await Filesystem.readJson<unknown>(approvalsFile()).catch(() => ({}))
    return raw && typeof raw === "object" && !Array.isArray(raw) ? (raw as Approvals) : {}
  }

  let writeQueue: Promise<void> = Promise.resolve()
  async function updateApprovals(mutate: (a: Approvals) => void) {
    const next = writeQueue.then(async () => {
      const current = await readApprovals()
      mutate(current)
      await Filesystem.writeJson(approvalsFile(), current, 0o600)
    })
    writeQueue = next.catch(() => {})
    await next
    cache.clear()
  }

  /**
   * The project root that holds the panels: the nearest folder from `start`
   * up to `stop` (the worktree) with a `.allternit/panels` folder, else `start`.
   */
  export async function findRoot(start: string, stop?: string): Promise<string> {
    let current = path.resolve(start)
    const end = stop && stop !== "/" ? path.resolve(stop) : current
    while (true) {
      if (await Filesystem.exists(path.join(current, DIR))) return current
      if (current === end) break
      const parent = path.dirname(current)
      if (parent === current || !current.startsWith(end)) break
      current = parent
    }
    return path.resolve(start)
  }

  /** The root for the current request or session. */
  export async function currentRoot(): Promise<string> {
    return findRoot(Instance.workdir, Instance.worktree)
  }

  export async function list(root: string): Promise<Listing> {
    const dir = path.join(root, DIR)
    const approvals = await readApprovals()
    let names: string[] = []
    try {
      names = (await fs.readdir(dir)).filter((n) => validFileName(n)).sort().slice(0, MAX_FILES)
    } catch {
      return { root, dir, panels: [] }
    }
    const panels: Info[] = []
    for (const file of names) {
      const full = path.join(dir, file)
      const rel = path.join(DIR, file)
      const fallbackTitle = file.replace(/\.json$/, "")
      try {
        const stat = await fs.stat(full)
        if (!stat.isFile()) continue
        if (stat.size > MAX_BYTES) {
          panels.push({ file, path: rel, sha256: "", content: "", error: `The file is over ${MAX_BYTES / 1024} KB.`, title: fallbackTitle, kind: null, place: "pane", agent: null, approved: false })
          continue
        }
        const content = await fs.readFile(full, "utf8")
        const hash = sha256(content)
        const parsed = parse(content)
        const panel = parsed.panel
        // strictNullChecks is off, so the union doesn't narrow: read the failure fields directly.
        const failed = parsed as { title?: string; kind?: "ops" | "app" }
        panels.push({
          file,
          path: rel,
          sha256: hash,
          content,
          error: parsed.error,
          title: panel?.title ?? failed.title ?? fallbackTitle,
          kind: panel?.kind ?? failed.kind ?? null,
          place: panel?.place ?? "pane",
          agent: panel?.agent ?? null,
          approved: isApproved(approvals, root, file, hash),
        })
      } catch (e) {
        panels.push({ file, path: rel, sha256: "", content: "", error: `Couldn't read the file: ${e instanceof Error ? e.message : String(e)}`, title: fallbackTitle, kind: null, place: "pane", agent: null, approved: false })
      }
    }
    return { root, dir, panels }
  }

  export class StaleError extends Error {
    constructor() {
      super("The panel file changed since you looked at it. Review it again.")
    }
  }
  export class NotFoundError extends Error {
    constructor(file: string) {
      super(`No panel file "${file}".`)
    }
  }

  async function readPanelFile(root: string, file: string): Promise<string> {
    if (!validFileName(file)) throw new NotFoundError(file)
    try {
      return await fs.readFile(path.join(root, DIR, file), "utf8")
    } catch {
      throw new NotFoundError(file)
    }
  }

  /** Approve the file's rules for exactly this content (the hash the user saw). */
  export async function approve(root: string, file: string, hash: string): Promise<void> {
    const content = await readPanelFile(root, file)
    if (sha256(content) !== hash) throw new StaleError()
    await updateApprovals((a) => {
      a[root] = { ...(a[root] ?? {}), [file]: hash }
    })
    log.info("approved", { root, file })
  }

  export async function revoke(root: string, file: string): Promise<void> {
    await updateApprovals((a) => {
      if (!a[root]) return
      delete a[root][file]
      if (!Object.keys(a[root]).length) delete a[root]
    })
  }

  // ─── Applying approved rules to sessions ──────────────────────────────────

  const CACHE_MS = 3_000
  const cache = new Map<string, { at: number; listing: Promise<Listing> }>()

  function cachedList(root: string): Promise<Listing> {
    const hit = cache.get(root)
    if (hit && Date.now() - hit.at < CACHE_MS) return hit.listing
    const listing = list(root)
    cache.set(root, { at: Date.now(), listing })
    return listing
  }

  /** Approved `agent.instructions` for the current session's project. */
  export async function instructions(): Promise<string[]> {
    try {
      const root = await currentRoot()
      return instructionBlocks((await cachedList(root)).panels)
    } catch {
      return []
    }
  }

  /** An approved guard that matches this call, if any. */
  export async function guardFor(permission: string, pattern: string): Promise<{ guard: Guard; panel: string } | undefined> {
    try {
      const root = await currentRoot()
      for (const p of (await cachedList(root)).panels) {
        if (!p.approved || p.error || !p.agent?.guards.length) continue
        const guard = guardMatches(p.agent.guards, permission, pattern)
        if (guard) return { guard, panel: p.title }
      }
    } catch {
      /* no panels folder or no instance: nothing to guard */
    }
    return undefined
  }

  // ─── Live feeds ────────────────────────────────────────────────────────────

  export class SourceError extends Error {
    constructor(
      message: string,
      readonly status: 400 | 403 | 404 | 409 | 502,
    ) {
      super(message)
    }
  }

  /** The bearer token of the user's connection `<name>` (an MCP server they signed in to). */
  async function connectionToken(name: string): Promise<string | undefined> {
    const { McpAuth } = await import("@/runtime/tools/mcp/auth")
    const entry = await McpAuth.get(name)
    if (!entry?.tokens?.accessToken || entry.revoked) return undefined
    if (entry.tokens.expiresAt && entry.tokens.expiresAt * 1000 < Date.now()) return undefined
    return entry.tokens.accessToken
  }

  /**
   * Fetch one URL source of a panel, as JSON. The URL always comes from the
   * file (never from the caller), and a `connection:` source is only signed
   * after the user approved this exact file content.
   */
  export async function fetchSource(root: string, file: string, key: string): Promise<unknown> {
    const content = await readPanelFile(root, file).catch(() => {
      throw new SourceError(`No panel file "${file}".`, 404)
    })
    const parsed = parse(content)
    if (!parsed.panel || parsed.panel.kind !== "ops") throw new SourceError(parsed.error ?? "Not an ops panel.", 400)
    const source = parsed.panel.sources[key]
    if (!source) throw new SourceError(`No source "${key}" in ${file}.`, 404)
    if (!("url" in source)) throw new SourceError(`Source "${key}" is not a URL source; the app reads it directly.`, 400)
    const headers: Record<string, string> = { accept: "application/json" }
    if (source.auth) {
      const approvals = await readApprovals()
      if (!isApproved(approvals, root, file, sha256(content))) {
        throw new SourceError(`Approve "${parsed.panel.title}" to read ${new URL(source.url).host} with your ${source.auth.slice(11)} connection.`, 403)
      }
      const token = await connectionToken(source.auth.slice("connection:".length))
      if (!token) throw new SourceError(`Connect ${source.auth.slice(11)} to read this feed.`, 409)
      headers.authorization = `Bearer ${token}`
    }
    const { createGuardedFetch } = await import("@/shared/utils/hooks/ssrfGuard")
    const guarded = createGuardedFetch({ allowLoopback: false })
    let res: Response
    try {
      res = await guarded(source.url, { headers, signal: AbortSignal.timeout(SOURCE_TIMEOUT_MS) })
    } catch (e) {
      throw new SourceError(`Couldn't reach ${new URL(source.url).host}: ${e instanceof Error ? e.message : String(e)}`, 502)
    }
    if (!res.ok) throw new SourceError(`${new URL(source.url).host} answered ${res.status}.`, 502)
    const length = Number(res.headers.get("content-length") ?? 0)
    if (length > MAX_SOURCE_BYTES) throw new SourceError("The feed is over 2 MB.", 502)
    const text = await res.text()
    if (text.length > MAX_SOURCE_BYTES) throw new SourceError("The feed is over 2 MB.", 502)
    try {
      return JSON.parse(text)
    } catch {
      throw new SourceError(`${new URL(source.url).host} didn't answer with JSON.`, 502)
    }
  }
}
