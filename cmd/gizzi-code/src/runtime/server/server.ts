import { BusEvent } from "@/shared/bus/bus-event"
import { Bus } from "@/shared/bus"
import { Log } from "@/shared/util/log"
import { describeRoute, generateSpecs, validator, resolver, openAPIRouteHandler } from "@/runtime/server/openapi"
import { Hono } from "hono"
import { cors } from "hono/cors"
import { streamSSE } from "hono/streaming"
import { proxy } from "hono/proxy"
import z from "zod/v4"
import { Provider } from "@/runtime/providers/provider"
import { NamedError, NamedErrorBase } from "@allternit/gizzi-util/error.js"
import { LSP } from "@/runtime/integrations/lsp"
import { Format } from "@/shared/format"
import { Config } from "@/runtime/context/config/config"
import { ModelsDev } from "@/runtime/providers/adapters/models"
import { ProviderAuth } from "@/runtime/providers/adapters/auth"
import { TuiRoutes } from "@/runtime/server/routes/tui"
import { Instance } from "@/runtime/context/project/instance"
import { Vcs } from "@/runtime/context/project/vcs"
import { Agent } from "@/runtime/loop/agent"

import { Skill } from "@/runtime/skills/skill"
import { Auth } from "@/runtime/integrations/auth"
import { Flag } from "@/runtime/context/flag/flag"
import { Command } from "@/runtime/loop/command"
import { Global } from "@/runtime/context/global"
import { ProjectRoutes } from "@/runtime/server/routes/project"
import { SessionRoutes } from "@/runtime/server/routes/session"
import { NativeSessionRoutes } from "@/runtime/server/routes/native-session"
import { AutomationsRoutes } from "@/runtime/server/routes/automations"
import { PtyRoutes } from "@/runtime/server/routes/pty"
import { McpRoutes } from "@/runtime/server/routes/mcp"
import { FileRoutes } from "@/runtime/server/routes/file"
import { ConfigRoutes } from "@/runtime/server/routes/config"
import { ExperimentalRoutes } from "@/runtime/server/routes/experimental"
import { ProviderRoutes } from "@/runtime/server/routes/provider"
import { ModelPoolRoutes } from "@/runtime/server/routes/model-pool"
import { SidecarRoutes } from "@/runtime/server/routes/sidecar"
import { Pty } from "@/runtime/integrations/pty"
import { lazy } from "@/shared/util/lazy"
import { InstanceBootstrap } from "@/runtime/context/project/bootstrap"
import { NotFoundError } from "@/runtime/session/storage/db"
import type { ContentfulStatusCode } from "hono/utils/http-status"
import { websocket } from "hono/bun"
import { HTTPException } from "hono/http-exception"
import { errors } from "@/runtime/server/error"
import { QuestionRoutes } from "@/runtime/server/routes/question"
import { PaneBrowserRoutes } from "@/runtime/server/routes/pane-browser"
import { PaneArtifactRoutes } from "@/runtime/server/routes/pane-artifact"
import { PaneRenderRoutes } from "@/runtime/server/routes/pane-render"
import { CliBridgeRoutes } from "@/runtime/server/routes/cli-bridge"
import { PermissionRoutes } from "@/runtime/server/routes/permission"
import { GlobalRoutes } from "@/runtime/server/routes/global"
import { CritiqueRoutes } from "@/runtime/server/routes/critique"
import { DesignRoutes } from "@/runtime/server/routes/design"
import { AuthRoutes } from "@/runtime/server/routes/auth"
import { AgentRoutes } from "@/runtime/server/routes/agent"
import { CommandRoutes } from "@/runtime/server/routes/command"
import { CronRoutes } from "@/runtime/server/routes/cron"
import { CronService } from "@/runtime/automation/cron/service"
import { ArsContextaRoutes } from "@/runtime/server/routes/ars-contexta"
import { WebProxyRoutes } from "@/runtime/server/routes/web-proxy"
import { MDNS } from "@/runtime/server/mdns"
import { Tunnel } from "@/runtime/server/tunnel"
import { Mesh } from "@/runtime/server/mesh"
import { InstanceRegistration } from "@/runtime/server/instance-registration"
import { ClerkAuth } from "@/runtime/server/middleware/clerk-auth"
import { TerminalClerkAuthRoutes } from "@/runtime/server/routes/terminal-clerk-auth"
import { UserRoutes } from "@/runtime/server/routes/user"
import { InstanceRoutes } from "@/runtime/server/routes/instance"
import { EventRoutes } from "@/runtime/server/routes/event"
import { LspRoutes } from "@/runtime/server/routes/lsp"
import { FormatterRoutes } from "@/runtime/server/routes/formatter"
import { SkillRoutes } from "@/runtime/server/routes/skill"
import { VcsRoutes } from "@/runtime/server/routes/vcs"
import { PathRoutes } from "@/runtime/server/routes/path"
import { MemoryRoutes } from "@/runtime/server/routes/memory"
import { VaultRoutes } from "@/runtime/server/routes/vault"
import { BrainRoutes } from "@/runtime/server/routes/brain"
import { SandboxRoutes } from "@/runtime/server/routes/sandbox"
import { VmSessionRoutes } from "@/runtime/server/routes/vm-session"
import { WorkspaceRoutes } from "@/runtime/server/routes/workspace"
import { PluginRoutes } from "@/runtime/server/routes/plugin"
import { CoworkRoutes } from "@/runtime/server/routes/cowork"
import { AcpRoutes } from "@/runtime/server/routes/acp"
import { PeerRoutes } from "@/runtime/server/routes/peers"
import { OrchestratorRoutes } from "@/runtime/server/routes/orchestrator"
import { RuntimeHeartbeat } from "@/runtime/runtime-heartbeat"
import { RuntimeRoutes } from "@/runtime/server/routes/runtime"
import { RemoteControlRoutes } from "@/runtime/server/routes/remote_control"
import { AgentCompatRoutes } from "@/runtime/server/routes/agent-compat"
import { createHash, randomUUID } from "node:crypto"

// @ts-ignore This global is needed to prevent ai-sdk from logging warnings to stdout https://github.com/vercel/ai/blob/2dc67e0ef538307f21368db32d5a12345d98831b/packages/ai/src/logger/log-warnings.ts#L85
globalThis.AI_SDK_LOG_WARNINGS = false

export namespace Server {
  const log = Log.create({ service: "server" })

  let _url: URL | undefined
  let _corsWhitelist: string[] = []
  let _hostname: string | undefined
  let _tunnel = false
  const rateWindows = new Map<string, { startedAt: number; count: number }>()

  function isLoopback(hostname: string | undefined) {
    return hostname === "localhost" || hostname === "127.0.0.1" || hostname === "::1"
  }

  function requestID(value: string | undefined) {
    return value && /^[a-zA-Z0-9._:-]{1,128}$/.test(value) ? value : randomUUID()
  }

  function rateKey(value: string) {
    return createHash("sha256").update(value).digest("hex").slice(0, 16)
  }

  export function url(): URL {
    return _url ?? new URL("http://localhost:4096")
  }

  const app = new Hono()
  export const App: () => Hono = lazy(
    () =>
      app
        .onError((err, c) => {
          const id = c.get("requestID") ?? randomUUID()
          log.error("failed", {
            error: err,
            requestID: id,
          })
          if (err instanceof NamedErrorBase) {
            let status: ContentfulStatusCode
            if (err instanceof NotFoundError) status = 404
            else if (Provider.ModelNotFoundError.isInstance(err)) status = 400
            else if (err.name.startsWith("Worktree")) status = 400
            else status = 500
            return c.json(err.toObject(), { status })
          }
          if (err instanceof HTTPException) return err.getResponse()
          const message = err instanceof Error ? err.message : String(err)
          return c.json(({ name: "Unknown", message, data: { requestID: id } }), {
            status: 500,
          })
        })
        .use(async (c, next) => {
          const id = requestID(c.req.header("x-request-id"))
          c.set("requestID", id)
          c.header("X-Request-ID", id)
          c.header("X-Allternit-Protocol", "1")
          c.header("X-Content-Type-Options", "nosniff")
          c.header("Referrer-Policy", "no-referrer")
          c.header("Permissions-Policy", "camera=(), microphone=(), geolocation=()")

          // Loopback-only DNS-rebinding guard. Skipped in tunnel mode: the
          // tunnel legitimately forwards the public *.trycloudflare.com (or
          // named-tunnel) Host header, and tunnel mode always requires auth
          // (serve refuses --tunnel without a password or Clerk), so the
          // rebinding protection is moot there.
          if (_hostname && isLoopback(_hostname) && !_tunnel) {
            const host = (c.req.header("host") ?? "").toLowerCase().replace(/:\d+$/, "")
            const allowed = new Set(["localhost", "127.0.0.1", "[::1]", "gizzi.internal", "tauri.localhost"])
            if (host && !allowed.has(host)) {
              return c.json({ error: "invalid_host", requestID: id }, 421)
            }
          }

          const identity =
            c.req.header("cf-connecting-ip") ??
            c.req.header("x-real-ip") ??
            c.req.header("authorization") ??
            c.req.header("host") ??
            "local"
          const sensitive = c.req.path.startsWith("/auth") || c.req.path.includes("/prompt")
          const limit = sensitive ? 60 : 600
          const now = Date.now()
          const key = `${rateKey(identity)}:${sensitive ? "sensitive" : "general"}`
          const window = rateWindows.get(key)
          if (!window || now - window.startedAt >= 60_000) rateWindows.set(key, { startedAt: now, count: 1 })
          else if (++window.count > limit) {
            c.header("Retry-After", String(Math.max(1, Math.ceil((60_000 - (now - window.startedAt)) / 1000))))
            return c.json({ error: "rate_limited", requestID: id }, 429)
          }
          if (rateWindows.size > 2048) {
            for (const [entry, value] of rateWindows) if (now - value.startedAt >= 60_000) rateWindows.delete(entry)
          }
          await next()
        })
        // CORS must run before auth so that auth failures (401) still carry
        // Access-Control-Allow-Origin headers; otherwise browsers report the
        // rejection as a CORS error instead of the real status.
        .use(
          cors({
            origin(input) {
              if (!input) return

              // Development escape hatch: reflect any origin. Gated behind an
              // explicit env flag (with a startup warning in listen()) so it
              // can never be on by default in production.
              if (Flag.GIZZI_DEV_CORS) return input

              if (input.startsWith("http://localhost:")) return input
              if (input.startsWith("http://127.0.0.1:")) return input
              if (
                input === "tauri://localhost" ||
                input === "http://tauri.localhost" ||
                input === "https://tauri.localhost"
              )
                return input

              // First-party HTTPS origins only
              if (/^https:\/\/([a-z0-9-]+\.)*allternit\.com$/.test(input)) {
                return input
              }
              if (/^https:\/\/([a-z0-9-]+\.)*gizziio\.com$/.test(input)) {
                return input
              }
              if (_corsWhitelist.includes(input)) {
                return input
              }

              return
            },
          }),
        )
        .use(ClerkAuth.middleware())
        .use(async (c, next) => {
          const skipLogging = c.req.path === "/log"
          if (!skipLogging) {
            log.info("request", {
              method: c.req.method,
              path: c.req.path,
              requestID: c.get("requestID"),
            })
          }
          const timer = log.time("request", {
            method: c.req.method,
            path: c.req.path,
          })
          await next()
          if (!skipLogging) {
            timer.stop()
          }
        })
        .get("/health", (c) => c.json({ status: "ok", service: "gizzi-code" }))
        .get("/golden-stream", (c) => {
          c.header("Content-Type", "application/x-ndjson; charset=utf-8")
          c.header("Cache-Control", "no-cache, no-transform")
          c.header("X-Accel-Buffering", "no")
          c.header("Connection", "keep-alive")

          return streamSSE(c, async (s) => {
            const write = async (obj: any) => {
              const data = JSON.stringify(obj) + "\n"
              await s.write(data)
              if ((s as any).flush) await (s as any).flush()
            }

            await write({ type: "ping", t: Date.now() })
            const testMsgId = "golden-msg-" + Date.now()
            await write({ type: "message_start", messageId: testMsgId })
            
            await write({ type: "content_block_start", messageId: testMsgId, content_block: { id: "p1", type: "thinking", thinking: "I am thinking" } })
            for (let i = 0; i < 10; i++) {
              await new Promise(r => setTimeout(r, 100))
              await write({ type: "content_block_delta", messageId: testMsgId, partId: "p1", delta: { type: "thinking_delta", thinking: "." } })
            }

            await write({ type: "content_block_start", messageId: testMsgId, content_block: { id: "p2", type: "text", text: "Hello " } })
            const words = ["this ", "is ", "a ", "play-by-play ", "streaming ", "test."]
            for (const word of words) {
              await new Promise(r => setTimeout(r, 200))
              await write({ type: "content_block_delta", messageId: testMsgId, partId: "p2", delta: { type: "text_delta", text: word } })
            }

            await write({ type: "finish", messageId: testMsgId, status: "completed" })
          })
        })
        .route("/global", GlobalRoutes())
        .route("/critique", CritiqueRoutes())
        .route("/design", DesignRoutes())
        .put(
          "/auth/:providerID",
          describeRoute({
            summary: "Set auth credentials",
            description: "Set authentication credentials",
            operationId: "auth.set",
            responses: {
              200: {
                description: "Successfully set authentication credentials",
                content: {
                  "application/json": {
                    schema: resolver(z.boolean()),
                  },
                },
              },
              ...errors(400),
            },
          }),
          validator(
            "param",
            z.object({
              providerID: z.string(),
            }),
          ),
          validator("json", z.any()),
          async (c) => {
            const providerID = c.req.valid("param").providerID
            const info = c.req.valid("json")
            await Auth.set(providerID, info)
            return c.json(true)
          },
        )
        .delete(
          "/auth/:providerID",
          describeRoute({
            summary: "Remove auth credentials",
            description: "Remove authentication credentials",
            operationId: "auth.remove",
            responses: {
              200: {
                description: "Successfully removed authentication credentials",
                content: {
                  "application/json": {
                    schema: resolver(z.boolean()),
                  },
                },
              },
              ...errors(400),
            },
          }),
          validator(
            "param",
            z.object({
              providerID: z.string(),
            }),
          ),
          async (c) => {
            const providerID = c.req.valid("param").providerID
            await Auth.remove(providerID)
            return c.json(true)
          },
        )
        .route("/auth/terminal/clerk", TerminalClerkAuthRoutes())
        // Cron routes do not require a project context; mount them before the
        // global Instance.provide middleware so the scheduler can be reached
        // even when no project directory is valid.
        .route("/cron", CronRoutes())
        .use(async (c, next) => {
          if (c.req.path === "/log") return next()
          // Cron routes are global and should not require a project directory.
          if (c.req.path.startsWith("/cron")) return next()
          const raw = c.req.query("directory") || c.req.header("x-gizzi-directory") || c.req.header("x-gizzi-directory") || process.cwd()
          const directory = (() => {
            try {
              return decodeURIComponent(raw)
            } catch {
              return raw
            }
          })()
          return Instance.provide({
            directory,
            init: InstanceBootstrap,
            async fn() {
              return next()
            },
          })
        })
        .get(
          "/doc",
          openAPIRouteHandler(app, {
            documentation: {
              info: {
                title: "gizzi",
                version: "0.0.3",
                description: "gizzi api",
              },
              openapi: "3.1.1",
            },
          }),
        )
        .get("/asyncapi", (c) => c.json(asyncapi()))
        .use(validator("query", z.any()))
        .route("/project", ProjectRoutes())
        .route("/pty", PtyRoutes())
        .route("/config", ConfigRoutes())
        .route("/experimental", ExperimentalRoutes())
        .route("/session", SessionRoutes())
        .route("/native-session", NativeSessionRoutes())
        .route("/automations", AutomationsRoutes())
        .route("/peers", PeerRoutes())
        .route("/permission", PermissionRoutes())
        .route("/question", QuestionRoutes())
        .route("/pane-browser", PaneBrowserRoutes())
        .route("/pane-artifact", PaneArtifactRoutes())
        .route("/pane-render", PaneRenderRoutes())
        .route("/cli-bridge", CliBridgeRoutes())
        .route("/provider", ProviderRoutes())
        .route("/model-pool", ModelPoolRoutes())
        .route("/sidecar", SidecarRoutes())
        .route("/", FileRoutes())
        .route("/mcp", McpRoutes())
        .route("/tui", TuiRoutes())
        .route("/agent", AgentRoutes())
        .route("/command", CommandRoutes())
        .route("/ars-contexta", ArsContextaRoutes())
        .route("/web-proxy", WebProxyRoutes())
        .route("/user", UserRoutes())
        .route("/instance", InstanceRoutes())
        .route("/path", PathRoutes())
        .route("/vcs", VcsRoutes())
        .route("/lsp", LspRoutes())
        .route("/formatter", FormatterRoutes())
        .route("/skill", SkillRoutes())
        .route("/memory", MemoryRoutes())
        .route("/vault", VaultRoutes())
        .route("/brain", BrainRoutes())
        .route("/sandbox", SandboxRoutes())
        .route("/vm-session", VmSessionRoutes())
        .route("/plugin", PluginRoutes())
        .route("/cowork", CoworkRoutes())
        .route("/acp", AcpRoutes())
        .route("/runtime", RuntimeRoutes())
        .post(
          "/log",
          describeRoute({
            summary: "Write log",
            description: "Write a log entry to the server logs with specified level and metadata.",
            operationId: "app.log",
            responses: {
              200: {
                description: "Log entry written successfully",
                content: {
                  "application/json": {
                    schema: resolver(z.boolean()),
                  },
                },
              },
              ...errors(400),
            },
          }),
          validator(
            "json",
            z.object({
              service: z.string(),
              level: z.enum(["debug", "info", "error", "warn"]),
              message: z.string(),
              extra: z
                .record(z.string(), z.any())
                .optional()
                ,
            }),
          ),
          async (c) => {
            const { service, level, message, extra } = c.req.valid("json")
            const logger = Log.create({ service })

            switch (level) {
              case "debug":
                logger.debug(message, extra)
                break
              case "info":
                logger.info(message, extra)
                break
              case "error":
                logger.error(message, extra)
                break
              case "warn":
                logger.warn(message, extra)
                break
            }

            return c.json(true)
          },
        )
        .route("/event", EventRoutes())
        // Allternit iOS app protocol facade (/api/v1/agent-sessions +
        // /api/agent-chat) — lets any gizzi instance serve as the app's
        // agent brain without allternit-api in the middle.
        .route("/api", AgentCompatRoutes())
        .route("/v1beta/remote-control", RemoteControlRoutes())
        // /v1/ — versioned API surface (same handlers, new path prefix)
        .route(
          "/v1",
          new Hono()
            .get("/asyncapi", (c) => c.json(asyncapi()))
            .route("/session", SessionRoutes())
            .route("/native-session", NativeSessionRoutes())
            .route("/automations", AutomationsRoutes())
            .route("/peers", PeerRoutes())
            .route("/orchestrator", OrchestratorRoutes())
            .route("/agent", AgentRoutes())
            .route("/command", CommandRoutes())
            .route("/provider", ProviderRoutes())
            .route("/model-pool", ModelPoolRoutes())
            .route("/sidecar", SidecarRoutes())
            .route("/config", ConfigRoutes())
            .route("/mcp", McpRoutes())
            .route("/cron", CronRoutes())
            .route("/permission", PermissionRoutes())
            .route("/question", QuestionRoutes())
            .route("/pane-browser", PaneBrowserRoutes())
            .route("/pane-artifact", PaneArtifactRoutes())
            .route("/pane-render", PaneRenderRoutes())
            .route("/file", FileRoutes())
            .route("/user", UserRoutes())
            .route("/pty", PtyRoutes())
            .route("/instance", InstanceRoutes())
            .route("/path", PathRoutes())
            .route("/vcs", VcsRoutes())
            .route("/lsp", LspRoutes())
            .route("/formatter", FormatterRoutes())
            .route("/skill", SkillRoutes())
            .route("/memory", MemoryRoutes())
            .route("/vault", VaultRoutes())
            .route("/brain", BrainRoutes())
            .route("/sandbox", SandboxRoutes())
            .route("/vm-session", VmSessionRoutes())
            .route("/plugin", PluginRoutes())
            .route("/event", EventRoutes())
            .route("/", CoworkRoutes())
            .route("/ars-contexta", ArsContextaRoutes())
            .route("/auth", AuthRoutes())
            .route("/auth/terminal/clerk", TerminalClerkAuthRoutes())
            .route("/global", GlobalRoutes())
        .route("/critique", CritiqueRoutes())
        .route("/design", DesignRoutes())
            .route("/project", ProjectRoutes())
            .route("/experimental", ExperimentalRoutes())
            .route("/tui", TuiRoutes())
            .route("/acp", AcpRoutes())
            .route("/runtime", RuntimeRoutes())
            .route("/remote-control", RemoteControlRoutes())
            .route("/workspace", WorkspaceRoutes()) as unknown as Hono,
        )
        .all("/*", async (c) => {
          const cloudProxyEnabled =
            process.env.GIZZI_ENABLE_CLOUD_PROXY === "true" ||
            process.env.GIZZI_ENABLE_CLOUD_PROXY === "1"

          if (!cloudProxyEnabled) {
            log.warn(
              `No local route for ${c.req.method} ${c.req.path}; cloud proxy is disabled. Set GIZZI_ENABLE_CLOUD_PROXY=true to forward unknown routes to api.allternit.com.`
            )
            return c.json(
              {
                error: "not_found",
                message: `No local handler for ${c.req.method} ${c.req.path}. Set GIZZI_ENABLE_CLOUD_PROXY=true to forward unknown routes to api.allternit.com.`,
              },
              404
            )
          }

          const path = c.req.path
          const response = await proxy(`https://api.allternit.com${path}`, {
            ...c.req,
            headers: {
              ...c.req.raw.headers,
              host: "api.allternit.com",
            },
          })
          response.headers.set(
            "Content-Security-Policy",
            "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: https:; font-src 'self' data:; media-src 'self' data:; connect-src 'self' data:",
          )
          return response
        }) as unknown as Hono,
  )

  export async function openapi() {
    // Cast to break excessive type recursion from long route chains
    const result = await generateSpecs(App() as Hono, {
      documentation: {
        info: {
          title: "gizzi",
          version: "1.0.0",
          description: "gizzi api",
        },
        openapi: "3.1.1",
      },
    })
    return result
  }

  export function asyncapi() {
    const base = url().toString().replace(/\/$/, "")
    return {
      asyncapi: "3.0.0",
      info: {
        title: "Allternit runtime events",
        version: "1.0.0",
        description: "Instance-scoped, resumable runtime event contract. REST replay cursors recover events missed during reconnect.",
      },
      servers: {
        local: { host: new URL(base).host, protocol: new URL(base).protocol.replace(":", "") },
      },
      channels: {
        events: {
          address: "/v1/event",
          messages: {
            runtimeEvent: { $ref: "#/components/messages/runtimeEvent" },
          },
          description: "Server-sent runtime event stream with heartbeat messages.",
        },
        sessionReplay: {
          address: "/v1/session/{sessionID}/replay",
          parameters: { sessionID: { description: "Durable session identifier" } },
          messages: {
            replayPage: { $ref: "#/components/messages/replayPage" },
          },
          description: "Cursor-bounded recovery channel used after an event-stream reconnect.",
        },
      },
      operations: {
        receiveRuntimeEvents: { action: "receive", channel: { $ref: "#/channels/events" } },
        receiveSessionReplay: { action: "receive", channel: { $ref: "#/channels/sessionReplay" } },
      },
      components: {
        messages: {
          runtimeEvent: {
            payload: {
              type: "object",
              required: ["type", "properties"],
              properties: { type: { type: "string" }, properties: { type: "object" } },
            },
          },
          replayPage: {
            payload: {
              type: "object",
              required: ["sessionID", "head", "cursor", "hasMore", "entries"],
              properties: {
                sessionID: { type: "string" }, head: { type: "integer" }, cursor: { type: "integer" },
                hasMore: { type: "boolean" }, entries: { type: "array", items: { type: "object" } },
              },
            },
          },
        },
      },
    }
  }

  export function listen(opts: {
    port: number
    hostname: string
    mdns?: boolean
    mdnsDomain?: string
    cors?: string[]
    tunnel?: boolean
    tunnelToken?: string
    tunnelHostname?: string
    mesh?: boolean
    meshAuthKey?: string
    meshControlUrl?: string
    onListen?: (server: Bun.Server<unknown>) => void
  }) {
    _corsWhitelist = opts.cors ?? []
    _hostname = opts.hostname
    _tunnel = opts.tunnel ?? false

    if (Flag.GIZZI_DEV_CORS) {
      process.stderr.write(
        "WARNING: GIZZI_DEV_CORS is set — Access-Control-Allow-Origin reflects ANY origin. Development only; never enable in production.\n",
      )
    }

    if (opts.tunnel && !Tunnel.available()) {
      throw new Error(
        "--tunnel requires cloudflared, which is not installed. Install it (`brew install cloudflared`, or see https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/) or set GIZZI_CLOUDFLARED_BIN to its path.",
      )
    }

    const args = {
      hostname: opts.hostname,
      idleTimeout: 0,
      fetch: App().fetch,
      websocket: websocket,
      // Forward onListen: worker.ts's `server` rpc handler awaits a promise
      // resolved by this callback — previously the option was typed away and
      // the await hung forever.
      ...(opts.onListen ? { onListen: opts.onListen } : {}),
    } as const
    const tryServe = (port: number) => {
      try {
        return Bun.serve({ ...args, port })
      } catch {
        return undefined
      }
    }
    const server = opts.port === 0 ? (tryServe(4096) ?? tryServe(0)) : tryServe(opts.port)
    if (!server) throw new Error(`Failed to start server on port ${opts.port}`)

    _url = server.url

    // Eagerly start the mux daemon so platform /terminal routes (served by
    // allternit-api) find the socket ready instead of failing on first use.
    void Pty.warmup()

    // Initialize local cron scheduler so /cron routes are functional.
    // The scheduler lifetime is bound to the gizzi server process (Kimi-style local scheduling).
    try {
      CronService.initialize({
        dbPath: `${Global.Path.data}/gizzi-cron.db`,
        checkIntervalMs: 60_000,
        maxConcurrentJobs: 10,
        defaultTimeoutSeconds: 300,
      })
      CronService.start()
      log.info("cron service initialized")
    } catch (err) {
      log.error("failed to initialize cron service", { error: err })
    }

    // Start runtime heartbeat to keep local/remote runtime status fresh.
    try {
      RuntimeHeartbeat.start()
      log.info("runtime heartbeat started")
    } catch (err) {
      log.error("failed to start runtime heartbeat", { error: err })
    }

    const shouldPublishMDNS =
      opts.mdns &&
      server.port &&
      opts.hostname !== "127.0.0.1" &&
      opts.hostname !== "localhost" &&
      opts.hostname !== "::1"
    if (shouldPublishMDNS) {
      MDNS.publish(server.port!, opts.mdnsDomain)
    } else if (opts.mdns) {
      log.warn("mDNS enabled but hostname is loopback; skipping mDNS publish")
    }

    if (opts.tunnel) {
      const tunnelOpts: Tunnel.Options = opts.tunnelToken
        ? { mode: "named", token: opts.tunnelToken, hostname: opts.tunnelHostname }
        : { mode: "quick" }
      Tunnel.start(server.port!, tunnelOpts)
        .then((url) => {
          if (url) {
            process.stderr.write(`gizzi tunnel ready at ${url}\n`)
            // Best-effort self-registration with the platform instance registry;
            // fire-and-forget so startup never blocks on it.
            void InstanceRegistration.register({ url }).catch((err) => {
              log.warn("instance registration failed", { error: err })
            })
          } else {
            // Named tunnel without a configured hostname: the tunnel runs, but
            // there is no URL to log or register.
            process.stderr.write("gizzi named tunnel running (hostname not configured)\n")
            log.info("skipping instance registration: named tunnel hostname not configured")
          }
        })
        .catch((err) => {
          log.error("failed to start cloudflared tunnel", { error: err })
          process.stderr.write(`gizzi tunnel failed: ${err instanceof Error ? err.message : String(err)}\n`)
        })
    }

    if (opts.mesh) {
      if (isLoopback(opts.hostname)) {
        // A loopback-bound server is only reachable on the tailnet when the
        // mesh-node sidecar path is used (it forwards to 127.0.0.1 itself);
        // the attach/userspace-tailscaled paths need a real interface bind.
        log.warn("mesh enabled on a loopback server; the mesh URL is only routable via the mesh-node sidecar path — attach mode needs a non-loopback bind (e.g. --hostname 0.0.0.0)")
      }
      // Mesh is strictly additive: any join failure (missing binaries, expired
      // or single-use auth key, unreachable control server) is logged and the
      // server keeps running without mesh.
      Mesh.start(server.port!, { authKey: opts.meshAuthKey, controlUrl: opts.meshControlUrl })
        .then((url) => {
          if (!url) return
          process.stderr.write(`gizzi mesh ready at ${url}\n`)
          // Best-effort self-registration with the platform instance registry,
          // same contract as the tunnel URL ({url, name} only); clients tell
          // mesh URLs apart by the 100.64.0.0/10 prefix.
          void InstanceRegistration.register({ url }).catch((err) => {
            log.warn("instance registration failed", { error: err })
          })
        })
        .catch((err) => {
          log.error("failed to join mesh tailnet", { error: err })
          process.stderr.write(
            `gizzi mesh failed (continuing without mesh): ${err instanceof Error ? err.message : String(err)}\n`,
          )
        })
    }

    const originalStop = server.stop.bind(server)
    server.stop = async (closeActiveConnections?: boolean) => {
      if (opts.tunnel) {
        Tunnel.stop()
        InstanceRegistration.stop()
      }
      if (opts.mesh) {
        await Mesh.stop()
        InstanceRegistration.stop()
      }
      if (shouldPublishMDNS) MDNS.unpublish()
      try {
        CronService.close()
        log.info("cron service stopped")
      } catch (err) {
        log.error("failed to stop cron service", { error: err })
      }
      return originalStop(closeActiveConnections)
    }

    return server
  }
}
