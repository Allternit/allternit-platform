import { createHash } from "node:crypto"
import z from "zod/v4"
import { Bus } from "@/shared/bus"
import { BusEvent } from "@/shared/bus/bus-event"
import { Log } from "@/shared/util/log"
import { Instance } from "@/runtime/context/project/instance"
import { Database, and, eq } from "@/runtime/session/storage/db"
import { SessionTable } from "@/runtime/session/session.sql"
import { Session } from "@/runtime/session"
import { MessageV2 } from "@/runtime/session/message-v2"
import { SessionStatus } from "@/runtime/session/status"
import { PermissionNext } from "@/runtime/tools/guard/permission/next"
import { Question } from "@/runtime/integrations/question"

/**
 * Live terminal sessions (Remote Control on Allternit sign-in).
 *
 * A Gizzi TUI session runs its own agent loop in the terminal process. It
 * attaches to the engine as an ordinary engine session (sourceRef.harness
 * `gizzi-tui`), pushes its transcript, status and permission asks here, and
 * holds the inbox stream open. Everything a surface already does to an engine
 * session works on it unchanged:
 *
 * - its rows and events (session.*, message.*, permission.asked/replied) are
 *   the engine's own, so Desktop, web, PWA and iOS see them on the streams they
 *   already read;
 * - a prompt (any route: /session/:id/message, prompt_async, remote-control)
 *   goes to the terminal instead of starting an engine turn, as do Stop and
 *   permission replies.
 *
 * When the terminal goes away the session stays an ordinary engine session:
 * a prompt from any surface then continues it on the engine.
 */
export namespace TerminalLive {
  const log = Log.create({ service: "terminal-live" })

  export const HARNESS = "gizzi-tui"

  /** One message of the terminal transcript, already in engine terms. */
  export const Part = z.discriminatedUnion("kind", [
    z.object({ kind: z.literal("text"), key: z.string(), text: z.string() }),
    z.object({ kind: z.literal("reasoning"), key: z.string(), text: z.string() }),
    z.object({
      kind: z.literal("tool"),
      key: z.string(),
      callID: z.string(),
      tool: z.string(),
      input: z.record(z.string(), z.any()),
      status: z.enum(["running", "completed", "error"]),
      title: z.string().optional(),
      output: z.string().optional(),
      start: z.number().optional(),
      end: z.number().optional(),
    }),
  ])
  export type Part = z.infer<typeof Part>

  export const Message = z.object({
    /** The terminal's stable id for the message (its uuid). */
    key: z.string(),
    role: z.enum(["user", "assistant"]),
    time: z.number(),
    completed: z.number().optional(),
    /** The prompt an assistant turn answers (its key and time). */
    parent: z.object({ key: z.string(), time: z.number() }).optional(),
    providerID: z.string().optional(),
    modelID: z.string().optional(),
    error: z.string().optional(),
    parts: Part.array(),
  })
  export type Message = z.infer<typeof Message>

  /** What the engine sends down a terminal's inbox stream. */
  export type Inbox =
    | { type: "input"; id: string; text: string }
    | { type: "stop" }
    | { type: "permission"; requestID: string; allow: boolean; message?: string }
    /** Answers in question order (labels per question); absent when declined. */
    | { type: "question"; requestID: string; answers?: string[][] }

  export const Event = {
    /** A terminal attached to or left a session (drives the live dot on surfaces). */
    Attached: BusEvent.define(
      "session.terminal",
      z.object({
        sessionID: z.string(),
        attached: z.boolean(),
        host: z.string().optional(),
        cwd: z.string().optional(),
      }),
    ),
  }

  type Live = {
    sessionID: string
    directory: string
    host?: string
    pid?: number
    attachedAt: number
    send: (event: Inbox) => void
    model?: { providerID: string; modelID: string }
  }

  // Module-level, not Instance state: prompts and replies for the session can
  // arrive under any directory (server.ts routes /session/<id> to its stored one).
  const live = new Map<string, Live>()
  // Text already sent as deltas: session → part → text. Dropped on detach.
  const streamed = new Map<string, Map<string, string>>()

  export function attached(sessionID: string): boolean {
    return live.has(sessionID)
  }

  export function list() {
    return [...live.values()].map(({ sessionID, directory, host, pid, attachedAt }) => ({
      sessionID,
      directory,
      host,
      pid,
      attachedAt,
    }))
  }

  /** Find or create the engine session for a terminal session id. */
  export async function adopt(input: {
    terminalID: string
    /** The terminal's working folder (stored on the row; the row itself lives in the engine's default project so every surface lists it). */
    cwd: string
    title?: string
    transcriptPath?: string
    model?: { providerID: string; modelID: string }
  }): Promise<Session.Info> {
    const row = Database.use((db) =>
      db
        .select({ id: SessionTable.id })
        .from(SessionTable)
        .where(and(eq(SessionTable.source_harness, HARNESS), eq(SessionTable.source_session_id, input.terminalID)))
        .get(),
    )
    if (row) {
      const info = await Session.get(row.id)
      if (input.title && info.title !== input.title) return Session.setTitle({ sessionID: info.id, title: input.title })
      return info
    }
    return Session.createNext({
      directory: input.cwd,
      title: input.title,
      surface: "code",
      defaultModel: input.model,
      defaultModelSource: input.model ? "user" : undefined,
      sourceRef: {
        harness: HARNESS,
        sessionId: input.terminalID,
        // Never empty: a row without a source path reads back with no sourceRef.
        path: input.transcriptPath || input.terminalID,
        snapshotHash: "live",
        snapshotAt: Date.now(),
      },
    })
  }

  /** Hold the inbox open for a terminal; returns the detach function. */
  export function attach(input: Omit<Live, "attachedAt">): () => void {
    const previous = live.get(input.sessionID)
    const entry: Live = { ...input, attachedAt: Date.now() }
    live.set(input.sessionID, entry)
    if (!previous)
      Bus.publish(Event.Attached, {
        sessionID: input.sessionID,
        attached: true,
        host: input.host,
        cwd: input.directory,
      })
    log.info("attached", { sessionID: input.sessionID, host: input.host, pid: input.pid })
    return () => {
      if (live.get(input.sessionID) !== entry) return
      live.delete(input.sessionID)
      streamed.delete(input.sessionID)
      SessionStatus.set(input.sessionID, { type: "idle" })
      Bus.publish(Event.Attached, {
        sessionID: input.sessionID,
        attached: false,
        host: input.host,
        cwd: input.directory,
      })
      log.info("detached", { sessionID: input.sessionID })
    }
  }

  // ── ids: stable per terminal key, ascending by time like engine ids ──────

  function stableID(prefix: "msg" | "prt", time: number, key: string) {
    const ts = (BigInt(Math.max(0, Math.floor(time))) * 0x1000n).toString(16).padStart(12, "0").slice(-12)
    return `${prefix}_${ts}${createHash("sha256").update(key).digest("hex").slice(0, 14)}`
  }

  export function messageID(m: Pick<Message, "key" | "time">) {
    return stableID("msg", m.time, m.key)
  }

  function toPart(p: Part, m: Message, id: string, sessionID: string): MessageV2.Part {
    const base = { id: stableID("prt", m.time, `${m.key}:${p.key}`), sessionID, messageID: id }
    const start = p.kind === "tool" ? (p.start ?? m.time) : m.time
    return p.kind === "text"
      ? { ...base, type: "text", text: p.text, time: { start: m.time, end: m.completed } }
      : p.kind === "reasoning"
        ? { ...base, type: "reasoning", text: p.text, time: { start: m.time, end: m.completed } }
        : {
            ...base,
            type: "tool",
            callID: p.callID,
            tool: p.tool,
            state:
              p.status === "running"
                ? { status: "running", input: p.input, title: p.title, time: { start } }
                : p.status === "completed"
                  ? {
                      status: "completed",
                      input: p.input,
                      output: p.output ?? "",
                      title: p.title ?? p.tool,
                      metadata: {},
                      time: { start, end: p.end ?? Date.now() },
                    }
                  : {
                      status: "error",
                      input: p.input,
                      error: p.output ?? "Failed",
                      time: { start, end: p.end ?? Date.now() },
                    },
          }
  }

  /** Upsert terminal messages as engine messages and parts, publishing the usual events. */
  export async function sync(sessionID: string, messages: Message[]) {
    const directory = live.get(sessionID)?.directory ?? Instance.directory
    let lastUser: string | undefined
    const session = await Session.get(sessionID)
    const fallback = session.defaultModel
    // Name an untitled session after its first prompt, like engine sessions are.
    const first = messages.find((m) => m.role === "user")?.parts.find((p) => p.kind === "text")
    if (first && first.kind === "text" && Session.isDefaultTitle(session.title))
      await Session.setTitle({ sessionID, title: first.text.replace(/\s+/g, " ").trim().slice(0, 80) })
    for (const m of messages) {
      const id = messageID(m)
      const model = {
        providerID: m.providerID ?? fallback?.providerID ?? HARNESS,
        modelID: m.modelID ?? fallback?.modelID ?? "terminal",
      }
      const info: MessageV2.Info =
        m.role === "user"
          ? { id, sessionID, role: "user", time: { created: m.time }, agent: "build", model, metadata: { terminal: true } }
          : ({
              id,
              sessionID,
              role: "assistant",
              parentID: m.parent ? messageID(m.parent) : (lastUser ?? id),
              time: { created: m.time, completed: m.completed },
              ...model,
              mode: "build",
              agent: "build",
              path: { cwd: directory, root: directory },
              cost: 0,
              tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } },
              ...(m.error ? { error: { name: "TerminalError", message: m.error } } : {}),
              ...(m.completed ? { finish: "stop" } : {}),
            } as MessageV2.Assistant)
      if (m.role === "user") lastUser = id
      const stored = Session.writeMessage(info, false)
      const parts = m.parts.map((p) => toPart(p, m, id, sessionID))
      for (const part of parts) await Session.updatePart(part)
      // Published once its parts are stored: the agent-sessions feed reads a
      // message's text from its parts when it maps message.updated.
      Bus.publish(MessageV2.Event.Updated, { info: stored })
      for (const part of parts) {
        Bus.publish(MessageV2.Event.PartUpdated, { part })
        // Text as deltas too, so a chat stream waiting on this turn (/api/agent-chat) shows the reply.
        if (m.role !== "assistant" || (part.type !== "text" && part.type !== "reasoning")) continue
        const sent = streamed.get(sessionID) ?? new Map<string, string>()
        streamed.set(sessionID, sent)
        const before = sent.get(part.id) ?? ""
        if (part.text.length > before.length && part.text.startsWith(before))
          Bus.publish(MessageV2.Event.PartDelta, {
            sessionID,
            messageID: part.messageID,
            partID: part.id,
            field: "text",
            delta: part.text.slice(before.length),
          })
        sent.set(part.id, part.text)
      }
    }
  }

  export function status(sessionID: string, busy: boolean) {
    SessionStatus.set(sessionID, busy ? { type: "busy" } : { type: "idle" })
  }

  // ── surface → terminal ───────────────────────────────────────────────────

  /** A prompt from any surface: hand it to the terminal. False when no terminal holds the session. */
  export function deliver(sessionID: string, text: string): boolean {
    // Chat bridges (allternit-api, /api/agent-chat) wrap the person's text with
    // the app's own instructions; the terminal gets only what the person typed.
    const request = /<user-request>\n?([\s\S]*?)\n?<\/user-request>/.exec(text)
    if (request) text = request[1]
    const entry = live.get(sessionID)
    if (!entry || !text.trim()) return false
    entry.send({ type: "input", id: crypto.randomUUID(), text })
    return true
  }

  export function stop(sessionID: string): boolean {
    const entry = live.get(sessionID)
    if (!entry) return false
    entry.send({ type: "stop" })
    return true
  }

  /** Publish a terminal permission ask; the first answer from any surface goes back to the terminal. */
  export function ask(input: {
    sessionID: string
    requestID: string
    tool: string
    description: string
    input: Record<string, unknown>
    callID?: string
  }) {
    const request: PermissionNext.Request = {
      id: input.requestID,
      sessionID: input.sessionID,
      permission: input.tool,
      // What the card names: the file, command or URL the tool acts on.
      patterns: [
        String(input.input.file_path ?? input.input.command ?? input.input.url ?? input.input.pattern ?? (input.description || input.tool)),
      ],
      metadata: { terminal: true, description: input.description, input: input.input },
      always: [],
      ...(input.callID ? { tool: { messageID: "", callID: input.callID } } : {}),
    }
    void PermissionNext.relay(request).then((result) => {
      live.get(input.sessionID)?.send({ type: "permission", requestID: input.requestID, ...result })
    })
  }

  /** The terminal answered (or dropped) its own ask: close the card everywhere. */
  export function settle(requestID: string, allow: boolean) {
    return PermissionNext.settle(requestID, allow ? "once" : "reject")
  }

  /** Publish a terminal question (AskUserQuestion); the first answer from any surface goes back to the terminal. */
  export function askQuestion(input: {
    sessionID: string
    requestID: string
    questions: Question.Info[]
    callID?: string
  }) {
    const request: Question.Request = {
      id: input.requestID,
      sessionID: input.sessionID,
      questions: input.questions,
      ...(input.callID ? { tool: { messageID: "", callID: input.callID } } : {}),
    }
    void Question.relay(request).then((answers) => {
      live.get(input.sessionID)?.send({ type: "question", requestID: input.requestID, answers })
    })
  }

  /** The terminal answered (or dropped) its own question: close it everywhere. */
  export function settleQuestion(requestID: string, answers: string[][] | undefined) {
    return Question.settle(requestID, answers)
  }
}
