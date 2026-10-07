import { dynamicTool, type Tool, jsonSchema, type JSONSchema7 } from "ai"
import {
  Client,
  StreamableHTTPClientTransport,
  SSEClientTransport,
  UnauthorizedError,
  type ClientOptions,
  type Tool as MCPToolDef,
} from "@modelcontextprotocol/client"
import { StdioClientTransport } from "@modelcontextprotocol/client/stdio"
import { createGuardedFetch } from "@/shared/utils/hooks/ssrfGuard"

/**
 * Remote MCP servers are user-configured, so localhost is allowed for local
 * dev; cloud metadata, link-local and private ranges are refused, including
 * after redirects.
 */
const mcpGuardedFetch = createGuardedFetch({ allowLoopback: true }) as unknown as typeof fetch
import { Config } from "@/runtime/context/config/config"
import { Log } from "@/shared/util/log"
import { NamedError } from "@allternit/gizzi-util/error.js"
import z from "zod/v4"
import { Instance } from "@/runtime/context/project/instance"
import { Installation } from "@/shared/installation"
import { withTimeout } from "@/shared/util/timeout"
import { McpOAuthProvider } from "@/runtime/tools/mcp/oauth-provider"
import { McpOAuthCallback } from "@/runtime/tools/mcp/oauth-callback"
import { McpAuth } from "@/runtime/tools/mcp/auth"
import { BusEvent } from "@/shared/bus/bus-event"
import { Bus } from "@/shared/bus"
import { TuiEvent } from "@/cli/ui/ink-app/event"
import open from "open"
import { withBundledMcpServers } from "@/runtime/tools/mcp/bundled"
import { createHash } from "crypto"
import { buildMcpToolName } from "@/runtime/services/mcp/mcpStringUtils"
import { RuntimeTelemetry } from "@/runtime/telemetry"
import {
  MCP_APPROVED_META,
  MCP_APPS_CLIENT_CAPABILITIES,
  MCP_CONNECTOR_META_KEY,
  isVisibleToModel,
  mcpAppResourceUri,
} from "@/runtime/tools/mcp/apps"
import { McpUserProxy } from "@/runtime/tools/mcp/user-proxy"
import { getStreamContext } from "@/runtime/session/stream-context"
import { McpEra } from "@/runtime/tools/mcp/era"

export namespace MCP {
  const log = Log.create({ service: "mcp" })
  const DEFAULT_TIMEOUT = 30_000

  export const Resource = z
    .object({
      name: z.string(),
      uri: z.string(),
      description: z.string().optional(),
      mimeType: z.string().optional(),
      client: z.string(),
    })
    
  export type Resource = z.infer<typeof Resource>

  export const ToolsChanged = BusEvent.define(
    "mcp.tools.changed",
    z.object({
      server: z.string(),
    }),
  )

  export const BrowserOpenFailed = BusEvent.define(
    "mcp.browser.open.failed",
    z.object({
      mcpName: z.string(),
      url: z.string(),
    }),
  )

  export const Failed = NamedError.create(
    "MCPFailed",
    z.object({
      name: z.string(),
    }),
  )

  type MCPClient = Client

  // TODO(types): Config's mcp record admits a bare `{ enabled: boolean }`
  // entry that withBundledMcpServers' parameter type doesn't list; runtime
  // passes entries through unchanged.
  function withBundled(config: Config.Info["mcp"]): ReturnType<typeof withBundledMcpServers> {
    return withBundledMcpServers(
      config as Parameters<typeof withBundledMcpServers>[0],
    )
  }

  export const Status = z
    .discriminatedUnion("status", [
      z
        .object({
          status: z.literal("connected"),
          /** `modern` = 2026-07-28 (stateless, server/discover); `legacy` = 2025-era initialize session. */
          era: z.enum(["modern", "legacy"]).optional(),
          protocolVersion: z.string().optional(),
        }),
      z
        .object({
          status: z.literal("disabled"),
        }),
      z
        .object({
          status: z.literal("failed"),
          error: z.string(),
        }),
      z
        .object({
          status: z.literal("needs_auth"),
        }),
      z
        .object({
          status: z.literal("needs_client_registration"),
          error: z.string(),
        }),
    ])
    
  export type Status = z.infer<typeof Status>

  /** A gizzi MCP client: MCP Apps capability + the connect's era negotiation. */
  export function newClient(versionNegotiation?: ClientOptions["versionNegotiation"]): MCPClient {
    return new Client(
      { name: "gizzi", version: Installation.VERSION },
      { capabilities: MCP_APPS_CLIENT_CAPABILITIES, ...(versionNegotiation ? { versionNegotiation } : {}) },
    )
  }

  function connectedStatus(client: MCPClient): Status {
    return {
      status: "connected",
      era: client.getProtocolEra(),
      protocolVersion: client.getNegotiatedProtocolVersion(),
    }
  }

  // Register notification handlers before connecting, so a modern-era connection's
  // listen stream and a legacy session both deliver `tools/list_changed`.
  function withNotificationHandlers(client: MCPClient, serverName: string): MCPClient {
    registerNotificationHandlers(client, serverName)
    return client
  }

  function registerNotificationHandlers(client: MCPClient, serverName: string) {
    client.setNotificationHandler("notifications/tools/list_changed", async () => {
      log.info("tools list changed notification received", { server: serverName })
      Bus.publish(ToolsChanged, { server: serverName })
    })
  }

  // Convert MCP tool definition to AI SDK Tool type
  /**
   * Allternit's own connectors MCP gets the calling session's id in `_meta`
   * (`allternit/session`): hosted-agent tools (knowledge search, channel post)
   * use it to know which agent is calling, never the model's input. Only sent
   * to that server; third-party servers never see session ids.
   */
  const SESSION_META = "allternit/session"
  const SESSION_META_SERVERS = new Set(["allternit-connectors"])

  async function convertMcpTool(mcpTool: MCPToolDef, client: MCPClient, timeout?: number, sendSession = false): Promise<Tool> {
    const inputSchema = mcpTool.inputSchema

    // Spread first, then override type to ensure it's always "object"
    const schema: JSONSchema7 = {
      ...(inputSchema as JSONSchema7),
      type: "object",
      properties: (inputSchema.properties ?? {}) as JSONSchema7["properties"],
      additionalProperties: false,
    }

    return dynamicTool({
      description: mcpTool.description ?? "",
      inputSchema: jsonSchema(schema),
      execute: async (args: unknown, options?: { toolCallId?: string }) => {
        // Proxy tools marked "requires confirmation" run only with the user's approval, recorded by
        // the permission ask that precedes this call; the proxy checks the flag.
        const approved = McpUserProxy.requiresConfirmation(mcpTool) && McpUserProxy.consumeApproval(options?.toolCallId)
        const sessionID = sendSession ? getStreamContext()?.sessionID : undefined
        const meta = {
          ...(approved ? { [MCP_APPROVED_META]: true } : {}),
          ...(sessionID ? { [SESSION_META]: sessionID } : {}),
        }
        return client.callTool(
          {
            name: mcpTool.name,
            arguments: (args || {}) as Record<string, unknown>,
            ...(Object.keys(meta).length ? { _meta: meta } : {}),
          },
          {
            resetTimeoutOnProgress: true,
            timeout,
          },
        )
      },
    })
  }

  // Store transports for OAuth servers to allow finishing auth
  type TransportWithAuth = StreamableHTTPClientTransport | SSEClientTransport
  const pendingOAuthTransports = new Map<string, TransportWithAuth>()
  const authToastShown = new Set<string>()

  // Prompt cache types
  type PromptInfo = Awaited<ReturnType<MCPClient["listPrompts"]>>["prompts"][number]

  type ResourceInfo = Awaited<ReturnType<MCPClient["listResources"]>>["resources"][number]
  type McpEntry = NonNullable<Config.Info["mcp"]>[string]
  const promptUnsupported = new Set<string>()
  const resourceUnsupported = new Set<string>()
  const promptUnsupportedLogged = new Set<string>()
  const resourceUnsupportedLogged = new Set<string>()

  function isMethodNotFoundError(error: unknown) {
    const message = error instanceof Error ? error.message : String(error)
    return message.includes("MCP error -32601") || message.includes("Method not found")
  }

  function isMcpConfigured(entry: McpEntry): entry is Config.Mcp {
    return typeof entry === "object" && entry !== null && "type" in entry
  }

  const state = Instance.state(
    async () => {
      const cfg = await Config.get()
      const config = withBundled(cfg.mcp)
      const clients: Record<string, MCPClient> = {}
      const status: Record<string, Status> = {}

      await Promise.all(
        Object.entries(config).map(async ([key, mcp]) => {
          if (!isMcpConfigured(mcp)) {
            log.error("Ignoring MCP config entry without type", { key })
            return
          }

          // If disabled by config, mark as disabled without trying to connect
          if (mcp.enabled === false) {
            status[key] = { status: "disabled" }
            return
          }

          const result = await create(key, mcp).catch(() => undefined)
          if (!result) return

          status[key] = result.status

          if (result.mcpClient) {
            clients[key] = result.mcpClient
          }
        }),
      )
      return {
        status,
        clients,
      }
    },
    async (state) => {
      await Promise.all(
        Object.values(state.clients).map((client) =>
          client.close().catch((error) => {
            log.error("Failed to close MCP client", {
              error,
            })
          }),
        ),
      )
      pendingOAuthTransports.clear()
    },
  )

  // Helper function to fetch prompts for a specific client
  async function fetchPromptsForClient(clientName: string, client: Client) {
    if (promptUnsupported.has(clientName)) return {}

    const prompts = await client.listPrompts().catch((e) => {
      if (isMethodNotFoundError(e)) {
        promptUnsupported.add(clientName)
        if (!promptUnsupportedLogged.has(clientName)) {
          promptUnsupportedLogged.add(clientName)
          log.info("MCP server does not support prompts, skipping prompt discovery", { clientName })
        }
        return { prompts: [] }
      }
      log.error("failed to get prompts", { clientName, error: e.message })
      return undefined
    })

    if (!prompts) {
      return
    }

    const commands: Record<string, PromptInfo & { client: string }> = {}

    for (const prompt of prompts.prompts) {
      const sanitizedClientName = clientName.replace(/[^a-zA-Z0-9_-]/g, "_")
      const sanitizedPromptName = prompt.name.replace(/[^a-zA-Z0-9_-]/g, "_")
      const key = sanitizedClientName + ":" + sanitizedPromptName

      commands[key] = { ...prompt, client: clientName }
    }
    return commands
  }

  async function fetchResourcesForClient(clientName: string, client: Client) {
    if (resourceUnsupported.has(clientName)) return {}

    const resources = await client.listResources().catch((e) => {
      if (isMethodNotFoundError(e)) {
        resourceUnsupported.add(clientName)
        if (!resourceUnsupportedLogged.has(clientName)) {
          resourceUnsupportedLogged.add(clientName)
          log.info("MCP server does not support resources, skipping resource discovery", { clientName })
        }
        return { resources: [] }
      }
      log.error("failed to get resources", { clientName, error: e.message })
      return undefined
    })

    if (!resources) {
      return
    }

    const commands: Record<string, ResourceInfo & { client: string }> = {}

    for (const resource of resources.resources) {
      const sanitizedClientName = clientName.replace(/[^a-zA-Z0-9_-]/g, "_")
      const sanitizedResourceName = resource.name.replace(/[^a-zA-Z0-9_-]/g, "_")
      const key = sanitizedClientName + ":" + sanitizedResourceName

      commands[key] = { ...resource, client: clientName }
    }
    return commands
  }

  export async function add(name: string, mcp: Config.Mcp) {
    const s = await state()
    const result = await create(name, mcp)
    if (!result) {
      const status = {
        status: "failed" as const,
        error: "unknown error",
      }
      s.status[name] = status
      return {
        status,
      }
    }
    if (!result.mcpClient) {
      s.status[name] = result.status
      return {
        status: s.status,
      }
    }
    // Close existing client if present to prevent memory leaks
    const existingClient = s.clients[name]
    if (existingClient) {
      await existingClient.close().catch((error) => {
        log.error("Failed to close existing MCP client", { name, error })
      })
    }
    s.clients[name] = result.mcpClient
    s.status[name] = result.status

    return {
      status: s.status,
    }
  }

  async function create(key: string, mcp: Config.Mcp) {
    if (mcp.enabled === false) {
      log.info("mcp server disabled", { key })
      return {
        mcpClient: undefined,
        status: { status: "disabled" as const },
      }
    }

    log.info("found", { key, type: mcp.type })
    let mcpClient: MCPClient | undefined
    let status: Status | undefined = undefined

    if (mcp.type === "remote") {
      // OAuth is enabled by default for remote servers unless explicitly disabled with oauth: false
      const oauthDisabled = mcp.oauth === false
      const oauthConfig = typeof mcp.oauth === "object" ? mcp.oauth : undefined
      let authProvider: McpOAuthProvider | undefined

      if (!oauthDisabled) {
        authProvider = new McpOAuthProvider(
          key,
          mcp.url,
          {
            clientId: oauthConfig?.clientId,
            clientSecret: oauthConfig?.clientSecret,
            scope: oauthConfig?.scope,
          },
          {
            onRedirect: async (url) => {
              log.info("oauth redirect requested", { key, url: url.toString() })
              // Store the URL - actual browser opening is handled by startAuth
            },
          },
        )
      }

      const requestInit = mcp.headers ? { headers: mcp.headers } : undefined
      // Streamable HTTP first (era-negotiated: 2026-07-28 or the 2025 handshake), then the
      // legacy HTTP+SSE transport, which predates 2026-07-28 and never probes.
      const transports: Array<{ name: string; mode: McpEra.Mode; transport: () => TransportWithAuth }> = [
        {
          name: "StreamableHTTP",
          mode: mcp.protocol ?? "auto",
          transport: () =>
            new StreamableHTTPClientTransport(new URL(mcp.url), { authProvider, requestInit, fetch: mcpGuardedFetch }),
        },
        {
          name: "SSE",
          mode: "legacy",
          transport: () => new SSEClientTransport(new URL(mcp.url), { authProvider, requestInit, fetch: mcpGuardedFetch }),
        },
      ]

      let lastError: Error | undefined
      const connectTimeout = mcp.timeout ?? DEFAULT_TIMEOUT
      for (const { name, mode, transport: makeTransport } of transports) {
        // `protocol: "modern"` pins 2026-07-28; the legacy SSE transport can't speak it.
        if (mcp.protocol === "modern" && mode === "legacy") continue
        let transport: TransportWithAuth | undefined
        try {
          const connected = await McpEra.connect({
            key,
            target: mcp.url,
            mode,
            timeoutMs: connectTimeout,
            client: (negotiation) => withNotificationHandlers(newClient(negotiation), key),
            transport: makeTransport,
            onTransport: (t) => (transport = t),
          })
          mcpClient = connected.client
          log.info("connected", { key, transport: name })
          status = connectedStatus(connected.client)
          authToastShown.delete(`${key}:needs_auth`)
          authToastShown.delete(`${key}:needs_client_registration`)
          break
        } catch (error) {
          lastError = error instanceof Error ? error : new Error(String(error))

          // Handle OAuth-specific errors
          if (error instanceof UnauthorizedError) {
            log.info("mcp server requires authentication", { key, transport: name })

            // Check if this is a "needs registration" error
            if (lastError.message.includes("registration") || lastError.message.includes("client_id")) {
              status = {
                status: "needs_client_registration" as const,
                error: "Server does not support dynamic client registration. Please provide clientId in config.",
              }
              const toastKey = `${key}:needs_client_registration`
              if (!authToastShown.has(toastKey)) {
                authToastShown.add(toastKey)
                Bus.publish(TuiEvent.ToastShow, {
                  title: "MCP Authentication Required",
                  message: `Server "${key}" requires a pre-registered client ID. Add clientId to your config.`,
                  variant: "warning",
                  duration: 8000,
                }).catch((e) => log.debug("failed to show toast", { error: e }))
              }
            } else {
              // Store transport for later finishAuth call
              if (transport) pendingOAuthTransports.set(key, transport)
              status = { status: "needs_auth" as const }
              const toastKey = `${key}:needs_auth`
              if (!authToastShown.has(toastKey)) {
                authToastShown.add(toastKey)
                Bus.publish(TuiEvent.ToastShow, {
                  title: "MCP Authentication Required",
                  message: `Server "${key}" requires authentication. Run: gizzi mcp auth ${key}`,
                  variant: "warning",
                  duration: 8000,
                }).catch((e) => log.debug("failed to show toast", { error: e }))
              }
            }
            break
          }

          log.debug("transport connection failed", {
            key,
            transport: name,
            url: mcp.url,
            error: lastError.message,
          })
          status = {
            status: "failed" as const,
            error: lastError.message,
          }
        }
      }
    }

    if (mcp.type === "local") {
      const [cmd, ...args] = mcp.command
      const cwd = Instance.directory
      const makeTransport = () => {
        const transport = new StdioClientTransport({
          stderr: "pipe",
          command: cmd,
          args,
          cwd,
          env: {
            ...process.env,
            ...(cmd === "gizzi" ? { BUN_BE_BUN: "1" } : {}),
            ...mcp.environment,
          } as Record<string, string>,
        })
        transport.stderr?.on("data", (chunk: Buffer) => {
          log.info(`mcp stderr: ${chunk.toString()}`, { key })
        })
        return transport
      }

      const connectTimeout = mcp.timeout ?? DEFAULT_TIMEOUT
      try {
        // On a first connect the SDK probes `server/discover` on a short-lived sibling
        // process; the verdict is cached, so later connects spawn the server once.
        const { client } = await McpEra.connect({
          key,
          target: JSON.stringify(mcp.command),
          mode: mcp.protocol ?? "auto",
          timeoutMs: connectTimeout,
          probeTimeoutMs: Math.min(connectTimeout, McpEra.STDIO_PROBE_TIMEOUT_MS),
          client: (negotiation) => withNotificationHandlers(newClient(negotiation), key),
          transport: makeTransport,
        })
        mcpClient = client
        status = connectedStatus(client)
      } catch (error) {
        log.error("local mcp startup failed", {
          key,
          command: mcp.command,
          cwd,
          error: error instanceof Error ? error.message : String(error),
        })
        status = {
          status: "failed" as const,
          error: error instanceof Error ? error.message : String(error),
        }
      }
    }

    if (!status) {
      status = {
        status: "failed" as const,
        error: "Unknown error",
      }
    }

    if (!mcpClient) {
      return {
        mcpClient: undefined,
        status,
      }
    }

    const result = await withTimeout(mcpClient.listTools(), mcp.timeout ?? DEFAULT_TIMEOUT).catch((err) => {
      log.error("failed to get tools from client", { key, error: err })
      return undefined
    })
    if (!result) {
      await mcpClient.close().catch((error) => {
        log.error("Failed to close MCP client", {
          error,
        })
      })
      status = {
        status: "failed",
        error: "Failed to get tools",
      }
      return {
        mcpClient: undefined,
        status: {
          status: "failed" as const,
          error: "Failed to get tools",
        },
      }
    }

    log.info("create() successfully created client", { key, toolCount: result.tools.length })
    return {
      mcpClient,
      status,
    }
  }

  export async function status() {
    const s = await state()
    const cfg = await Config.get()
    const config = withBundled(cfg.mcp)
    const result: Record<string, Status> = {}

    // Include all configured MCPs from config, not just connected ones
    for (const [key, mcp] of Object.entries(config)) {
      if (!isMcpConfigured(mcp)) continue
      result[key] = s.status[key] ?? { status: "disabled" }
    }

    return result
  }

  export async function clients() {
    return state().then((state) => state.clients)
  }

  export async function connect(name: string) {
    const cfg = await Config.get()
    const config = withBundled(cfg.mcp)
    const mcp = config[name]
    if (!mcp) {
      log.error("MCP config not found", { name })
      return
    }

    if (!isMcpConfigured(mcp)) {
      log.error("Ignoring MCP connect request for config without type", { name })
      return
    }

    const result = await create(name, { ...mcp, enabled: true })

    if (!result) {
      const s = await state()
      s.status[name] = {
        status: "failed",
        error: "Unknown error during connection",
      }
      return
    }

    const s = await state()
    s.status[name] = result.status
    if (result.mcpClient) {
      // Close existing client if present to prevent memory leaks
      const existingClient = s.clients[name]
      if (existingClient) {
        await existingClient.close().catch((error) => {
          log.error("Failed to close existing MCP client", { name, error })
        })
      }
      s.clients[name] = result.mcpClient
    }
  }

  export async function disconnect(name: string) {
    const s = await state()
    const client = s.clients[name]
    if (client) {
      await client.close().catch((error) => {
        log.error("Failed to close MCP client", { name, error })
      })
      delete s.clients[name]
    }
    s.status[name] = { status: "disabled" }
  }

  export interface ToolDescriptor {
    qualifiedName: string
    serverName: string
    originalName: string
    normalizedBase: string
    collision: boolean
    /** MCP Apps: the tool's `_meta.ui.resourceUri`, when it renders a ui:// resource. */
    uiResourceUri?: string
    /** The per-user proxy marked this tool: each model call needs the user's approval. */
    requiresConfirmation?: boolean
    /** Display name of the connector/app the tool belongs to (proxy tools). */
    appName?: string
    title?: string
  }

  /**
   * `extraClients`: turn-scoped clients that are not part of the instance's MCP state — the
   * per-user connector proxy. They are listed like any other server but never recorded in
   * `s.clients`/`s.status`, so one user's turn cannot leak into another's on a shared project.
   */
  export async function toolCatalog(extraClients: Record<string, MCPClient> = {}) {
    const result: Record<string, Tool> = {}
    const descriptors: Record<string, ToolDescriptor> = {}
    const collisions: Array<{ qualifiedName: string; existing: ToolDescriptor; incoming: Omit<ToolDescriptor, "qualifiedName" | "collision"> }> = []
    const s = await state()
    const cfg = await Config.get()
    const config = withBundled(cfg.mcp)
    const clientsSnapshot = await clients()
    const defaultTimeout = cfg.experimental?.mcp_timeout

    // Deterministic: servers connect concurrently, so `clients` insertion order varies run to
    // run. Merge in name order (configured servers before turn-scoped ones) and each server's
    // tools in name order, so collision suffixes and the model's tool list are stable.
    const byName = <T,>(a: [string, T], b: [string, T]) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0)
    const connectedClients = [
      ...Object.entries(clientsSnapshot)
        .filter(([clientName]) => s.status[clientName]?.status === "connected")
        .sort(byName),
      ...Object.entries(extraClients).sort(byName),
    ]

    const toolsResults = await Promise.all(
      connectedClients.map(async ([clientName, client]) => {
        const toolsResult = await client.listTools().catch((e) => {
          log.error("failed to get tools", { clientName, error: e.message })
          if (clientName in extraClients) return undefined
          const failedStatus = {
            status: "failed" as const,
            error: e instanceof Error ? e.message : String(e),
          }
          s.status[clientName] = failedStatus
          delete s.clients[clientName]
          return undefined
        })
        return { clientName, client, toolsResult }
      }),
    )

    for (const { clientName, client, toolsResult } of toolsResults) {
      if (!toolsResult) continue
      const mcpConfig = config[clientName]
      const entry = isMcpConfigured(mcpConfig) ? mcpConfig : undefined
      const timeout = entry?.timeout ?? defaultTimeout
      const sortedTools = [...toolsResult.tools].sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0))
      for (const mcpTool of sortedTools) {
        // MCP Apps: tools scoped to the rendered app (`_meta.ui.visibility` without "model")
        // are reachable only through the app bridge and are never offered to the model.
        if (!isVisibleToModel(mcpTool)) continue
        const identity = `${clientName}\0${mcpTool.name}`
        const normalizedBase = qualifiedToolName(clientName, mcpTool.name)
        const incoming = { serverName: clientName, originalName: mcpTool.name, normalizedBase }
        let qualifiedName = normalizedBase
        const existing = descriptors[qualifiedName]
        if (existing) {
          collisions.push({ qualifiedName, existing, incoming })
          qualifiedName = withToolSuffix(normalizedBase, createHash("sha256").update(identity).digest("hex").slice(0, 8))
          let counter = 2
          while (descriptors[qualifiedName]) qualifiedName = withToolSuffix(normalizedBase, String(counter++))
        }
        result[qualifiedName] = await convertMcpTool(mcpTool, client, timeout, SESSION_META_SERVERS.has(clientName))
        descriptors[qualifiedName] = {
          qualifiedName,
          ...incoming,
          collision: Boolean(existing),
          uiResourceUri: mcpAppResourceUri(mcpTool),
          requiresConfirmation: McpUserProxy.requiresConfirmation(mcpTool),
          appName: ((mcpTool._meta as Record<string, any> | undefined)?.[MCP_CONNECTOR_META_KEY] as { name?: string } | undefined)?.name,
          title: mcpTool.title ?? (mcpTool.annotations as { title?: string } | undefined)?.title,
        }
      }
    }
    if (collisions.length) {
      RuntimeTelemetry.track("mcp_tool_collision", {
        count: collisions.length,
        servers: connectedClients.length,
      })
    }
    return { tools: result, descriptors, collisions }
  }

  export async function tools() {
    return (await toolCatalog()).tools
  }

  function boundedToolName(name: string, identity: string) {
    if (name.length <= 64) return name
    const suffix = createHash("sha256").update(identity).digest("hex").slice(0, 8)
    return `${name.slice(0, 55)}_${suffix}`
  }

  export function qualifiedToolName(serverName: string, toolName: string) {
    return boundedToolName(buildMcpToolName(serverName, toolName), `${serverName}\0${toolName}`)
  }

  function withToolSuffix(base: string, suffix: string) {
    const tail = `__${suffix}`
    return `${base.slice(0, 64 - tail.length)}${tail}`
  }

  /** Record entries in key order, so merged prompts/resources don't depend on connect timing. */
  function sortedEntries<T>(record: Record<string, T>): Array<[string, T]> {
    return Object.entries(record).sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0))
  }

  export async function prompts() {
    const s = await state()
    const clientsSnapshot = await clients()

    const prompts = Object.fromEntries<PromptInfo & { client: string }>(
      (
        await Promise.all(
          sortedEntries(clientsSnapshot).map(async ([clientName, client]) => {
            if (s.status[clientName]?.status !== "connected") {
              return []
            }

            return Object.entries((await fetchPromptsForClient(clientName, client)) ?? {})
          }),
        )
      ).flat(),
    )

    return prompts
  }

  export async function resources() {
    const s = await state()
    const clientsSnapshot = await clients()

    const result = Object.fromEntries<ResourceInfo & { client: string }>(
      (
        await Promise.all(
          sortedEntries(clientsSnapshot).map(async ([clientName, client]) => {
            if (s.status[clientName]?.status !== "connected") {
              return []
            }

            return Object.entries((await fetchResourcesForClient(clientName, client)) ?? {})
          }),
        )
      ).flat(),
    )

    return result
  }

  export async function getPrompt(clientName: string, name: string, args?: Record<string, string>) {
    const clientsSnapshot = await clients()
    const client = clientsSnapshot[clientName]

    if (!client) {
      log.warn("client not found for prompt", {
        clientName,
      })
      return undefined
    }

    const result = await client
      .getPrompt({
        name: name,
        arguments: args,
      })
      .catch((e) => {
        log.error("failed to get prompt from MCP server", {
          clientName,
          promptName: name,
          error: e.message,
        })
        return undefined
      })

    return result
  }

  export async function readResource(clientName: string, resourceUri: string) {
    const clientsSnapshot = await clients()
    const client = clientsSnapshot[clientName]

    if (!client) {
      log.warn("client not found for prompt", {
        clientName: clientName,
      })
      return undefined
    }

    const result = await client
      .readResource({
        uri: resourceUri,
      })
      .catch((e) => {
        log.error("failed to get prompt from MCP server", {
          clientName: clientName,
          resourceUri: resourceUri,
          error: e.message,
        })
        return undefined
      })

    return result
  }

  /**
   * Start OAuth authentication flow for an MCP server.
   * Returns the authorization URL that should be opened in a browser.
   */
  export async function startAuth(mcpName: string): Promise<{ authorizationUrl: string }> {
    const cfg = await Config.get()
    const mcpConfig = cfg.mcp?.[mcpName]

    if (!mcpConfig) {
      throw new Error(`MCP server not found: ${mcpName}`)
    }

    if (!isMcpConfigured(mcpConfig)) {
      throw new Error(`MCP server ${mcpName} is disabled or missing configuration`)
    }

    if (mcpConfig.type !== "remote") {
      throw new Error(`MCP server ${mcpName} is not a remote server`)
    }

    if (mcpConfig.oauth === false) {
      throw new Error(`MCP server ${mcpName} has OAuth explicitly disabled`)
    }

    // Start the callback server
    await McpOAuthCallback.ensureRunning()

    // Generate and store a cryptographically secure state parameter BEFORE creating the provider
    // The SDK will call provider.state() to read this value
    const oauthState = Array.from(crypto.getRandomValues(new Uint8Array(32)))
      .map((b) => b.toString(16).padStart(2, "0"))
      .join("")
    await McpAuth.updateOAuthState(mcpName, oauthState)

    // Create a new auth provider for this flow
    // OAuth config is optional - if not provided, we'll use auto-discovery
    const oauthConfig = typeof mcpConfig.oauth === "object" ? mcpConfig.oauth : undefined
    let capturedUrl: URL | undefined
    const authProvider = new McpOAuthProvider(
      mcpName,
      mcpConfig.url,
      {
        clientId: oauthConfig?.clientId,
        clientSecret: oauthConfig?.clientSecret,
        scope: oauthConfig?.scope,
      },
      {
        onRedirect: async (url) => {
          capturedUrl = url
        },
      },
    )

    // Try to connect - this will trigger the OAuth flow
    let transport: StreamableHTTPClientTransport | undefined
    try {
      const { client } = await McpEra.connect({
        key: mcpName,
        target: mcpConfig.url,
        mode: mcpConfig.protocol ?? "auto",
        timeoutMs: mcpConfig.timeout ?? DEFAULT_TIMEOUT,
        client: newClient,
        transport: () => new StreamableHTTPClientTransport(new URL(mcpConfig.url), { authProvider, fetch: mcpGuardedFetch }),
        onTransport: (t) => (transport = t),
      })
      // If we get here, we're already authenticated
      await client.close().catch(() => {})
      return { authorizationUrl: "" }
    } catch (error) {
      if (error instanceof UnauthorizedError && capturedUrl && transport) {
        // Store transport for finishAuth
        pendingOAuthTransports.set(mcpName, transport)
        return { authorizationUrl: capturedUrl.toString() }
      }
      throw error
    }
  }

  /**
   * Complete OAuth authentication after user authorizes in browser.
   * Opens the browser and waits for callback.
   */
  export async function authenticate(mcpName: string): Promise<Status> {
    const { authorizationUrl } = await startAuth(mcpName)

    if (!authorizationUrl) {
      // Already authenticated
      const s = await state()
      return s.status[mcpName] ?? { status: "connected" }
    }

    // Get the state that was already generated and stored in startAuth()
    const oauthState = await McpAuth.getOAuthState(mcpName)
    if (!oauthState) {
      throw new Error("OAuth state not found - this should not happen")
    }

    // The SDK has already added the state parameter to the authorization URL
    // We just need to open the browser
    log.info("opening browser for oauth", { mcpName, url: authorizationUrl, state: oauthState })

    // Register the callback BEFORE opening the browser to avoid race condition
    // when the IdP has an active SSO session and redirects immediately
    const callbackPromise = McpOAuthCallback.waitForCallbackResult(oauthState)

    try {
      const subprocess = await open(authorizationUrl)
      // The open package spawns a detached process and returns immediately.
      // We need to listen for errors which fire asynchronously:
      // - "error" event: command not found (ENOENT)
      // - "exit" with non-zero code: command exists but failed (e.g., no display)
      await new Promise<void>((resolve, reject) => {
        // Give the process a moment to fail if it's going to
        const timeout = setTimeout(() => resolve(), 500)
        subprocess.on("error", (error) => {
          clearTimeout(timeout)
          reject(error)
        })
        subprocess.on("exit", (code) => {
          if (code !== null && code !== 0) {
            clearTimeout(timeout)
            reject(new Error(`Browser open failed with exit code ${code}`))
          }
        })
      })
    } catch (error) {
      // Browser opening failed (e.g., in remote/headless sessions like SSH, devcontainers)
      // Emit event so CLI can display the URL for manual opening
      log.warn("failed to open browser, user must open URL manually", { mcpName, error })
      Bus.publish(BrowserOpenFailed, { mcpName, url: authorizationUrl })
    }

    // Wait for callback using the already-registered promise
    const { code, iss } = await callbackPromise

    // Validate and clear the state
    const storedState = await McpAuth.getOAuthState(mcpName)
    if (storedState !== oauthState) {
      await McpAuth.clearOAuthState(mcpName)
      throw new Error("OAuth state mismatch - potential CSRF attack")
    }

    await McpAuth.clearOAuthState(mcpName)

    // Finish auth
    return finishAuth(mcpName, code, iss)
  }

  /**
   * Complete OAuth authentication with the authorization code.
   */
  export async function finishAuth(mcpName: string, authorizationCode: string, iss?: string): Promise<Status> {
    const transport = pendingOAuthTransports.get(mcpName)

    if (!transport) {
      throw new Error(`No pending OAuth flow for MCP server: ${mcpName}`)
    }

    try {
      // `iss` (RFC 9207) is validated by the SDK against the issuer it recorded.
      await transport.finishAuth(authorizationCode, iss)
      // New credentials, new authorization context: re-probe the server's era.
      await McpEra.forget(mcpName)

      // Clear the code verifier after successful auth
      await McpAuth.clearCodeVerifier(mcpName)

      // Now try to reconnect
      const cfg = await Config.get()
      const mcpConfig = withBundled(cfg.mcp)[mcpName]

      if (!mcpConfig) {
        throw new Error(`MCP server not found: ${mcpName}`)
      }

      if (!isMcpConfigured(mcpConfig)) {
        throw new Error(`MCP server ${mcpName} is disabled or missing configuration`)
      }

      // Re-add the MCP server to establish connection
      pendingOAuthTransports.delete(mcpName)
      const result = await add(mcpName, mcpConfig)

      const statusRecord = result.status as Record<string, Status>
      return statusRecord[mcpName] ?? { status: "failed", error: "Unknown error after auth" }
    } catch (error) {
      log.error("failed to finish oauth", { mcpName, error })
      return {
        status: "failed",
        error: error instanceof Error ? error.message : String(error),
      }
    }
  }

  /**
   * Remove OAuth credentials for an MCP server.
   */
  export async function removeAuth(mcpName: string): Promise<void> {
    await McpAuth.remove(mcpName)
    McpOAuthCallback.cancelPending(mcpName)
    pendingOAuthTransports.delete(mcpName)
    await McpAuth.clearOAuthState(mcpName)
    await McpEra.forget(mcpName)
    log.info("removed oauth credentials", { mcpName })
  }

  /**
   * Check if an MCP server supports OAuth (remote servers support OAuth by default unless explicitly disabled).
   */
  export async function supportsOAuth(mcpName: string): Promise<boolean> {
    const cfg = await Config.get()
    const mcpConfig = cfg.mcp?.[mcpName]
    if (!mcpConfig) return false
    if (!isMcpConfigured(mcpConfig)) return false
    return mcpConfig.type === "remote" && mcpConfig.oauth !== false
  }

  /**
   * Check if an MCP server has stored OAuth tokens.
   */
  export async function hasStoredTokens(mcpName: string): Promise<boolean> {
    const entry = await McpAuth.get(mcpName)
    return !!entry?.tokens
  }

  export type AuthStatus = "authenticated" | "expired" | "not_authenticated"

  /**
   * Get the authentication status for an MCP server.
   */
  export async function getAuthStatus(mcpName: string): Promise<AuthStatus> {
    const hasTokens = await hasStoredTokens(mcpName)
    if (!hasTokens) return "not_authenticated"
    const expired = await McpAuth.isTokenExpired(mcpName)
    return expired ? "expired" : "authenticated"
  }
}
