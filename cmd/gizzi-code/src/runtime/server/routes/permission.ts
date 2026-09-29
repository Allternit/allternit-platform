import { Hono } from "hono"
import { describeRoute, validator, resolver } from "@/runtime/server/openapi"
import z from "zod/v4"
import { PermissionNext } from "@/runtime/tools/guard/permission/next"
import { errors } from "@/runtime/server/error"
import { lazy } from "@/shared/util/lazy"

export const PermissionRoutes = lazy(() =>
  new Hono()
    .get(
      "/mode/:sessionID",
      describeRoute({
        summary: "Get session permission mode",
        description: "Read the durable permission mode for a session.",
        operationId: "permission.mode.get",
        responses: { 200: { description: "Permission mode", content: { "application/json": { schema: resolver(z.any()) } } } },
      }),
      validator("param", z.object({ sessionID: z.string() })),
      async (c) => {
        const { sessionID } = c.req.valid("param")
        return c.json({ sessionID, mode: await PermissionNext.getMode(sessionID) })
      },
    )
    .put(
      "/mode/:sessionID",
      describeRoute({
        summary: "Set session permission mode",
        description: "Persist manual, plan, unattended auto, or reviewed yolo behavior for a session.",
        operationId: "permission.mode.set",
        responses: { 200: { description: "Permission mode updated", content: { "application/json": { schema: resolver(z.any()) } } }, ...errors(400) },
      }),
      validator("param", z.object({ sessionID: z.string() })),
      validator("json", z.object({ mode: PermissionNext.Mode })),
      async (c) => {
        const { sessionID } = c.req.valid("param")
        const { mode } = c.req.valid("json")
        await PermissionNext.setMode(sessionID, mode)
        return c.json({ sessionID, mode })
      },
    )
    .post(
      "/:requestID/reply",
      describeRoute({
        summary: "Respond to permission request",
        description: "Approve or deny a permission request from the AI assistant.",
        operationId: "permission.reply",
        responses: {
          200: {
            description: "Permission processed successfully",
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
        const params = c.req.valid("param") as any
        const json = c.req.valid("json") as any
        await PermissionNext.reply({
          requestID: params.requestID,
          reply: json.reply,
          message: json.message,
          // Subscription asks: the human action allternit-api minted when the
          // person approved, and their answer to a provider question.
          ...(typeof json.humanAction === "string" ? { humanAction: json.humanAction } : {}),
          ...(typeof json.answer === "string" ? { answer: json.answer } : {}),
        })
        return c.json(true)
      },
    )
    .get(
      "/",
      describeRoute({
        summary: "List pending permissions",
        description: "Get all pending permission requests across all sessions.",
        operationId: "permission.list",
        responses: {
          200: {
            description: "List of pending permissions",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
        },
      }),
      async (c) => {
        const permissions = await PermissionNext.list()
        return c.json(permissions)
      },
    ),
)
