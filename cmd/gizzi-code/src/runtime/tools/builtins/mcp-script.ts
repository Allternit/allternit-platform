import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import DESCRIPTION from "@/runtime/tools/builtins/mcp-script.txt"
import { ScriptSandbox } from "@/runtime/services/mcp/script-sandbox"
import { McpScriptShared } from "@/runtime/tools/builtins/mcp-script-shared"
import { PermissionNext } from "@/runtime/tools/guard/permission/next"
import { Bus } from "@/shared/bus"
import { ToolDedupe } from "@/runtime/tools/dedupe"
import { KernelTurn } from "@/runtime/kernel/compilers/turn-hook"
import type { ToolCallOptions } from "ai"

/**
 * Code Mode (MCP 2027 step 5): `mcp_script { script }` runs one TypeScript
 * script that calls several MCP tools, inside the QuickJS sandbox. The model
 * gets only the script's return value (<= 8 KB); the script, the call log and
 * the result ride `metadata.mcpScript` to the app part.
 *
 * Flag `mcp.codeMode` (default on): set GIZZI_DISABLE_MCP_CODE_MODE=1 to turn it off.
 */
export namespace McpScript {
  export const ID = McpScriptShared.ID
  export const enabled = McpScriptShared.enabled
  export const bridgeName = McpScriptShared.bridgeName
  export const tsType = McpScriptShared.tsType
  export const declarations = McpScriptShared.declarations
  export const toJavaScript = McpScriptShared.toJavaScript
  export const scriptValue = McpScriptShared.scriptValue

  /** One MCP tool offered this turn, bound to the session loop's wrapped execute. */
  export interface Entry extends McpScriptShared.Declared {
    key: string
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

  /**
   * Each call a script makes gets its own id, `<script call id>:<n>`, so the
   * prefix is the parent link. Keyed by it: the running-part metadata update
   * (finds no part, so the mcp_script part is never written over), the proxy's
   * one-shot approval (one call can't consume or clear another's) and the
   * permission ask.
   */
  export function nestedCallID(parent: string | undefined, n: number): string {
    return `${parent ?? ""}:${n}`
  }
  /** True when `callID` is a call made from inside the script whose id is `parent`. */
  export function isNestedOf(callID: string | undefined, parent: string | undefined): boolean {
    return callID !== undefined && parent !== undefined && callID.startsWith(`${parent}:`)
  }

  // Options objects of calls made from a script (not by the model).
  const scripted = new WeakSet<object>()
  /** The options a bridged call hands the wrapped execute; marked as a script call. */
  export function callOptions(ctx: Tool.Context): ToolCallOptions {
    const opts = { toolCallId: ctx.callID ?? "", abortSignal: ctx.abort, messages: [] } as ToolCallOptions
    scripted.add(opts)
    return opts
  }
  /**
   * ToolDedupe for model calls only. A script is explicit code: two identical
   * calls in it (a poll, a retry) both run, and a script call never shares a
   * model call's promise or gets the "repeated call" reminder.
   */
  export function dedupe<T>(opts: object, input: Parameters<typeof ToolDedupe.execute<T>>[0]): Promise<T> {
    return scripted.has(opts) ? input.run() : ToolDedupe.execute<T>(input)
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
      if (p.sessionID === ctx.sessionID && McpScript.isNestedOf(p.tool?.callID, ctx.callID)) waiting.set(p.id, Date.now())
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
    let n = 0
    try {
      run = await ScriptSandbox.run({
        js,
        abort: ctx.abort,
        approvalWaitMs,
        bridges: list.map((e) => ({
          server: e.server,
          tool: e.tool,
          call: (args) => e.call(args, { ...ctx, callID: McpScript.nestedCallID(ctx.callID, ++n) }),
        })),
      })
    } finally {
      offAsked()
      offReplied()
      // The inner calls' gate decisions belong to this call's kernel record.
      for (let i = 1; i <= n; i++)
        for (const d of KernelTurn.takeGateDecisions(McpScript.nestedCallID(ctx.callID, i))) KernelTurn.noteGateDecision(ctx.callID, d)
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
