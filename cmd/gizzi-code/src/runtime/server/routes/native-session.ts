import { Hono } from "hono"
import { describeRoute, validator, resolver } from "@/runtime/server/openapi"
import z from "zod/v4"
import { lazy } from "@/shared/util/lazy"
import { errors } from "@/runtime/server/error"
import { NativeSource } from "@/runtime/session/native-source"
import { Session } from "@/runtime/session"
import { runTeleport } from "@/runtime/teleport/run"

const Harness = z.string().min(1)

export const NativeSessionRoutes = lazy(() =>
  new Hono()
    .get(
      "/harnesses",
      describeRoute({
        summary: "List native CLI harness adapters",
        operationId: "nativeSession.harnesses",
        responses: { 200: { description: "Harness registry", content: { "application/json": { schema: resolver(z.any()) } } } },
      }),
      async (c) => c.json({ harnesses: NativeSource.listHarnesses() }),
    )
    .get(
      "/list",
      describeRoute({
        summary: "List native CLI sessions (read-only catalog)",
        operationId: "nativeSession.list",
        responses: { 200: { description: "Native sessions", content: { "application/json": { schema: resolver(z.any()) } } } },
      }),
      validator("query", z.object({ cwd: z.string().optional(), harness: z.string().optional() })),
      async (c) => {
        const query = c.req.valid("query")
        const harnesses = query.harness ? [query.harness as never] : undefined
        return c.json({ sessions: NativeSource.list({ cwd: query.cwd, harnesses }) })
      },
    )
    .get(
      "/show/:harness/:id",
      describeRoute({
        summary: "Show a native CLI session as inert portable events",
        operationId: "nativeSession.show",
        responses: { 200: { description: "Native transcript", content: { "application/json": { schema: resolver(z.any()) } } }, ...errors(404) },
      }),
      validator("param", z.object({ harness: Harness, id: z.string() })),
      validator("query", z.object({ cwd: z.string().optional() })),
      async (c) => {
        const { harness, id } = c.req.valid("param")
        const query = c.req.valid("query")
        const shown = NativeSource.show(harness as never, id, { cwd: query.cwd })
        if (!shown.session.installed && shown.warnings.some((w) => w.code === "not_found")) {
          return c.json({ error: "Native session not found" }, 404)
        }
        return c.json(shown)
      },
    )
    .post(
      "/teleport",
      describeRoute({
        summary: "Move a live Claude or Codex session between this Mac and a connected cloud computer",
        operationId: "nativeSession.teleport",
        responses: { 200: { description: "Where the session resumes", content: { "application/json": { schema: resolver(z.any()) } } }, ...errors(400) },
      }),
      validator(
        "json",
        z.object({
          sessionId: z.string().min(1),
          to: z.enum(["cloud", "local"]),
          harness: z.enum(["claude", "codex"]).default("claude"),
          targetId: z.string().optional(),
          model: z.string().optional(),
          effort: z.string().optional(),
          base: z.string().optional(),
        }),
      ),
      async (c) => {
        const body = c.req.valid("json")
        try {
          const result = await runTeleport({ session: body.sessionId, to: body.to, harness: body.harness, targetId: body.targetId, model: body.model, effort: body.effort, base: body.base })
          return c.json(result)
        } catch (e) {
          return c.json({ error: e instanceof Error ? e.message : String(e) }, 400)
        }
      },
    )
    .post(
      "/pickup",
      describeRoute({
        summary: "Snapshot a native CLI session into a new Gizzi session with source_ref",
        operationId: "nativeSession.pickup",
        responses: { 200: { description: "Created Gizzi session", content: { "application/json": { schema: resolver(z.any()) } } }, ...errors(400, 404) },
      }),
      validator(
        "json",
        z.object({
          harness: Harness,
          sessionId: z.string().min(1),
          surface: z.enum(["chat", "cowork", "code", "browser", "design"]).optional(),
          cwd: z.string().optional(),
        }),
      ),
      async (c) => {
        const body = c.req.valid("json")
        try {
          const result = await NativeSource.pickup({
            harness: body.harness as never,
            sessionId: body.sessionId,
            surface: body.surface,
            cwd: body.cwd,
          })
          return c.json(result)
        } catch (error) {
          return c.json({ error: error instanceof Error ? error.message : String(error) }, 404)
        }
      },
    )
    .post(
      "/:sessionID/fetch",
      describeRoute({
        summary: "Fetch native origin delta into session_source_event (does not rewrite Allternit turns)",
        operationId: "nativeSession.fetch",
        responses: { 200: { description: "Origin delta", content: { "application/json": { schema: resolver(z.any()) } } }, ...errors(400, 404) },
      }),
      validator("param", z.object({ sessionID: z.string() })),
      async (c) => {
        const { sessionID } = c.req.valid("param")
        await Session.get(sessionID)
        try {
          return c.json(await NativeSource.fetch(sessionID))
        } catch (error) {
          return c.json({ error: error instanceof Error ? error.message : String(error) }, 400)
        }
      },
    )
    .post(
      "/:sessionID/export",
      describeRoute({
        summary: "Export the Allternit session to a NEW native CLI session (never overwrites origin)",
        operationId: "nativeSession.export",
        responses: { 200: { description: "Exported native session", content: { "application/json": { schema: resolver(z.any()) } } }, ...errors(400, 404) },
      }),
      validator("param", z.object({ sessionID: z.string() })),
      validator("json", z.object({ harness: Harness.optional() })),
      async (c) => {
        const { sessionID } = c.req.valid("param")
        await Session.get(sessionID)
        try {
          const body = c.req.valid("json")
          return c.json(await NativeSource.export(sessionID, body.harness as never))
        } catch (error) {
          return c.json({ error: error instanceof Error ? error.message : String(error) }, 400)
        }
      },
    )
    .get(
      "/:sessionID/origin",
      describeRoute({
        summary: "Read fetched native origin timeline for a Gizzi session",
        operationId: "nativeSession.origin",
        responses: { 200: { description: "Origin events", content: { "application/json": { schema: resolver(z.any()) } } }, ...errors(404) },
      }),
      validator("param", z.object({ sessionID: z.string() })),
      async (c) => {
        const { sessionID } = c.req.valid("param")
        await Session.get(sessionID)
        return c.json({ events: NativeSource.origin(sessionID) })
      },
    ),
)
