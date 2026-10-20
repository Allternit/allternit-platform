import { randomBytes, timingSafeEqual } from "node:crypto"
import fs from "node:fs/promises"
import path from "node:path"
import z from "zod/v4"
import type { Tool } from "@/runtime/tools/builtins/tool"
import { PaneArtifactTool } from "@/runtime/tools/builtins/pane-artifact"
import { PaneBrowserTool } from "@/runtime/tools/builtins/pane-browser"
import { MediaGenerateTool } from "@/runtime/tools/builtins/media-generate"
import { ARTIFACT_TOOLS } from "@/runtime/tools/builtins/artifact"
import { QuestionTool } from "@/runtime/tools/builtins/question"
import { TodoWriteTool } from "@/runtime/tools/builtins/todo"
import { Log } from "@/shared/util/log"
import { Bus } from "@/shared/bus"
import { CLI_PERMISSION_TOOL } from "@/runtime/drivers/cli-session-flags"

/**
 * CLI tool bridge: an installed CLI agent (Claude Code, Codex, Kimi…) runs a
 * gizzi session turn with its own tools only. This serves gizzi's
 * session-bound tools — the document open in the pane, the pane's browser,
 * native image/video rendering, Artifacts v2 — to that CLI as an MCP server over HTTP
 * (stateless, JSON responses), scoped to one session and guarded by a
 * per-process token. The CLI driver hands the CLI this server's config.
 */
export namespace CliBridge {
  const log = Log.create({ service: "cli-bridge" })

  export const SERVER_NAME = "allternit"
  const PROTOCOL_VERSION = "2025-06-18"

  /** The tools a CLI gets: the ones only gizzi can do for this session. */
  // todowrite: CLI brains (Claude Code -p has no todo tool) keep the task list
  // that Cowork's Progress panel reads.
  export const TOOLS: Tool.Info[] = [PaneArtifactTool, PaneBrowserTool, MediaGenerateTool, TodoWriteTool, ...ARTIFACT_TOOLS]

  const token = randomBytes(32).toString("hex")

  /**
   * The session's plan file when its current turn runs the plan agent, else
   * undefined. A CLI runs its own tools, so gizzi's plan mode reaches it as
   * the vendor's own plan mode plus these bridge tools (see planTools).
   */
  export async function planFile(sessionID: string): Promise<string | undefined> {
    const [{ Session }, { MessageV2 }] = await Promise.all([import("@/runtime/session"), import("@/runtime/session/message-v2")])
    for await (const item of MessageV2.stream(sessionID)) {
      if (item.info.role !== "user") continue
      if (item.info.agent !== "plan") return undefined
      return Session.plan(await Session.get(sessionID))
    }
    return undefined
  }

  const PLAN_EXIT_DESCRIPTION =
    "Present your finished plan to the user for approval. Pass the whole plan as Markdown. Gizzi saves it as the session's plan file and asks the user; if they approve, end your turn: the session switches to the build agent, which carries the plan out. If they do not, end your turn and wait for their reply."
  const PERMISSION_DESCRIPTION =
    "Gizzi's permission check for Claude Code's plan mode (--permission-prompt-tool). Not for direct use."

  /** Tools a CLI gets only while the session plans: the plan handoff and Claude's permission check. */
  const planTools = [
    { name: "plan_exit", description: PLAN_EXIT_DESCRIPTION, inputSchema: { type: "object", properties: { plan: { type: "string", description: "The plan, in Markdown" } }, required: ["plan"] } },
    { name: CLI_PERMISSION_TOOL, description: PERMISSION_DESCRIPTION, inputSchema: { type: "object", properties: { tool_name: { type: "string" }, input: { type: "object" }, tool_use_id: { type: "string" } }, required: ["tool_name"] } },
  ]

  /** Edit tools that may touch the plan file during plan mode (Claude Code names). */
  const EDIT_TOOLS = new Set(["Write", "Edit", "MultiEdit", "NotebookEdit"])

  /**
   * Own header, not Authorization: the server's auth middleware treats any
   * Bearer token as a Clerk/API credential.
   */
  export const TOKEN_HEADER = "X-Allternit-Bridge-Token"

  export function authorized(header: string | undefined): boolean {
    const given = Buffer.from(header ?? "")
    const want = Buffer.from(token)
    return given.length === want.length && timingSafeEqual(given, want)
  }

  export interface ServerConfig {
    name: string
    url: string
    headers: Record<string, string>
  }

  /** MCP server config for one session, on this gizzi server. */
  export function serverConfig(input: { baseURL: URL | string; sessionID: string; directory?: string }): ServerConfig {
    const url = new URL(`/cli-bridge/${encodeURIComponent(input.sessionID)}/mcp`, input.baseURL)
    if (input.directory) url.searchParams.set("directory", input.directory)
    const headers: Record<string, string> = { [TOKEN_HEADER]: token }
    // A password-protected server (serve --password) needs basic auth too.
    const password = process.env.GIZZI_SERVER_PASSWORD
    if (password) {
      const username = process.env.GIZZI_SERVER_USERNAME ?? "gizzi"
      headers.Authorization = `Basic ${Buffer.from(`${username}:${password}`).toString("base64")}`
    }
    return { name: SERVER_NAME, url: url.toString(), headers }
  }

  type JsonRpcRequest = { jsonrpc?: string; id?: string | number | null; method?: string; params?: any }
  type JsonRpcResponse = { jsonrpc: "2.0"; id: string | number | null; result?: unknown; error?: { code: number; message: string } }

  const ok = (id: JsonRpcRequest["id"], result: unknown): JsonRpcResponse => ({ jsonrpc: "2.0", id: id ?? null, result })
  const fail = (id: JsonRpcRequest["id"], code: number, message: string): JsonRpcResponse => ({
    jsonrpc: "2.0",
    id: id ?? null,
    error: { code, message },
  })

  async function listTools(sessionID: string) {
    const tools = await Promise.all(
      [...TOOLS, QuestionTool].map(async (t) => {
        const info = await t.init()
        const schema = z.toJSONSchema(info.parameters) as Record<string, unknown>
        delete schema.$schema
        const description = t.id === QuestionTool.id ? `${QUESTION_PREFACE} ${info.description}` : info.description
        return { name: t.id, description, inputSchema: schema }
      }),
    )
    const plan = await planFile(sessionID).catch(() => undefined)
    return plan ? [...tools, ...planTools] : tools
  }

  /** The open "Build Agent" question per session, so a newer plan replaces it instead of stacking a second one. */
  const pendingPlans = new Map<string, string>()

  /**
   * Save the plan and ask the user (the same "Build Agent" question as the
   * plan agent's own plan_exit), without waiting for the answer: a CLI gives
   * up on a tool call after about a minute (Claude Code, Codex), and a person
   * reviewing a plan takes longer. The CLI ends its turn; when the user says
   * yes, PlanExitTool queues the build turn and this starts it. Unavailable:
   * the session does not allow plan_exit (headless `gizzi run` denies it).
   */
  async function exitPlan(sessionID: string, file: string, plan: string): Promise<"asked" | "unavailable"> {
    const [{ PlanExitTool }, { Question }, { Session }, { PermissionNext }] = await Promise.all([
      import("@/runtime/tools/builtins/plan"),
      import("@/runtime/integrations/question"),
      import("@/runtime/session"),
      import("@/runtime/tools/guard/permission/next"),
    ])
    if (plan.trim()) {
      await fs.mkdir(path.dirname(file), { recursive: true })
      await fs.writeFile(file, plan.trim() + "\n")
    }
    const session = await Session.get(sessionID)
    if (PermissionNext.disabled(["plan_exit"], session.permission ?? []).has("plan_exit")) return "unavailable"
    const earlier = pendingPlans.get(sessionID)
    if (earlier) await Question.reject(earlier)
    let mine: string | undefined
    const unsubscribe = Bus.subscribe(Question.Event.Asked, (event) => {
      if (mine || event.properties.sessionID !== sessionID || event.properties.questions[0]?.header !== "Build Agent") return
      mine = event.properties.id
      pendingPlans.set(sessionID, mine)
    })
    const info = await PlanExitTool.init()
    // No callID: the question stands on its own (there is no gizzi tool part for it to attach to).
    const ctx = { sessionID, messageID: "cli-bridge", agent: "cli", abort: new AbortController().signal, callID: "", messages: [], metadata: () => {}, ask: async () => {} } as unknown as Tool.Context
    void (async () => {
      let approved = false
      try {
        await info.execute({}, ctx)
        approved = true
      } catch (error) {
        if (!(error instanceof Question.RejectedError)) log.error("plan handoff failed", { sessionID, error: error instanceof Error ? error.message : String(error) })
      } finally {
        unsubscribe()
        if (mine && pendingPlans.get(sessionID) === mine) pendingPlans.delete(sessionID)
      }
      if (!approved) return
      const { SessionPrompt } = await import("@/runtime/session/prompt")
      await SessionPrompt.loop({ sessionID }).catch((error) =>
        log.error("build turn failed to start", { sessionID, error: error instanceof Error ? error.message : String(error) }),
      )
    })()
    return "asked"
  }

  const QUESTION_PREFACE =
    "Ask the user multiple-choice questions in Allternit's question card (this session has no AskUserQuestion; use this instead). After calling it, end your turn: the user's answers arrive as their next message."

  /**
   * The question tool for a CLI: shows gizzi's question card (the same one as
   * the native `question` tool) and returns at once. A CLI gives up on a tool
   * call after about a minute (Codex, Claude Code), and people take longer to
   * answer, so — like the plan handoff — the CLI ends its turn and the answers
   * start the next turn as the user's message. Dismissing the card sends
   * nothing.
   */
  async function askQuestion(sessionID: string, args: any) {
    const [{ Question }] = await Promise.all([import("@/runtime/integrations/question")])
    const parsed = (await QuestionTool.init()).parameters.safeParse(args ?? {})
    if (!parsed.success) {
      return { content: [{ type: "text", text: `Invalid questions: ${parsed.error.message}` }], isError: true }
    }
    const questions = parsed.data.questions
    void (async () => {
      let answers: string[][]
      try {
        // No tool: the card stands on its own (there is no gizzi tool part for it).
        answers = await Question.ask({ sessionID, questions })
      } catch (error) {
        if (!(error instanceof Question.RejectedError)) log.error("question failed", { sessionID, error: error instanceof Error ? error.message : String(error) })
        return
      }
      const text = questions
        .map((q, i) => `${q.question}\n→ ${answers[i]?.length ? answers[i].join(", ") : "(skipped)"}`)
        .join("\n\n")
      const { SessionPrompt } = await import("@/runtime/session/prompt")
      await SessionPrompt.prompt({ sessionID, parts: [{ type: "text", text }] }).catch((error) =>
        log.error("answer turn failed to start", { sessionID, error: error instanceof Error ? error.message : String(error) }),
      )
    })()
    return {
      content: [{ type: "text", text: "The question is on the user's screen. End your turn now and make no more tool calls; their answers arrive as their next message. When they do, carry on with the task you were working on, using those answers." }],
      isError: false,
    }
  }

  const EXIT_REPLY = {
    asked:
      "Gizzi has asked the user to approve this plan. End your turn now and make no more tool calls. If they approve, the session switches to the build agent, which carries the plan out; if not, they reply with notes to revise it.",
    unavailable: "Plan approval is not available in this session. Give the plan as your final reply and end your turn.",
  }

  async function planToolCall(sessionID: string, file: string, params: any, signal: AbortSignal) {
    const args = params?.arguments ?? {}
    const text = (t: string) => ({ content: [{ type: "text", text: t }], isError: false })
    if (params?.name === "plan_exit") return text(EXIT_REPLY[await exitPlan(sessionID, file, String(args.plan ?? ""))])
    // Claude Code asks here for anything its plan mode does not allow on its own.
    const name = String(args.tool_name ?? "")
    const input = args.input && typeof args.input === "object" ? args.input : {}
    const decide = (d: Record<string, unknown>) => text(JSON.stringify(d))
    if (name === "ExitPlanMode") {
      return decide({ behavior: "deny", message: EXIT_REPLY[await exitPlan(sessionID, file, String(input.plan ?? ""))] })
    }
    const target = String(input.file_path ?? input.notebook_path ?? "")
    if (EDIT_TOOLS.has(name) && target && path.resolve(target) === path.resolve(file)) return decide({ behavior: "allow", updatedInput: input })
    log.info("plan mode refused", { sessionID, tool: name })
    return decide({
      behavior: "deny",
      message: `Plan mode is read-only: ${name} was not run. Only the plan file (${file}) may be edited. Finish the plan and present it with ExitPlanMode.`,
    })
  }

  function content(result: Awaited<ReturnType<Awaited<ReturnType<Tool.Info["init"]>>["execute"]>>) {
    const parts: Array<Record<string, unknown>> = [{ type: "text", text: result.output }]
    for (const a of result.attachments ?? []) {
      const m = /^data:([^;,]+);base64,(.*)$/s.exec(a.url ?? "")
      if (m && m[1].startsWith("image/")) parts.push({ type: "image", mimeType: m[1], data: m[2] })
    }
    return parts
  }

  async function callTool(sessionID: string, params: any, signal: AbortSignal) {
    if (planTools.some((t) => t.name === params?.name)) {
      const file = await planFile(sessionID)
      if (file) return planToolCall(sessionID, file, params, signal)
      // The session left plan mode: nothing to gate any more.
      if (params?.name === CLI_PERMISSION_TOOL) return { content: [{ type: "text", text: JSON.stringify({ behavior: "allow", updatedInput: params?.arguments?.input ?? {} }) }], isError: false }
    }
    if (params?.name === QuestionTool.id) return askQuestion(sessionID, params?.arguments)
    const tool = TOOLS.find((t) => t.id === params?.name)
    if (!tool) throw new Error(`Unknown tool: ${params?.name}`)
    // Loaded on use: the session store pulls in most of the runtime.
    const [{ Session }, { PermissionNext }] = await Promise.all([
      import("@/runtime/session"),
      import("@/runtime/tools/guard/permission/next"),
    ])
    const session = await Session.get(sessionID)
    const info = await tool.init()
    const callID = `cli_${randomBytes(6).toString("hex")}`
    const ctx: Tool.Context = {
      sessionID,
      messageID: "cli-bridge",
      agent: "cli",
      abort: signal,
      callID,
      messages: [],
      metadata: () => {},
      async ask(req) {
        // The task list is bookkeeping, allowed for every agent (loop/agent.ts); never prompt for it.
        if (req.permission === "todowrite") return
        return await PermissionNext.ask({ ...req, sessionID, ruleset: session.permission ?? [] })
      },
    }
    log.info("call", { sessionID, tool: tool.id })
    try {
      const result = await info.execute(params?.arguments ?? {}, ctx)
      return { content: content(result), isError: false }
    } catch (error) {
      return { content: [{ type: "text", text: error instanceof Error ? error.message : String(error) }], isError: true }
    }
  }

  /** Answer one JSON-RPC message (or a batch). Notifications get no reply. */
  export async function handle(
    sessionID: string,
    body: JsonRpcRequest | JsonRpcRequest[],
    signal: AbortSignal,
  ): Promise<JsonRpcResponse | JsonRpcResponse[] | null> {
    if (Array.isArray(body)) {
      const replies = (await Promise.all(body.map((b) => handleOne(sessionID, b, signal)))).filter(
        (r): r is JsonRpcResponse => r !== null,
      )
      return replies.length ? replies : null
    }
    return handleOne(sessionID, body, signal)
  }

  async function handleOne(sessionID: string, req: JsonRpcRequest, signal: AbortSignal): Promise<JsonRpcResponse | null> {
    const isNotification = req.id === undefined || req.id === null
    switch (req.method) {
      case "initialize":
        return ok(req.id, {
          protocolVersion: req.params?.protocolVersion ?? PROTOCOL_VERSION,
          capabilities: { tools: { listChanged: false } },
          serverInfo: { name: SERVER_NAME, version: "1.0.0" },
          instructions:
            "Allternit session tools: the document open beside the chat (pane_artifact), the pane's browser (pane_browser), image/video generation rendered in the user's app (media_generate), the user's Artifacts (artifact_create, artifact_update, artifact_read), the task list shown as the session's Progress (todowrite), and multiple-choice questions to the user (question).",
        })
      case "ping":
        return ok(req.id, {})
      case "tools/list":
        return ok(req.id, { tools: await listTools(sessionID) })
      case "tools/call":
        return ok(req.id, await callTool(sessionID, req.params, signal))
      default:
        if (isNotification) return null
        return fail(req.id, -32601, `Method not found: ${req.method}`)
    }
  }
}
