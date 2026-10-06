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
import { hashValue, toAbiId } from "./jcs"

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

  // -------------------------------------------------------------------------
  // Tool descriptors from the tool registry's real schema
  // -------------------------------------------------------------------------

  const JSON_TYPES = new Set(["string", "number", "integer", "boolean", "object", "array"])
  const RESOURCE_PARAM = /^(file_?path|path|filepath|directory|dir|cwd)$/i

  /** Build an OperationDescriptor from a registry tool's JSON Schema (z.toJSONSchema(parameters)). */
  export function descriptorFromJsonSchema(tool: string, description: string | undefined, schema: any): OperationDescriptor {
    const props: Record<string, any> = schema?.properties ?? {}
    const required = new Set<string>(Array.isArray(schema?.required) ? schema.required : [])
    const params: OperationParam[] = Object.keys(props)
      .sort()
      .map((name) => {
        const p = props[name] ?? {}
        const t = Array.isArray(p.type) ? p.type.filter((x: string) => x !== "null") : [p.type]
        const type = (t.length === 1 && JSON_TYPES.has(t[0]) ? t[0] : "any") as OperationParam["type"]
        return {
          name,
          type,
          required: required.has(name),
          ...(typeof p.description === "string" ? { description: p.description } : {}),
          ...(Array.isArray(p.enum) ? { enum: p.enum } : {}),
          ...(RESOURCE_PARAM.test(name) ? { resource: "fs" } : {}),
        }
      })
    const desc = (description ?? tool).trim()
    return {
      tool_id: tool,
      operation_id: "invoke",
      summary: desc.split("\n")[0].slice(0, 200),
      effect_class: effectClassFor(tool),
      params,
      docs: desc,
    }
  }

  // -------------------------------------------------------------------------
  // Gate decisions (PermissionNext) per tool call
  // -------------------------------------------------------------------------

  export interface GateDecision {
    permission: string
    pattern: string
    action: "allow" | "deny" | "ask"
    /** Rule source, or "user_reply"/"catastrophic_floor". */
    source: string
  }
  const MAX_DECISION_CALLS = 500
  const decisions = new Map<string, GateDecision[]>()

  /** Called by PermissionNext.ask for each evaluated pattern. No-op with the flag off. */
  export function noteGateDecision(callID: string | undefined, d: GateDecision) {
    if (!callID || !enabled()) return
    const list = decisions.get(callID) ?? []
    list.push(d)
    decisions.set(callID, list)
    if (decisions.size > MAX_DECISION_CALLS) decisions.delete(decisions.keys().next().value!)
  }

  export function takeGateDecisions(callID: string | undefined): GateDecision[] {
    if (!callID) return []
    const d = decisions.get(callID) ?? []
    decisions.delete(callID)
    return d
  }

  /** Stable policy decision id derived from the gate's actual decisions for this call. */
  export function policyDecisionIdFor(callID: string | undefined, ds: GateDecision[]): string {
    if (!ds.length) return `pd.ungated.${toAbiId(callID ?? "call")}`
    return `pd.gate.${hashValue(ds).slice(7, 31)}`
  }

  /**
   * Wraps one tool execution. Flag off → `return run()` exactly. Flag on →
   * run it unchanged, then compile the call from the tool's registry schema
   * (all model-supplied args are GENERATED) with the gate's real decision for
   * this call, and record a ToolReceiptV1. The run's result or error passes
   * through as-is.
   *
   * External receipts are appended to the local Factory engine chain after the
   * unchanged tool call. Transport errors are logged and never break a turn.
   */
  export async function withToolReceipt<T>(
    input: {
      sessionID: string
      callID?: string
      tool: string
      args: unknown
      directory?: string
      description?: string
      /** Lazy JSON Schema of the tool's parameters (only evaluated with the flag on). */
      schema?: () => unknown
    },
    run: () => Promise<T>,
  ): Promise<T> {
    if (!enabled()) return run()
    const rec = entry(input.sessionID)
    const started_at = new Date().toISOString()
    const finish = async (exit: ToolReceiptV1["exit_class"], output?: string) => {
      const gate = takeGateDecisions(input.callID)
      try {
        const args = (input.args && typeof input.args === "object" ? input.args : {}) as Record<string, unknown>
        const op = descriptorFromJsonSchema(input.tool, input.description, input.schema?.() ?? inferSchema(args))
        const bindings: Record<string, ArgBinding> = {}
        for (const [k, v] of Object.entries(args)) if (v !== undefined) bindings[k] = { class: "GENERATED", value: v }
        const policy_decision_id = policyDecisionIdFor(input.callID, gate)
        const denied = gate.some((d) => d.action === "deny")
        const { invocation, diagnostics } = compileToolCall({
          state: sessionState({ sessionID: input.sessionID, directory: input.directory ?? "", objective: "", stateVersion: rec.state_version }),
          op,
          bindings,
          policy: {
            policy_decision_id,
            environment_id: "env.session",
            network_policy_id: op.effect_class === "NETWORK" ? "netpol.session" : null,
          },
          now: started_at,
        })
        const receipt = buildToolReceipt(
          invocation,
          {
            started_at,
            finished_at: new Date().toISOString(),
            exit_class: exit === "FAILURE" && denied ? "DENIED" : exit,
            output,
            policy_receipt_id: policy_decision_id,
            diagnostics: diagnostics.map((d) => ({ code: d })),
          },
          op,
        )
        receipt.extensions = {
          ...receipt.extensions,
          ...(gate.length
            ? { "x-gate_decisions": gate }
            : { "x-gate_reason": "no PermissionNext check was made for this call (tool did not ask)" }),
          "x-chain_append": "pending",
        }
        await appendReceipt(receipt, rec)
        rec.receipts.push(receipt)
        if (rec.receipts.length > MAX_RECEIPTS) rec.receipts.splice(0, rec.receipts.length - MAX_RECEIPTS)
      } catch (e) {
        rec.errors.push(`tool:${input.tool}:${(e as Error).message}`)
      }
    }
    try {
      const result = await run()
      const out = (result as any)?.output
      await finish("SUCCESS", typeof out === "string" ? out : undefined)
      return result
    } catch (e) {
      await finish("FAILURE")
      throw e
    }
  }

  let warnedOldEngineUrl = false
  /**
   * The Factory engine URL: `$ALLTERNIT_FACTORY_URL`, else the local engine
   * (3011). A URL a user still exports under the pre-Factory names is honoured
   * with a one-time deprecation notice.
   */
  export function engineUrl(env: NodeJS.ProcessEnv = process.env): string {
    const current = env.ALLTERNIT_FACTORY_URL
    if (current) return current.replace(/\/+$/, "")
    const old = env.GIZZI_COMMRAILS_URL ?? env.GIZZI_RAILS_URL // old-names: keep (user shell config from before the Factory rename)
    if (old) {
      if (!warnedOldEngineUrl) {
        warnedOldEngineUrl = true
        process.stderr.write("gizzi: GIZZI_RAILS_URL / GIZZI_COMMRAILS_URL are deprecated; set ALLTERNIT_FACTORY_URL instead.\n") // old-names: keep (deprecation notice names the old vars)
      }
      return old.replace(/\/+$/, "")
    }
    return "http://127.0.0.1:3011"
  }

  /** Factory engine receipt chain (see engineUrl).
   * Read per call so long-lived sessions can follow service configuration.
   * No retries: an uncertain response must not blindly duplicate evidence.
   */
  async function appendReceipt(receipt: ToolReceiptV1, rec: TurnRecord): Promise<void> {
    const base = engineUrl()
    try {
      const response = await fetch(`${base}/v1/receipts/chain/${encodeURIComponent(receipt.envelope.run_id)}`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(receipt),
        signal: AbortSignal.timeout(1_000),
      })
      if (!response.ok) throw new Error(`HTTP ${response.status}`)
      const chained = await response.json() as { chain?: { receipt_id?: string; run_id?: string } }
      if (!chained.chain?.receipt_id || chained.chain.run_id !== receipt.envelope.run_id) throw new Error("invalid chain acknowledgement")
      receipt.extensions = { ...receipt.extensions, "x-chain_append": "appended", "x-chain_receipt_id": chained.chain.receipt_id }
    } catch (e) {
      const message = `chain_append:${(e as Error).message}`
      rec.errors.push(message)
      receipt.extensions = { ...receipt.extensions, "x-chain_append": "failed" }
      console.warn(`[kernel-turn] ${message}`)
    }
  }

  /** Fallback only when a caller supplies no registry schema. */
  function inferSchema(args: Record<string, unknown>) {
    return { type: "object", properties: Object.fromEntries(Object.keys(args).map((k) => [k, {}])) }
  }
}
