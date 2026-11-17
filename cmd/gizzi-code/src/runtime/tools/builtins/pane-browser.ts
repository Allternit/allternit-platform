import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { PaneBrowser } from "@/runtime/integrations/pane-browser"
import { Question } from "@/runtime/integrations/question"
import { Agent } from "@/runtime/loop/agent"
import { Session } from "@/runtime/session"
import { PermissionNext } from "@/runtime/tools/guard/permission/next"
import { ActionClasses } from "@/runtime/tools/guard/permission/action-classes"
import { Untrusted } from "@/runtime/tools/guard/untrusted"
import DESCRIPTION from "@/runtime/tools/builtins/pane-browser.txt"

const MAX_TEXT = PaneBrowser.PAGE_TEXT_CHARS
const MAX_ERROR = 1_000

// One flat enum: some providers reject a union in a tool schema.
const Action = z.enum([...PaneBrowser.Action.options, "plan"] as const)

const parameters = z.object({
  action: Action.describe(
    "read_page, find, get_page_text, read_console, list_tabs (read; never ask) | goto, click, fill, new_tab, switch_tab (act) | plan (ask once to approve a multi-step job) | read, screenshot",
  ),
  ref: z.string().optional().describe("click/fill: an element ref from read_page or find (\"ref_7\"); read_page: read only this element's subtree"),
  target: z.string().optional().describe("goto: the URL. click/fill without a ref: a CSS selector or the element's visible text"),
  text: z.string().optional().describe("fill: the text to type"),
  query: z.string().optional().describe("find: what to look for (a label, a name, visible words, a role such as 'search box')"),
  pattern: z.string().optional().describe("read_console: only lines matching this (regex, case-insensitive)"),
  tab_id: z.string().optional().describe("The tab to act on (from list_tabs); the active tab when omitted. switch_tab: the tab to make active"),
  url: z.string().optional().describe("new_tab: the URL to open in the new tab"),
  depth: z.number().int().min(1).max(40).optional().describe("read_page: tree depth (default 15)"),
  filter: z.enum(["all", "interactive"]).optional().describe("read_page: 'interactive' for controls only; 'all' adds off-screen elements"),
  page: z.number().int().min(1).optional().describe("get_page_text: which page of text (20,000 characters each), from 1"),
  summary: z.string().optional().describe("plan: one or two lines on what you will do and why"),
  sites: z.array(z.string()).optional().describe("plan: every site (host or URL) the plan acts on, 1-10"),
  steps: z.array(z.string()).optional().describe("plan: 3-7 short steps, in order"),
  check_ins: z.array(z.string()).optional().describe("plan: the points where you will stop and ask the user (before paying, sending, signing in, deleting…)"),
})
type Params = z.infer<typeof parameters>

type Output = Awaited<ReturnType<Awaited<ReturnType<Tool.Info["init"]>>["execute"]>>

function fail(action: string, message: string, metadata: Record<string, unknown> = {}): Output {
  return { title: `Browser pane: ${action} failed`, output: message, metadata: { ok: false, ...metadata } }
}

function cap(text: string, max: number, note: string): string {
  return text.length > max ? `${text.slice(0, max)}\n…(${note})` : text
}

/** A label the class gate can read: the element's own words, not a CSS selector. */
function readableLabel(text: string | undefined): string | undefined {
  const t = text?.trim()
  if (!t) return undefined
  // Selectors (#id, .class, [attr], a > b, xpath) say nothing about what the control does.
  if (/^[#.\[\/]|[>~]\s*\w|^\w+\[|::|^\w+(\.[\w-]+)+$/.test(t)) return undefined
  return t
}

/** Whether a config rule or the mode refuses this host anyway (a plan approval never beats a "deny"). */
async function planMayCover(ctx: Tool.Context, host: string): Promise<boolean> {
  try {
    const mode = await PermissionNext.getMode(ctx.sessionID)
    if (mode === "plan" || mode === "dontAsk") return false
    const agent = await Agent.get(ctx.agent)
    const session = await Session.get(ctx.sessionID).catch(() => undefined)
    const rule = PermissionNext.evaluate("browser", host, agent?.permission ?? [], session?.permission ?? [])
    return rule.action !== "deny"
  } catch {
    return false
  }
}

function regexOf(pattern: string): RegExp {
  try {
    return new RegExp(pattern, "i")
  } catch {
    return new RegExp(pattern.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "i")
  }
}

function describeRef(r: PaneBrowser.Ref): string {
  const bits = [r.role, r.name ? JSON.stringify(r.name.length > 120 ? r.name.slice(0, 119) + "…" : r.name) : undefined]
  if (r.secure) bits.push("(secret field)")
  return `[${r.ref}] ${bits.filter(Boolean).join(" ") || "(unnamed element)"}`
}

export const PaneBrowserTool = Tool.define("pane_browser", {
  description: DESCRIPTION,
  parameters,
  async execute(params: Params, ctx) {
    const action = params.action
    if (action === "plan") return plan(params, ctx)

    // ---- input checks -------------------------------------------------
    if (action === "goto" && !params.target) throw new Error("The 'goto' action needs a target URL")
    if ((action === "click" || action === "fill") && !params.target && !params.ref) {
      throw new Error(`The '${action}' action needs a ref (from read_page or find) or a target`)
    }
    if (action === "fill" && params.text === undefined) throw new Error("The 'fill' action needs text")
    if (action === "find" && !params.query?.trim()) throw new Error("The 'find' action needs a query")
    if (action === "switch_tab" && !params.tab_id) throw new Error("The 'switch_tab' action needs a tab_id (from list_tabs)")

    // ---- refs: checked here, so a stale one never reaches the page ----
    let ref: (PaneBrowser.Ref & { url?: string }) | undefined
    if (params.ref && (action === "click" || action === "fill")) {
      const found = await PaneBrowser.lookupRef(ctx.sessionID, params.ref, params.tab_id)
      if (found.ok === false) return fail(action, found.error, { code: found.code, ref: params.ref })
      ref = found.ref
    }

    // ---- the host an acting action touches ------------------------------
    const acts = !PaneBrowser.READS.has(action)
    let host: string | undefined
    let pageUrl: string | undefined
    if (action === "goto") {
      pageUrl = params.target
      host = PaneBrowser.hostKey(params.target)
    } else if (action === "new_tab" && params.url) {
      pageUrl = params.url
      host = PaneBrowser.hostKey(params.url)
    } else if (action === "click" || action === "fill") {
      pageUrl = ref?.url ?? (await PaneBrowser.lastUrl(ctx.sessionID, params.tab_id))
      if (!pageUrl) {
        // Nothing read yet in this session: ask the pane which page it is on.
        const tabs = await PaneBrowser.request(
          { sessionID: ctx.sessionID, action: "list_tabs", args: {} },
          { abort: ctx.abort },
        )
        if (!tabs.ok) return fail(action, cap(tabs.error ?? "The pane could not say which page it is on.", MAX_ERROR, "cut"))
        await PaneBrowser.observe(ctx.sessionID, { action: "list_tabs", result: tabs })
        const tab = params.tab_id ? tabs.tabs?.find((t) => t.tab_id === params.tab_id) : tabs.tabs?.find((t) => t.active)
        pageUrl = tab?.url ?? tabs.url
      }
      host = PaneBrowser.hostKey(pageUrl)
    }

    // ---- ask per host (reads never ask; tab moves without a URL touch no site)
    const needsAsk = acts && action !== "switch_tab" && !(action === "new_tab" && !params.url)
    if (needsAsk) {
      const label = ref ? (ref.name || undefined) : action === "goto" ? params.target : readableLabel(params.target)
      let covered = false
      if (host && (await PaneBrowser.planCovers(ctx.sessionID, host)) && (action === "goto" || action === "new_tab" || label)) {
        // An approved plan stands in for the per-host ask; the action class
        // gate (money, credentials, sends, deletes…) still asks through ctx.ask.
        const cls = ActionClasses.classify({
          tool: "pane_browser",
          action,
          args: {
            ...(label ? { label } : {}),
            ...(ref?.role ? { role: ref.role } : {}),
            ...(ref?.secure ? { secure: true } : {}),
            ...(action === "fill" ? { text: params.text } : {}),
            host,
          },
        })
        covered = cls.class === "free" && (await planMayCover(ctx, host))
      }
      if (!covered) {
        await ctx.ask({
          permission: "browser",
          patterns: [host ?? "unknown page"],
          always: host ? [host] : [],
          metadata: {
            action,
            target: label ?? params.target,
            ...(params.ref ? { ref: params.ref } : {}),
            ...(ref?.role ? { role: ref.role } : {}),
            ...(ref?.secure ? { secure: true } : {}),
            ...(pageUrl ? { url: pageUrl } : {}),
            ...(host ? { host } : {}),
            surface: "pane",
          },
        })
      }
    }

    // ---- the request ----------------------------------------------------
    const args: PaneBrowser.Args = {}
    if (params.ref) args.ref = params.ref
    if (ref?.url) args.ref_url = ref.url
    if (host && (action === "click" || action === "fill")) args.expect_host = host
    if (params.tab_id) args.tab_id = params.tab_id
    if (action === "find") Object.assign(args, { query: params.query, limit: PaneBrowser.MAX_FIND_HITS })
    if (action === "read_console") Object.assign(args, { pattern: params.pattern, limit: PaneBrowser.MAX_CONSOLE_LINES })
    if (action === "get_page_text") Object.assign(args, { page: params.page ?? 1, limit: MAX_TEXT })
    if (action === "read_page") Object.assign(args, { depth: params.depth, filter: params.filter, limit: PaneBrowser.MAX_TREE_CHARS })
    if (action === "new_tab" && params.url) args.url = params.url
    for (const k of Object.keys(args) as (keyof PaneBrowser.Args)[]) if (args[k] === undefined) delete args[k]

    const result = await PaneBrowser.request(
      {
        sessionID: ctx.sessionID,
        action,
        target: action === "goto" ? params.target : ref ? undefined : params.target,
        text: params.text,
        ...(Object.keys(args).length ? { args } : {}),
        tool: ctx.callID ? { messageID: ctx.messageID, callID: ctx.callID } : undefined,
      },
      { abort: ctx.abort },
    )

    if (!result.ok) {
      if (result.code === "stale_ref" && params.ref) {
        await PaneBrowser.forgetRefs(ctx.sessionID, params.tab_id)
        return fail(action, PaneBrowser.staleRefError(params.ref, result.url), { code: "stale_ref", url: result.url })
      }
      if (result.code === "host_changed") {
        return fail(
          action,
          `The pane moved to another site before the ${action} ran, so nothing was done (it was approved for ${host}). Call read_page to see where it is now.`,
          { code: "host_changed", url: result.url },
        )
      }
      return fail(action, cap(result.error ?? "The action failed.", MAX_ERROR, "cut"), {
        ...(result.code ? { code: result.code } : {}),
        url: result.url,
      })
    }

    await PaneBrowser.observe(ctx.sessionID, { action, tab_id: params.tab_id, result })
    return format(action, params, result, host)
  },
})

/** Everything the page wrote goes through Untrusted.spotlight before the model reads it. */
function format(action: PaneBrowser.Action, params: Params, result: PaneBrowser.Result, host: string | undefined): Output {
  const from = Untrusted.sourceOf(result.url, host ?? "the page")
  const where = [result.title, result.url].filter(Boolean).join(" — ")
  const plain: string[] = []
  let body = ""
  let kind = "page content"

  switch (action) {
    case "read_page": {
      kind = "page tree"
      body = cap(result.text ?? "", PaneBrowser.MAX_TREE_CHARS, "cut at 50,000 characters: read a subtree with ref, or use a smaller depth or filter 'interactive'")
      plain.push(`${result.refs?.length ?? 0} element refs. Use them with click/fill (ref) until the page changes.`)
      break
    }
    case "find": {
      kind = "find results"
      const hits = (result.refs ?? []).slice(0, PaneBrowser.MAX_FIND_HITS)
      body = hits.length ? hits.map(describeRef).join("\n") : (result.text ?? "")
      plain.push(hits.length ? `${hits.length} match${hits.length === 1 ? "" : "es"} for the query (best first).` : "No element matched the query.")
      break
    }
    case "get_page_text": {
      kind = "page text"
      const full = result.text ?? ""
      let page = result.page ?? params.page ?? 1
      let pages = result.pages
      let text = full
      if (pages === undefined) {
        // The pane sent the whole text: page it here.
        pages = Math.max(1, Math.ceil(full.length / MAX_TEXT))
        page = Math.min(page, pages)
        text = full.slice((page - 1) * MAX_TEXT, page * MAX_TEXT)
      } else {
        text = full.slice(0, MAX_TEXT)
      }
      body = text
      plain.push(page < pages ? `Page ${page} of ${pages}. Call get_page_text with page ${page + 1} for more.` : `Page ${page} of ${pages}.`)
      break
    }
    case "read_console": {
      kind = "console output"
      let lines = result.lines ?? (result.text ? result.text.split("\n") : [])
      if (params.pattern) {
        const re = regexOf(params.pattern)
        lines = lines.filter((l) => re.test(l))
      }
      const total = lines.length
      lines = lines.slice(-PaneBrowser.MAX_CONSOLE_LINES).map((l) => (l.length > 1000 ? l.slice(0, 999) + "…" : l))
      body = lines.join("\n")
      plain.push(
        total === 0
          ? "No console lines" + (params.pattern ? " matched the pattern." : " since the pane attached.")
          : total > lines.length
            ? `The last ${lines.length} of ${total} lines.`
            : `${total} line${total === 1 ? "" : "s"}.`,
      )
      break
    }
    case "list_tabs":
    case "new_tab":
    case "switch_tab": {
      kind = "tab titles"
      const tabs = result.tabs ?? []
      body = tabs.map((t) => `${t.tab_id}${t.active ? " (active)" : ""} — ${t.title ?? ""} — ${t.url ?? ""}`).join("\n") || (result.text ?? "")
      if (action !== "list_tabs" && result.tab_id) plain.push(`Active tab: ${result.tab_id}.`)
      break
    }
    default: {
      body = cap(result.text ?? "", MAX_TEXT, "truncated")
    }
  }

  const page = [where && `Page: ${where}`, body].filter(Boolean).join("\n\n")
  const wrapped = page ? Untrusted.spotlight(page, { from, kind }) : undefined
  const output = [...plain, wrapped?.text].filter(Boolean).join("\n\n") || "Done."
  const target = params.ref ?? (action === "find" ? params.query : params.target) ?? params.tab_id
  return {
    title: `Browser pane: ${action}${target ? ` ${target.length > 60 ? target.slice(0, 59) + "…" : target}` : ""}`,
    output,
    metadata: {
      ok: true,
      url: result.url,
      ...(result.tab_id ? { tab_id: result.tab_id } : {}),
      ...(wrapped?.flags.length ? { flagged: wrapped.flags.length } : {}),
    },
    ...(result.image
      ? { attachments: [{ type: "file" as const, mime: "image/png", url: result.image, filename: "pane.png" }] }
      : {}),
  }
}

/**
 * `plan`: one question for a multi-step job. Approving it covers the
 * per-host browser ask on the listed sites for the rest of the session; the
 * action class gates (money, credentials, sends, deletes, signing) still ask.
 */
async function plan(params: Params, ctx: Tool.Context): Promise<Output> {
  const summary = params.summary?.trim()
  if (!summary) throw new Error("The 'plan' action needs a summary")
  const steps = (params.steps ?? []).map((s) => s.trim()).filter(Boolean)
  if (steps.length < 3 || steps.length > 7) throw new Error("The 'plan' action needs 3 to 7 steps")
  const sites = (params.sites ?? []).map((s) => s.trim()).filter(Boolean)
  if (sites.length < 1 || sites.length > 10) throw new Error("The 'plan' action needs 1 to 10 sites")
  const hosts = [...new Set(sites.map(PaneBrowser.hostKey))]
  if (hosts.some((h) => !h)) throw new Error("Every site in a plan must be a host or URL (e.g. 'github.com')")
  const checkIns = (params.check_ins ?? []).map((s) => s.trim()).filter(Boolean).slice(0, 7)

  const question: Question.Info = {
    question: summary.length > 300 ? summary.slice(0, 299) + "…" : summary,
    header: "Browser plan",
    risk: "medium",
    context: [
      `Sites: ${hosts.join(", ")}`,
      checkIns.length ? `Stops to ask: ${checkIns.join("; ")}` : "Payments, sign-ins, sends and deletes still ask.",
    ].join("\n"),
    options: [
      {
        label: "Approve",
        description: `Work in the browser pane on ${hosts.join(", ")} for this session without asking per site. Payments, sign-ins, sends and deletes still ask.`,
        recommended: true,
      },
      { label: "Not now", description: "Don't run this plan. The agent asks before each step instead, or stops." },
    ],
    plan: { steps: steps.map((title) => ({ title: title.length > 200 ? title.slice(0, 199) + "…" : title })) },
  }
  const { answers, plan: edited } = await Question.askWithPlan({
    sessionID: ctx.sessionID,
    questions: [question],
    tool: ctx.callID ? { messageID: ctx.messageID, callID: ctx.callID } : undefined,
  })
  const answer = Question.formatAnswer(question, answers[0])
  const approved = answers[0]?.[0] === "Approve"
  const finalSteps = edited?.length ? edited.map((s) => s.title) : steps
  if (!approved) {
    return {
      title: "Browser plan: not approved",
      output: `The user did not approve the plan (answer: "${answer || "none"}"). Each acting step on a site will ask again; adjust the plan to the answer or ask what they want.`,
      metadata: { ok: false, approved: false, hosts },
    }
  }
  const granted = await PaneBrowser.grantPlan(ctx.sessionID, hosts as string[])
  return {
    title: `Browser plan approved: ${granted.join(", ")}`,
    output: [
      `Approved for this session on: ${granted.join(", ")}. Acting on other sites still asks, and so do payments, sign-ins, sends, uploads and deletes.`,
      edited?.length ? "The user edited the plan. Follow this version:" : "The plan:",
      ...finalSteps.map((s, i) => `${i + 1}. ${s}`),
      ...(checkIns.length ? ["Stop and ask the user at: " + checkIns.join("; ")] : []),
    ].join("\n"),
    metadata: { ok: true, approved: true, hosts: granted, steps: finalSteps },
  }
}
