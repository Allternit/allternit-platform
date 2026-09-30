/**
 * WP9 wiring: where gizzi assembles a turn's context and executes tool calls.
 *
 * Behind `GIZZI_KERNEL_COMPILERS` (off by default). Off → every entry point is a
 * pass-through: no compile, no ledger write, no allocation beyond the env read.
 * On → SHADOW mode: the Context Compiler and Tool Call Compiler run alongside the
 * existing pipeline and record projections + ToolReceiptV1s in an in-process
 * ledger, but never change the system prompt, the tool set, tool arguments or
 * results. Launch/permission behaviour (WP4/WP4b) is untouched. Compiler errors
 * are recorded and swallowed, never surfaced to the turn.
 */
import type { AgentStateV1, ToolReceiptV1 } from "../../../../../../spec/Contracts/kernel/v1/generated/ts/kernel-abi"
import { compileContext, type CompiledContext, type ContextCandidate } from "./context-compiler"
import {
  buildToolReceipt,
  compileToolCall,
  type ArgBinding,
  type EffectClass,
  type OperationDescriptor,
  type OperationParam,
} from "./tool-call-compiler"
import { toAbiId } from "./jcs"

export namespace KernelTurn {
  export const FLAG = "GIZZI_KERNEL_COMPILERS"

  /** Read per call so tests/CLI can toggle mid-process. */
  export function enabled(): boolean {
    const v = process.env[FLAG]?.toLowerCase()
    return v === "1" || v === "true"
  }

  export interface TurnRecord {
    state_version: number
    projection: CompiledContext["projection"] | null
    receipts: ToolReceiptV1[]
    errors: string[]
  }

  const MAX_RECEIPTS = 200
  const ledger = new Map<string, TurnRecord>()

  function entry(sessionID: string): TurnRecord {
    let r = ledger.get(sessionID)
    if (!r) {
      r = { state_version: 0, projection: null, receipts: [], errors: [] }
      ledger.set(sessionID, r)
    }
    return r
  }

  export function record(sessionID: string): TurnRecord | undefined {
    return ledger.get(sessionID)
  }

  export function reset() {
    ledger.clear()
  }

  /** Minimal AgentState derived from a gizzi session turn. */
  export function sessionState(input: { sessionID: string; directory: string; objective: string; stateVersion: number }): AgentStateV1 {
    const id = toAbiId(input.sessionID, "ses")
    return {
      schema_id: "allternit.kernel.AgentStateV1",
      schema_version: "1.0.0",
      identity: { run_id: id, session_id: id, task_id: id, state_version: input.stateVersion },
      user_intent: { objective: input.objective },
      acceptance: [],
      constraints: [],
      environment: { workspace_root: input.directory },
      active_packs: [],
      plan: {},
      graph_cursor: { graph_id: "graph.session", graph_version: 0 },
      evidence: [],
      permissions: { grant_ids: [] },
      budgets: {},
      completion: { coverage: [] },
      instruction_predicates: [],
      sensitivity_labels: [],
      retrieval_snapshot_refs: [],
    }
  }

  /** Called once per turn after the system prompt is assembled. Never mutates `system`. */
  export function compileTurnContext(input: {
    sessionID: string
    directory: string
    objective: string
    system: readonly string[]
    budgetTokens: number
    now?: string
  }): CompiledContext | undefined {
    if (!enabled()) return undefined
    const rec = entry(input.sessionID)
    try {
      rec.state_version += 1
      const state = sessionState({ ...input, stateVersion: rec.state_version })
      const candidates: ContextCandidate[] = input.system.map((text, i) => ({
        key: `system:${i}`,
        kind: "INSTRUCTION",
        text,
        source: "SYSTEM",
        source_id: `system-fragment-${i}`,
        trust_class: "INTERNAL",
        priority: 0.85,
        pinned: true,
        extraction_method: "session.system",
      }))
      const compiled = compileContext({
        state,
        node: { node_id: `turn-${rec.state_version}`, capability: "cap.session.turn" },
        budget_tokens: input.budgetTokens,
        now: input.now ?? new Date().toISOString(),
        candidates,
      })
      rec.projection = compiled.projection
      return compiled
    } catch (e) {
      rec.errors.push(`context:${(e as Error).message}`)
      return undefined
    }
  }

  const READ_TOOLS = new Set(["read", "glob", "grep", "list", "ls", "lsp", "codesearch"])
  const WRITE_TOOLS = new Set(["edit", "write", "patch", "multiedit", "apply_patch"])
  const NET_TOOLS = new Set(["webfetch", "websearch"])
  export function effectClassFor(tool: string): EffectClass {
    const t = tool.toLowerCase()
    if (READ_TOOLS.has(t)) return "READ"
    if (WRITE_TOOLS.has(t)) return "WORKSPACE_WRITE"
    if (NET_TOOLS.has(t)) return "NETWORK"
    return "EXECUTE" // unknown tools are treated conservatively
  }

  function paramFor(name: string, v: unknown): OperationParam {
    const type: OperationParam["type"] = Array.isArray(v)
      ? "array"
      : v === null || typeof v === "object"
        ? "object"
        : typeof v === "number"
          ? "number"
          : typeof v === "boolean"
            ? "boolean"
            : "string"
    return { name, type, ...(/^(file_?path|path|filepath)$/i.test(name) ? { resource: "fs" } : {}) }
  }

  /**
   * Wraps one tool execution. Flag off → `return run()` exactly. Flag on →
   * compile the call (all model-supplied args are GENERATED), run it unchanged,
   * and record a ToolReceiptV1. The run's result or error passes through as-is.
   */
  export async function withToolReceipt<T>(
    input: { sessionID: string; callID?: string; tool: string; args: unknown; directory?: string },
    run: () => Promise<T>,
  ): Promise<T> {
    if (!enabled()) return run()
    const rec = entry(input.sessionID)
    let compiled: ReturnType<typeof compileToolCall> | undefined
    let op: OperationDescriptor | undefined
    try {
      const args = (input.args && typeof input.args === "object" ? input.args : {}) as Record<string, unknown>
      const effect = effectClassFor(input.tool)
      op = {
        tool_id: input.tool,
        operation_id: "invoke",
        summary: input.tool,
        effect_class: effect,
        params: Object.entries(args)
          .filter(([, v]) => v !== undefined)
          .map(([k, v]) => paramFor(k, v)),
      }
      const bindings: Record<string, ArgBinding> = {}
      for (const p of op.params) bindings[p.name] = { class: "GENERATED", value: args[p.name] }
      compiled = compileToolCall({
        state: sessionState({ sessionID: input.sessionID, directory: input.directory ?? "", objective: "", stateVersion: rec.state_version }),
        op,
        bindings,
        policy: {
          // The live gate decision (WP4/WP4b) is not plumbed here yet; shadow ids say so.
          policy_decision_id: `pd.shadow.${toAbiId(input.callID ?? "call")}`,
          environment_id: "env.session",
          network_policy_id: effect === "NETWORK" ? "netpol.session" : null,
        },
        now: new Date().toISOString(),
      })
    } catch (e) {
      rec.errors.push(`tool:${input.tool}:${(e as Error).message}`)
    }
    const started_at = new Date().toISOString()
    const finish = (exit_class: ToolReceiptV1["exit_class"], output?: string) => {
      if (!compiled) return
      try {
        rec.receipts.push(
          buildToolReceipt(
            compiled.invocation,
            {
              started_at,
              finished_at: new Date().toISOString(),
              exit_class,
              output,
              policy_receipt_id: compiled.invocation.policy_decision_id,
            },
            op,
          ),
        )
        if (rec.receipts.length > MAX_RECEIPTS) rec.receipts.splice(0, rec.receipts.length - MAX_RECEIPTS)
      } catch (e) {
        rec.errors.push(`receipt:${input.tool}:${(e as Error).message}`)
      }
    }
    try {
      const result = await run()
      const out = (result as any)?.output
      finish("SUCCESS", typeof out === "string" ? out : undefined)
      return result
    } catch (e) {
      finish("FAILURE")
      throw e
    }
  }
}
