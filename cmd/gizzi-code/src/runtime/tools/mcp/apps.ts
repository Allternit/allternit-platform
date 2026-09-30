// MCP Apps (SEP-1865) host support shared by every MCP client path that serves chat.
//
// A host declares the extension in `initialize` so servers know they may attach
// `_meta.ui` to tools and serve `ui://` resources. Tools whose `_meta.ui.visibility`
// omits "model" are callable only by the rendered app and must never reach the model.

export const MCP_APPS_EXTENSION_ID = "io.modelcontextprotocol/ui"
export const MCP_APP_RESOURCE_MIME_TYPE = "text/html;profile=mcp-app"

// Not in the SDK's ClientCapabilities type at 1.29; the wire format is what matters.
export const MCP_APPS_CLIENT_CAPABILITIES = {
  extensions: {
    [MCP_APPS_EXTENSION_ID]: { mimeTypes: [MCP_APP_RESOURCE_MIME_TYPE] },
  },
} as Record<string, unknown>

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

/** `_meta.ui.resourceUri`, else the legacy flat `_meta["ui/resourceUri"]` / `_meta["openai/outputTemplate"]`. */
export function mcpAppResourceUri(tool: { _meta?: unknown }): string | undefined {
  const meta = tool._meta
  if (!isRecord(meta)) return undefined
  const nested = isRecord(meta.ui) && typeof meta.ui.resourceUri === "string" ? meta.ui.resourceUri : undefined
  const legacy = [meta["ui/resourceUri"], meta["openai/outputTemplate"]].find(
    (v): v is string => typeof v === "string",
  )
  return nested ?? legacy
}

/** Visibility list of a tool; absent means visible to both model and app. */
function visibility(tool: { _meta?: unknown }): string[] {
  const meta = tool._meta
  const raw = isRecord(meta) && isRecord(meta.ui) ? meta.ui.visibility : undefined
  if (!Array.isArray(raw)) return ["model", "app"]
  const list = raw.filter((v): v is string => v === "model" || v === "app")
  return list.length > 0 ? list : ["model", "app"]
}

export function isVisibleToModel(tool: { _meta?: unknown }): boolean {
  return visibility(tool).includes("model")
}

/** Largest serialized MCP result kept on a tool part for the host to render. */
const MAX_APP_RESULT_BYTES = 256 * 1024

/**
 * Tool-part metadata that lets the host (allternit-api) emit an `mcp_app` frame:
 * which MCP server/tool produced the result, and the raw result. Only tools that
 * declare a `ui://` resource carry it; results too large to persist are flagged.
 */
export function mcpAppMetadata(
  descriptor: { serverName: string; originalName: string; uiResourceUri?: string } | undefined,
  result: { content?: unknown; structuredContent?: unknown; _meta?: unknown; isError?: boolean },
): { mcp?: Record<string, unknown> } {
  if (!descriptor?.uiResourceUri) return {}
  const raw: Record<string, unknown> = {
    content: result.content,
    structuredContent: result.structuredContent,
    _meta: result._meta,
    isError: result.isError,
  }
  const kept = Object.fromEntries(Object.entries(raw).filter(([, v]) => v !== undefined))
  const withinLimit = JSON.stringify(kept).length <= MAX_APP_RESULT_BYTES
  return {
    mcp: {
      server: descriptor.serverName,
      tool: descriptor.originalName,
      ...(withinLimit ? { result: kept } : { resultOmitted: "too_large" }),
    },
  }
}

// ── `mcp_app` stream frame (gizzi's own agent-chat route) ────────────────────
// Mirrors allternit-api's `mcp_apps.rs::build_app_frame`; the field names are what the web client's
// `buildMcpAppPart` requires.

/** `_meta` key the per-user proxy adds to each tool: `{ id, name }` of the owning connector. */
export const MCP_CONNECTOR_META_KEY = "allternit/connector"

const ALLOW_DIRECTIVES: Array<[string, string]> = [
  ["camera", "camera"],
  ["microphone", "microphone"],
  ["geolocation", "geolocation"],
  ["clipboardWrite", "clipboard-write"],
]

export function mcpAllowAttribute(permissions: unknown): string {
  if (!isRecord(permissions)) return ""
  return ALLOW_DIRECTIVES.filter(([key]) => isRecord(permissions[key])).map(([, d]) => d).join("; ")
}

/** The HTML of a `resources/read` result and its `_meta.ui` (content-level first, then result-level). */
export function mcpAppHtml(result: unknown): { html: string; ui: Record<string, unknown> } | undefined {
  if (!isRecord(result) || !Array.isArray(result.contents)) return undefined
  const item = result.contents.find(
    (c) => isRecord(c) && (c.mimeType === MCP_APP_RESOURCE_MIME_TYPE || c.mimeType === "text/html"),
  ) as Record<string, unknown> | undefined
  if (!item) return undefined
  let html: string | undefined
  if (typeof item.text === "string") html = item.text
  else if (typeof item.blob === "string") html = Buffer.from(item.blob, "base64").toString("utf8")
  if (html === undefined) return undefined
  const ui = [item._meta, result._meta]
    .map((m) => (isRecord(m) ? (m.ui ?? m[MCP_APPS_EXTENSION_ID]) : undefined))
    .find(isRecord)
  return { html, ui: (ui as Record<string, unknown>) ?? {} }
}

function camelCsp(ui: Record<string, unknown>): Record<string, unknown> | undefined {
  if (!isRecord(ui.csp)) return undefined
  const out: Record<string, unknown> = {}
  for (const [camel, snake] of [
    ["connectDomains", "connect_domains"],
    ["resourceDomains", "resource_domains"],
    ["frameDomains", "frame_domains"],
    ["baseUriDomains", "base_uri_domains"],
  ]) {
    const v = ui.csp[camel] ?? ui.csp[snake]
    if (Array.isArray(v)) out[camel] = v
  }
  return out
}

/** Largest app document emitted (same cap as allternit-api). */
const MAX_APP_HTML_BYTES = 2 * 1024 * 1024

export function buildMcpAppFrame(args: {
  messageId: string
  callId: string
  connector: { id: string; name: string }
  tool: Record<string, unknown>
  resourceUri: string
  html: string
  ui: Record<string, unknown>
  toolInput: unknown
  toolResult: unknown
}): Record<string, unknown> | undefined {
  if (args.html.length > MAX_APP_HTML_BYTES) return undefined
  const { tool, ui, connector } = args
  const str = (v: unknown) => (typeof v === "string" && v !== "" ? v : undefined)
  const permissions = isRecord(ui.permissions) ? ui.permissions : undefined
  const frame: Record<string, unknown> = {
    type: "mcp_app",
    messageId: args.messageId,
    toolCallId: args.callId,
    toolName: str(tool.name) ?? "",
    connectorId: connector.id,
    connectorName: connector.name,
    title: str(tool.title) ?? str(tool.description) ?? connector.name,
    resourceUri: args.resourceUri,
    html: args.html,
    allow: mcpAllowAttribute(permissions),
    prefersBorder: typeof ui.prefersBorder === "boolean" ? ui.prefersBorder : true,
    tool: {
      name: tool.name,
      title: tool.title,
      description: tool.description,
      inputSchema: tool.inputSchema,
      annotations: tool.annotations,
      _meta: tool._meta,
    },
    toolInput: args.toolInput,
    toolResult: args.toolResult,
  }
  if (str(tool.description)) frame.description = tool.description
  const csp = camelCsp(ui)
  if (csp) frame.csp = csp
  if (permissions) frame.permissions = permissions
  if (str(ui.domain)) frame.domain = ui.domain
  return frame
}
