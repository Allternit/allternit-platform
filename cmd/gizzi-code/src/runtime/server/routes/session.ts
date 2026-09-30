import { Hono } from "hono"
import { SessionHandoff } from "@/runtime/session/handoff"
import { VendorMessage } from "@/runtime/session/vendor-message"
import { SessionPause } from "@/runtime/session/pause"
import { Budget } from "@/runtime/session/budget"
import { stream } from "hono/streaming"
import { describeRoute, validator, resolver } from "@/runtime/server/openapi"
import z from "zod/v4"
import { Session } from "@/runtime/session"
import { MessageV2 } from "@/runtime/session/message-v2"
import { SessionPrompt } from "@/runtime/session/prompt"
import { SessionCompaction } from "@/runtime/session/compaction"
import { SessionRevert } from "@/runtime/session/revert"
import { SessionStatus } from "@/runtime/session/status"
import { RunRegistry } from "@/runtime/session/run-registry"
import { SessionSummary } from "@/runtime/session/summary"
import { Todo } from "@/runtime/session/todo"
import { BackgroundTask } from "@/runtime/session/background-task"
import { Agent } from "@/runtime/loop/agent"
import { Snapshot } from "@/runtime/session/snapshot"
import { Log } from "@/shared/util/log"
import { PermissionNext } from "@/runtime/tools/guard/permission/next"
import { Bus } from "@/shared/bus"
import { errors } from "@/runtime/server/error"
import { lazy } from "@/shared/util/lazy"
import { SessionTrace } from "@/runtime/session/trace"
import { SessionSupportBundle } from "@/runtime/session/support-bundle"
import { Scratchpad } from "@/runtime/session/scratchpad"

const log = Log.create({ service: "server" })

export const SessionRoutes = lazy(() =>
  new Hono()
    .get(
      "/:sessionID/scratchpad",
      describeRoute({
        summary: "List session scratchpad files",
        description: "List private and root-session shared scratchpad metadata without returning file contents.",
        operationId: "session.scratchpad.list",
        responses: { 200: { description: "Scratchpad metadata", content: { "application/json": { schema: resolver(z.any()) } } }, ...errors(404) },
      }),
      validator("param", z.object({ sessionID: z.string() })),
      async (c) => {
        const { sessionID } = c.req.valid("param")
        await Session.get(sessionID)
        const result = await Scratchpad.list(sessionID)
        return c.json({
          sessionID: result.scope.sessionID,
          rootSessionID: result.scope.rootSessionID,
          entries: result.entries,
        })
      },
    )
    .get(
      "/:sessionID/scratchpad/file",
      describeRoute({
        summary: "Read a scratchpad file",
        operationId: "session.scratchpad.read",
        responses: { 200: { description: "Scratchpad file", content: { "application/json": { schema: resolver(z.any()) } } }, ...errors(404) },
      }),
      validator("param", z.object({ sessionID: z.string() })),
      validator("query", z.object({ path: z.string().min(1), shared: z.enum(["true", "false"]).optional() })),
      async (c) => {
        const { sessionID } = c.req.valid("param")
        const query = c.req.valid("query")
        await Session.get(sessionID)
        return c.json(await Scratchpad.read({ sessionID, path: query.path, shared: query.shared === "true" }))
      },
    )
    .put(
      "/:sessionID/scratchpad/file",
      describeRoute({
        summary: "Write a scratchpad file",
        operationId: "session.scratchpad.write",
        responses: { 200: { description: "Scratchpad write metadata", content: { "application/json": { schema: resolver(z.any()) } } }, ...errors(400, 404) },
      }),
      validator("param", z.object({ sessionID: z.string() })),
      validator("json", z.object({ path: z.string().min(1), content: z.string(), shared: z.boolean().optional() })),
      async (c) => {
        const { sessionID } = c.req.valid("param")
        const body = c.req.valid("json")
        await Session.get(sessionID)
        return c.json(await Scratchpad.write({ sessionID, ...body }))
      },
    )
    .delete(
      "/:sessionID/scratchpad/file",
      describeRoute({
        summary: "Remove a scratchpad file",
        operationId: "session.scratchpad.remove",
        responses: { 200: { description: "Whether the file was removed", content: { "application/json": { schema: resolver(z.boolean()) } } }, ...errors(400, 404) },
      }),
      validator("param", z.object({ sessionID: z.string() })),
      validator("query", z.object({ path: z.string().min(1), shared: z.enum(["true", "false"]).optional() })),
      async (c) => {
        const { sessionID } = c.req.valid("param")
        const query = c.req.valid("query")
        await Session.get(sessionID)
        return c.json(await Scratchpad.remove({ sessionID, path: query.path, shared: query.shared === "true" }))
      },
    )
    .get(
      "/:sessionID/replay",
      describeRoute({
        summary: "Replay durable session events",
        description: "Return a cursor-bounded append-only trace, optionally with a current-state snapshot for initial hydration.",
        operationId: "session.replay",
        responses: { 200: { description: "Replay page", content: { "application/json": { schema: resolver(z.any()) } } }, ...errors(404) },
      }),
      validator("param", z.object({ sessionID: z.string() })),
      validator("query", z.object({
        after: z.coerce.number().int().min(0).default(0),
        limit: z.coerce.number().int().min(1).max(5000).default(500),
        snapshot: z.enum(["true", "false"]).optional(),
      })),
      async (c) => {
        const { sessionID } = c.req.valid("param")
        const query = c.req.valid("query")
        const info = await Session.get(sessionID).catch(() => undefined)
        if (!info) return c.json({ error: "Session not found" }, 404)
        const head = SessionTrace.head(sessionID)
        const entries = SessionTrace.list({ sessionID, after: query.after, through: head, limit: query.limit })
        const cursor = entries.at(-1)?.sequence ?? query.after
        const includeSnapshot = query.snapshot === "true" || (query.snapshot === undefined && query.after === 0)
        return c.json({
          sessionID,
          head,
          cursor,
          hasMore: cursor < head,
          snapshot: includeSnapshot ? { info, messages: await Session.messages({ sessionID }) } : undefined,
          entries,
        })
      },
    )
    .get(
      "/:sessionID/support-bundle",
      describeRoute({
        summary: "Export a redacted support bundle",
        description: "Create a local diagnostic ZIP with session state, replay trace, task state, and bounded recent logs.",
        operationId: "session.supportBundle",
        responses: { 200: { description: "Redacted ZIP archive", content: { "application/zip": { schema: resolver(z.any()) } } }, ...errors(404) },
      }),
      validator("param", z.object({ sessionID: z.string() })),
      async (c) => {
        const { sessionID } = c.req.valid("param")
        const exists = await Session.get(sessionID).catch(() => undefined)
        if (!exists) return c.json({ error: "Session not found" }, 404)
        const archive = await SessionSupportBundle.create(sessionID)
        c.header("Content-Type", "application/zip")
        c.header("Content-Disposition", `attachment; filename="gizzi-support-${sessionID}.zip"`)
        return c.body(archive as Uint8Array<ArrayBuffer>)
      },
    )
    .get(
      "/:sessionID/background-tasks",
      describeRoute({
        summary: "List session background tasks",
        description: "Return durable background task lifecycle records for a parent session.",
        operationId: "session.backgroundTasks.list",
        responses: { 200: { description: "Background tasks", content: { "application/json": { schema: resolver(z.any()) } } } },
      }),
      validator("param", z.object({ sessionID: z.string() })),
      async (c) => {
        const { sessionID } = c.req.valid("param")
        return c.json(await BackgroundTask.list(sessionID))
      },
    )
    .post(
      "/:sessionID/background-tasks/:taskID/cancel",
      describeRoute({
        summary: "Cancel a session background task",
        description: "Cancel the owned child session and durably settle its task record.",
        operationId: "session.backgroundTasks.cancel",
        responses: { 200: { description: "Background task cancelled", content: { "application/json": { schema: resolver(z.any()) } } }, ...errors(404) },
      }),
      validator("param", z.object({ sessionID: z.string(), taskID: z.string() })),
      async (c) => {
        const { sessionID, taskID } = c.req.valid("param")
        const task = await BackgroundTask.get(taskID)
        if (!task || task.parentSessionID !== sessionID) return c.json({ error: "Background task not found" }, 404)
        if (task.childSessionID) SessionPrompt.cancel(task.childSessionID)
        return c.json(await BackgroundTask.cancel(taskID))
      },
    )
    .get(
      "/list",
      describeRoute({
        summary: "List sessions",
        description: "Retrieve a list of all active and archived sessions.",
        operationId: "session.list",
        responses: {
          200: {
            description: "List of sessions",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
        },
      }),
      async (c) => {
        const agentID = c.req.query("agentID")
        const sessions = Array.from(Session.list(agentID ? { agentID } : undefined))
        return c.json(sessions)
      },
    )
    .post(
      "/",
      describeRoute({
        summary: "Create session",
        description: "Create a new session.",
        operationId: "session.create",
        responses: {
          200: {
            description: "Newly created session",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
          ...errors(400),
        },
      }),
      validator("json", z.any()),
      async (c) => {
        const input = c.req.valid("json") as any
        const model = input.model
        if (model && !input.defaultModel) {
          input.defaultModel = model
          input.defaultModelSource = "user"
        }
        const session = await Session.create(input)
        return c.json(session)
      },
    )
    .get(
      "/status",
      describeRoute({
        summary: "Get all session statuses",
        description: "Retrieve the current status (idle, busy, etc.) for all active sessions.",
        operationId: "session.allStatus",
        responses: {
          200: {
            description: "Session statuses",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
        },
      }),
      async (c) => {
        const status = await SessionStatus.all()
        return c.json(status)
      },
    )
    .get(
      "/:sessionID",
      describeRoute({
        summary: "Get session details",
        description: "Retrieve detailed information about a specific session by its ID.",
        operationId: "session.get",
        responses: {
          200: {
            description: "Session details",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
          ...errors(404),
        },
      }),
      validator("param", z.any()),
      async (c) => {
        const { sessionID } = c.req.valid("param") as any
        const session = await Session.get(sessionID)
        return c.json(session)
      },
    )
    .post(
      "/:sessionID/initialize",
      describeRoute({
        summary: "Initialize session",
        description: "Initialize a session with a starting message or context.",
        operationId: "session.initialize",
        responses: {
          200: {
            description: "Session initialized successfully",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
          ...errors(400, 404),
        },
      }),
      validator("param", z.any()),
      validator("json", z.any()),
      async (c) => {
        const { sessionID } = c.req.valid("param") as any
        const input = c.req.valid("json") as any
        const session = await Session.initialize({ ...input, sessionID })
        return c.json(session as unknown as null)
      },
    )
    .get(
      "/:sessionID/messages",
      describeRoute({
        summary: "List session messages",
        description: "Retrieve messages belonging to a specific session, optionally only the newest `limit`.",
        operationId: "session.messages",
        responses: {
          200: {
            description: "List of messages",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
          ...errors(404),
        },
      }),
      validator("param", z.any()),
      validator("query", z.object({ limit: z.coerce.number().int().min(1).optional() })),
      async (c) => {
        const { sessionID } = c.req.valid("param") as any
        // `limit` keeps only the newest N messages (list previews need one).
        const { limit } = c.req.valid("query")
        const msgs = await Session.messages({ sessionID, limit })
        return c.json(msgs)
      },
    )
    .post(
      "/:sessionID/message",
      describeRoute({
        summary: "Send message to session",
        description: "Send a prompt message to a session and trigger the agent loop.",
        operationId: "session.prompt",
        responses: {
          200: {
            description: "Message sent successfully",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
          ...errors(400, 404),
        },
      }),
      validator("param", z.any()),
      validator("json", z.any()),
      async (c) => {
        const { sessionID } = c.req.valid("param") as any
        const input = c.req.valid("json") as any
        const result = await SessionPrompt.prompt({ ...input, sessionID })
        return c.json(result)
      },
    )
    .post(
      "/:sessionID/command",
      describeRoute({
        summary: "Run command in session",
        description: "Execute a command within a session context.",
        operationId: "session.command",
        responses: {
          200: {
            description: "Command executed",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
          ...errors(400, 404),
        },
      }),
      validator("param", z.any()),
      validator("json", z.any()),
      async (c) => {
        const { sessionID } = c.req.valid("param") as any
        const input = c.req.valid("json") as any
        const result = await SessionPrompt.command({ ...input, sessionID })
        return c.json(result)
      },
    )
    .post(
      "/:sessionID/abort",
      describeRoute({
        summary: "Abort session",
        description: "Abort the currently running agent loop for a session.",
        operationId: "session.abort",
        responses: {
          200: {
            description: "Session aborted",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
          ...errors(404),
        },
      }),
      validator("param", z.any()),
      async (c) => {
        const { sessionID } = c.req.valid("param") as any
        SessionPrompt.cancel(sessionID)
        return c.json(true)
      },
    )
    .delete(
      "/:sessionID",
      describeRoute({
        summary: "Delete session",
        description: "Delete a session and all its messages.",
        operationId: "session.delete",
        responses: {
          200: {
            description: "Session deleted",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
          ...errors(404),
        },
      }),
      validator("param", z.any()),
      async (c) => {
        const { sessionID } = c.req.valid("param") as any
        await Session.remove(sessionID)
        return c.json(true)
      },
    )
    .patch(
      "/:sessionID",
      describeRoute({
        summary: "Update session",
        description: "Update session properties like title.",
        operationId: "session.update",
        responses: {
          200: {
            description: "Session updated",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
          ...errors(404),
        },
      }),
      validator("param", z.any()),
      validator("json", z.any()),
      async (c) => {
        const { sessionID } = c.req.valid("param") as any
        const input = c.req.valid("json") as any
        if (input.title !== undefined) {
          await Session.setTitle({ sessionID, title: input.title })
        }
        if (input.archived !== undefined) {
          await Session.setArchived({ sessionID, time: input.archived ? Date.now() : undefined })
        }
        if (input.permission !== undefined) {
          await Session.setPermission({ sessionID, permission: input.permission })
        }
        if (input.surface !== undefined) {
          await Session.setSurface({ sessionID, surface: input.surface })
        }
        const session = await Session.get(sessionID)
        return c.json(session)
      },
    )
    .post(
      "/:sessionID/fork",
      describeRoute({
        summary: "Fork session",
        description: "Create a fork of an existing session.",
        operationId: "session.fork",
        responses: {
          200: {
            description: "Forked session",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
          ...errors(404),
        },
      }),
      validator("param", z.any()),
      async (c) => {
        const { sessionID } = c.req.valid("param") as any
        const result = await Session.fork({ sessionID })
        return c.json(result)
      },
    )
    .post(
      "/:sessionID/vendor-message",
      describeRoute({
        summary: "Append a vendor reply",
        description:
          "Append an assistant message attributed to a vendor bot (Agent Gateway) without running a turn. Deduped by metadata.remote_event_id.",
        operationId: "session.vendorMessage",
        responses: {
          200: { description: "The stored (or already-stored) message", content: { "application/json": { schema: resolver(z.any()) } } },
          ...errors(400, 404),
        },
      }),
      validator("param", z.object({ sessionID: z.string() })),
      validator("json", z.object({ text: z.string().min(1), metadata: z.record(z.string(), z.any()).optional() })),
      async (c) => {
        const { sessionID } = c.req.valid("param")
        const body = c.req.valid("json")
        return c.json(await VendorMessage.append({ sessionID, text: body.text, metadata: body.metadata }))
      },
    )
    .post(
      "/:sessionID/handoff",
      describeRoute({
        summary: "Hand off session",
        description:
          "End this session's context window with a checkpoint baton and continue in a fresh, linked session. Idempotent: a session that already handed off returns its lineage head.",
        operationId: "session.handoff",
        responses: {
          200: {
            description: "The session that now holds the conversation, and the baton it started from",
            content: { "application/json": { schema: resolver(z.any()) } },
          },
          ...errors(400, 404),
        },
      }),
      validator("param", z.object({ sessionID: z.string() })),
      validator(
        "json",
        z
          .object({
            reason: SessionHandoff.Reason.optional(),
            context: z.string().optional(),
            baton: SessionHandoff.Baton.partial().optional(),
          })
          .optional(),
      ),
      async (c) => {
        const { sessionID } = c.req.valid("param")
        const body = c.req.valid("json") ?? {}
        const result = await SessionHandoff.run({
          sessionID,
          reason: body.reason ?? "manual",
          context: body.context,
          baton: body.baton,
        })
        return c.json(result)
      },
    )
    .get(
      "/:sessionID/lineage",
      describeRoute({
        summary: "Session lineage",
        description: "Every context window of this conversation, oldest first; the last one is the head.",
        operationId: "session.lineage",
        responses: {
          200: {
            description: "Sessions in the lineage",
            content: { "application/json": { schema: resolver(z.any()) } },
          },
          ...errors(404),
        },
      }),
      validator("param", z.object({ sessionID: z.string() })),
      async (c) => {
        const { sessionID } = c.req.valid("param")
        const sessions = await SessionHandoff.lineage(sessionID)
        return c.json({ head: sessions[sessions.length - 1].id, sessions })
      },
    )
    .post(
      "/:sessionID/resume",
      describeRoute({
        summary: "Resume a paused session",
        description:
          "Resume a session paused before a usage limit. Without a model it continues on its own model (normally done automatically at the reset); with one, it is the explicit \"Resume now on …\" on another model.",
        operationId: "session.resume",
        responses: {
          200: { description: "Resumed", content: { "application/json": { schema: resolver(z.any()) } } },
          ...errors(404),
        },
      }),
      validator("param", z.object({ sessionID: z.string() })),
      validator(
        "json",
        z.object({ model: z.object({ providerID: z.string(), modelID: z.string() }).optional() }).optional(),
      ),
      async (c) => {
        const { sessionID } = c.req.valid("param")
        const body = c.req.valid("json") ?? {}
        // The resumed turn runs in the background; the caller follows it on /event.
        void SessionPause.resume(sessionID, { model: body.model }).catch(() => undefined)
        return c.json({ resumed: true })
      },
    )
    .get(
      "/budget/:scope/:targetID",
      describeRoute({
        summary: "Spend against a budget",
        description: "A bot's monthly budget (scope agent) or a thread's budget (scope session): limit, spent, reset.",
        operationId: "session.budget.get",
        responses: { 200: { description: "Budget", content: { "application/json": { schema: resolver(z.any()) } } } },
      }),
      validator("param", z.object({ scope: z.enum(["agent", "session"]), targetID: z.string() })),
      async (c) => {
        const { scope, targetID } = c.req.valid("param")
        const info = await Budget.info(scope, targetID)
        if (info) return c.json(info)
        return c.json({
          scope,
          targetID,
          limitUsd: null,
          spentUsd: scope === "agent" ? Budget.agentSpend(targetID, Budget.monthStart()) : await Budget.lineageSpend(targetID),
        })
      },
    )
    .put(
      "/budget/:scope/:targetID",
      describeRoute({
        summary: "Set a budget",
        description: "Set (or clear with null) a bot's monthly budget or a thread's budget in USD.",
        operationId: "session.budget.set",
        responses: { 200: { description: "Budget", content: { "application/json": { schema: resolver(z.any()) } } } },
      }),
      validator("param", z.object({ scope: z.enum(["agent", "session"]), targetID: z.string() })),
      validator("json", z.object({ limitUsd: z.number().min(0).nullable() })),
      async (c) => {
        const { scope, targetID } = c.req.valid("param")
        const { limitUsd } = c.req.valid("json")
        Budget.set(scope, targetID, limitUsd)
        return c.json((await Budget.info(scope, targetID)) ?? { scope, targetID, limitUsd: null })
      },
    )
    .post(
      "/:sessionID/share",
      describeRoute({
        summary: "Share session",
        description: "Share a session publicly.",
        operationId: "session.share",
        responses: {
          200: {
            description: "Session shared",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
          ...errors(404),
        },
      }),
      validator("param", z.any()),
      async (c) => {
        const { sessionID } = c.req.valid("param") as any
        const result = await Session.share(sessionID)
        return c.json(result)
      },
    )
    .get(
      "/:sessionID/diff",
      describeRoute({
        summary: "Get session diff",
        description: "Get file diffs for a session.",
        operationId: "session.diff",
        responses: {
          200: {
            description: "Session diff",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
          ...errors(404),
        },
      }),
      validator("param", z.any()),
      async (c) => {
        const { sessionID } = c.req.valid("param") as any
        const result = await Session.diff(sessionID)
        return c.json(result)
      },
    )
    .post(
      "/:sessionID/summarize",
      describeRoute({
        summary: "Summarize session",
        description: "Generate a summary for a session.",
        operationId: "session.summarize",
        responses: {
          200: {
            description: "Session summary",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
          ...errors(404),
        },
      }),
      validator("param", z.any()),
      async (c) => {
        const { sessionID } = c.req.valid("param") as any
        // SessionSummary.summarize wants the anchor message id (a USER
        // message — assistant anchors fail the Assistant.summary schema);
        // this route previously passed a bare sessionID and 500'd.
        const messages = await Session.messages({ sessionID })
        const anchor = messages.findLast((m: any) => m.info?.role === "user")
        if (!anchor) return c.json({ error: "Session has no messages to summarize" }, 400)
        const result = await SessionSummary.summarize({ sessionID, messageID: anchor.info.id })
        return c.json((result ?? null) as null)
      },
    )
    .post(
      "/:sessionID/revert",
      describeRoute({
        summary: "Revert session",
        description: "Revert file changes made during a session.",
        operationId: "session.revert",
        responses: {
          200: {
            description: "Session reverted",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
          ...errors(404),
        },
      }),
      validator("param", z.any()),
      validator("json", z.any()),
      async (c) => {
        const { sessionID } = c.req.valid("param") as any
        const input = c.req.valid("json") as any
        const result = await SessionRevert.revert({ sessionID, messageID: input.messageID })
        return c.json(result)
      },
    )
    .post(
      "/:sessionID/unrevert",
      describeRoute({
        summary: "Unrevert session",
        description: "Undo a revert, restoring the session changes.",
        operationId: "session.unrevert",
        responses: {
          200: {
            description: "Session unreverted",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
          ...errors(404),
        },
      }),
      validator("param", z.any()),
      async (c) => {
        const { sessionID } = c.req.valid("param") as any
        const result = await SessionRevert.unrevert({ sessionID })
        return c.json(result)
      },
    )
    .get(
      "/:sessionID/children",
      describeRoute({
        summary: "List session children",
        description: "List child sessions (forks) of a session.",
        operationId: "session.children",
        responses: {
          200: {
            description: "Child sessions",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
          ...errors(404),
        },
      }),
      validator("param", z.any()),
      async (c) => {
        const { sessionID } = c.req.valid("param") as any
        const result = await Session.children(sessionID)
        return c.json(result)
      },
    )
    .get(
      "/:sessionID/todo",
      describeRoute({
        summary: "Get session todos",
        description: "Get todo items for a session.",
        operationId: "session.todo",
        responses: {
          200: {
            description: "Todo items",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
          ...errors(404),
        },
      }),
      validator("param", z.any()),
      async (c) => {
        const { sessionID } = c.req.valid("param") as any
        const result = await Todo.get(sessionID)
        return c.json(result)
      },
    )
    .post(
      "/:sessionID/clear",
      describeRoute({
        summary: "Clear session messages",
        description: "Delete all messages and parts for a session.",
        operationId: "session.clear",
        responses: {
          200: {
            description: "Session cleared",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
          ...errors(404),
        },
      }),
      validator("param", z.any()),
      async (c) => {
        const { sessionID } = c.req.valid("param") as any
        await Session.clear(sessionID)
        return c.json(true)
      },
    ),
)
