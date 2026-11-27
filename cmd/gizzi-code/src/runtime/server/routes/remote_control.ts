import { Hono } from "hono"
import { streamSSE } from "hono/streaming"
import { describeRoute, validator, resolver } from "@/runtime/server/openapi"
import { errors } from "@/runtime/server/error"
import z from "zod/v4"
import { Session } from "@/runtime/session"
import { MessageV2 } from "@/runtime/session/message-v2"
import { SessionPrompt } from "@/runtime/session/prompt"
import { SessionStatus } from "@/runtime/session/status"
import { Bus } from "@/shared/bus"
import { Log } from "@/shared/util/log"
import { lazy } from "@/shared/util/lazy"
import { PermissionNext } from "@/runtime/tools/guard/permission/next"
import { Question } from "@/runtime/integrations/question"
import { TerminalLive } from "@/runtime/session/terminal-live"

const log = Log.create({ service: "remote-control" })

// NOTE: `Session`/`SessionStatus` live in a module that (transitively)
// imports these routes, so their `.Info` schemas may only be touched inside
// the lazy factory — module-scope access races the circular import and
// crashes with "undefined is not an object" at import time.
export const RemoteControlRoutes = lazy(() => {
  const RemoteSessionStatus = z.object({
    session: Session.Info,
    status: SessionStatus.Info,
    /** Remote Control: connected | reconnecting | offline | off, and since when (ms, or null). */
    rc: TerminalLive.Rc,
  })

  return (
  new Hono()
    .get(
      "/sessions",
      describeRoute({
        summary: "List remote-controllable sessions",
        description: "List non-archived sessions with their current busy/idle status and Remote Control state.",
        operationId: "remoteControl.sessions.list",
        responses: {
          200: {
            description: "List of sessions and statuses",
            content: { "application/json": { schema: resolver(z.array(RemoteSessionStatus)) } },
          },
        },
      }),
      async (c) => {
        const sessions = Array.from(Session.list()).filter((s) => s.time.archived === undefined)
        const result = sessions.map((session) => ({
          session,
          status: SessionStatus.get(session.id),
          rc: TerminalLive.rc(session),
        }))
        return c.json(result)
      },
    )
    .get(
      "/sessions/:sessionID",
      describeRoute({
        summary: "Get a session for remote control",
        description: "Return session metadata and its full message history.",
        operationId: "remoteControl.session.get",
        responses: {
          200: {
            description: "Session details and messages",
            content: { "application/json": { schema: resolver(z.any()) } },
          },
          ...errors(404),
        },
      }),
      validator("param", z.object({ sessionID: z.string() })),
      async (c) => {
        const { sessionID } = c.req.valid("param")
        const session = await Session.get(sessionID)
        const messages = await Session.messages({ sessionID })
        return c.json({
          session,
          status: SessionStatus.get(sessionID),
          rc: TerminalLive.rc(session),
          messages,
        })
      },
    )
    .post(
      "/sessions/:sessionID/messages",
      describeRoute({
        summary: "Send a remote message",
        description: "Append a user message to a session and start the agent loop.",
        operationId: "remoteControl.session.message",
        responses: {
          200: {
            description: "The created user message and accepted status",
            content: { "application/json": { schema: resolver(z.any()) } },
          },
          ...errors(400, 404, 409),
        },
      }),
      validator("param", z.object({ sessionID: z.string() })),
      validator(
        "json",
        z.object({
          text: z.string().min(1),
          attachments: z
            .array(
              z.object({
                mime: z.string(),
                url: z.string(),
                filename: z.string().optional(),
              }),
            )
            .optional(),
        }),
      ),
      async (c) => {
        const { sessionID } = c.req.valid("param")
        const body = c.req.valid("json")
        await Session.get(sessionID)

        const parts: SessionPrompt.PromptInput["parts"] = [{ type: "text", text: body.text }]
        for (const attachment of body.attachments ?? []) {
          parts.push({
            type: "file",
            mime: attachment.mime,
            url: attachment.url,
            filename: attachment.filename,
          })
        }

        SessionPrompt.prompt({ sessionID, parts })
        return c.json({ accepted: true, sessionID })
      },
    )
    .post(
      "/sessions/:sessionID/abort",
      describeRoute({
        summary: "Abort a remote session",
        description: "Cancel the running agent loop for a session.",
        operationId: "remoteControl.session.abort",
        responses: {
          200: {
            description: "Abort signalled",
            content: { "application/json": { schema: resolver(z.boolean()) } },
          },
          ...errors(404),
        },
      }),
      validator("param", z.object({ sessionID: z.string() })),
      async (c) => {
        const { sessionID } = c.req.valid("param")
        await Session.get(sessionID)
        SessionPrompt.cancel(sessionID)
        return c.json(true)
      },
    )
    .get(
      "/sessions/:sessionID/events",
      describeRoute({
        summary: "Stream session events",
        description:
          "Server-sent events for a single session: metadata changes, new messages, part updates, and status changes.",
        operationId: "remoteControl.session.events",
        responses: {
          200: {
            description: "SSE stream",
            content: { "text/event-stream": { schema: resolver(z.any()) } },
          },
          ...errors(404),
        },
      }),
      validator("param", z.object({ sessionID: z.string() })),
      async (c) => {
        const { sessionID } = c.req.valid("param")
        await Session.get(sessionID)

        c.header("X-Accel-Buffering", "no")
        c.header("X-Content-Type-Options", "nosniff")

        return streamSSE(c, async (stream) => {
          const writeEvent = async (event: { type: string; properties: any }) => {
            await stream.writeSSE({ data: JSON.stringify(event) })
          }

          await writeEvent({
            type: "remote.connected",
            properties: { sessionID, status: SessionStatus.get(sessionID) },
          })

          const unsubs: (() => void)[] = []
          unsubs.push(
            Bus.subscribe(Session.Event.Updated, (event) => {
              if (event.properties.info.id === sessionID) return writeEvent(event)
            }),
            Bus.subscribe(MessageV2.Event.Updated, (event) => {
              if (event.properties.info.sessionID === sessionID) return writeEvent(event)
            }),
            Bus.subscribe(MessageV2.Event.PartUpdated, (event) => {
              if (event.properties.part.sessionID === sessionID) return writeEvent(event)
            }),
            Bus.subscribe(SessionStatus.Event.Status, (event) => {
              if (event.properties.sessionID === sessionID) return writeEvent(event)
            }),
            Bus.subscribe(PermissionNext.Event.Asked, (event) => {
              if (event.properties.sessionID === sessionID) return writeEvent(event)
            }),
            Bus.subscribe(Question.Event.Asked, (event) => {
              if (event.properties.sessionID === sessionID) return writeEvent(event)
            }),
          )

          const heartbeat = setInterval(() => {
            stream.writeSSE({
              data: JSON.stringify({ type: "remote.heartbeat", properties: { sessionID } }),
            })
          }, 10_000)

          await new Promise<void>((resolve) => {
            stream.onAbort(() => {
              clearInterval(heartbeat)
              for (const unsub of unsubs) unsub()
              resolve()
              log.info("remote session events disconnected", { sessionID })
            })
          })
        })
      },
    )
  )
})
