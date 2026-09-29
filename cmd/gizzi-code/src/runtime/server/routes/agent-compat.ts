//! Agent-sessions compatibility layer for the Allternit iOS app.
//!
//! The app speaks the platform protocol implemented by allternit-api
//! (cmd/allternit-api/src/agent_session_routes.rs +
//! cmd/allternit-api/src/v1_routes.rs agent_chat_bridge):
//!
//!   GET    /api/v1/agent-sessions              → { sessions, count }
//!   POST   /api/v1/agent-sessions              → 201 session record
//!   GET    /api/v1/agent-sessions/sync         → SSE session/event feed
//!   GET    /api/v1/agent-sessions/:id          → session record
//!   PATCH  /api/v1/agent-sessions/:id          → session record
//!   DELETE /api/v1/agent-sessions/:id          → 204
//!   GET    /api/v1/agent-sessions/:id/messages → bare message array
//!   POST   /api/v1/agent-sessions/:id/messages → message record
//!   POST   /api/v1/agent-sessions/:id/abort    → { success: true }
//!   POST   /api/v1/agent-sessions/:id/revert   → session record
//!   POST   /api/v1/agent-sessions/:id/unrevert → session record
//!   POST   /api/v1/agent-sessions/:id/compact  → summary result
//!   POST   /api/agent-chat                     → SSE reply stream
//!
//! allternit-api implements this as a thin gateway that translates the app
//! contract into gizzi's native session/event contract over HTTP. This module
//! is the same translation, but running INSIDE gizzi-code: it calls the
//! session engine (Session/SessionPrompt/Bus) in-process instead of looping
//! back over HTTP, so any gizzi instance can serve as the app's "agent brain"
//! directly. Response shapes mirror the Rust handlers field-for-field; the
//! consumer is AgentChatClient.swift (unchanged).
//!
//! Deliberate differences from the Rust bridge:
//!
//! - System instructions: the Rust bridge composes persona files (SOUL.md,
//!   STYLE.md, registry system_prompt, response-style prefs) server-side and
//!   wraps them in <system-instructions>. Here we rely on gizzi's native
//!   context loading (InstructionPrompt.system() in session/instruction.ts
//!   resolves GIZZI.md/CLAUDE.md/AGENTS.md/CONTEXT.md per directory level and
//!   injects them into every turn) instead of duplicating that composition. A
//!   client-sent systemPrompt is still honored via the same
//!   <system-instructions>/<user-request> wrap the bridge uses.
//!
//! - The platform agent registry (allternit-api's sqlite: enabled_modes
//!   surface allow-list, harness_config, SOUL/STYLE workspace files) does not
//!   exist inside gizzi-code. `agent_id` is stored on the session natively
//!   (Session.Info.agentID) but there is no registry lookup, no per-surface
//!   allow-list check, and no harness push — the engine's own configured
//!   providers/models are the brain.
//!
//! - Ephemeral (incognito) sessions and cowork `metadata.projectId` stamps are
//!   kept in process memory (the Rust API keeps them in its own sqlite;
//!   gizzi's session table has no columns for them). A process restart
//!   forgets the flags; abandoned ephemeral sessions would reappear in list
//!   responses until deleted.
import { Hono } from "hono"
import { streamSSE } from "hono/streaming"
import { Session } from "@/runtime/session"
import { SessionPrompt } from "@/runtime/session/prompt"
import { SessionRevert } from "@/runtime/session/revert"
import { SessionSummary } from "@/runtime/session/summary"
import { Provider } from "@/runtime/providers/provider"
import { Bus } from "@/shared/bus"
import { Log } from "@/shared/util/log"
import { toolFramesForPart, usageFromMessageInfo } from "./tool-frames"

const log = Log.create({ service: "agent-compat" })

/** chatId → gizzi session id (mirrors GIZZI_CHAT_SESSIONS in v1_routes.rs). */
const chatSessions = new Map<string, string>()
/** Incognito sessions: excluded from list responses, purged on abort. */
const ephemeralSessions = new Set<string>()
/** Original frontend surface when it isn't natively storable on the session. */
const originSurfaces = new Map<string, string>()
/** Cowork project stamp from `metadata.projectId` at create time. */
const projectStamps = new Map<string, string>()

/** Session.Info.surface is a fixed enum; anything else is memory-only. */
const NATIVE_SURFACES = new Set(["chat", "cowork", "code", "browser", "design"])

function toISO(timestampMs: number | undefined | null): string {
  if (timestampMs) {
    const dt = new Date(timestampMs)
    if (!Number.isNaN(dt.getTime())) return dt.toISOString()
  }
  return new Date().toISOString()
}

function originSurfaceOf(info: any): string {
  return originSurfaces.get(info.id) ?? info.surface ?? ""
}

function projectIDOf(info: any): string | null {
  return projectStamps.get(info.id) ?? info.projectID ?? null
}

/// Mirrors `transform_session` (agent_session_routes.rs:274-311).
function transformSession(info: any) {
  const createdAt = toISO(info.time?.created)
  const updatedAt = toISO(info.time?.updated ?? info.time?.created)
  return {
    id: info.id,
    name: info.title ?? null,
    description: null,
    created_at: createdAt,
    updated_at: updatedAt,
    last_accessed: updatedAt,
    message_count: 0,
    active: info.time?.archived == null,
    tags: [],
    metadata: {
      project_id: projectIDOf(info),
      directory: info.directory ?? null,
      version: info.version ?? null,
      agent_id: info.agentID ?? null,
      surface: info.surface ?? null,
      originSurface: originSurfaceOf(info),
      permission: info.permission ?? null,
      ephemeral: ephemeralSessions.has(info.id),
      continuesFrom: info.continuesFrom ?? null,
      handoff: info.handoff ?? null,
      paused: info.paused ?? null,
    },
  }
}

/// Mirrors `extract_message_content` (agent_session_routes.rs:313-346).
/// `reasoning` is deliberately excluded — it ships separately as `thinking`.
function extractMessageContent(parts: any[]): string {
  const textParts: string[] = []
  for (const part of parts) {
    switch (part?.type) {
      case "text":
      case "agent":
        if (part.text) textParts.push(part.text)
        break
      case "file":
        textParts.push(`[File ${part.filename ?? part.url ?? "attachment"}]`)
        break
      case "tool":
        if (part.tool) textParts.push(`[Tool ${part.tool}]`)
        break
    }
  }
  return textParts.join("\n")
}

/// Mirrors `extract_reasoning` (agent_session_routes.rs:348-360).
function extractReasoning(parts: any[]): string | null {
  const reasoning = parts
    .filter((part) => part?.type === "reasoning")
    .map((part) => part.text)
    .filter(Boolean)
    .join("\n")
  return reasoning === "" ? null : reasoning
}

/** The message a `message.updated` event names (by id); the newest one when it names none. */
export function eventMessage<T extends { info?: { id?: string } }>(messages: T[], id: string | undefined): T | undefined {
  return id ? messages.findLast((m) => m.info?.id === id) : messages.at(-1)
}

/// Mirrors `transform_message` (agent_session_routes.rs:362-394).
function transformMessage(message: any) {
  const info = message?.info ?? {}
  const parts = message?.parts ?? []
  let content = extractMessageContent(parts)
  if (content === "") content = info.error?.message ?? "[No text content]"
  return {
    id: info.id,
    role: info.role,
    content,
    thinking: extractReasoning(parts),
    timestamp: toISO(info.time?.completed ?? info.time?.created),
    metadata: {
      agent: info.agent ?? null,
      model: info.model ?? null,
      // Handoff checkpoint seed (P3.16): clients draw the rip, not a bubble.
      handoff: parts.find((p: any) => p?.metadata?.handoff)?.metadata.handoff ?? null,
      parts,
      error: info.error?.data ?? null,
    },
  }
}

/// Mirrors `select_model` (agent_session_routes.rs:406-439): the client may
/// carry the model as `metadata.model.{providerID,modelID}` or flattened on
/// `metadata` itself. Without one, the engine's own configured default
/// applies (the native equivalent of the Rust AppConfig default), so the
/// model field is simply omitted.
function selectModel(metadata: any): { providerID: string; modelID: string } | undefined {
  const model = metadata?.model
  if (model && typeof model.providerID === "string" && typeof model.modelID === "string") {
    return { providerID: model.providerID, modelID: model.modelID }
  }
  if (typeof metadata?.providerID === "string" && typeof metadata?.modelID === "string") {
    return { providerID: metadata.providerID, modelID: metadata.modelID }
  }
  return undefined
}

function ephemeralFrom(body: any): boolean {
  if (body?.ephemeral === true) return true
  const flag = body?.metadata?.ephemeral
  return flag === true || flag === "true"
}

function surfaceFrom(body: any): string | undefined {
  const surface = body?.origin_surface ?? body?.metadata?.surface
  return typeof surface === "string" && surface !== "" ? surface : undefined
}

/// Session get that tolerates ids failing the native id-format validation
/// (returns undefined instead of throwing for non-`ses_*` ids).
async function findSession(id: string) {
  if (!id.startsWith("ses")) return undefined
  return Session.get(id).catch(() => undefined)
}

export const AgentCompatRoutes = () =>
  new Hono()
    // /sync must be registered before /:sessionID so the literal wins.
    .get("/v1/agent-sessions/sync", async (c) => {
      // SSE mirror of the Rust sync_sessions handler: transform gizzi bus
      // events into the frontend feed shapes (transform_bus_event,
      // agent_session_routes.rs:994-1099). Honors Last-Event-ID / ?since for
      // reconnect replay via Bus.historySince, like routes/event.ts.
      const lastEventID = c.req.header("Last-Event-ID") ?? c.req.query("since")
      const sinceSeq = lastEventID ? Number(lastEventID) : NaN
      c.header("X-Accel-Buffering", "no")

      const transform = async (event: any): Promise<any | undefined> => {
        const type = event?.type
        const props = event?.properties ?? {}
        switch (type) {
          case "session.created": {
            const info = props.info
            if (!info) return undefined
            return { ...transformSession(info), type: "created" }
          }
          case "session.updated": {
            const info = props.info
            if (!info) return undefined
            return {
              type: "updated",
              session_id: info.id,
              name: info.title ?? null,
              description: null,
              active: info.time?.archived == null,
              tags: [],
              metadata: {
                project_id: projectIDOf(info),
                directory: info.directory ?? null,
                version: info.version ?? null,
                agent_id: info.agentID ?? null,
                surface: info.surface ?? null,
                originSurface: originSurfaceOf(info),
                permission: info.permission ?? null,
                continuesFrom: info.continuesFrom ?? null,
                handoff: info.handoff ?? null,
                paused: info.paused ?? null,
              },
            }
          }
          case "session.handoff": {
            // Context handoff: the conversation moved to a fresh window.
            if (!props.from || !props.to) return undefined
            if (ephemeralSessions.has(props.from)) ephemeralSessions.add(props.to)
            return {
              type: "handed_off",
              session_id: props.from,
              to: props.to,
              reason: props.reason ?? null,
              generation: props.generation ?? null,
            }
          }
          case "session.deleted": {
            const info = props.info
            if (!info) return undefined
            return { type: "deleted", session_id: info.id }
          }
          case "message.updated": {
            const sessionID = props.info?.sessionID
            if (!sessionID) return undefined
            const messages = await Session.messages({ sessionID }).catch(() => [])
            // The message the event names, not the newest one: a turn's reply
            // row lands a few ms after the user's, so "newest" dropped the
            // user message from the feed.
            const message = eventMessage(messages, props.info?.id)
            if (!message) return undefined
            return { ...transformMessage(message), type: "message_added", session_id: sessionID }
          }
          case "permission.asked":
            return {
              type: "permission_asked",
              request_id: props.id ?? null,
              session_id: props.sessionID ?? null,
              permission: props.permission ?? null,
              patterns: props.patterns ?? null,
              metadata: props.metadata ?? null,
              always: props.always ?? null,
              tool: props.tool ?? null,
            }
          case "permission.replied":
            return {
              type: "permission_replied",
              request_id: props.requestID ?? null,
              session_id: props.sessionID ?? null,
              reply: props.reply ?? null,
            }
          case "question.asked":
            return {
              type: "question_asked",
              request_id: props.id ?? null,
              session_id: props.sessionID ?? null,
              questions: props.questions ?? null,
            }
          case "pane_browser.requested":
            return {
              type: "pane_browser_requested",
              request_id: props.id ?? null,
              session_id: props.sessionID ?? null,
              action: props.action ?? null,
              target: props.target ?? null,
              text: props.text ?? null,
              time: props.time ?? null,
            }
          case "pane_render.requested":
            return {
              type: "pane_render_requested",
              request_id: props.id ?? null,
              session_id: props.sessionID ?? null,
              kind: props.kind ?? null,
              format: props.format ?? null,
              code: props.code ?? null,
              width: props.width ?? null,
              height: props.height ?? null,
              title: props.title ?? null,
              time: props.time ?? null,
            }
          case "pane_artifact.requested":
            return {
              type: "pane_artifact_requested",
              request_id: props.id ?? null,
              session_id: props.sessionID ?? null,
              action: props.action ?? null,
              tool: props.tool ?? null,
              input: props.input ?? null,
              time: props.time ?? null,
            }
          case "message.part.updated":
            return {
              type: "part_updated",
              session_id: props.part?.sessionID ?? null,
              message_id: props.part?.messageID ?? null,
              part: props.part ?? null,
            }
          case "message.part.delta":
            return {
              type: "part_delta",
              session_id: props.sessionID ?? null,
              message_id: props.messageID ?? null,
              part_id: props.partID ?? null,
              field: props.field ?? null,
              delta: props.delta ?? null,
            }
          case "message.part.removed":
            return {
              type: "part_removed",
              session_id: props.sessionID ?? null,
              message_id: props.messageID ?? null,
              part_id: props.partID ?? null,
            }
          case "session.compacted":
            return {
              type: "session_compacted",
              session_id: props.sessionID ?? null,
            }
          default:
            return undefined
        }
      }

      return streamSSE(c, async (stream) => {
        if (Number.isFinite(sinceSeq) && sinceSeq > 0) {
          for (const { seq, event } of Bus.historySince(sinceSeq)) {
            const payload = await transform(event)
            if (payload) await stream.writeSSE({ id: String(seq), data: JSON.stringify(payload) })
          }
        }
        const unsub = Bus.subscribeAll(async (event: any) => {
          if (event?.type === "server.heartbeat") return // keep-alive; no SSE comment support in hono
          const seq = Bus.currentSeq()
          const payload = await transform(event)
          if (payload) await stream.writeSSE({ id: String(seq), data: JSON.stringify(payload) })
          if (event?.type === Bus.InstanceDisposed.type) stream.close()
        })
        await new Promise<void>((resolve) => {
          stream.onAbort(() => {
            unsub()
            resolve()
          })
        })
      })
    })
    .get("/v1/agent-sessions", async (c) => {
      // list_sessions (agent_session_routes.rs:514-613): { sessions, count }
      // with surface / project_id / q filters and ephemeral exclusion.
      const surfaceFilter = c.req.query("surface")
      const projectFilter = c.req.query("project_id")
      const textFilter = c.req.query("q")?.trim().toLowerCase() || undefined

      const filtered: any[] = []
      for (const info of Array.from(Session.list())) {
        if (ephemeralSessions.has(info.id)) continue
        const transformed = transformSession(info)
        if (surfaceFilter && transformed.metadata.originSurface !== surfaceFilter) continue
        if (projectFilter && transformed.metadata.project_id !== projectFilter) continue
        if (textFilter) {
          const titleMatches = (transformed.name ?? "").toLowerCase().includes(textFilter)
          if (!titleMatches) {
            const messages = await Session.messages({ sessionID: info.id }).catch(() => [])
            const contentMatches = messages.some((message: any) =>
              extractMessageContent(message.parts ?? [])
                .toLowerCase()
                .includes(textFilter),
            )
            if (!contentMatches) continue
          }
        }
        filtered.push(transformed)
      }
      return c.json({ sessions: filtered, count: filtered.length })
    })
    .post("/v1/agent-sessions", async (c) => {
      // create_session (agent_session_routes.rs:634-737) → 201 record.
      const body = await c.req.json().catch(() => ({}))
      const surface = surfaceFrom(body)
      const session = await Session.create({
        title: typeof body.name === "string" && body.name !== "" ? body.name : "New Session",
        surface: surface && NATIVE_SURFACES.has(surface) ? (surface as any) : undefined,
        agentID: typeof body.agent_id === "string" && body.agent_id !== "" ? body.agent_id : undefined,
      })
      // The Rust API restores the original surface from its own table when it
      // had to normalize for gizzi; here only non-native values need that.
      if (surface && !NATIVE_SURFACES.has(surface)) originSurfaces.set(session.id, surface)
      // Cowork project stamp (`metadata.projectId`, snake_case accepted too).
      const projectID = body?.metadata?.projectId ?? body?.metadata?.project_id
      if (typeof projectID === "string" && projectID !== "") projectStamps.set(session.id, projectID)
      if (ephemeralFrom(body)) ephemeralSessions.add(session.id)
      // NOTE: the Rust handler also resolves the platform agent's harness
      // config and surface allow-list from its registry DB, and pins a default
      // model on the session. Neither registry nor per-session model pinning
      // exists in gizzi-code — the engine's configured providers/models apply.
      return c.json(transformSession(session), 201)
    })
    .get("/v1/agent-sessions/:sessionID", async (c) => {
      const info = await findSession(c.req.param("sessionID"))
      if (!info) return c.json({ error: "Session not found" }, 404)
      return c.json(transformSession(info))
    })
    .patch("/v1/agent-sessions/:sessionID", async (c) => {
      // update_session (agent_session_routes.rs:752-802).
      const sessionID = c.req.param("sessionID")
      const existing = await findSession(sessionID)
      if (!existing) return c.json({ error: "Session not found" }, 404)
      const body = await c.req.json().catch(() => ({}))
      if (typeof body.name === "string") await Session.setTitle({ sessionID, title: body.name })
      if (typeof body.active === "boolean") {
        await Session.setArchived({ sessionID, time: body.active ? undefined : Date.now() })
      }
      if (body?.metadata?.permission !== undefined) {
        await Session.setPermission({ sessionID, permission: body.metadata.permission })
      }
      const surface = surfaceFrom(body)
      if (surface) {
        if (NATIVE_SURFACES.has(surface)) {
          await Session.setSurface({ sessionID, surface: surface as any })
          originSurfaces.delete(sessionID)
        } else {
          originSurfaces.set(sessionID, surface)
        }
      }
      return c.json(transformSession(await Session.get(sessionID)))
    })
    .delete("/v1/agent-sessions/:sessionID", async (c) => {
      const sessionID = c.req.param("sessionID")
      await Session.remove(sessionID)
      ephemeralSessions.delete(sessionID)
      originSurfaces.delete(sessionID)
      projectStamps.delete(sessionID)
      return c.body(null, 204)
    })
    .get("/v1/agent-sessions/:sessionID/messages", async (c) => {
      // list_messages: bare array of transformed messages.
      const sessionID = c.req.param("sessionID")
      const existing = await findSession(sessionID)
      if (!existing) return c.json({ error: "Session not found" }, 404)
      const messages = await Session.messages({ sessionID })
      return c.json(messages.map(transformMessage))
    })
    .post("/v1/agent-sessions/:sessionID/messages", async (c) => {
      // send_message (agent_session_routes.rs:835-869). Non-"user" roles are
      // stored client-side only; the API echoes a local record.
      const sessionID = c.req.param("sessionID")
      const body = await c.req.json().catch(() => ({}))
      const role = typeof body.role === "string" && body.role !== "" ? body.role : "user"
      if (role !== "user") {
        return c.json({
          id: `local-${crypto.randomUUID()}`,
          role,
          content: body.text ?? "",
          thinking: body.thinking ?? null,
          timestamp: new Date().toISOString(),
          metadata: body.metadata ?? null,
        })
      }
      const existing = await findSession(sessionID)
      if (!existing) return c.json({ error: "Session not found" }, 404)
      // Blocking, like the native /session/:id/message route: the response is
      // the completed assistant message.
      // `noReply` records the message without a model turn (a note typed in
      // ACI while a run is going); `source` labels where it was typed.
      const source = typeof body.source === "string" && body.source !== "" ? body.source : undefined
      const result = await SessionPrompt.prompt({
        sessionID,
        parts: [{ type: "text", text: String(body.text ?? ""), ...(source ? { metadata: { source } } : {}) }],
        model: selectModel(body.metadata),
        ...(body.noReply === true ? { noReply: true } : {}),
      })
      return c.json(transformMessage(result))
    })
    .post("/v1/agent-sessions/:sessionID/abort", async (c) => {
      // abort_session: stop the loop; incognito sessions are purged on abort.
      const sessionID = c.req.param("sessionID")
      SessionPrompt.cancel(sessionID)
      if (ephemeralSessions.has(sessionID)) {
        await Session.remove(sessionID)
        ephemeralSessions.delete(sessionID)
        originSurfaces.delete(sessionID)
        projectStamps.delete(sessionID)
      }
      return c.json({ success: true })
    })
    .post("/v1/agent-sessions/:sessionID/revert", async (c) => {
      // revert_session: iOS sends no body (Phase 8 edit-resend probed 200
      // against the API); revert only when a messageId is actually given,
      // then return the re-fetched session record either way.
      const sessionID = c.req.param("sessionID")
      const existing = await findSession(sessionID)
      if (!existing) return c.json({ error: "Session not found" }, 404)
      const body = await c.req.json().catch(() => ({}))
      if (typeof body.messageId === "string" && body.messageId !== "") {
        await SessionRevert.revert({ sessionID, messageID: body.messageId })
      }
      return c.json(transformSession(await Session.get(sessionID)))
    })
    .post("/v1/agent-sessions/:sessionID/unrevert", async (c) => {
      const sessionID = c.req.param("sessionID")
      const existing = await findSession(sessionID)
      if (!existing) return c.json({ error: "Session not found" }, 404)
      await SessionRevert.unrevert({ sessionID })
      return c.json(transformSession(await Session.get(sessionID)))
    })
    .post("/v1/agent-sessions/:sessionID/compact", async (c) => {
      // compact_session → gizzi's on-demand summarize. SessionSummary.summarize
      // wants the anchor message id; use the session's latest user message
      // (same anchor the native /session/:id/summarize route now uses).
      const sessionID = c.req.param("sessionID")
      const existing = await findSession(sessionID)
      if (!existing) return c.json({ error: "Session not found" }, 404)
      const messages = await Session.messages({ sessionID })
      // summarizeMessage annotates the anchor with a User-schema summary
      // object, so the anchor must be a user message (assistant anchors fail
      // Assistant.summary's boolean schema).
      const anchor = messages.findLast((m: any) => m.info?.role === "user")
      if (!anchor) return c.json({ error: "Session has no messages to compact" }, 400)
      const result = await SessionSummary.summarize({ sessionID, messageID: anchor.info.id })
      return c.json((result ?? null) as null)
    })
    .post("/agent-chat", async (c) => {
      // agent_chat_bridge (v1_routes.rs:395-890): subscribe to the bus BEFORE
      // prompting, stream message.part.delta events as content_block_delta
      // frames (reasoning parts as thinking_delta), finish when the session
      // goes idle after busy or the turn settles.
      const body = await c.req.json().catch(() => undefined)
      if (!body || typeof body !== "object") return c.json({ error: "bad request body" }, 400)

      const chatID = typeof body.chatId === "string" ? body.chatId : ""
      const message = typeof body.message === "string" ? body.message : ""
      if (chatID === "" || message === "") {
        return c.json({ error: "chatId and message are required" }, 400)
      }
      const clientSystemPrompt = typeof body.systemPrompt === "string" ? body.systemPrompt : undefined
      // agentId is accepted for contract parity. Unlike the Rust bridge there
      // is no platform registry to resolve it against (404 "Agent not found"
      // is an API-side concept); gizzi's own agent personas apply natively.

      // runtimeModelId ("provider/model" or the frontend's "provider::model"
      // convention) wins; without one the engine default applies. `effort`
      // has no PromptInput equivalent — the Rust bridge forwards it into the
      // gizzi payload where the engine drops unknown fields, so dropping it
      // here matches the effective behavior.
      const rawModel =
        (typeof body.runtimeModelId === "string" && body.runtimeModelId) ||
        (typeof body.modelId === "string" && body.modelId) ||
        undefined
      let modelRef: { providerID: string; modelID: string } | undefined
      let modelLabel: string
      if (rawModel) {
        const normalized = rawModel.replaceAll("::", "/")
        const slash = normalized.indexOf("/")
        modelRef =
          slash > 0
            ? { providerID: normalized.slice(0, slash), modelID: normalized.slice(slash + 1) }
            : undefined
        modelLabel = normalized
      } else {
        const fallback = await Provider.defaultModel().catch(() => undefined)
        modelLabel = fallback ? `${fallback.providerID}/${fallback.modelID}` : "auto/auto"
      }

      // get_or_create_gizzi_session (v1_routes.rs:33-88): ses_* chat ids are
      // already gizzi session ids (created via /api/v1/agent-sessions); other
      // chat ids get a backing session created once and cached.
      let sessionID = chatSessions.get(chatID)
      try {
        if (!sessionID) {
          const existing = await findSession(chatID)
          sessionID = existing?.id ?? (await Session.create({ title: `Allternit chat ${chatID}` })).id
          chatSessions.set(chatID, sessionID)
        }
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err)
        log.error("failed to resolve agent-chat session", { error: message })
        return c.json({ error: message }, 500)
      }

      const msgID = `msg_${crypto.randomUUID().replaceAll("-", "")}`

      // The Rust bridge wraps its server-composed instructions and the user
      // text into one message body. Here the persona/workspace layers are
      // gizzi's own (context packing); only the client-sent systemPrompt
      // rides the same wrap.
      const effectiveMessage = clientSystemPrompt?.trim()
        ? `<system-instructions>\n${clientSystemPrompt}\n</system-instructions>\n\n<user-request>\n${message}\n</user-request>`
        : message

      const parts: any[] = [{ type: "text", text: effectiveMessage }]
      // Composer attachments (mobile "+" sheet): {url?, dataBase64?,
      // mediaType, name?} → gizzi file parts, dataBase64 inlined as a data
      // URL (v1_routes.rs:695-723).
      if (Array.isArray(body.attachments)) {
        for (const attachment of body.attachments) {
          const mediaType = typeof attachment?.mediaType === "string" ? attachment.mediaType : "application/octet-stream"
          const url =
            (typeof attachment?.url === "string" && attachment.url) ||
            (typeof attachment?.dataBase64 === "string"
              ? `data:${mediaType};base64,${attachment.dataBase64}`
              : undefined)
          if (!url) continue
          const part: any = { type: "file", url, mime: mediaType }
          if (typeof attachment?.name === "string") part.filename = attachment.name
          parts.push(part)
        }
      }
      // Composer tool options (metadata.tools) — stashed into the prompt
      // metadata like the bridge does (runtime may ignore them).
      const tools = body?.metadata?.tools
      const metadata = tools && typeof tools === "object" ? { tools } : undefined

      c.header("X-Accel-Buffering", "no")
      return streamSSE(c, async (stream) => {
        const write = (frame: any) => stream.writeSSE({ data: JSON.stringify(frame) })
        // Latest assistant usage seen on the bus (message.updated carries the
        // full message info incl. tokens) — attached to the finish frame so
        // clients can render an exact tok/s instead of a chars/4 estimate.
        let lastUsage: Record<string, number> | undefined
        // Latest context report + whether token counts were estimated.
        let lastContext: { used: number; window?: number; basis: string } | undefined
        let usageEstimated = false
        const finish = (status: "complete" | "error", error?: { error: string; errorDetails?: any }) => ({
          type: "finish",
          messageId: msgID,
          status,
          ...(status === "complete" && (lastUsage || lastContext || usageEstimated)
            ? {
                usage: {
                  ...(lastUsage ?? {}),
                  ...(lastContext
                    ? {
                        contextUsed: lastContext.used,
                        ...(lastContext.window ? { contextWindow: lastContext.window } : {}),
                        contextBasis: lastContext.basis,
                      }
                    : {}),
                  ...(usageEstimated ? { estimated: true } : {}),
                },
              }
            : {}),
          metadata: { status, ...error },
        })

        await write({
          type: "message_start",
          messageId: msgID,
          modelId: modelLabel,
          runtimeModelId: modelLabel,
        })

        // Frames published by the engine while the turn runs. The turn is
        // blocking (SessionPrompt.prompt awaits the loop), so events are
        // queued from the Bus subscription and drained in order.
        const queue: any[] = []
        let notify: (() => void) | undefined
        const push = (frame: any) => {
          queue.push(frame)
          notify?.()
        }

        // partID → type tracking: message.part.updated carries the part type
        // ("reasoning") while deltas don't (v1_routes.rs:793-796).
        const reasoningParts = new Set<string>()
        // Parts whose type a message.part.updated has declared. Deltas for an
        // undeclared part wait (in order) for the declaration so an early
        // reasoning delta never leaks into the reply as text.
        const knownParts = new Set<string>()
        let pendingDeltas: Array<{ partID: string; delta: string }> = []
        const deltaFrame = (partID: string, delta: string) => ({
          type: "content_block_delta",
          messageId: msgID,
          partId: partID,
          delta: reasoningParts.has(partID)
            ? { type: "thinking_delta", thinking: delta }
            : { type: "text_delta", text: delta },
        })
        // callID → last tool frame sent ("start" | "end"), so each call
        // yields exactly one tool_use start and one result/error.
        const toolFramesSent = new Map<string, "start" | "end">()
        let wasBusy = false
        const unsub = Bus.subscribeAll((event: any) => {
          const type = event?.type
          const props = event?.properties ?? {}
          if (type === "message.part.updated") {
            const part = props.part
            if (part?.sessionID !== sessionID) return
            if (part?.type === "reasoning" && typeof part?.id === "string") reasoningParts.add(part.id)
            if (typeof part?.id === "string" && !knownParts.has(part.id)) {
              knownParts.add(part.id)
              const ready = pendingDeltas.filter((d) => d.partID === part.id)
              pendingDeltas = pendingDeltas.filter((d) => d.partID !== part.id)
              for (const d of ready) push(deltaFrame(d.partID, d.delta))
            }
            if (part?.type === "tool") {
              for (const frame of toolFramesForPart(part, msgID, toolFramesSent)) push(frame)
            }
            return
          }
          if (type === "message.updated") {
            // message.updated carries the full message info; keep the newest
            // assistant usage so the finish frame can report real tokens.
            const info = props.info
            if (info?.sessionID !== sessionID || info?.role !== "assistant") return
            lastUsage = usageFromMessageInfo(info) ?? lastUsage
            return
          }
          const evtSession = typeof props.sessionID === "string" ? props.sessionID : ""
          if (evtSession !== "" && evtSession !== sessionID) return // different session — ignore
          if (type === "message.part.delta") {
            const partID = typeof props.partID === "string" ? props.partID : "text-1"
            const delta = typeof props.delta === "string" ? props.delta : ""
            if (!delta) return
            if (!knownParts.has(partID)) pendingDeltas.push({ partID, delta })
            else push(deltaFrame(partID, delta))
            return
          }
          if (type === "session.status") {
            const statusType = props.status?.type
            if (statusType === "busy") wasBusy = true
            else if (statusType === "idle" && wasBusy) push(finish("complete"))
            return
          }
          if (type === "session.context.updated") {
            if (props.usageEstimated === true) usageEstimated = true
            if (typeof props.used === "number") {
              lastContext = {
                used: props.used,
                window: typeof props.window === "number" ? props.window : undefined,
                basis: props.basis === "provider" ? "provider" : "estimated",
              }
              push({ type: "context_usage", messageId: msgID, context: lastContext })
            }
            return
          }
          if (type === "session.compacted") {
            // Context compaction ran on this session (auto or manual) while
            // the turn is in flight: surface it so the chat can render a
            // "context compacted" divider instead of going silent.
            push({ type: "context_compacted", messageId: msgID, sessionID })
            return
          }
        })

        // The session's working folder (its project's folder), same field
        // allternit-api's bridge forwards.
        const workdir = typeof body.workdir === "string" ? body.workdir : undefined
        const turn = SessionPrompt.prompt({ sessionID, parts, model: modelRef, metadata, workdir })
          .then(() => push(finish("complete")))
          .catch((err: any) => {
            // Pass the engine's structured error through so clients can
            // render targeted UI (e.g. ProviderModelNotFoundError), like the
            // bridge does with the runtime's error body (v1_routes.rs:755-770).
            const details = typeof err?.toObject === "function" ? err.toObject() : undefined
            const message = err instanceof Error ? err.message : String(err)
            log.error("agent-chat turn failed", { error: message })
            push(
              finish("error", {
                error: `gizzi message failed: ${message}`,
                ...(details ? { errorDetails: details } : {}),
              }),
            )
          })

        try {
          for (;;) {
            while (queue.length > 0) {
              const frame = queue.shift()
              if (frame?.type === "finish" && pendingDeltas.length > 0) {
                // Deltas whose part was never declared can only be reply text.
                for (const d of pendingDeltas) await write(deltaFrame(d.partID, d.delta))
                pendingDeltas = []
              }
              await write(frame)
              if (frame?.type === "finish") return
            }
            await new Promise<void>((resolve) => {
              notify = resolve
            })
            notify = undefined
          }
        } finally {
          unsub()
          // Like the Rust bridge, a client disconnect does not abort the turn
          // server-side; abort is the caller's job (POST .../abort).
          void turn
        }
      })
    })
