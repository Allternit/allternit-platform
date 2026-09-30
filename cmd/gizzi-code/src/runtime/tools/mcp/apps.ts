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
