import { Bus } from "@/shared/bus"
import { BusEvent } from "@/shared/bus/bus-event"
import { Identifier } from "@/shared/id/id"
import { Instance } from "@/runtime/context/project/instance"
import { Log } from "@/shared/util/log"
import z from "zod/v4"

/**
 * Browser actions the app runs on the page the user sees (the session's
 * browser pane), instead of a separate automation browser. The tool publishes
 * a request; the app that shows this session's pane runs it and replies.
 * Same request/reply shape as Question.
 *
 * Wave C (lane G) adds the `browser` toolset's reading members
 * (contracts/computer-toolset/allternit-browser-v1.json): read_page with
 * element refs, find (matched in the page, no model call), get_page_text in
 * pages, read_console, list_tabs, new_tab and switch_tab, and `ref` targets
 * for click and fill. Everything new travels in one `args` object, so the
 * relays that forward pane events field by field (allternit-api, cloud-api,
 * agent-compat) forward one more field, not ten.
 *
 * This module also keeps two pieces of session state, in memory only:
 *   - the refs each read_page/find returned, per tab, with the page URL they
 *     came from, so a ref used after the page navigated fails with a clear
 *     "read the page again" instead of clicking the wrong element;
 *   - the hosts a `pane_browser plan` approval named, which stand in for the
 *     per-host browser ask on those hosts for the rest of the session.
 */
export namespace PaneBrowser {
  const log = Log.create({ service: "pane-browser" })

  export const Action = z.enum([
    "read",
    "goto",
    "click",
    "fill",
    "screenshot",
    "read_page",
    "find",
    "get_page_text",
    "read_console",
    "list_tabs",
    "new_tab",
    "switch_tab",
  ])
  export type Action = z.infer<typeof Action>

  /** Actions that only read: they never ask. */
  export const READS = new Set<Action>(["read", "screenshot", "read_page", "find", "get_page_text", "read_console", "list_tabs"])

  /** Limits (wave C contract proposals). */
  export const MAX_FIND_HITS = 20
  export const PAGE_TEXT_CHARS = 20_000
  export const MAX_CONSOLE_LINES = 200
  /** read_page tree cap (the browser contract's read_page cap). */
  export const MAX_TREE_CHARS = 50_000

  /** The new actions' inputs. Every field is optional; the action decides which apply. */
  export const Args = z.object({
    /** click / fill / read_page: an element ref from read_page or find ("ref_7"). */
    ref: z.string().optional(),
    /** The page URL the ref came from. The pane refuses a ref when the page has moved on (code stale_ref). */
    ref_url: z.string().optional(),
    /** Acting actions: the host the person approved. The pane refuses when the page is on another host (code host_changed). */
    expect_host: z.string().optional(),
    /** find: what to look for, matched in the page (labels, names, text, roles). */
    query: z.string().optional(),
    /** read_console: only lines matching this (regex, case-insensitive). */
    pattern: z.string().optional(),
    /** The tab to act on; the active tab when omitted. */
    tab_id: z.string().optional(),
    /** read_page: tree depth (default 15). */
    depth: z.number().optional(),
    /** read_page: "interactive" for controls only, "all" to include off-screen elements. */
    filter: z.enum(["all", "interactive"]).optional(),
    /** get_page_text: 1-based page of PAGE_TEXT_CHARS characters. */
    page: z.number().optional(),
    /** The most the pane should return: hits (find), characters (get_page_text, read_page), lines (read_console). */
    limit: z.number().optional(),
    /** new_tab: the URL to open in it. */
    url: z.string().optional(),
  })
  export type Args = z.infer<typeof Args>

  export const Request = z.object({
    id: Identifier.schema("question"),
    sessionID: Identifier.schema("session"),
    action: Action,
    target: z.string().optional(),
    text: z.string().optional(),
    args: Args.optional(),
    /** When it was asked (ms). The app skips requests older than TIMEOUT_MS,
     * so a replayed event never repeats an action. */
    time: z.number(),
    tool: z
      .object({
        messageID: z.string(),
        callID: z.string(),
      })
      .optional(),
  })
  export type Request = z.infer<typeof Request>

  /** One element a ref names: what read_page and find return per element. */
  export const Ref = z.object({
    ref: z.string(),
    role: z.string().optional(),
    name: z.string().optional(),
    /** A password or other secret field. */
    secure: z.boolean().optional(),
  })
  export type Ref = z.infer<typeof Ref>

  export const Tab = z.object({
    tab_id: z.string(),
    title: z.string().optional(),
    url: z.string().optional(),
    active: z.boolean().optional(),
  })
  export type Tab = z.infer<typeof Tab>

  export const Result = z.object({
    ok: z.boolean(),
    url: z.string().optional(),
    title: z.string().optional(),
    /** Page text (read, get_page_text), the tree (read_page), what happened (other actions). */
    text: z.string().optional(),
    /** PNG data URL (screenshot). */
    image: z.string().optional(),
    error: z.string().optional(),
    /** A machine-readable failure: stale_ref, unknown_ref, host_changed, unsupported. */
    code: z.string().optional(),
    /** The tab the action ran in. */
    tab_id: z.string().optional(),
    /** read_page / find: the elements their refs name (find: the hits, best first). */
    refs: z.array(Ref).optional(),
    /** list_tabs / new_tab / switch_tab: the open tabs. */
    tabs: z.array(Tab).optional(),
    /** read_console: one entry per line. */
    lines: z.array(z.string()).optional(),
    /** get_page_text: the page returned and how many there are. */
    page: z.number().optional(),
    pages: z.number().optional(),
  })
  export type Result = z.infer<typeof Result>

  export const Event = {
    Requested: BusEvent.define("pane_browser.requested", Request),
    Replied: BusEvent.define(
      "pane_browser.replied",
      z.object({ sessionID: z.string(), requestID: z.string(), ok: z.boolean() }),
    ),
  }

  /** No app answered in time: the pane is closed or no app shows this session. */
  export const TIMEOUT_MS = 45_000

  /** A plan approval lasts the session, and never more than this (same bound as computer access grants). */
  export const PLAN_GRANT_MS = 12 * 60 * 60 * 1000

  type TabRefs = { url?: string; refs: Map<string, Ref & { url?: string }>; stale: Set<string>; movedTo?: string }
  type SessionPage = { active?: string; tabs: Map<string, TabRefs> }

  const state = Instance.state(async () => {
    const pending: Record<string, { info: Request; resolve: (r: Result) => void; timer: ReturnType<typeof setTimeout> }> = {}
    const pages = new Map<string, SessionPage>()
    const plans = new Map<string, { hosts: Set<string>; until: number }>()
    return { pending, pages, plans }
  })

  export async function request(
    input: Omit<Request, "id" | "time">,
    options: { timeoutMs?: number; abort?: AbortSignal } = {},
  ): Promise<Result> {
    const s = await state()
    const id = Identifier.ascending("question")
    const info: Request = { ...input, id, time: Date.now() }
    log.info("requesting", { id, action: input.action })
    return new Promise<Result>((resolve) => {
      const finish = (result: Result) => {
        const entry = s.pending[id]
        if (!entry) return
        clearTimeout(entry.timer)
        delete s.pending[id]
        resolve(result)
      }
      const timer = setTimeout(
        () =>
          finish({
            ok: false,
            error: "The browser pane didn't answer. It may be closed; ask the user to open it, or use the browser tool.",
          }),
        options.timeoutMs ?? TIMEOUT_MS,
      )
      s.pending[id] = { info, resolve, timer }
      // The turn may already be cancelled by the time this runs.
      if (options.abort?.aborted) return finish({ ok: false, error: "Cancelled." })
      options.abort?.addEventListener("abort", () => finish({ ok: false, error: "Cancelled." }), { once: true })
      Bus.publish(Event.Requested, info)
    })
  }

  export async function reply(input: { requestID: string; result: Result }): Promise<boolean> {
    const s = await state()
    const entry = s.pending[input.requestID]
    if (!entry) {
      log.warn("reply for unknown request", { requestID: input.requestID })
      return false
    }
    clearTimeout(entry.timer)
    delete s.pending[input.requestID]
    Bus.publish(Event.Replied, { sessionID: entry.info.sessionID, requestID: input.requestID, ok: input.result.ok })
    entry.resolve(input.result)
    return true
  }

  export async function list(): Promise<Request[]> {
    const s = await state()
    return Object.values(s.pending).map((entry) => entry.info)
  }

  /* ------------------------------------------------------------------ */
  /* Hosts                                                               */
  /* ------------------------------------------------------------------ */

  /**
   * A comparable host: lowercase, no port, no leading "www.". Accepts a URL or
   * a bare host ("github.com", "https://www.github.com/x"). Undefined when
   * there is no host (about:blank, a data: URL, junk).
   */
  export function hostKey(value: string | undefined): string | undefined {
    if (!value) return undefined
    const raw = value.trim()
    if (!raw) return undefined
    let host: string
    try {
      host = new URL(/^[a-z][a-z0-9+.-]*:/i.test(raw) ? raw : `https://${raw}`).hostname
    } catch {
      return undefined
    }
    host = host.toLowerCase().replace(/\.$/, "").replace(/^www\./, "")
    if (!host || !/^[a-z0-9.\-\[\]:]+$/.test(host)) return undefined
    return host
  }

  /* ------------------------------------------------------------------ */
  /* Refs and the page each tab is on                                     */
  /* ------------------------------------------------------------------ */

  /** A URL compared for navigation: the fragment is not a navigation. */
  function pageOf(url: string | undefined): string | undefined {
    if (!url) return undefined
    try {
      const u = new URL(url)
      u.hash = ""
      return u.toString()
    } catch {
      return url
    }
  }

  async function sessionPage(sessionID: string): Promise<SessionPage> {
    const s = await state()
    let page = s.pages.get(sessionID)
    if (!page) {
      page = { tabs: new Map() }
      s.pages.set(sessionID, page)
      if (s.pages.size > 500) s.pages.delete(s.pages.keys().next().value!)
    }
    return page
  }

  function tabKey(page: SessionPage, tabID: string | undefined): string {
    return tabID ?? page.active ?? ""
  }

  function tabOf(page: SessionPage, key: string): TabRefs {
    let tab = page.tabs.get(key)
    if (!tab) {
      tab = { refs: new Map(), stale: new Set() }
      page.tabs.set(key, tab)
    }
    return tab
  }

  /** The page moved on: every ref the tab handed out is stale now. */
  function moved(tab: TabRefs, to: string | undefined) {
    for (const ref of tab.refs.keys()) tab.stale.add(ref)
    if (tab.stale.size > 2000) tab.stale = new Set([...tab.stale].slice(-1000))
    tab.refs.clear()
    tab.url = to
    tab.movedTo = to
  }

  /**
   * Record what a reply says about the page: the tab it ran in, the URL it is
   * on now (a different page makes that tab's refs stale), and the refs a
   * read_page or find handed out. Called with every successful reply.
   */
  export async function observe(
    sessionID: string,
    input: { action: Action; tab_id?: string; result: Result },
  ): Promise<void> {
    const page = await sessionPage(sessionID)
    const { result } = input
    const active = result.tabs?.find((t) => t.active)?.tab_id
    if (input.action === "switch_tab" || input.action === "new_tab") page.active = result.tab_id ?? active ?? page.active
    else if (active) page.active = active
    else if (!input.tab_id && result.tab_id) page.active = result.tab_id
    const key = tabKey(page, result.tab_id ?? input.tab_id)
    const tab = tabOf(page, key)
    // list_tabs reports every tab's page: one that moved loses its refs too.
    for (const t of result.tabs ?? []) {
      const known = page.tabs.get(t.tab_id)
      const now = pageOf(t.url)
      if (known && now && known.url && known.url !== now) moved(known, now)
      else if (known && now && !known.url) known.url = now
    }
    const now = pageOf(result.url)
    if (now && tab.url && tab.url !== now) moved(tab, now)
    else if (now && !tab.url) tab.url = now
    if (input.action === "read_page" || input.action === "find") {
      for (const r of result.refs ?? []) {
        tab.refs.set(r.ref, { ...r, url: result.url ?? tab.url })
        tab.stale.delete(r.ref)
      }
      if (tab.refs.size > 5000) {
        const keep = [...tab.refs.entries()].slice(-2500)
        tab.refs = new Map(keep)
      }
    }
  }

  /** The pane says the ref is stale (code stale_ref): drop the tab's refs. */
  export async function forgetRefs(sessionID: string, tabID?: string): Promise<void> {
    const page = await sessionPage(sessionID)
    const tab = page.tabs.get(tabKey(page, tabID))
    if (tab) moved(tab, tab.url)
  }

  export type RefLookup =
    | { ok: true; ref: Ref & { url?: string }; tab_id?: string }
    | { ok: false; code: "stale_ref" | "unknown_ref"; error: string }

  export function staleRefError(ref: string, now?: string): string {
    return (
      `Ref ${ref} is stale: the page changed${now ? ` (it is now ${now})` : ""} after read_page or find returned it. ` +
      "Call read_page (or find) again and use a ref from the new result."
    )
  }

  /** The ref a click or fill names, checked against what this session's reads handed out. */
  export async function lookupRef(sessionID: string, ref: string, tabID?: string): Promise<RefLookup> {
    const page = await sessionPage(sessionID)
    const key = tabKey(page, tabID)
    const tab = page.tabs.get(key)
    const hit = tab?.refs.get(ref)
    if (hit) return { ok: true, ref: hit, tab_id: key || undefined }
    if (tab?.stale.has(ref)) return { ok: false, code: "stale_ref", error: staleRefError(ref, tab.movedTo) }
    return {
      ok: false,
      code: "unknown_ref",
      error: `Unknown ref ${ref}: no read_page or find in this session returned it for this tab. Call read_page (or find) first and use a ref from its result.`,
    }
  }

  /** The URL the tab was last seen on (from any reply), if known. */
  export async function lastUrl(sessionID: string, tabID?: string): Promise<string | undefined> {
    const page = await sessionPage(sessionID)
    return page.tabs.get(tabKey(page, tabID))?.url
  }

  /* ------------------------------------------------------------------ */
  /* Plan approval: hosts the person approved for this session            */
  /* ------------------------------------------------------------------ */

  /** The person approved a plan naming these sites: the per-host ask is covered on them for the session. */
  export async function grantPlan(sessionID: string, sites: string[], now = Date.now()): Promise<string[]> {
    const s = await state()
    const hosts = [...new Set(sites.map(hostKey).filter((h): h is string => !!h))]
    const prior = s.plans.get(sessionID)
    const keep = prior && prior.until > now ? prior.hosts : new Set<string>()
    for (const h of hosts) keep.add(h)
    s.plans.set(sessionID, { hosts: keep, until: now + PLAN_GRANT_MS })
    log.info("plan approved", { sessionID, hosts })
    return hosts
  }

  /** Whether an approved plan covers this host (exact host; "www." ignored). Never a subdomain or a parent. */
  export async function planCovers(sessionID: string, host: string | undefined, now = Date.now()): Promise<boolean> {
    const key = hostKey(host)
    if (!key) return false
    const s = await state()
    const plan = s.plans.get(sessionID)
    if (!plan) return false
    if (plan.until <= now) {
      s.plans.delete(sessionID)
      return false
    }
    return plan.hosts.has(key)
  }

  export async function planHosts(sessionID: string, now = Date.now()): Promise<string[]> {
    const s = await state()
    const plan = s.plans.get(sessionID)
    if (!plan || plan.until <= now) return []
    return [...plan.hosts]
  }

  /** Revoke the session's plan approval (the person took it back, or the session ended). */
  export async function revokePlan(sessionID: string): Promise<boolean> {
    const s = await state()
    return s.plans.delete(sessionID)
  }
}
