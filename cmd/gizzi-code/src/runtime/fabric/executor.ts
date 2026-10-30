/**
 * Capability executor registry.
 *
 * Registers worker functions that implement harness capabilities and dispatches
 * `{ capability, inputs, lease? }` invocations to the right handler. This is the
 * runtime side of the invariant: every capability the agent can use must be
 * exposed through the harness/Fabric capability system.
 *
 * The registry is intentionally simple: a capability name maps to an async
 * executor. Leases are validated before execution (when enforcement is on), and
 * every invocation returns a structured receipt.
 */
import path from "node:path"
import { BotTurnInfo } from "@/runtime/bots/bot-turn"
import { Log } from "@/shared/util/log"
import { Session } from "@/runtime/session"
import { SessionStatus } from "@/runtime/session/status"
import { SessionPrompt } from "@/runtime/session/prompt"
import { PermissionNext } from "@/runtime/tools/guard/permission/next"
import { Question } from "@/runtime/integrations/question/question"
import { Instance } from "@/runtime/context/project/instance"
import { File } from "@/shared/file"
import { Filesystem } from "@/runtime/util/filesystem"
import { buildNodeIdentity } from "./capability-catalog"
import { FabricJournal } from "./journal"
import { type FabricLease, type NodeIdentity, fabricLeaseSchema } from "./transport"

const log = Log.create({ service: "fabric:executor" })

export interface ExecutionContext {
  requestId: string
  node: NodeIdentity
  lease?: FabricLease
}

export interface CapabilityExecutor<TInput = unknown, TOutput = unknown> {
  /** Dot-namespaced capability name, e.g. `harness.session.message`. */
  readonly capability: string
  /** Execute the capability and return a result or receipt. */
  execute(input: TInput, ctx: ExecutionContext): Promise<TOutput>
}

export interface InvocationResult {
  ok: boolean
  capability: string
  nodeId: string
  result?: unknown
  error?: string
  leaseId?: string
}

const registry = new Map<string, CapabilityExecutor>()

function executorResource(capability: string): string | undefined {
  const parts = capability.split(".")
  if (parts.length >= 2) return parts[1]
  return parts[0]
}

export namespace CapabilityExecutors {
  /** Register an executor for a capability. */
  export function register(executor: CapabilityExecutor) {
    if (registry.has(executor.capability)) {
      log.warn("overwriting capability executor", { capability: executor.capability })
    }
    registry.set(executor.capability, executor)
    log.debug("registered capability executor", { capability: executor.capability })
  }

  /** True when an executor exists for the capability. */
  export function has(capability: string): boolean {
    return registry.has(capability)
  }

  /** List registered capability names. */
  export function list(): string[] {
    return Array.from(registry.keys())
  }

  /**
   * Dispatch an invocation to the registered executor.
   *
   * If `GIZZI_ENFORCE_LEASES=true` and no valid lease is in the context, the
   * invocation is rejected. Otherwise the lease is checked only for logging.
   */
  export async function dispatch(
    capability: string,
    input: unknown,
    ctx: ExecutionContext,
  ): Promise<InvocationResult> {
    const enforceLeases = process.env.GIZZI_ENFORCE_LEASES === "true"
    const nodeId = ctx.node.nodeId
    const inputKeys = typeof input === "object" && input !== null ? Object.keys(input as object) : []

    const reject = async (error: string): Promise<InvocationResult> => {
      const result: InvocationResult = { ok: false, capability, error, nodeId, leaseId: ctx.lease?.id }
      await FabricJournal.write({
        capability,
        nodeId,
        requestId: ctx.requestId,
        leaseId: ctx.lease?.id,
        ok: false,
        error,
        inputKeys,
      })
      return result
    }

    if (enforceLeases) {
      if (!ctx.lease) {
        return reject("lease_required")
      }
      const leaseCheck = fabricLeaseSchema.safeParse(ctx.lease)
      if (!leaseCheck.success) {
        return reject("lease_malformed")
      }
      const lease = leaseCheck.data
      if (lease.status !== "active") {
        return reject("lease_not_active")
      }
      if (lease.expiresAt && Date.parse(lease.expiresAt) <= Date.now()) {
        return reject("lease_expired")
      }
      if (lease.capabilityId !== capability) {
        return reject("lease_capability_mismatch")
      }
    }

    const executor = registry.get(capability)
    if (!executor) {
      return reject(`capability_not_implemented: ${capability}`)
    }

    try {
      const result = await executor.execute(input, ctx)
      const output: InvocationResult = {
        ok: true,
        capability,
        result,
        nodeId,
        leaseId: ctx.lease?.id,
      }
      await FabricJournal.write({
        capability,
        nodeId,
        requestId: ctx.requestId,
        leaseId: ctx.lease?.id,
        ok: true,
        result,
        inputKeys,
        resource: executorResource(capability),
      })
      return output
    } catch (err) {
      const error = err instanceof Error ? err.message : String(err)
      log.warn("capability execution failed", { capability, error, requestId: ctx.requestId })
      await FabricJournal.write({
        capability,
        nodeId,
        requestId: ctx.requestId,
        leaseId: ctx.lease?.id,
        ok: false,
        error,
        inputKeys,
        resource: executorResource(capability),
      })
      return { ok: false, capability, error, nodeId, leaseId: ctx.lease?.id }
    }
  }
}

// ── Built-in session executors ───────────────────────────────────────────────

const sessionListExecutor: CapabilityExecutor<unknown, unknown> = {
  capability: "harness.session",
  async execute() {
    const sessions = Array.from(Session.list()).filter((s) => s.time.archived === undefined)
    return sessions.map((session) => ({
      session,
      status: SessionStatus.get(session.id),
    }))
  },
}

const sessionGetExecutor: CapabilityExecutor<{ sessionID: string }, unknown> = {
  capability: "harness.session.get",
  async execute(input) {
    const sessionID = String(input.sessionID)
    const session = await Session.get(sessionID)
    const messages = await Session.messages({ sessionID })
    return {
      session,
      status: SessionStatus.get(sessionID),
      messages,
    }
  },
}

const sessionMessageExecutor: CapabilityExecutor<
  {
    sessionID: string
    text: string
    attachments?: unknown
    agent?: string
    model?: { providerID: string; modelID: string }
    system?: string
    effort?: string
    metadata?: Record<string, unknown>
  },
  unknown
> = {
  capability: "harness.session.message",
  async execute(input) {
    const sessionID = String(input.sessionID)
    await Session.get(sessionID)
    const parts: SessionPrompt.PromptInput["parts"] = [{ type: "text", text: String(input.text) }]
    const attachments = Array.isArray(input.attachments) ? input.attachments : []
    for (const attachment of attachments) {
      if (attachment && typeof attachment === "object") {
        const a = attachment as Record<string, string>
        parts.push({
          type: "file",
          mime: a.mime ?? "application/octet-stream",
          url: a.url ?? "",
          filename: a.filename,
        })
      }
    }

    const promptOptions: SessionPrompt.PromptInput = { sessionID, parts }

    if (input.agent && String(input.agent).length > 0) {
      promptOptions.agent = String(input.agent)
    }

    if (
      input.model &&
      typeof input.model === "object" &&
      "providerID" in input.model &&
      "modelID" in input.model
    ) {
      promptOptions.model = {
        providerID: String(input.model.providerID),
        modelID: String(input.model.modelID),
      }
    }

    if (input.system && String(input.system).length > 0) {
      // "+" prefix: append to gizzi's default assembled system prompt rather
      // than replace it, matching the platform agent-chat bridge.
      const system = String(input.system)
      promptOptions.system = system.startsWith("+") ? system : `+${system}`
    }

    if (input.metadata && typeof input.metadata === "object") {
      promptOptions.metadata = input.metadata as Record<string, unknown>
    }

    // A bot's turn (runtime/bots/bot-turn.ts), same marker as /v1/session/:id/message.
    const bot = BotTurnInfo.safeParse((input as { bot?: unknown }).bot)
    if (bot.success) promptOptions.bot = bot.data

    // Effort is forwarded via metadata so the runtime can apply it to
    // reasoning-capable models and ignore it for models without reasoning.
    if (input.effort && String(input.effort).length > 0) {
      promptOptions.metadata = {
        ...promptOptions.metadata,
        effort: String(input.effort),
      }
    }

    SessionPrompt.prompt(promptOptions)
    return { accepted: true, sessionID }
  },
}

const sessionAbortExecutor: CapabilityExecutor<{ sessionID: string }, boolean> = {
  capability: "harness.session.abort",
  async execute(input) {
    const sessionID = String(input.sessionID)
    await Session.get(sessionID)
    SessionPrompt.cancel(sessionID)
    return true
  },
}

const sessionEventsExecutor: CapabilityExecutor<{ sessionID: string }, unknown> = {
  capability: "harness.session.events",
  async execute(input) {
    const sessionID = String(input.sessionID)
    await Session.get(sessionID)
    // The bus SSE stream carries every session's events; callers filter on
    // sessionID. (There is no per-session events route; the old
    // /v1/session-worker/sessions/:id/events was never served.)
    return { streamEndpoint: "/v1/event", sessionID }
  },
}

const sessionCreateExecutor: CapabilityExecutor<
  {
    title?: string
    agentID?: string
    surface?: string
    permission?: unknown
  },
  unknown
> = {
  capability: "harness.session.create",
  async execute(input) {
    const session = await Session.create({
      title: input.title,
      agentID: input.agentID,
      surface: input.surface as any,
      permission: input.permission as any,
    })
    return session
  },
}

const sessionPermissionsListExecutor: CapabilityExecutor<unknown, unknown> = {
  capability: "harness.session.permissions.list",
  async execute() {
    return PermissionNext.list()
  },
}

const sessionPermissionsReplyExecutor: CapabilityExecutor<
  { requestID: string; reply: "once" | "always" | "reject"; message?: string },
  unknown
> = {
  capability: "harness.session.permissions.reply",
  async execute(input) {
    await PermissionNext.reply({
      requestID: String(input.requestID),
      reply: input.reply as PermissionNext.Reply,
      message: input.message,
    })
    return true
  },
}

const sessionQuestionsListExecutor: CapabilityExecutor<unknown, unknown> = {
  capability: "harness.session.questions.list",
  async execute() {
    return Question.list()
  },
}

const sessionQuestionsReplyExecutor: CapabilityExecutor<
  { requestID: string; answers: string[][] },
  unknown
> = {
  capability: "harness.session.questions.reply",
  async execute(input) {
    await Question.reply({
      requestID: String(input.requestID),
      answers: input.answers,
    })
    return true
  },
}

const sessionQuestionsRejectExecutor: CapabilityExecutor<{ requestID: string }, unknown> = {
  capability: "harness.session.questions.reject",
  async execute(input) {
    await Question.reject(String(input.requestID))
    return true
  },
}

const nodeCapabilitiesExecutor: CapabilityExecutor<unknown, NodeIdentity> = {
  capability: "harness.node.capabilities",
  async execute(_, ctx) {
    return ctx.node
  },
}

// ── Shell executor ───────────────────────────────────────────────────────────

interface ShellExecInput {
  command: string
  args?: string[]
  cwd?: string
  env?: Record<string, string>
  timeoutMs?: number
}

const shellExecExecutor: CapabilityExecutor<ShellExecInput, unknown> = {
  capability: "harness.shell",
  async execute(input) {
    const cwd = input.cwd || Instance.directory
    const args = input.args ?? []
    const timeout = input.timeoutMs ?? 60_000
    const proc = Bun.spawn([input.command, ...args], {
      cwd,
      env: { ...process.env, ...input.env },
      stdout: "pipe",
      stderr: "pipe",
    })
    const timer = setTimeout(() => proc.kill("SIGTERM"), timeout)
    const stdout = await new Response(proc.stdout).text().catch(() => "")
    const stderr = await new Response(proc.stderr).text().catch(() => "")
    const exitCode = await proc.exited
    clearTimeout(timer)
    return {
      stdout,
      stderr,
      exitCode,
      cwd,
    }
  },
}

// ── File executors ───────────────────────────────────────────────────────────

interface FileReadInput {
  path: string
}

interface FileWriteInput {
  path: string
  content: string
}

interface FileListInput {
  path?: string
}

interface FileSearchInput {
  query: string
  limit?: number
}

const fileReadExecutor: CapabilityExecutor<FileReadInput, unknown> = {
  capability: "harness.file.read",
  async execute(input) {
    return File.read(input.path)
  },
}

const fileListExecutor: CapabilityExecutor<FileListInput, unknown> = {
  capability: "harness.file.list",
  async execute(input) {
    return File.list(input.path)
  },
}

const fileSearchExecutor: CapabilityExecutor<FileSearchInput, unknown> = {
  capability: "harness.file.search",
  async execute(input) {
    return File.search({ query: input.query, limit: input.limit })
  },
}

const fileWriteExecutor: CapabilityExecutor<FileWriteInput, unknown> = {
  capability: "harness.file.write",
  async execute(input) {
    const full = path.join(Instance.directory, input.path)
    if (!Instance.containsPath(full)) {
      throw new Error(`Access denied: path escapes project directory`)
    }
    await Filesystem.write(full, input.content)
    return { written: true, path: input.path }
  },
}

// ── Browser executor ─────────────────────────────────────────────────────────

interface BrowserNavigateInput {
  url: string
  includeContent?: boolean
  maxContentLength?: number
}

const browserNavigateExecutor: CapabilityExecutor<BrowserNavigateInput, unknown> = {
  capability: "harness.browser.navigate",
  async execute(input) {
    const targetUrl = String(input.url)
    const parsed = new URL(targetUrl)
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
      throw new Error("Only http/https URLs are allowed")
    }

    const upstream = await fetch(targetUrl, {
      headers: {
        "User-Agent":
          "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
        Accept: "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
      },
      redirect: "follow",
      signal: AbortSignal.timeout(15_000),
    })

    const contentType = upstream.headers.get("content-type") ?? ""
    const isHtml = contentType.includes("text/html") || contentType === ""
    const maxLen = input.maxContentLength ?? 32_768

    let text: string | undefined
    let title: string | undefined
    if (isHtml) {
      text = await upstream.text().catch(() => "")
      const titleMatch = text.match(/<title[^>]*>([\s\S]*?)<\/title>/i)
      title = titleMatch?.[1]?.trim()
      if (text.length > maxLen) text = text.slice(0, maxLen) + "…"
    } else if (input.includeContent) {
      text = await upstream.text().catch(() => "")
      if (text.length > maxLen) text = text.slice(0, maxLen) + "…"
    }

    return {
      url: upstream.url,
      status: upstream.status,
      contentType,
      title,
      content: input.includeContent ? text : undefined,
    }
  },
}

/** Eagerly register built-in executors. */
export function registerBuiltinExecutors() {
  CapabilityExecutors.register(sessionListExecutor)
  CapabilityExecutors.register(sessionGetExecutor)
  CapabilityExecutors.register(sessionCreateExecutor)
  CapabilityExecutors.register(sessionMessageExecutor)
  CapabilityExecutors.register(sessionAbortExecutor)
  CapabilityExecutors.register(sessionEventsExecutor)
  CapabilityExecutors.register(sessionPermissionsListExecutor)
  CapabilityExecutors.register(sessionPermissionsReplyExecutor)
  CapabilityExecutors.register(sessionQuestionsListExecutor)
  CapabilityExecutors.register(sessionQuestionsReplyExecutor)
  CapabilityExecutors.register(sessionQuestionsRejectExecutor)
  CapabilityExecutors.register(nodeCapabilitiesExecutor)
  CapabilityExecutors.register(shellExecExecutor)
  CapabilityExecutors.register(fileReadExecutor)
  CapabilityExecutors.register(fileListExecutor)
  CapabilityExecutors.register(fileSearchExecutor)
  CapabilityExecutors.register(fileWriteExecutor)
  CapabilityExecutors.register(browserNavigateExecutor)
}

registerBuiltinExecutors()
