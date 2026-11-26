import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import DESCRIPTION from "@/runtime/tools/builtins/mcp-script.txt"
import { ScriptSandbox } from "@/runtime/services/mcp/script-sandbox"
import { PermissionNext } from "@/runtime/tools/guard/permission/next"
import { Bus } from "@/shared/bus"

/**
 * Code Mode (MCP 2027 step 5): `mcp_script { script }` runs one TypeScript
 * script that calls several MCP tools, inside the QuickJS sandbox. The model
 * gets only the script's return value (<= 8 KB); the script, the call log and
 * the result ride `metadata.mcpScript` to the app part.
 *
 * Flag `mcp.codeMode` (default on): set GIZZI_DISABLE_MCP_CODE_MODE=1 to turn it off.
 */
export namespace McpScript {
  export const ID = "mcp_script"

  export function enabled(): boolean {
    const v = process.env.GIZZI_DISABLE_MCP_CODE_MODE?.toLowerCase()
    return !(v === "1" || v === "true")
  }

  /** One MCP tool offered this turn, bound to the session loop's wrapped execute. */
  export interface Entry {
    key: string
    server: string
    tool: string
    description?: string
    inputSchema?: unknown
    outputSchema?: unknown
    /** The same call a model MCP tool call takes (gates, approvals, dedupe, dispatcher). */
    call(args: Record<string, unknown>, ctx: Tool.Context): Promise<unknown>
  }

  // Per-session entries for the current turn (prompt.ts sets them each turn).
  const sessions = new Map<string, Entry[]>()
  const MAX_SESSIONS = 200

  export function register(sessionID: string, entries: Entry[]) {
    sessions.delete(sessionID)
    sessions.set(sessionID, entries)
    while (sessions.size > MAX_SESSIONS) sessions.delete(sessions.keys().next().value as string)
  }

  /**
   * How a tool is named inside a script. App-chat tools reach gizzi through one
   * proxy server as `<prefix>__<tool>` with `_meta["allternit/connector"]`
   * (descriptor `connectorId` / `appName`): those become `<connector>.<tool>`,
   * the connector name slugged to an identifier, else the prefix. A server
   * connected straight to gizzi stays `<server>.<tool>`. `calls[].server` is this label.
   */
  export function bridgeName(
    descriptor: { serverName: string; originalName: string; appName?: string; connectorId?: string } | undefined,
    key: string,
  ): { server: string; tool: string } {
    if (!descriptor) return { server: key, tool: key }
    const split = descriptor.originalName.indexOf("__")
    const proxied = descriptor.connectorId !== undefined || descriptor.appName !== undefined
    if (!proxied || split <= 0) return { server: descriptor.serverName, tool: descriptor.originalName }
    const prefix = descriptor.originalName.slice(0, split)
    const tool = descriptor.originalName.slice(split + 2) || descriptor.originalName
    const slug = (descriptor.appName ?? "")
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "_")
      .replace(/^_+|_+$/g, "")
    return { server: ScriptSandbox.identifier(slug || prefix), tool }
  }

  // The raw CallToolResult of a bridged call, keyed by the options object the
  // bridge passed in (the wrapped execute returns model-ready output only).
  const raw = new WeakMap<object, unknown>()
  export function noteRaw(opts: object, result: unknown) {
    raw.set(opts, result)
  }
  export function takeRaw(opts: object): unknown {
    const result = raw.get(opts)
    raw.delete(opts)
    return scriptValue(result)
  }

  export function entries(sessionID: string): Entry[] {
    return sessions.get(sessionID) ?? []
  }

  /**
   * Called by the session loop after the MCP tools are added: offers
   * `mcp_script` only when the flag is on and 2+ MCP tools are connected, and
   * adds the typed declarations to its description.
   */
  export function attach(tools: Record<string, { description?: string }>, sessionID: string, list: Entry[]) {
    const tool = tools[ID]
    if (!tool) return
    if (!enabled() || list.length < 2) {
      delete tools[ID]
      sessions.delete(sessionID)
      return
    }
    register(sessionID, list)
    tool.description = `${tool.description ?? DESCRIPTION}\n\n<declarations>\n${declarations(list)}\n</declarations>`
  }

  // ── typed declarations from JSON Schema ────────────────────────────────────
  const MAX_DECLARATIONS = 24_000

  export function tsType(schema: unknown, depth = 0): string {
    if (!schema || typeof schema !== "object" || depth > 5) return "unknown"
    const s = schema as Record<string, any>
    if (Array.isArray(s.enum) && s.enum.length) return s.enum.map((v: unknown) => JSON.stringify(v)).join(" | ")
    if ("const" in s) return JSON.stringify(s.const)
    const union = s.anyOf ?? s.oneOf
    if (Array.isArray(union) && union.length) return union.map((u: unknown) => tsType(u, depth + 1)).join(" | ")
    const type = s.type
    if (Array.isArray(type)) return type.map((t: unknown) => tsType({ ...s, type: t }, depth + 1)).join(" | ")
    switch (type) {
      case "string":
        return "string"
      case "number":
      case "integer":
        return "number"
      case "boolean":
        return "boolean"
      case "null":
        return "null"
      case "array":
        return `Array<${tsType(s.items, depth + 1)}>`
      case "object":
      case undefined: {
        const props = s.properties as Record<string, unknown> | undefined
        if (!props || typeof props !== "object") return type === "object" ? "Record<string, unknown>" : "unknown"
        const required = new Set<string>(Array.isArray(s.required) ? s.required : [])
        const fields = Object.entries(props).map(([name, sub]) => {
          const key = /^[A-Za-z_$][A-Za-z0-9_$]*$/.test(name) ? name : JSON.stringify(name)
          return `${key}${required.has(name) ? "" : "?"}: ${tsType(sub, depth + 1)}`
        })
        return `{ ${fields.join("; ")} }`
      }
      default:
        return "unknown"
    }
  }

  export function declarations(list: Entry[]): string {
    const byServer = new Map<string, Entry[]>()
    for (const e of list) {
      const id = ScriptSandbox.identifier(e.server)
      byServer.set(id, [...(byServer.get(id) ?? []), e])
    }
    let out = ""
    const skipped: string[] = []
    for (const [server, items] of byServer) {
      let block = `declare const ${server}: {\n`
      for (const e of items) {
        const doc = (e.description ?? "").replace(/\*\//g, "* /").split("\n")[0]?.slice(0, 200)
        const line = `${doc ? `  /** ${doc} */\n` : ""}  ${ScriptSandbox.identifier(e.tool)}(args: ${tsType(e.inputSchema)}): Promise<${e.outputSchema ? tsType(e.outputSchema) : "unknown"}>;\n`
        if (out.length + block.length + line.length > MAX_DECLARATIONS) skipped.push(`${server}.${ScriptSandbox.identifier(e.tool)}`)
        else block += line
      }
      out += `${block}};\n`
    }
    if (skipped.length) out += `// Also callable (declarations left out for length): ${skipped.join(", ")}\n`
    return out.trimEnd()
  }

  // ── script → JavaScript ────────────────────────────────────────────────────
  /** Strips TypeScript types. The script is an async function body; `return` gives the result. */
  export function toJavaScript(script: string): string {
    const transpiler = new Bun.Transpiler({ loader: "ts", target: "browser" })
    const wrapped = `globalThis.__mcpScriptBody = async function () {\n${script}\n};`
    const js = transpiler.transformSync(wrapped)
    const open = js.indexOf("{")
    const close = js.lastIndexOf("}")
    if (open < 0 || close <= open) throw new Error("The script could not be read.")
    return js.slice(open + 1, close)
  }

  /** What a bridged call gives the script: structured content, else parsed text, else text. */
  export function scriptValue(result: unknown): unknown {
    const r = (result ?? {}) as { structuredContent?: unknown; content?: unknown; isError?: boolean }
    const texts = Array.isArray(r.content)
      ? r.content.filter((c: any) => c?.type === "text" && typeof c.text === "string").map((c: any) => c.text as string)
      : []
    if (r.isError) throw new Error(texts.join("\n").slice(0, 500) || "The tool returned an error.")
    if (r.structuredContent !== undefined) return r.structuredContent
    const text = texts.join("\n")
    try {
      return JSON.parse(text)
    } catch {
      return text
    }
  }
}

export const McpScriptTool = Tool.define(McpScript.ID, {
  description: DESCRIPTION,
  parameters: z.object({
    script: z
      .string()
      .min(1)
      .describe("TypeScript: the body of an async function. Call tools as `await <server>.<tool>(args)` and `return` the result."),
  }),
  async execute(params, ctx) {
    const list = McpScript.entries(ctx.sessionID)
    if (list.length < 2) throw new Error("Code Mode needs 2 or more connected MCP tools in this chat.")

    // Time spent waiting on the person's approval does not count toward the 60 s limit.
    const waiting = new Map<string, number>()
    let waitedMs = 0
    const offAsked = Bus.subscribe(PermissionNext.Event.Asked, (event) => {
      const p = event.properties
      if (p.sessionID === ctx.sessionID && p.tool?.callID === ctx.callID) waiting.set(p.id, Date.now())
    })
    const offReplied = Bus.subscribe(PermissionNext.Event.Replied, (event) => {
      const at = waiting.get(event.properties.requestID)
      if (at === undefined) return
      waiting.delete(event.properties.requestID)
      waitedMs += Date.now() - at
    })
    const approvalWaitMs = () => {
      let open = 0
      for (const at of waiting.values()) open += Date.now() - at
      return waitedMs + open
    }

    let js: string
    try {
      js = McpScript.toJavaScript(params.script)
    } catch (error) {
      offAsked()
      offReplied()
      throw new Error(`The script has a syntax error: ${error instanceof Error ? error.message : String(error)}`)
    }

    let run: ScriptSandbox.RunResult
    try {
      run = await ScriptSandbox.run({
        js,
        abort: ctx.abort,
        approvalWaitMs,
        bridges: list.map((e) => ({
          server: e.server,
          tool: e.tool,
          call: (args) => e.call(args, ctx),
        })),
      })
    } finally {
      offAsked()
      offReplied()
    }

    const servers = [...new Set(run.calls.map((c) => c.server))]
    const output = run.ok ? run.result || "(the script returned nothing)" : `The script failed: ${run.error}`
    return {
      title: `${run.calls.length} MCP call${run.calls.length === 1 ? "" : "s"}`,
      output,
      metadata: {
        // Display only (decision 3): the model reads `output` and nothing else.
        mcpScript: {
          lang: "ts" as const,
          script: params.script,
          servers,
          calls: run.calls,
          durationMs: run.durationMs,
          result: run.ok ? run.result : "",
        },
        ...(run.ok ? {} : { error: run.error }),
        // The sandbox already capped the result at 8 KB.
        truncated: false,
      },
    }
  },
})
