/**
 * Linked PRs: GitHub pull requests a code session created, pushed to, or
 * worked on, recorded on the session so the surfaces can show PR badges on
 * session rows (Code mode rail) and an "Open PR" action.
 *
 * Contract A in docs/CODE_PARITY_MAP.md:
 *   - Detection runs on every completed tool part (Session.updatePart). Only
 *     shell commands that create/view/push PRs (`gh pr create|view|edit|ready|
 *     merge|reopen|comment`, `gh api …/pulls`, `git push`) and GitHub MCP
 *     pull-request tools count. File reads and web fetches never do.
 *     `pull/new/<branch>` hints are not PRs and are ignored (the regex needs a
 *     PR number).
 *   - Status comes from `gh api repos/<o>/<r>/pulls/<n>` when the gh CLI is
 *     installed and signed in, else GitHub REST with GITHUB_TOKEN / GH_TOKEN
 *     (or anonymously for public repos). Results are cached for 60s per PR,
 *     refreshed in the background, and a failed refresh keeps the last values.
 *   - Persisted per session in Storage ["session_prs", sessionID], dedup by url.
 *   - `session.prs.updated` { sessionID, prs } fires when the list or a status
 *     changes.
 *
 * Everything here is best-effort: the detector is wrapped so it can never
 * throw into the tool pipeline, and nothing awaits a GitHub call on a turn.
 */
import z from "zod/v4"
import { existsSync } from "fs"
import { BusEvent } from "@/shared/bus/bus-event"
import { Bus } from "@/shared/bus"
import { Storage } from "@/runtime/session/storage/storage"
import { Database, eq } from "@/runtime/session/storage/db"
import { SessionTable } from "@/runtime/session/session.sql"
import { Log } from "@/shared/util/log"

export namespace LinkedPRs {
  const log = Log.create({ service: "session.linked-prs" })

  export const Info = z
    .object({
      url: z.string(),
      owner: z.string(),
      repo: z.string(),
      number: z.number(),
      title: z.string(),
      state: z.enum(["open", "draft", "merged", "closed"]),
      mergeable: z.enum(["clean", "conflict", "unknown"]),
      additions: z.number(),
      deletions: z.number(),
      updatedAt: z.string(),
      detectedAt: z.string(),
    })
    .meta({ ref: "LinkedPR" })
  export type Info = z.infer<typeof Info>

  export const Event = {
    Updated: BusEvent.define(
      "session.prs.updated",
      z.object({
        sessionID: z.string(),
        prs: z.array(Info),
      }),
    ),
  }

  /** Fresh enough: open/draft PRs refresh after 60s, finished ones after 10 min. */
  const OPEN_TTL_MS = 60_000
  const DONE_TTL_MS = 10 * 60_000
  const FETCH_TIMEOUT_MS = 10_000
  const MAX_CONCURRENT_FETCHES = 3
  const MAX_SCAN_CHARS = 200_000

  // ── URL parsing ────────────────────────────────────────────────────────

  const WEB_PR = /https?:\/\/github\.com\/([A-Za-z0-9][A-Za-z0-9-]*)\/([A-Za-z0-9._-]+)\/pull\/(\d+)/g
  const API_PR = /https?:\/\/api\.github\.com\/repos\/([A-Za-z0-9][A-Za-z0-9-]*)\/([A-Za-z0-9._-]+)\/pulls\/(\d+)/g

  export type Ref = { url: string; owner: string; repo: string; number: number }

  function ref(owner: string, repo: string, number: string | number): Ref | undefined {
    const n = Number(number)
    if (!Number.isInteger(n) || n <= 0) return undefined
    const cleanRepo = repo.replace(/\.git$/, "")
    if (!cleanRepo) return undefined
    return { owner, repo: cleanRepo, number: n, url: `https://github.com/${owner}/${cleanRepo}/pull/${n}` }
  }

  /** Every distinct PR URL in `text` (web or API form), canonicalised. */
  export function extract(text: string): Ref[] {
    if (!text) return []
    const scan = text.length > MAX_SCAN_CHARS ? text.slice(0, MAX_SCAN_CHARS / 2) + "\n" + text.slice(-MAX_SCAN_CHARS / 2) : text
    const found = new Map<string, Ref>()
    for (const re of [WEB_PR, API_PR]) {
      re.lastIndex = 0
      for (const m of scan.matchAll(re)) {
        const r = ref(m[1]!, m[2]!, m[3]!)
        if (r) found.set(r.url.toLowerCase(), r)
      }
    }
    return [...found.values()]
  }

  /** Parse a single user-supplied PR URL (manual link). */
  export function parse(url: string): Ref | undefined {
    return extract(String(url ?? "").trim())[0]
  }

  // ── Detection ──────────────────────────────────────────────────────────

  const SHELL_TOOLS = new Set([
    "bash",
    "shell",
    "sh",
    "zsh",
    "exec_command",
    "run_shell_command",
    "local_shell",
    "shell_command",
    "terminal",
    "run_terminal_cmd",
    "execute_command",
  ])

  const PR_COMMAND = [
    /\bgh\s+pr\s+(create|new|view|edit|ready|merge|reopen|comment)\b/,
    /\bgh\s+api\b[^\n]*\/pulls\b/,
    /\bgit\s+push\b/,
  ]

  const MCP_PR_TOOL = /create_pull_request|pull_request_create|createpullrequest|update_pull_request|merge_pull_request|get_pull_request/i

  function commandOf(input: any): string {
    if (!input || typeof input !== "object") return ""
    const raw = input.command ?? input.cmd ?? input.script ?? ""
    if (Array.isArray(raw)) return raw.map(String).join(" ")
    return typeof raw === "string" ? raw : ""
  }

  function outputOf(state: any): string {
    const parts: string[] = []
    if (typeof state?.output === "string") parts.push(state.output)
    const metaOut = state?.metadata?.output
    if (typeof metaOut === "string" && metaOut !== state?.output) parts.push(metaOut)
    return parts.join("\n")
  }

  /**
   * Which PRs (if any) a completed tool part links to its session. Pure and
   * cheap: a tool-name check and a couple of regexes over the output.
   */
  export function detect(part: { tool: string; state: any }): { refs: Ref[]; draft: boolean } {
    const tool = String(part.tool ?? "")
    const lower = tool.toLowerCase()
    if (SHELL_TOOLS.has(lower)) {
      const command = commandOf(part.state?.input)
      if (!command || !PR_COMMAND.some((re) => re.test(command))) return { refs: [], draft: false }
      const draft = /\bgh\s+pr\s+(create|new)\b[^\n]*(--draft|\s-d\b)/.test(command)
      return { refs: extract(outputOf(part.state)), draft }
    }
    if (MCP_PR_TOOL.test(lower) || (lower.includes("github") && lower.includes("pull"))) {
      const draft = part.state?.input?.draft === true
      return { refs: extract(outputOf(part.state)), draft }
    }
    return { refs: [], draft: false }
  }

  /**
   * Called for every part write. Never throws and never blocks: detection is
   * synchronous and cheap, recording and status fetches run detached.
   */
  export function observe(part: any) {
    try {
      if (!part || part.type !== "tool" || part.state?.status !== "completed") return
      const { refs, draft } = detect(part)
      if (refs.length === 0) return
      const sessionID = String(part.sessionID ?? "")
      if (!sessionID) return
      void record(sessionID, refs, { draft }).catch((error) => log.warn("record failed", { sessionID, error }))
    } catch (error) {
      log.warn("detect failed", { error })
    }
  }

  // ── Persistence ────────────────────────────────────────────────────────

  const locks = new Map<string, Promise<unknown>>()
  function serialize<T>(sessionID: string, fn: () => Promise<T>): Promise<T> {
    const prev = locks.get(sessionID) ?? Promise.resolve()
    const next = prev.catch(() => {}).then(fn)
    locks.set(sessionID, next)
    void next.finally(() => {
      if (locks.get(sessionID) === next) locks.delete(sessionID)
    }).catch(() => {})
    return next
  }

  async function load(sessionID: string): Promise<Info[]> {
    try {
      const data = await Storage.read<{ prs?: Info[] }>(["session_prs", sessionID])
      const prs = Array.isArray(data?.prs) ? data.prs : []
      return prs.filter((p) => p && typeof p.url === "string")
    } catch {
      return []
    }
  }

  async function save(sessionID: string, prs: Info[]) {
    if (prs.length === 0) {
      await Storage.remove(["session_prs", sessionID]).catch(() => {})
      return
    }
    await Storage.write(["session_prs", sessionID], { prs })
  }

  function publish(sessionID: string, prs: Info[]) {
    try {
      void Promise.resolve(Bus.publish(Event.Updated, { sessionID, prs })).catch(() => {})
    } catch (error) {
      // No instance context (e.g. a detached refresh after the instance was
      // disposed): the next GET returns the saved list anyway.
      log.debug("publish skipped", { sessionID, error: String(error) })
    }
  }

  /** Root session of a subagent chain, so a PR opened by a subagent also shows on the parent row. */
  function rootOf(sessionID: string): string | undefined {
    try {
      let current = sessionID
      for (let i = 0; i < 16; i++) {
        const row = Database.use((db) =>
          db.select({ parent: SessionTable.parent_id }).from(SessionTable).where(eq(SessionTable.id, current)).get(),
        )
        if (!row?.parent) return current === sessionID ? undefined : current
        current = row.parent
      }
      return current
    } catch {
      return undefined
    }
  }

  function placeholder(r: Ref, now: string, draft: boolean): Info {
    const cached = statusCache.get(r.url)?.data
    if (cached) return { ...cached, detectedAt: now }
    return {
      ...r,
      title: "",
      state: draft ? "draft" : "open",
      mergeable: "unknown",
      additions: 0,
      deletions: 0,
      updatedAt: now,
      detectedAt: now,
    }
  }

  /** Add refs to a session (and its root session). Dedup by url; existing entries keep their detectedAt. */
  export async function record(sessionID: string, refs: Ref[], opts: { draft?: boolean } = {}) {
    const targets = [sessionID]
    const root = rootOf(sessionID)
    if (root && root !== sessionID) targets.push(root)
    let result: Info[] = []
    for (const target of targets) {
      const prs = await serialize(target, async () => {
        const current = await load(target)
        const known = new Set(current.map((p) => p.url.toLowerCase()))
        const now = new Date().toISOString()
        const added = refs.filter((r) => !known.has(r.url.toLowerCase())).map((r) => placeholder(r, now, !!opts.draft))
        if (added.length === 0) return current
        const next = [...current, ...added]
        await save(target, next)
        publish(target, next)
        return next
      })
      if (target === sessionID) result = prs
    }
    // Background status fetch for what was just seen (cache-aware).
    for (const r of refs) void refreshUrl(r, { force: false, sessions: targets }).catch(() => {})
    return result
  }

  export async function list(sessionID: string): Promise<Info[]> {
    return load(sessionID)
  }

  /** All sessions with linked PRs (bulk route with no ids). */
  export async function listAll(): Promise<Record<string, Info[]>> {
    const keys = await Storage.list(["session_prs"]).catch(() => [] as string[][])
    const out: Record<string, Info[]> = {}
    await Promise.all(
      keys.map(async (key) => {
        const id = key[key.length - 1]
        if (!id) return
        const prs = await load(id)
        if (prs.length) out[id] = prs
      }),
    )
    return out
  }

  export async function unlink(sessionID: string, url: string): Promise<Info[]> {
    const target = parse(url)?.url.toLowerCase() ?? String(url).trim().toLowerCase()
    return serialize(sessionID, async () => {
      const current = await load(sessionID)
      const next = current.filter((p) => p.url.toLowerCase() !== target)
      if (next.length !== current.length) {
        await save(sessionID, next)
        publish(sessionID, next)
      }
      return next
    })
  }

  export async function removeSession(sessionID: string) {
    await Storage.remove(["session_prs", sessionID]).catch(() => {})
    statusSessions.forEach((set) => set.delete(sessionID))
  }

  // ── Status fetching ────────────────────────────────────────────────────

  type CacheEntry = { data?: Info; fetchedAt: number; inflight?: Promise<Info | undefined> }
  const statusCache = new Map<string, CacheEntry>()
  /** url → sessions that link it, so one fetch updates every row. */
  const statusSessions = new Map<string, Set<string>>()

  let active = 0
  const waiting: Array<() => void> = []
  async function slot<T>(fn: () => Promise<T>): Promise<T> {
    if (active >= MAX_CONCURRENT_FETCHES) await new Promise<void>((resolve) => waiting.push(resolve))
    active++
    try {
      return await fn()
    } finally {
      active--
      waiting.shift()?.()
    }
  }

  function ttl(info: Info | undefined) {
    return info && (info.state === "merged" || info.state === "closed") ? DONE_TTL_MS : OPEN_TTL_MS
  }

  function isStale(url: string, data?: Info) {
    const entry = statusCache.get(url)
    if (!entry) return true
    return Date.now() - entry.fetchedAt >= ttl(entry.data ?? data)
  }

  /** GitHub pulls API JSON → status fields of LinkedPR. */
  export function fromGitHub(r: Ref, json: any): Omit<Info, "detectedAt"> | undefined {
    if (!json || typeof json !== "object" || typeof json.number !== "number") return undefined
    const state: Info["state"] = json.merged_at || json.merged === true
      ? "merged"
      : json.state === "closed"
        ? "closed"
        : json.draft === true
          ? "draft"
          : "open"
    const mergeable: Info["mergeable"] =
      json.mergeable === false || json.mergeable_state === "dirty"
        ? "conflict"
        : json.mergeable === true
          ? "clean"
          : "unknown"
    return {
      url: r.url,
      owner: r.owner,
      repo: r.repo,
      number: r.number,
      title: typeof json.title === "string" ? json.title : "",
      state,
      mergeable,
      additions: Number(json.additions) || 0,
      deletions: Number(json.deletions) || 0,
      updatedAt: typeof json.updated_at === "string" ? json.updated_at : new Date().toISOString(),
    }
  }

  const GH_CANDIDATES = ["gh", "/opt/homebrew/bin/gh", "/usr/local/bin/gh", "/usr/bin/gh"]
  let ghBinary: string | null | undefined
  function ghPath(): string | null {
    if (ghBinary !== undefined) return ghBinary
    ghBinary = null
    try {
      for (const candidate of GH_CANDIDATES) {
        const found = candidate.includes("/") ? (existsSync(candidate) ? candidate : null) : Bun.which(candidate)
        if (found) {
          ghBinary = found
          break
        }
      }
    } catch {
      ghBinary = null
    }
    return ghBinary
  }
  /** gh failed auth/network: skip it for a while instead of spawning per PR. */
  let ghBackoffUntil = 0

  async function viaGh(r: Ref): Promise<any | undefined> {
    const bin = ghPath()
    if (!bin || Date.now() < ghBackoffUntil) return undefined
    try {
      const proc = Bun.spawn([bin, "api", `repos/${r.owner}/${r.repo}/pulls/${r.number}`], {
        stdin: "ignore",
        stdout: "pipe",
        stderr: "pipe",
        env: { ...process.env, GH_PROMPT_DISABLED: "1", NO_COLOR: "1" },
      })
      const timer = setTimeout(() => proc.kill(), FETCH_TIMEOUT_MS)
      const [out, code] = await Promise.all([new Response(proc.stdout).text(), proc.exited])
      clearTimeout(timer)
      if (code !== 0) {
        const err = await new Response(proc.stderr).text().catch(() => "")
        // Not signed in / bad credentials: fall back to REST for 5 minutes.
        if (/auth|login|credential|401|403/i.test(err)) ghBackoffUntil = Date.now() + 5 * 60_000
        if (/404|Not Found/i.test(err)) return { __notFound: true }
        return undefined
      }
      return JSON.parse(out)
    } catch {
      return undefined
    }
  }

  async function viaRest(r: Ref): Promise<any | undefined> {
    const token = process.env.GITHUB_TOKEN || process.env.GH_TOKEN
    const headers: Record<string, string> = {
      Accept: "application/vnd.github+json",
      "X-GitHub-Api-Version": "2022-11-28",
      "User-Agent": "gizzi-code",
    }
    if (token) headers.Authorization = `Bearer ${token}`
    try {
      const res = await fetch(`https://api.github.com/repos/${r.owner}/${r.repo}/pulls/${r.number}`, {
        headers,
        signal: AbortSignal.timeout(FETCH_TIMEOUT_MS),
      })
      if (!res.ok) return undefined
      return await res.json()
    } catch {
      return undefined
    }
  }

  async function fetchStatus(r: Ref): Promise<Omit<Info, "detectedAt"> | undefined> {
    const fromCli = await viaGh(r)
    if (fromCli && !fromCli.__notFound) return fromGitHub(r, fromCli)
    return fromGitHub(r, await viaRest(r))
  }

  function sameStatus(a: Info, b: Omit<Info, "detectedAt">) {
    return (
      a.title === b.title &&
      a.state === b.state &&
      a.mergeable === b.mergeable &&
      a.additions === b.additions &&
      a.deletions === b.deletions &&
      a.updatedAt === b.updatedAt
    )
  }

  /** Write a fetched status into every session that links the url. */
  async function apply(status: Omit<Info, "detectedAt">, sessions: Iterable<string>) {
    for (const sessionID of sessions) {
      await serialize(sessionID, async () => {
        const current = await load(sessionID)
        const key = status.url.toLowerCase()
        let changed = false
        const next = current.map((p) => {
          if (p.url.toLowerCase() !== key || sameStatus(p, status)) return p
          changed = true
          return { ...status, detectedAt: p.detectedAt }
        })
        if (!changed) return
        await save(sessionID, next)
        publish(sessionID, next)
      }).catch((error) => log.warn("apply failed", { sessionID, error }))
    }
  }

  async function refreshUrl(r: Ref, opts: { force: boolean; sessions?: string[] }): Promise<Info | undefined> {
    if (opts.sessions?.length) {
      const set = statusSessions.get(r.url) ?? new Set<string>()
      opts.sessions.forEach((s) => set.add(s))
      statusSessions.set(r.url, set)
    }
    const entry = statusCache.get(r.url)
    if (entry?.inflight) return entry.inflight
    if (!opts.force && entry && !isStale(r.url)) return entry.data
    const inflight = slot(async () => {
      const status = await fetchStatus(r)
      const now = Date.now()
      if (!status) {
        // Keep last values; try again after the normal TTL.
        statusCache.set(r.url, { data: entry?.data, fetchedAt: now })
        return entry?.data
      }
      const data: Info = { ...status, detectedAt: entry?.data?.detectedAt ?? new Date(now).toISOString() }
      statusCache.set(r.url, { data, fetchedAt: now })
      const sessions = new Set(statusSessions.get(r.url) ?? [])
      opts.sessions?.forEach((s) => sessions.add(s))
      await apply(status, sessions)
      return data
    }).catch((error) => {
      log.warn("status refresh failed", { url: r.url, error })
      statusCache.set(r.url, { data: entry?.data, fetchedAt: Date.now() })
      return entry?.data
    })
    statusCache.set(r.url, { data: entry?.data, fetchedAt: entry?.fetchedAt ?? 0, inflight })
    return inflight
  }

  /** Kick off background refreshes for stale PRs of these sessions; returns immediately. */
  export function refreshStale(bySession: Record<string, Info[]>) {
    try {
      for (const [sessionID, prs] of Object.entries(bySession)) {
        for (const pr of prs) {
          const set = statusSessions.get(pr.url) ?? new Set<string>()
          set.add(sessionID)
          statusSessions.set(pr.url, set)
          if (!statusCache.has(pr.url)) {
            // First sight since process start: treat the persisted values as a
            // fetch from "long ago" so it refreshes, finished PRs included.
            statusCache.set(pr.url, { data: pr, fetchedAt: 0 })
          }
          if (isStale(pr.url, pr)) void refreshUrl(pr, { force: false, sessions: [sessionID] }).catch(() => {})
        }
      }
    } catch (error) {
      log.warn("refreshStale failed", { error })
    }
  }

  /** Force-refresh every PR on a session (POST …/prs/refresh); waits up to the fetch timeout. */
  export async function refresh(sessionID: string): Promise<Info[]> {
    const prs = await load(sessionID)
    await Promise.all(prs.map((pr) => refreshUrl(pr, { force: true, sessions: [sessionID] }).catch(() => undefined)))
    return load(sessionID)
  }

  /** Manual link (POST …/prs): records and waits for the first status so the response has a title. */
  export async function link(sessionID: string, url: string): Promise<Info[] | undefined> {
    const r = parse(url)
    if (!r) return undefined
    await serialize(sessionID, async () => {
      const current = await load(sessionID)
      if (current.some((p) => p.url.toLowerCase() === r.url.toLowerCase())) return
      const next = [...current, placeholder(r, new Date().toISOString(), false)]
      await save(sessionID, next)
      publish(sessionID, next)
    })
    await refreshUrl(r, { force: false, sessions: [sessionID] }).catch(() => undefined)
    return load(sessionID)
  }
}
