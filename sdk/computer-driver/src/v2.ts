// Contract v2 (`allternit.computer.v2`): the ten driver-backed structured
// members — read_ui, act, run_batch, verify, request_human, use_credential,
// run_subtask, run_parallel, run_skill, skills — as typed client methods, plus
// the `computer_v2` function tool every adapter exposes next to the pixel tool.
//
// Input types and member metadata come from src/v2-generated.ts (emitted by
// contracts/computer-toolset/generate.mjs from allternit-computer-v2.json), so
// they cannot drift from the server contract. Result shapes are the executor's
// JSON answers (allternit-api computer_v2.rs / computer_subtask.rs /
// computer_parallel.rs and the driver docs tools/allternit-driver.mdx).
import {
  resultText,
  type AllternitComputers,
  type Approval,
  type ToolsetResult,
} from "./client.ts"
import {
  COMPUTER_V2_STRUCTURED_MEMBER_NAMES,
  COMPUTER_V2_STRUCTURED_MEMBERS,
  COMPUTER_V2_TOOL_SCHEMAS,
} from "./v2-generated.ts"
import type {
  ComputerV2ActInput,
  ComputerV2ReadUiInput,
  ComputerV2RequestHumanInput,
  ComputerV2RunBatchInput,
  ComputerV2RunParallelInput,
  ComputerV2RunSkillInput,
  ComputerV2RunSubtaskInput,
  ComputerV2SkillsInput,
  ComputerV2StructuredMemberName,
  ComputerV2UseCredentialInput,
  ComputerV2VerifyInput,
} from "./v2-generated.ts"

export type {
  ComputerV2ActInput,
  ComputerV2MemberInputs,
  ComputerV2ReadUiInput,
  ComputerV2RequestHumanInput,
  ComputerV2RunBatchInput,
  ComputerV2RunParallelInput,
  ComputerV2RunSkillInput,
  ComputerV2RunSubtaskInput,
  ComputerV2SkillsInput,
  ComputerV2StructuredMemberName,
  ComputerV2UseCredentialInput,
  ComputerV2VerifyInput,
} from "./v2-generated.ts"
export { COMPUTER_V2_MEMBER_NAMES, COMPUTER_V2_STRUCTURED_MEMBER_NAMES } from "./v2-generated.ts"

/** Marker the Allternit wire tooling uses to recognise the v2 function tool. */
export const V2_TOOL_MARKER = "[allternit.computer.v2]"
/** The function-tool name the structured members are exposed under. */
export const COMPUTER_V2_TOOL_NAME = "computer_v2" as const

// --------------------------------------------------------------------- errors

/** A v2 member answered is_error:true, or returned no JSON where JSON was expected. */
export class ComputerV2Error extends Error {
  readonly member: string
  readonly result: ToolsetResult
  constructor(member: string, result: ToolsetResult, message?: string) {
    super(message ?? (resultText(result) || `The ${member} call failed.`))
    this.name = "ComputerV2Error"
    this.member = member
    this.result = result
  }
}

// ------------------------------------------------------------------- results
// Field shapes follow the executor and the Allternit Driver docs; extra keys
// the server adds are preserved via index signatures.

/** One element of a read_ui element map. Tree ids start with "e"; vision marks with "v". */
export interface UiElement {
  id: string
  role: string
  name?: string
  value?: unknown
  /** [x, y, w, h] in screen points. */
  bbox?: [number, number, number, number]
  enabled?: boolean
  focused?: boolean
  actions?: string[]
  parent?: string
  /** Tree path (role:ordinal chain); only with read_ui paths:true. */
  path?: string
  /** 8x8 pixel hash; only with read_ui crops:true. */
  crop?: string
  source?: "vision"
  /** Set-of-marks number for vision elements. */
  mark?: number
}

/** The grounder's answer when read_ui got a `target`. */
export interface GroundedResult {
  status: string
  id?: string
  point?: [number, number]
  confidence?: number
  error?: string
  [key: string]: unknown
}

/** read_ui result: one window's element map (or the diff since `since`). */
export interface ReadUiResult {
  window?: { pid?: number; window_id?: number; app?: string; title?: string }
  version: number
  engine?: string | null
  cached?: boolean
  live?: Record<string, unknown>
  /** Which source answered: ax, vision or hybrid. */
  source?: string
  source_reason?: string
  degraded?: boolean
  degraded_reason?: string
  /** Present when `since` was answered with a diff. */
  diff?: { added?: UiElement[]; changed?: UiElement[]; removed?: Array<Record<string, unknown>>; [key: string]: unknown }
  /** True when the `since` version had aged out and the whole map was re-sent. */
  reset?: boolean
  elements?: UiElement[]
  total?: number
  truncated?: boolean
  marks?: Array<Record<string, unknown>>
  vision_version?: number
  grounded?: GroundedResult
  ms?: number
  [key: string]: unknown
}

/** act result. `stale_version` carries the fresh map under `map`: re-plan and retry. */
export interface ActResult {
  status: "done" | "changed" | "stale" | "stale_version" | string
  version: number
  engine?: string | null
  map?: ReadUiResult
  changes?: unknown
  settled?: boolean
  ms?: number
  [key: string]: unknown
}

export interface RunBatchStepResult {
  status: "ok" | "failed" | string
  code?: string
  error?: string
  ms: number
  [key: string]: unknown
}

/** run_batch result: one row per step; a failed check stops the batch at `failed_at`. */
export interface RunBatchResult {
  ok: boolean
  steps: RunBatchStepResult[]
  version: number
  engine?: string | null
  failed_at?: number
  changes?: unknown
  cross_check?: unknown
  ms?: number
  [key: string]: unknown
}

export type VerifyCheck = ComputerV2VerifyInput["checks"][number]

export interface VerifyCheckResult {
  check: VerifyCheck
  ok: boolean
  detail: string
}

/** verify result: `ok` is true only when every check held. */
export interface VerifyResult {
  ok: boolean
  results: VerifyCheckResult[]
  ms?: number
  [key: string]: unknown
}

/** run_subtask / run_skill end statuses (safety verdicts included). */
export type SubtaskStatus = "done" | "escalated" | "failed" | "needs_confirmation" | "paused" | "denied" | "use_api" | string

export interface SubtaskCache {
  status: "hit" | "healed" | "recorded" | "miss" | "diverged" | string
  replayed_steps: number
  healed_steps: Array<Record<string, unknown>>
  stored: boolean
  key?: string
  skill?: string
  reason?: string
  [key: string]: unknown
}

/**
 * run_subtask / run_skill result. Anything but `done` carries `next` (what the
 * server suggests) and usually `screen`; `needs_confirmation`/`paused`/`denied`
 * carry `held_step`; `use_api` carries `api` (the MCP/API call to make yourself).
 */
export interface SubtaskResult {
  status: SubtaskStatus
  goal: string
  decisions: number
  actions: number
  elapsed_ms: number
  decision_ms: number
  action_ms: number
  oracle: { tokens: number; cost_usd: number }
  inputs_typed: number
  success: unknown
  steps: Array<Record<string, unknown>>
  cache: SubtaskCache
  reason?: string
  held_step?: Record<string, unknown>
  api?: { tool?: string; description?: string; [key: string]: unknown }
  screen?: unknown
  next?: string
  [key: string]: unknown
}

/** One run_parallel result row: a run_subtask result plus its slot and computer. */
export interface ParallelSubtaskResult extends SubtaskResult {
  subtask?: number
  computer?: string
}

/**
 * run_parallel result. Plain mode: `results` in order, `status` done/partial/
 * failed. Best-of-N mode (best_of + computers): `rollouts` with narratives and
 * `chosen` naming the winning rollout, `status` done or escalated.
 */
export interface ParallelResult {
  status: "done" | "partial" | "failed" | "escalated" | string
  done?: number
  subtasks: number
  max_parallel?: number
  best_of?: number
  elapsed_ms: number
  cost_usd: number
  results?: Array<ParallelSubtaskResult | { status: "skipped"; reason: string; subtask?: number; computer?: string }>
  chosen?: { rollout?: string; computer?: string; why?: unknown } | null
  reason?: unknown
  judge_decisions?: number
  rollouts?: Array<{ rollout: string; computer: string; status: string; narrative: string; result: SubtaskResult }>
  next?: string
  [key: string]: unknown
}

/** One saved skill (a recording taught with run_subtask save_as). */
export interface SkillInfo {
  name: string
  goal: string
  app?: string | null
  inputs: string[]
  steps: number
  runs: number
  healed: number
  updated_at?: string
  last_used_at?: string | null
  [key: string]: unknown
}

/** skills result: the saved skills this person can run with run_skill. */
export interface SkillsResult {
  skills: SkillInfo[]
  forgot?: string
  [key: string]: unknown
}

// ----------------------------------------------------------- call + driver

export interface V2CallOptions {
  client: AllternitComputers
  computerId: string
  /** Called when the server holds the call (409 approval_required). true → approve + resend with the grant; false/absent → the held result resolves. */
  onApproval?: (approval: Approval) => boolean | Promise<boolean>
}

/** Run one structured v2 member through POST /v1/computers/{id}/toolset, answering an approval hold via `onApproval`. */
export function runComputerV2Member(o: V2CallOptions, member: ComputerV2StructuredMemberName | string, input: unknown): Promise<ToolsetResult> {
  return o.client.toolsetWithApproval(o.computerId, { toolset: "computer", member, input: (input ?? {}) as Record<string, unknown> }, o.onApproval)
}

/**
 * Typed methods for the ten structured v2 members of one computer. Construct
 * once per computer and pass it to your loop, or call the members directly.
 */
export class ComputerV2Driver {
  readonly #o: V2CallOptions

  constructor(o: V2CallOptions) {
    this.#o = o
  }

  /** Read the target window or app's UI as a structured element tree (no screenshot). */
  read_ui(input: ComputerV2ReadUiInput = {}): Promise<ReadUiResult> {
    return this.#json("read_ui", input)
  }
  /** Act on an element id from read_ui. */
  act(input: ComputerV2ActInput): Promise<ActResult> {
    return this.#json("act", input)
  }
  /** Run ordered steps in one window in a single call; a failed check stops the batch. */
  run_batch(input: ComputerV2RunBatchInput): Promise<RunBatchResult> {
    return this.#json("run_batch", input)
  }
  /** Check bounded conditions on the current UI without acting. */
  verify(input: ComputerV2VerifyInput): Promise<VerifyResult> {
    return this.#json("verify", input)
  }
  /** Pause and hand the computer to a person; resolves when they signal done or the timeout elapses. */
  request_human(input: ComputerV2RequestHumanInput = {}): Promise<string> {
    return this.#text("request_human", input)
  }
  /** Type a vault credential into the focused field; the value never enters the model context. */
  use_credential(input: ComputerV2UseCredentialInput): Promise<string> {
    return this.#text("use_credential", input)
  }
  /** Hand a bounded UI subtask to the fast decision loop. */
  run_subtask(input: ComputerV2RunSubtaskInput): Promise<SubtaskResult> {
    return this.#json("run_subtask", input)
  }
  /** Run independent bounded subtasks on separate computers at the same time. */
  run_parallel(input: ComputerV2RunParallelInput): Promise<ParallelResult> {
    return this.#json("run_parallel", input)
  }
  /** Run a saved skill (a recording taught with run_subtask save_as) by name. */
  run_skill(input: ComputerV2RunSkillInput): Promise<SubtaskResult> {
    return this.#json("run_skill", input)
  }
  /** List the saved skills this person can run (pass forget to delete one first). */
  skills(input: ComputerV2SkillsInput = {}): Promise<SkillsResult> {
    return this.#json("skills", input)
  }

  async #text(member: string, input: unknown): Promise<string> {
    const res = await runComputerV2Member(this.#o, member, input)
    if (res.is_error) throw new ComputerV2Error(member, res)
    return resultText(res)
  }

  async #json<T>(member: string, input: unknown): Promise<T> {
    const res = await runComputerV2Member(this.#o, member, input)
    if (res.is_error) throw new ComputerV2Error(member, res)
    const text = resultText(res)
    try {
      return JSON.parse(text) as T
    } catch {
      throw new ComputerV2Error(member, res, `The ${member} call returned no JSON.`)
    }
  }
}

// ---------------------------------------------------------- the computer_v2 tool

// Steering every model gets for the structured members. The canonical copy
// gizzi sends is cmd/gizzi-code/src/runtime/tools/computer-toolset/adapter.ts
// (PREFER_STRUCTURED / PREFER_SUBTASK / toolDescriptionV2) — keep the wording
// in sync when either side changes.
const PREFER_STRUCTURED = [
  "Prefer read_ui + act/run_batch over screenshot + pixel-by-pixel loops: the element tree is faster, cheaper and stable across resizes.",
  "When the tree is empty (canvas/game/remote desktop), read_ui falls back to vision by itself: elements with source \"vision\" and a mark number act like any other id; pass target (e.g. 'the Export button') to ground one element. Reach for screenshots only when that still isn't enough.",
].join(" ")

const PREFER_SUBTASK = [
  "Plan, then delegate: give each bounded UI step sequence (fill a form, search and pick, toggle settings) to run_subtask with the goal, the literal inputs it may type and success checks, instead of choosing every click yourself.",
  "A typical task is one run_subtask call plus your final answer. Keep your own calls for planning, for judgment the subtask hands back (status escalated: continue from the screen it returns), and for steps outside the UI.",
  "API over GUI: when one of your MCP tools or an API does the goal, call it instead of driving the screen; when unsure, pass the candidates as run_subtask api_options (status use_api names the one to call).",
  "Independent subtasks on separate computers go in one run_parallel call. For a high-value subtask, best_of N with N sandbox computers runs N rollouts and a judge picks one by their step narratives (never on the person's own machine).",
].join(" ")

/** The `computer_v2` function tool, provider-neutral. Adapt `schema` to your SDK's spelling (Anthropic `input_schema`, OpenAI `parameters`, Gemini `parameters`). */
export interface ComputerV2ToolDefinition {
  name: typeof COMPUTER_V2_TOOL_NAME
  description: string
  /** JSON Schema for the tool's arguments: `action` plus every member's fields (all optional besides action). */
  schema: Record<string, unknown>
}

/** Description gizzi-equivalent for the `computer_v2` tool, built from the contract's member descriptions. */
export function computerV2ToolDescription(): string {
  const lines = COMPUTER_V2_STRUCTURED_MEMBERS.map((m) => `- ${m.name}: ${m.description}`)
  return [
    `${V2_TOOL_MARKER} Read and drive this computer's UI as a structured element tree (no screenshots needed). Set \`action\` to one of the actions below and pass that action's fields.`,
    PREFER_SUBTASK,
    PREFER_STRUCTURED,
    "Element ids come from read_ui and are stable until the UI changes; pass the version back to act/run_batch to catch a moved UI.",
    "run_batch runs many steps in one call and stops at the first failed check, then returns one fresh read — batch aggressively.",
    "use_credential types a vault secret or TOTP code into the focused field; the value is never shown to you.",
    "request_human pauses for a person (CAPTCHA, 2FA, judgment calls) and resumes the session when they signal done.",
    "",
    ...lines,
  ].join("\n")
}

type ToolSchema = { properties?: Record<string, Record<string, unknown>>; required?: string[] }

const STRUCTURED_SCHEMAS = COMPUTER_V2_TOOL_SCHEMAS as unknown as Record<string, ToolSchema>

/**
 * The combined JSON Schema for the `computer_v2` tool: `action` (one of the ten
 * structured members) plus the union of every member's input fields, each
 * optional — a call maps 1:1 onto `{ member: action, input: <the rest> }`, and
 * the server validates against the member's own contract schema.
 */
export function computerV2Parameters(): Record<string, unknown> {
  const properties: Record<string, Record<string, unknown>> = {
    action: { type: "string", enum: [...COMPUTER_V2_STRUCTURED_MEMBER_NAMES], description: "Which computer_v2 action to run." },
  }
  for (const name of COMPUTER_V2_STRUCTURED_MEMBER_NAMES) {
    for (const [k, v] of Object.entries(STRUCTURED_SCHEMAS[name]?.properties ?? {})) {
      if (!properties[k]) properties[k] = v
    }
  }
  return { type: "object", properties, required: ["action"], additionalProperties: false }
}

/** The provider-neutral `computer_v2` function tool definition. */
export function computerV2Tool(): ComputerV2ToolDefinition {
  return { name: COMPUTER_V2_TOOL_NAME, description: computerV2ToolDescription(), schema: computerV2Parameters() }
}
