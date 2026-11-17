/**
 * Resources (Primitives Wave D, D1 recall): id-based views of large tool
 * output and MCP resources. Shared core for the `resource` tool (runtime
 * builtin tools/builtins/resource.ts and the Ink adapter in
 * cli/ui/ink-app/tools/ResourceTool).
 *
 * - Refs are scoped to the session: `out:<id>` names a tool output that
 *   truncation.ts saved for this session; `mcp:<server>:<uri>` names a
 *   resource of an MCP server mounted in this session. A ref never resolves
 *   to a path outside the session's own outputs (`out:../x` is refused).
 * - read: a line window (`offset`, `limit` up to MAX_LINES).
 * - find: lines matching `pattern`, with up to MAX_CONTEXT lines around each.
 * - list: the session's refs.
 *
 * W1 stub: final signatures and limits; lane R fills the bodies.
 * Free of UI imports so the Ink adapter and bun tests can use it.
 */
import z from "zod/v4"

export namespace Resource {
  export const MAX_LINES = 400
  export const MAX_CONTEXT = 5

  export const Input = z.object({
    action: z.enum(["list", "read", "find"]).describe("list the session's refs, read a window, or find lines"),
    ref: z.string().optional().describe('A ref such as "out:<id>" or "mcp:<server>:<uri>" (read and find)'),
    offset: z.number().int().min(0).optional().describe("First line to read, 0-based (read)"),
    limit: z.number().int().min(1).max(MAX_LINES).optional().describe(`Lines to read (max ${MAX_LINES})`),
    pattern: z.string().max(500).optional().describe("Regular expression or plain words to find (find)"),
    context: z.number().int().min(0).max(MAX_CONTEXT).optional().describe(`Lines of context around each hit (max ${MAX_CONTEXT})`),
  })
  export type Input = z.infer<typeof Input>

  export type Ref = { kind: "out"; id: string } | { kind: "mcp"; server: string; uri: string }

  export interface Entry {
    ref: string
    title: string
    lines?: number
    bytes?: number
  }

  export interface Result {
    action: Input["action"]
    ref?: string
    /** list */
    entries?: Entry[]
    /** read and find: the text shown to the model. */
    text?: string
    total_lines?: number
    next_offset?: number | null
  }

  export class NotReadyError extends Error {
    constructor() {
      super("resource is not available in this build yet.")
      this.name = "ResourceNotReadyError"
    }
  }

  /** Parse a ref string; null when it is malformed or tries to leave the session. */
  export function parseRef(_ref: string): Ref | null {
    return null
  }

  export async function run(_sessionID: string, _input: Input, _signal?: AbortSignal): Promise<Result> {
    throw new NotReadyError()
  }
}
