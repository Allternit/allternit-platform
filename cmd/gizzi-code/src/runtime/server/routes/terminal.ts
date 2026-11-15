import { Hono } from "hono"
import { streamSSE } from "hono/streaming"
import z from "zod/v4"
import { validator } from "@/runtime/server/openapi"
import { TerminalLive } from "@/runtime/session/terminal-live"
import { Session } from "@/runtime/session"
import { Question } from "@/runtime/integrations/question"

/**
 * Live terminal sessions (contract: surfaces/docs/dev/live-terminal-sessions.mdx).
 * Called by the Gizzi TUI on this machine; surfaces never call these — they
 * keep using the session, message, permission and event routes they already use.
 */
export const TerminalRoutes = () =>
  new Hono()
    .get("/sessions", (c) => c.json(TerminalLive.list()))
    .post(
      "/sessions",
      validator(
        "json",
        z.object({
          terminalID: z.string().min(1),
          cwd: z.string().min(1),
          title: z.string().optional(),
          transcriptPath: z.string().optional(),
          model: z.object({ providerID: z.string(), modelID: z.string() }).optional(),
        }),
      ),
      async (c) => {
        const session = await TerminalLive.adopt(c.req.valid("json"))
        return c.json({ sessionID: session.id, session })
      },
    )
    .post(
      "/sessions/:sessionID/messages",
      validator("json", z.object({ messages: TerminalLive.Message.array() })),
      async (c) => {
        const sessionID = c.req.param("sessionID")
        await Session.get(sessionID)
        await TerminalLive.sync(sessionID, c.req.valid("json").messages)
        return c.json(true)
      },
    )
    .post("/sessions/:sessionID/status", validator("json", z.object({ busy: z.boolean() })), async (c) => {
      TerminalLive.status(c.req.param("sessionID"), c.req.valid("json").busy)
      return c.json(true)
    })
    .post(
      "/sessions/:sessionID/permission",
      validator(
        "json",
        z.object({
          requestID: z.string().startsWith("per"),
          tool: z.string(),
          description: z.string(),
          input: z.record(z.string(), z.any()),
          callID: z.string().optional(),
        }),
      ),
      async (c) => {
        TerminalLive.ask({ sessionID: c.req.param("sessionID"), ...c.req.valid("json") })
        return c.json(true)
      },
    )
    .post("/permission/:requestID/settle", validator("json", z.object({ allow: z.boolean() })), async (c) => {
      await TerminalLive.settle(c.req.param("requestID"), c.req.valid("json").allow)
      return c.json(true)
    })
    .post(
      "/sessions/:sessionID/question",
      validator(
        "json",
        z.object({
          requestID: z.string().startsWith("que"),
          questions: Question.Info.array(),
          callID: z.string().optional(),
        }),
      ),
      async (c) => {
        TerminalLive.askQuestion({ sessionID: c.req.param("sessionID"), ...c.req.valid("json") })
        return c.json(true)
      },
    )
    .post(
      "/question/:requestID/settle",
      validator("json", z.object({ answers: z.array(z.array(z.string())).optional() })),
      async (c) => {
        await TerminalLive.settleQuestion(c.req.param("requestID"), c.req.valid("json").answers)
        return c.json(true)
      },
    )
    // The terminal's inbox: prompts, stops and permission answers from every surface.
    // Holding it open is what marks the session live; closing it hands the session back to the engine.
    .get("/sessions/:sessionID/inbox", async (c) => {
      const sessionID = c.req.param("sessionID")
      const session = await Session.get(sessionID)
      const host = c.req.query("host")
      const pid = Number(c.req.query("pid")) || undefined
      c.header("X-Accel-Buffering", "no")
      return streamSSE(c, async (stream) => {
        const detach = TerminalLive.attach({
          sessionID,
          directory: session.directory,
          host,
          pid,
          send: (event) => void stream.writeSSE({ data: JSON.stringify(event) }),
        })
        await stream.writeSSE({ data: JSON.stringify({ type: "attached", sessionID }) })
        const heartbeat = setInterval(
          () => void stream.writeSSE({ data: JSON.stringify({ type: "heartbeat" }) }),
          10_000,
        )
        await new Promise<void>((resolve) =>
          stream.onAbort(() => {
            clearInterval(heartbeat)
            detach()
            resolve()
          }),
        )
      })
    })
