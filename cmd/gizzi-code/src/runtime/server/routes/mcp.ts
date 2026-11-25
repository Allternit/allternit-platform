import { Hono } from "hono"
import { describeRoute, validator, resolver } from "@/runtime/server/openapi"
import z from "zod/v4"
import { MCP } from "@/runtime/tools/mcp"
import { errors } from "@/runtime/server/error"
import { lazy } from "@/shared/util/lazy"
import { McpTaskStream } from "@/runtime/tools/mcp/task-stream"

export const McpRoutes = lazy(() =>
  new Hono()
    .post(
      "/elicitation/:requestID/reply",
      describeRoute({
        summary: "Answer an MCP elicitation",
        description:
          "Answer an `mcp.elicitation` stream part (form or URL ask from an MCP server or task). Body is an ElicitationResponse: { action: accept | decline | cancel, content? }.",
        operationId: "mcp.elicitation.reply",
        responses: {
          200: { description: "Answered", content: { "application/json": { schema: resolver(z.any()) } } },
          ...errors(400, 404),
        },
      }),
      validator("param", z.object({ requestID: z.string() })),
      validator(
        "json",
        z.object({
          action: z.enum(["accept", "decline", "cancel"]),
          content: z.record(z.string(), z.any()).optional(),
        }),
      ),
      async (c) => {
        const { requestID } = c.req.valid("param")
        const body = c.req.valid("json")
        const ok = McpTaskStream.reply(decodeURIComponent(requestID), {
          action: body.action,
          ...(body.action === "accept" && body.content ? { content: body.content } : {}),
        })
        return ok ? c.json(true) : c.json({ error: "unknown_or_answered", message: "No open elicitation with that id" }, 404)
      },
    )
    .post(
      "/tool/call",
      describeRoute({
        summary: "Call one MCP tool directly (no model turn)",
        description:
          "Runs one MCP tool in the given session id (any string, e.g. `bar:<job_id>`; no session needs to exist). The `mcp.task` part goes working -> completed on the bus, even for a tool that returns at once; elicitation uses the elicitation reply route. `meta` is passed to the call's `_meta` (e.g. `allternit/approvalId`).",
        operationId: "mcp.tool.call",
        responses: {
          200: { description: "Tool result", content: { "application/json": { schema: resolver(z.any()) } } },
          ...errors(400, 404),
        },
      }),
      validator(
        "json",
        z.object({
          sessionID: z.string().min(1),
          server: z.string().min(1),
          tool: z.string().min(1),
          args: z.record(z.string(), z.any()).optional(),
          meta: z.record(z.string(), z.any()).optional(),
        }),
      ),
      async (c) => {
        const body = c.req.valid("json")
        try {
          const out = await MCP.callDirect({ ...body, signal: c.req.raw.signal })
          if (!out) return c.json({ error: "unknown_tool", message: "No connected MCP tool with that server and name" }, 404)
          return c.json({ sessionID: body.sessionID, toolCallID: out.toolCallID, result: out.result })
        } catch (e) {
          return c.json({ error: "tool_call_failed", message: String((e as Error)?.message ?? e) }, 400)
        }
      },
    )
    .post(
      "/task/:taskId/cancel",
      describeRoute({
        summary: "Cancel an MCP task",
        description: "Stop a running MCP task (an `mcp.task` card's Stop button): sends tasks/cancel to the server.",
        operationId: "mcp.task.cancel",
        responses: {
          200: { description: "Cancelling", content: { "application/json": { schema: resolver(z.any()) } } },
          ...errors(404),
        },
      }),
      validator("param", z.object({ taskId: z.string() })),
      async (c) => {
        const { taskId } = c.req.valid("param")
        const ok = McpTaskStream.cancel(decodeURIComponent(taskId))
        return ok ? c.json(true) : c.json({ error: "unknown_task", message: "No running task with that id" }, 404)
      },
    )
    .get(
      "/",
      describeRoute({
        summary: "Get MCP status",
        description: "Retrieve the current status of all configured and connected MCP servers.",
        operationId: "mcp.status",
        responses: {
          200: {
            description: "MCP status",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
        },
      }),
      async (c) => {
        const status = await MCP.status()
        return c.json(status)
      },
    )
    .get(
      "/list",
      describeRoute({
        summary: "List MCP servers",
        description: "Retrieve a list of all configured and active MCP (Model Context Protocol) servers.",
        operationId: "mcp.list",
        responses: {
          200: {
            description: "List of MCP servers",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
        },
      }),
      async (c) => {
        const status = await MCP.status()
        return c.json(status)
      },
    )
    .get(
      "/tools/catalog",
      describeRoute({
        summary: "Inspect qualified MCP tools",
        description: "Return MCP tools with server provenance and normalization collision diagnostics.",
        operationId: "mcp.tools.catalog",
        responses: { 200: { description: "Qualified MCP tool catalog", content: { "application/json": { schema: resolver(z.any()) } } } },
      }),
      async (c) => {
        const catalog = await MCP.toolCatalog()
        return c.json({ descriptors: catalog.descriptors, collisions: catalog.collisions })
      },
    )
    .get(
      "/:name/auth",
      describeRoute({
        summary: "Get MCP OAuth state",
        operationId: "mcp.auth.status",
        responses: { 200: { description: "OAuth status", content: { "application/json": { schema: resolver(z.any()) } } } },
      }),
      validator("param", z.object({ name: z.string() })),
      async (c) => {
        const { name } = c.req.valid("param")
        return c.json({ supported: await MCP.supportsOAuth(name), status: await MCP.getAuthStatus(name) })
      },
    )
    .post(
      "/:name/auth/start",
      describeRoute({
        summary: "Start MCP OAuth",
        description: "Start PKCE/dynamic-registration OAuth and return a URL for browser or headless clients.",
        operationId: "mcp.auth.start",
        responses: { 200: { description: "OAuth flow started", content: { "application/json": { schema: resolver(z.any()) } } }, ...errors(400, 404) },
      }),
      validator("param", z.object({ name: z.string() })),
      async (c) => c.json(await MCP.startAuth(c.req.valid("param").name)),
    )
    .post(
      "/:name/auth/complete",
      describeRoute({
        summary: "Complete MCP OAuth",
        operationId: "mcp.auth.complete",
        responses: { 200: { description: "OAuth flow completed", content: { "application/json": { schema: resolver(z.any()) } } }, ...errors(400, 404) },
      }),
      validator("param", z.object({ name: z.string() })),
      validator("json", z.object({ code: z.string().min(1), iss: z.string().min(1).optional() })),
      async (c) =>
        c.json(await MCP.finishAuth(c.req.valid("param").name, c.req.valid("json").code, c.req.valid("json").iss)),
    )
    .delete(
      "/:name/auth",
      describeRoute({
        summary: "Remove MCP OAuth credentials",
        operationId: "mcp.auth.logout",
        responses: { 200: { description: "OAuth credentials removed", content: { "application/json": { schema: resolver(z.boolean()) } } } },
      }),
      validator("param", z.object({ name: z.string() })),
      async (c) => {
        await MCP.removeAuth(c.req.valid("param").name)
        return c.json(true)
      },
    )
    .post(
      "/:name/connect",
      describeRoute({ summary: "Connect MCP server", operationId: "mcp.connect", responses: { 200: { description: "Connected", content: { "application/json": { schema: resolver(z.any()) } } } } }),
      validator("param", z.object({ name: z.string() })),
      async (c) => {
        const name = c.req.valid("param").name
        await MCP.connect(name)
        return c.json((await MCP.status())[name])
      },
    )
    .post(
      "/:name/disconnect",
      describeRoute({ summary: "Disconnect MCP server", operationId: "mcp.disconnect", responses: { 200: { description: "Disconnected", content: { "application/json": { schema: resolver(z.any()) } } } } }),
      validator("param", z.object({ name: z.string() })),
      async (c) => {
        const name = c.req.valid("param").name
        await MCP.disconnect(name)
        return c.json((await MCP.status())[name])
      },
    )
    .post(
      "/add",
      describeRoute({
        summary: "Add MCP server",
        description: "Configure and add a new MCP server to the system.",
        operationId: "mcp.add",
        responses: {
          200: {
            description: "MCP server added successfully",
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
        const { name, ...config } = input
        const status = await MCP.add(name, config)
        return c.json(status)
      },
    )
    .delete(
      "/:name",
      describeRoute({
        summary: "Remove MCP server",
        description: "Remove a configured MCP server by its name.",
        operationId: "mcp.remove",
        responses: {
          200: {
            description: "MCP server removed successfully",
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
        const { name } = c.req.valid("param") as any
        await MCP.disconnect(name)
        return c.json(true)
      },
    ),
)
