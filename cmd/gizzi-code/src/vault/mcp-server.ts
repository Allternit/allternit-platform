/**
 * Lens Context MCP Server
 *
 * Serves the Vault's structured/tagged notes to external MCP clients
 * (Claude Desktop, Cursor, etc.) over stdio — the "distribution" layer of
 * the Allternit Lens context stack. See docs/LENS_CONTEXT_LAYER_PLAN.md
 * Phase 4.
 *
 * Deliberately separate from entrypoints/mcp.ts, which exposes gizzi-code's
 * own agent *tools* over MCP — a different concern (agent-invoked actions,
 * not context reads). This server only ever reads the vault; it has no
 * tool that writes, connects, or executes anything external.
 *
 * Sensitivity is fail-closed: notes without an explicit sensitivity label
 * are treated as "private" by Vault.query() (see vault/index.ts), and this
 * server never serves "restricted" notes at all — there is no capsule-scoped
 * permission system yet to decide when that would be safe (see
 * mcp/servers/src/permissions.ts for the closest existing precedent, which
 * this does not yet integrate with).
 */

// SDK v2: `serveStdio` answers both `server/discover` (2026-07-28, stateless) and the
// 2025-era `initialize` handshake from one server factory, so any MCP client era can use
// the Lens server. (A bare `server.connect(new StdioServerTransport())` is 2025-era only.)
import { Server, type CallToolResult, type ListToolsResult } from "@modelcontextprotocol/server"
import { serveStdio } from "@modelcontextprotocol/server/stdio"
import { Log } from "@/shared/util/log"
import { VaultManager } from "./index"
import type { Vault } from "./types"

const log = Log.create({ service: "vault-mcp-server" })

const DEFAULT_SENSITIVITY: Vault.Sensitivity[] = ["public", "private"]

function summarize(note: Vault.Note, snippetLen = 400): Record<string, unknown> {
  return {
    title: note.title,
    path: note.relPath,
    source: note.frontmatter.source ?? null,
    sensitivity: note.frontmatter.sensitivity ?? "private",
    tags: note.frontmatter.tags ?? [],
    date: note.frontmatter.date ?? null,
    snippet: note.content.length > snippetLen ? `${note.content.slice(0, snippetLen)}...` : note.content,
  }
}

export async function startLensMcpServer(vaultPath?: string): Promise<void> {
  const vault = new VaultManager(vaultPath ? { vaultPath } : undefined)
  await vault.initialize()

  serveStdio(() => lensServer(vault))
  log.info("Lens MCP server started", { vaultPath: vault.rootPath })
}

function lensServer(vault: VaultManager): Server {
  const server = new Server(
    { name: "allternit-lens", version: "0.1.0" },
    { capabilities: { tools: {} } },
  )

  server.setRequestHandler("tools/list", async (): Promise<ListToolsResult> => ({
    tools: [
      {
        name: "search_context",
        description:
          "Search the connected context vault (Gmail, GitHub, Notion, etc. once connected) by free text, tags, or source. Only public/private notes are ever returned — never restricted ones.",
        inputSchema: {
          type: "object",
          properties: {
            query: { type: "string", description: "Free-text search" },
            sources: { type: "array", items: { type: "string" }, description: "Restrict to these source ids (e.g. \"github\", \"gmail\")" },
            tags: { type: "array", items: { type: "string" } },
            limit: { type: "number", default: 20 },
          },
        },
      },
      {
        name: "get_recent_activity",
        description: "Most recently updated notes across all connected sources, newest first.",
        inputSchema: {
          type: "object",
          properties: {
            sources: { type: "array", items: { type: "string" } },
            limit: { type: "number", default: 20 },
          },
        },
      },
      {
        name: "get_profile",
        description:
          "Read the synthesized profile note for a connected source (regenerated each sync — see extraction-pipeline.ts). Returns a clear not-found message if that source hasn't generated a profile yet.",
        inputSchema: {
          type: "object",
          properties: {
            source: { type: "string", description: "Source id, e.g. \"github\"" },
          },
          required: ["source"],
        },
      },
    ],
  }))

  server.setRequestHandler("tools/call", async ({ params }): Promise<CallToolResult> => {
    const { name, arguments: args } = params as { name: string; arguments?: Record<string, unknown> }
    try {
      switch (name) {
        case "search_context": {
          const result = await vault.query({
            text: typeof args?.query === "string" ? args.query : undefined,
            sources: Array.isArray(args?.sources) ? (args.sources as string[]) : undefined,
            tags: Array.isArray(args?.tags) ? (args.tags as string[]) : undefined,
            sensitivity: DEFAULT_SENSITIVITY,
            limit: typeof args?.limit === "number" ? args.limit : 20,
          })
          return {
            content: [{ type: "text", text: JSON.stringify({ total: result.total, truncated: result.truncated, notes: result.notes.map((n) => summarize(n)) }, null, 2) }],
          }
        }
        case "get_recent_activity": {
          const limit = typeof args?.limit === "number" ? args.limit : 20
          const sources = Array.isArray(args?.sources) ? (args.sources as string[]) : undefined
          const result = await vault.query({ sources, sensitivity: DEFAULT_SENSITIVITY, limit: 500 })
          const sorted = result.notes.sort((a, b) => b.mtime - a.mtime).slice(0, limit)
          return { content: [{ type: "text", text: JSON.stringify(sorted.map((n) => summarize(n)), null, 2) }] }
        }
        case "get_profile": {
          const source = args?.source
          if (typeof source !== "string" || !source) {
            return { isError: true, content: [{ type: "text", text: "source is required" }] }
          }
          const note = await vault.getNote(`profile/${source}.md`)
          if (!note) {
            return {
              content: [{ type: "text", text: `No profile generated yet for source "${source}". Profile notes are written by the extraction pipeline during sync — run a sync for this source first.` }],
            }
          }
          if ((note.frontmatter.sensitivity ?? "private") === "restricted") {
            return { isError: true, content: [{ type: "text", text: `Profile for "${source}" is marked restricted and cannot be served over MCP.` }] }
          }
          return { content: [{ type: "text", text: JSON.stringify(summarize(note, note.content.length), null, 2) }] }
        }
        default:
          return { isError: true, content: [{ type: "text", text: `Unknown tool: ${name}` }] }
      }
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e)
      log.error("Lens MCP tool call failed", { name, error: message })
      return { isError: true, content: [{ type: "text", text: message }] }
    }
  })

  return server
}
