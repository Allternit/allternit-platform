import { ScriptSandbox } from "@/runtime/services/mcp/script-sandbox"

/**
 * Code Mode helpers with no side effects and no server imports (no Tool,
 * PermissionNext, Bus, DB or Config), so the terminal (cli/ui/ink-app) can use
 * them without loading the session permission stack. The session loop's tool
 * (mcp-script.ts) re-exports these under `McpScript`.
 */
export namespace McpScriptShared {
  export const ID = "mcp_script"

  /** Flag `mcp.codeMode` (default on): GIZZI_DISABLE_MCP_CODE_MODE=1 turns it off. */
  export function enabled(): boolean {
    const v = process.env.GIZZI_DISABLE_MCP_CODE_MODE?.toLowerCase()
    return !(v === "1" || v === "true")
  }

  /** What `declarations` needs to know about one tool. */
  export interface Declared {
    server: string
    tool: string
    description?: string
    inputSchema?: unknown
    outputSchema?: unknown
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

  export function declarations(list: Declared[]): string {
    const byServer = new Map<string, Declared[]>()
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
