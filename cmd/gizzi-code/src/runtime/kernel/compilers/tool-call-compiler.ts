/**
 * WP9 Tool Call Compiler (C6, 02 §14).
 *
 *   op + args from state → ToolInvocationV1 with a provenance class on every
 *   argument, tiered disclosure of the operation to the model, and a
 *   ToolReceiptV1 per compiled call that the WP3 receipt chain can append
 *   (commrails ChainStore::append needs envelope.{schema_id,schema_version,run_id}).
 *
 * Provenance rules (02 §14, L2097–L2111):
 *  - POLICY values can't be overridden by the model: a policy-fixed argument
 *    always wins over any other binding.
 *  - GENERATED values must pass schema checks before they compile.
 *  - USER values are kept verbatim.
 *  - STATE values are resolved from AgentState by path, never guessed.
 *
 * Tiered disclosure (L1716, L1825): capability snippet → schema on demand →
 * docs on demand. Policy-only parameters are never disclosed to the model, and
 * SECRET argument values are redacted from anything disclosed or logged.
 */
import type {
  AbiEnvelopeV1,
  AgentStateV1,
  SensitivityClass,
  ToolInvocationV1,
  ToolReceiptV1,
} from "../../../../../../spec/Contracts/kernel/v1/generated/ts/kernel-abi"
import { hashValue, sha256Tagged, toAbiId } from "./jcs"

export const TOOL_SCHEMA_VERSION = "1.0.0"
export const ABI_VERSION = "1.0.0"

export type ArgProvenanceClass = "STATE" | "RETRIEVAL" | "POLICY" | "GENERATED" | "USER"
export type EffectClass = ToolInvocationV1["effect_class"]
export type ExitClass = ToolReceiptV1["exit_class"]
export type DisclosureTier = "SNIPPET" | "SCHEMA" | "DOCS"

export interface OperationParam {
  name: string
  type: "string" | "number" | "integer" | "boolean" | "object" | "array"
  required?: boolean
  description?: string
  enum?: unknown[]
  /** Only policy may set it; never disclosed to the model. */
  policy_only?: boolean
  sensitivity?: SensitivityClass
  /** Resource prefix when the value names a resource, e.g. "fs". */
  resource?: string
}

export interface OperationDescriptor {
  tool_id: string
  operation_id: string
  summary: string
  effect_class: EffectClass
  params: OperationParam[]
  docs?: string
  timeout_ms?: number
}

export interface ArgBinding {
  class: ArgProvenanceClass
  value?: unknown
  /** For STATE: dotted path into AgentState (e.g. "environment.workspace_root"). */
  state_path?: string
}

export interface PolicyContext {
  policy_decision_id: string
  environment_id: string
  network_policy_id?: string | null
  sandbox_ref?: string | null
  /** Policy-fixed argument values; these override every other binding. */
  fixed?: Record<string, unknown>
}

export class ToolCompileError extends Error {
  constructor(
    public code: string,
    message: string,
  ) {
    super(`${code}: ${message}`)
  }
}

const IDEMPOTENT_REQUIRED: EffectClass[] = ["EXTERNAL_WRITE", "FINANCIAL", "PUBLISH", "PERMISSION_CHANGE", "NETWORK"]
const WRITE_EFFECTS: EffectClass[] = ["WORKSPACE_WRITE", "EXTERNAL_WRITE", "FINANCIAL", "PUBLISH", "PERMISSION_CHANGE"]

function resolveStatePath(state: AgentStateV1, path: string): unknown {
  return path.split(".").reduce<any>((o, k) => (o == null ? undefined : o[k]), state)
}

function typeOk(p: OperationParam, v: unknown): boolean {
  switch (p.type) {
    case "string":
      return typeof v === "string"
    case "number":
      return typeof v === "number" && Number.isFinite(v)
    case "integer":
      return Number.isInteger(v)
    case "boolean":
      return typeof v === "boolean"
    case "array":
      return Array.isArray(v)
    case "object":
      return !!v && typeof v === "object" && !Array.isArray(v)
  }
}

export function envelopeFor(
  state: AgentStateV1,
  schemaId: string,
  opts: { now: string; node_id?: string | null; trace_seed?: unknown },
): AbiEnvelopeV1 {
  const id = state.identity
  return {
    abi_version: ABI_VERSION,
    schema_id: schemaId,
    schema_version: TOOL_SCHEMA_VERSION,
    run_id: id.run_id,
    session_id: id.session_id,
    task_id: id.task_id,
    graph_id: state.graph_cursor?.graph_id ?? null,
    node_id: opts.node_id ?? null,
    state_version: id.state_version,
    created_at: opts.now,
    producer: { component_id: "gizzi.kernel.tool-call-compiler", component_version: "1.0.0", kind: "RUNTIME" },
    trace_id: `trace:${hashValue({ run: id.run_id, seed: opts.trace_seed ?? null }).slice(7, 31)}`,
    provenance: [
      {
        source_type: "SYSTEM",
        source_id: toAbiId(`state:${id.run_id}@${id.state_version}`),
        trust_class: "INTERNAL",
      },
    ],
  }
}

export interface CompileToolCallInput {
  state: AgentStateV1
  op: OperationDescriptor
  bindings: Record<string, ArgBinding>
  policy: PolicyContext
  now: string
  node_id?: string | null
}

export interface CompiledToolCall {
  invocation: ToolInvocationV1
  /** Non-fatal notes, e.g. a model-supplied value overridden by policy. */
  diagnostics: string[]
}

export function compileToolCall(input: CompileToolCallInput): CompiledToolCall {
  const { state, op, bindings, policy } = input
  const fixed = policy.fixed ?? {}
  const known = new Set(op.params.map((p) => p.name))
  for (const k of Object.keys(bindings))
    if (!known.has(k)) throw new ToolCompileError("UNKNOWN_ARGUMENT", `${op.tool_id}.${op.operation_id} has no parameter "${k}"`)

  const args: Record<string, unknown> = {}
  const prov: Record<string, ArgProvenanceClass> = {}
  const diagnostics: string[] = []

  for (const p of op.params) {
    const b = bindings[p.name]
    if (Object.prototype.hasOwnProperty.call(fixed, p.name)) {
      if (b && b.class !== "POLICY") diagnostics.push(`POLICY_OVERRIDE:${p.name}:${b.class}`)
      args[p.name] = fixed[p.name]
      prov[p.name] = "POLICY"
      continue
    }
    if (!b) {
      if (p.required) throw new ToolCompileError("MISSING_ARGUMENT", `required parameter "${p.name}" has no binding`)
      continue
    }
    if (p.policy_only) throw new ToolCompileError("POLICY_ONLY_ARGUMENT", `"${p.name}" may only be set by policy`)
    if (b.class === "POLICY") throw new ToolCompileError("UNBACKED_POLICY_ARGUMENT", `"${p.name}" claims POLICY but policy did not fix it`)
    let value: unknown
    if (b.class === "STATE") {
      if (!b.state_path) throw new ToolCompileError("STATE_PATH_REQUIRED", `"${p.name}" is STATE-bound without a state_path`)
      value = resolveStatePath(state, b.state_path)
      if (value === undefined) throw new ToolCompileError("STATE_UNRESOLVED", `state path "${b.state_path}" is undefined`)
    } else value = b.value // USER verbatim; RETRIEVAL/GENERATED as supplied
    if (value === undefined) {
      if (p.required) throw new ToolCompileError("MISSING_ARGUMENT", `required parameter "${p.name}" is undefined`)
      continue
    }
    if (!typeOk(p, value) || (p.enum && !p.enum.some((e) => e === value)))
      throw new ToolCompileError(
        b.class === "GENERATED" ? "GENERATED_SCHEMA_VIOLATION" : "SCHEMA_VIOLATION",
        `"${p.name}" does not match ${p.type}${p.enum ? " enum" : ""}`,
      )
    args[p.name] = value
    prov[p.name] = b.class
  }

  const resources = op.params
    .filter((p) => p.resource && typeof args[p.name] === "string")
    .map((p) => `${p.resource}:${args[p.name]}`)
    .sort()
  const writes = WRITE_EFFECTS.includes(op.effect_class)
  const envelope = envelopeFor(state, "allternit.kernel.ToolInvocationV1", {
    now: input.now,
    node_id: input.node_id ?? null,
    trace_seed: { tool: op.tool_id, op: op.operation_id, args },
  })
  const callHash = hashValue({ run: state.identity.run_id, v: state.identity.state_version, tool: op.tool_id, op: op.operation_id, args })
  const needsKey = IDEMPOTENT_REQUIRED.includes(op.effect_class)
  if (op.effect_class === "NETWORK" && !policy.network_policy_id)
    throw new ToolCompileError("NETWORK_POLICY_REQUIRED", "NETWORK effects need a network_policy_id")

  const invocation: ToolInvocationV1 = {
    envelope,
    invocation_id: `inv:${callHash.slice(7, 31)}`,
    tool_id: toAbiId(op.tool_id, "tool"),
    operation_id: op.operation_id,
    arguments: args,
    argument_provenance: prov,
    read_set: writes ? [] : resources,
    write_set: writes ? resources : [],
    effect_class: op.effect_class,
    policy_decision_id: toAbiId(policy.policy_decision_id, "pd"),
    idempotency_key: needsKey ? `idem:${callHash.slice(7)}` : null,
    timeout_ms: Math.max(1, op.timeout_ms ?? 120_000),
    environment_id: toAbiId(policy.environment_id, "env"),
    network_policy_id: policy.network_policy_id ?? null,
    sandbox_ref: policy.sandbox_ref ?? null,
  }
  return { invocation, diagnostics }
}

// ---------------------------------------------------------------------------
// Tiered disclosure
// ---------------------------------------------------------------------------

export interface ToolDisclosure {
  tier: DisclosureTier
  tool_id: string
  operation_id: string
  summary: string
  effect_class: EffectClass
  parameters?: { name: string; type: string; required: boolean; description?: string; enum?: unknown[] }[]
  docs?: string
}

export function disclose(op: OperationDescriptor, tier: DisclosureTier): ToolDisclosure {
  const out: ToolDisclosure = {
    tier,
    tool_id: op.tool_id,
    operation_id: op.operation_id,
    summary: op.summary,
    effect_class: op.effect_class,
  }
  if (tier === "SCHEMA" || tier === "DOCS")
    out.parameters = op.params
      .filter((p) => !p.policy_only)
      .map((p) => ({
        name: p.name,
        type: p.type,
        required: !!p.required,
        ...(p.description ? { description: p.description } : {}),
        ...(p.enum ? { enum: p.enum } : {}),
      }))
  if (tier === "DOCS" && op.docs) out.docs = op.docs
  return out
}

/** Disclose a tool set: snippet by default, schema/docs only when requested. */
export function discloseAll(ops: OperationDescriptor[], requested: Record<string, DisclosureTier> = {}) {
  return [...ops]
    .sort((a, b) => (a.tool_id + a.operation_id < b.tool_id + b.operation_id ? -1 : 1))
    .map((op) => disclose(op, requested[`${op.tool_id}.${op.operation_id}`] ?? requested[op.tool_id] ?? "SNIPPET"))
}

/** Argument values safe to disclose/log: SECRET → hash-only redaction. */
export function redactArguments(op: OperationDescriptor, args: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {}
  const sens = new Map(op.params.map((p) => [p.name, p.sensitivity]))
  for (const [k, v] of Object.entries(args))
    out[k] = sens.get(k) === "SECRET" ? `[REDACTED ${hashValue(v ?? null)}]` : v
  return out
}

// ---------------------------------------------------------------------------
// Receipts (WP3-compatible)
// ---------------------------------------------------------------------------

export interface ToolOutcome {
  started_at: string
  finished_at: string
  exit_class: ExitClass
  exit_code?: number | null
  output?: string
  policy_receipt_id: string
  diagnostics?: Record<string, unknown>[]
}

export function buildToolReceipt(invocation: ToolInvocationV1, outcome: ToolOutcome, op?: OperationDescriptor): ToolReceiptV1 {
  const invocationHash = hashValue(invocation)
  const hashes = [invocationHash, ...(outcome.output !== undefined ? [sha256Tagged(outcome.output)] : [])]
  return {
    envelope: {
      ...invocation.envelope,
      schema_id: "allternit.kernel.ToolReceiptV1",
      created_at: outcome.finished_at,
      provenance: [
        ...invocation.envelope.provenance,
        { source_type: "TOOL", source_id: invocation.tool_id, trust_class: "INTERNAL", content_hash: invocationHash },
      ],
    },
    invocation_id: invocation.invocation_id!,
    tool_id: invocation.tool_id,
    operation_id: invocation.operation_id,
    started_at: outcome.started_at,
    finished_at: outcome.finished_at,
    exit_class: outcome.exit_class,
    exit_code: outcome.exit_code ?? null,
    stdout_ref: null,
    stderr_ref: null,
    outputs: [],
    observed_reads: [],
    observed_writes: [],
    diagnostics: outcome.diagnostics ?? [],
    policy_receipt_id: toAbiId(outcome.policy_receipt_id, "pr"),
    content_hashes: hashes,
    error: null,
    extensions: {
      "x-argument_provenance": invocation.argument_provenance,
      ...(op ? { "x-arguments_disclosed": redactArguments(op, invocation.arguments) } : {}),
      "x-effect_class": invocation.effect_class,
    },
  }
}
